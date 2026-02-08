use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::os::fd::{AsFd, AsRawFd};

use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use nix::net::if_::if_nametoindex;
use nix::sched::{CloneFlags, setns};
use nix::unistd::Pid;
use rand::Rng;
use rand::distributions::Alphanumeric;
use rtnetlink::{Handle, LinkBridge, LinkUnspec, LinkVeth, new_connection};
use tokio::runtime::Runtime;
use tracing::{debug, debug_span, error, warn, warn_span};
use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::error::{ConfigError, LinkError, NamespaceError, NetError};
use crate::{NS_DIR, NetResult, mount_device};

// ==== trait FromYamlConfig ====

pub trait FromYamlConfig: Sized {
    fn from_yaml_config(name: &str, config: &Hash) -> NetResult<Self>;
}

// ==== struct Interface ====

#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub addresses: Vec<IpNetwork>,
}

impl Interface {
    fn new(name: String) -> Self {
        Self {
            name,
            addresses: vec![],
        }
    }
    async fn add_addresses(&self, handle: &Handle) -> NetResult<()> {
        let ifindex = match if_nametoindex(self.name.as_str()) {
            Ok(ifindex) => ifindex,
            Err(_) => {
                warn!(
                    "Address not added. Interfaces without attached links not added."
                );
                return Ok(());
            }
        };

        for addr in &self.addresses {
            let request =
                handle.address().add(ifindex, addr.ip(), addr.prefix());
            request.execute().await.map_err(|err| {
                error!(addr=%addr ,"Unable to add address");
                NetError::LinkError(LinkError::AddressAdd {
                    iface: self.name.clone(),
                    addr: *addr,
                    source: err,
                })
            })?;
        }
        Ok(())
    }
}

impl FromYamlConfig for Interface {
    fn from_yaml_config(name: &str, yaml_config: &Hash) -> NetResult<Self> {
        let (router_name, iface_name) = match name.split_once("!!!!") {
            Some((router_name, iface_name)) => (router_name, iface_name),
            _ => {
                return Err(NetError::BasicError(format!(
                    "Improperly formatter name in Interface::from_yaml_config. \
                    Received {name}.\
                    Must be in the format {{router-name}}!!!!{{interface-name}}"
                ))
                .into());
            }
        };
        let mut interface = Interface::new(iface_name.to_string());
        let router_name = format!("routers->{router_name}->interfaces->");
        let iface_name = format!("{router_name}->{iface_name}");

        // --- Get the interface's Ipv4 addresses ---
        if let Some(ipv4_addresses) =
            yaml_config.get(&Yaml::String(String::from("ipv4")))
        {
            match ipv4_addresses {
                Yaml::Array(ipv4_addresses) => {
                    let mut addr_iter = ipv4_addresses.iter();
                    while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                        match addr_str.parse::<Ipv4Network>() {
                            Ok(ip_net) => {
                                interface.addresses.push(IpNetwork::V4(ip_net));
                            }
                            Err(err) => {
                                return Err(ConfigError::InvalidAddress {
                                    addr_type: "ipv4".to_string(),
                                    address: "addr_str".to_string(),
                                    interface: format!(
                                        "{iface_name}->ipv4->??"
                                    ),
                                    source: err,
                                }
                                .into());
                            }
                        }
                    }
                }
                Yaml::Null => {
                    // Ipv4 addresses list is blank.
                }
                _ => {
                    return {
                        Err(ConfigError::IncorrectType {
                            field: format!("{iface_name}->ipv4??"),
                            expected: "array".to_string(),
                        }
                        .into())
                    };
                }
            }
        }

        // --- Get the interface's Ipv6 addresses ---
        if let Some(ipv6_addresses) =
            yaml_config.get(&Yaml::String(String::from("ipv6")))
        {
            match ipv6_addresses {
                Yaml::Array(ipv6_addresses) => {
                    let mut addr_iter = ipv6_addresses.iter();
                    while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                        match addr_str.parse::<Ipv6Network>() {
                            Ok(ip_net) => {
                                interface.addresses.push(IpNetwork::V6(ip_net));
                            }
                            Err(err) => {
                                return Err(ConfigError::InvalidAddress {
                                    addr_type: "ipv6".to_string(),
                                    address: "addr_str".to_string(),
                                    interface: format!(
                                        "{iface_name}->ipv6->??"
                                    ),
                                    source: err,
                                }
                                .into());
                            }
                        }
                    }
                }
                Yaml::Null => {
                    // Ipv6 addresses list is blank.
                }
                _ => {
                    return {
                        Err(ConfigError::IncorrectType {
                            field: format!("{iface_name}->ipv6??"),
                            expected: "array".to_string(),
                        }
                        .into())
                    };
                }
            }
        }
        Ok(interface)
    }
}

// ==== Node ====

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Router(Router),
    Switch(Switch),
}

impl Node {
    pub fn power_off(&self) -> NetResult<()> {
        if let Self::Router(router) = self {
            router.power_off()?;
        }
        Ok(())
    }
}

// ==== Router =====

#[derive(Debug, Clone)]
pub struct Router {
    pub name: String,
    pub file_path: Option<String>,
    pub interfaces: Vec<Interface>,
    pub pid: Option<Pid>,
}

impl Router {
    /// Creates a Router object that will represent the
    /// router
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            file_path: None,
            interfaces: vec![],
            pid: None,
        }
    }

    /// Creates a namespace representing the router and turns on the
    /// loopback interface.
    pub fn power_on(&mut self) -> NetResult<()> {
        let file_path = mount_device(Some(self.name.clone()))?;
        self.file_path = Some(file_path);
        debug!(router=%self.name, "Powered on");
        Ok(())
    }

    /// Change interface state to up.
    pub fn iface_up(&self, ifindex: u32, runtime: &Runtime) -> NetResult<()> {
        let router_name = self.name.clone();
        runtime.block_on(async {
            self.in_ns(move || async move {
                let (connection, handle, _) =
                    new_connection().map_err(|err| {
                        LinkError::ConnectionFailed { source: err }
                    })?;

                tokio::spawn(connection);

                let message = LinkUnspec::new_with_index(ifindex).up().build();

                handle.link().set(message).execute().await.map_err(|err| {
                    error!(router=%router_name, ifindex=%ifindex,
                        "problem bringing up"
                    );
                    NetError::LinkError(LinkError::ChangeStateUp {
                        device: router_name.clone(),
                        ifindex,
                        source: err,
                    })
                })?;
                Ok::<(), NetError>(())
            })
            .await?
        })
    }

    /// Deletes the namespace created by the Router (if it exists)
    pub fn power_off(&self) -> NetResult<()> {
        crate::delete_ns(Some(self.name.clone()))?;
        Ok(())
    }

    /// Executes instructions inside the
    /// router's namespace.
    ///
    /// ```no_run
    /// use std::process::Command;
    ///
    /// use topology::Router;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let router = Router::new("r1");
    ///     router
    ///         .in_ns(move || async move {
    ///             let output = Command::new("ip").args(vec!["link"]).output();
    ///
    ///             // this will show you the output
    ///             // of the `ip link` in the router.
    ///             // If no modifications have been made
    ///             // to the namespace, should only show
    ///             // the loopback ("lo") interface
    ///             println!("{#:?}", output);
    ///         })
    ///         .await;
    /// }
    /// ```
    pub async fn in_ns<Fut, T, R>(&self, f: Fut) -> NetResult<R>
    where
        Fut: FnOnce() -> T + Send + 'static,
        T: Future<Output = R> + Send,
    {
        match &self.file_path {
            Some(file_path) => {
                // Move into the Router namespace.
                let ns_file =
                    File::open(file_path.as_str()).map_err(|err| {
                        NamespaceError::FileOpen {
                            path: file_path.clone(),
                            source: err,
                        }
                    })?;

                setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(
                    |err| NamespaceError::Entry {
                        device: self.name.clone(),
                        source: err,
                    },
                )?;

                let result = (f)().await;

                // Go back to the main namespace.
                let main_namespace_path = format!("{NS_DIR}/main/net");

                let main_file =
                    File::open(&main_namespace_path).map_err(|err| {
                        NetError::BasicError(format!(
                            "Unable to open file {main_namespace_path}: {err:?}"
                        ))
                    })?;

                setns(main_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(
                    |err| {
                        NetError::NamespaceError(NamespaceError::ReturnToMain {
                            source: err,
                        })
                    },
                )?;
                Ok(result)
            }
            None => Err(NamespaceError::NotFound {
                device: self.name.clone(),
            }
            .into()),
        }
    }

    /// adds the addresses of the said router as
    /// per the topology yaml file.
    ///
    /// Example:
    /// ```yaml
    /// 
    /// rt2:
    ///   interfaces:
    ///     lo:
    ///       ipv4:
    ///       - 2.2.2.2/32
    ///     eth-sw1:
    ///       ipv4:
    ///       - 10.0.1.2/24
    /// ```
    /// Above yaml config in topo file will add the address
    /// 10.0.1.2/24 to the eth-sw1 interface and 2.2.2.2/32
    /// to the lo address
    pub fn add_iface_addresses(&self, runtime: &Runtime) -> NetResult<()> {
        let interfaces = self.interfaces.clone();
        let router_name = self.name.clone();

        runtime.block_on(async {
            self.in_ns(move || async move {
                let (connection, handle, _) =
                    new_connection().map_err(|err| {
                        LinkError::ConnectionFailed { source: err }
                    })?;
                tokio::spawn(connection);
                for iface in interfaces {
                    let iface_name = iface.name.clone();
                    let add_iface_addr_span =
                        warn_span!("add-address", %iface_name, %router_name);
                    let _span_guard = add_iface_addr_span.enter();
                    iface.add_addresses(&handle).await?;
                }
                Ok(())
            })
            .await?
        })
    }
}

impl FromYamlConfig for Router {
    fn from_yaml_config(name: &str, router_config: &Hash) -> NetResult<Self> {
        let mut router = Self::new(name);

        match router_config.get(&Yaml::String(String::from("interfaces"))) {
            Some(Yaml::Hash(interfaces_config)) => {
                for (iface_name, iface_config) in interfaces_config {
                    let iface_name = match iface_name {
                        Yaml::String(iface_name) => iface_name,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: format!(
                                    "routers->{name}->interfaces->[??:config]"
                                ),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    };

                    match iface_config {
                        Yaml::Hash(iface_config) => {
                            let name = format!("{name}!!!!{iface_name}");
                            let interface = Interface::from_yaml_config(
                                &name,
                                iface_config,
                            )?;
                            router.interfaces.push(interface);
                        }
                        Yaml::Null => {
                            // TODO: Make sure interface come up successfully
                            // on this config branch.
                            let interface =
                                Interface::new(iface_name.to_string());
                            router.interfaces.push(interface);
                        }
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: format!("routers->{name}->interfaces->{iface_name}->??"),
                                expected: "hash".to_string(),
                            }.into());
                        }
                    }
                }
                Ok(router)
            }
            // Interfaces have not been configured.
            Some(Yaml::Null) | None => Ok(router),
            Some(_) => Err(ConfigError::IncorrectType {
                field: format!("routers->{name}->interfaces->??"),
                expected: "hash".to_string(),
            }
            .into()),
        }
    }
}

// ==== Switch ====
#[derive(Debug, Clone)]
pub struct Switch {
    pub name: String,
    pub ifindex: Option<u32>,
    pub interfaces: Vec<Interface>,
}

impl Switch {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ifindex: None,
            interfaces: vec![],
        }
    }

    /// Initializes a network bridge representing the switch.
    pub fn power_on(&mut self, runtime: &Runtime) -> NetResult<()> {
        let name = self.name.as_str();

        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);

            let message = LinkBridge::new(name).up().build();
            let request = handle.link().add(message);

            request.execute().await.map_err(|e| {
                NetError::BasicError(format!(
                    "Failed to create bridge {name}: {e}",
                ))
            })?;

            if let Ok(ifindex) = if_nametoindex(name) {
                self.ifindex = Some(ifindex);
                debug!(switch = %self.name, "Powered on");
            }

            Ok(())
        })
    }
}

impl FromYamlConfig for Switch {
    /// Handles config that is in the form of:
    ///
    /// ```yaml
    /// sw1:
    ///   interfaces:
    ///     eth0:
    ///       ipv4:
    ///         - 192.168.100.20/24
    ///       ipv6:
    ///         - 2001:db8::/96
    /// ```
    /// converted into a yaml_rust2::yaml::Hash;
    fn from_yaml_config(
        switch_name: &str,
        switch_config: &Hash,
    ) -> NetResult<Self> {
        let mut switch = Self::new(switch_name);

        match switch_config.get(&Yaml::String(String::from("interfaces"))) {
            Some(Yaml::Hash(interfaces_config)) => {
                for (iface_name, iface_config) in interfaces_config {
                    match iface_name {
                        Yaml::String(iface_name) => match iface_config {
                            Yaml::Hash(iface_config) => {
                                let iface_name =
                                    format!("{switch_name}!!!!{iface_name}");
                                let interface = Interface::from_yaml_config(
                                    &iface_name,
                                    iface_config,
                                )?;
                                switch.interfaces.push(interface);
                            }
                            _ => {
                                return Err(ConfigError::IncorrectType {
                                    field: format!(
                                        "switches.switch[{switch_name}].interfaces[config??]"
                                    ),
                                    expected: "hash".to_string(),
                                }
                                .into());
                            }
                        },
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: format!(
                                    "switches.switch[{switch_name}].interfaces[name??]"
                                ),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    }
                }
                Ok(switch)
            }
            _ => Err(ConfigError::IncorrectType {
                field: format!("switches.switch[{switch_name}][config??]"),
                expected: "hash".to_string(),
            }
            .into()),
        }
    }
}

// ==== LinkManager ====

pub(crate) struct LinkManager;

impl LinkManager {
    pub(crate) fn setup_all(
        &self,
        runtime: &Runtime,
        nodes: &BTreeMap<String, Node>,
        links: &[Link],
    ) -> NetResult<()> {
        // Bring up the Routers' loopback interfaces.
        for node in nodes.values() {
            if let Node::Router(router) = node {
                router.iface_up(1, runtime)?;
            }
        }

        for link in links {
            Self::create_link(runtime, nodes, link)?;
        }

        // Add addresses for links in the router nodes.
        for node in nodes.values() {
            if let Node::Router(router) = node {
                router.add_iface_addresses(runtime)?;
            }
        }
        Ok(())
    }

    fn create_link(
        runtime: &Runtime,
        nodes: &BTreeMap<String, Node>,
        link: &Link,
    ) -> NetResult<()> {
        let src_iface = format!("{}:{}", link.src_device, link.src_iface);
        let dst_iface = format!("{}:{}", link.dst_device, link.dst_iface);
        let link_span = debug_span!("link-setup", %src_iface, %dst_iface);
        let _span_guard = link_span.enter();
        debug!("Setting up");

        // generate random names for veth link
        // we do this to avoid conflict in the
        // parent device of interface names.
        let mut link_name: String;
        link_name = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(4)
            .map(char::from)
            .collect();

        let node1_link = format!("eth-{link_name}");

        link_name = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(4)
            .map(char::from)
            .collect();
        let node2_link = format!("eth-{link_name}");

        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);
            let request = handle.link().add(
                LinkVeth::new(node1_link.as_str(), node2_link.as_str()).build(),
            );

            //request.message_mut().header.flags.push(LinkFlag::Up);
            request.execute().await.map_err(|err| {
                LinkError::ExecuteFailed {
                    operation: "create_link".to_string(),
                    source: err,
                }
            })?;

            Ok::<(), NetError>(())
        })?;
        if let Some(src_node) = nodes.get(&link.src_device)
            && let Some(dst_node) = nodes.get(&link.dst_device)
        {
            // attaches the links to their respective nodes
            Self::attach_link(
                runtime,
                src_node,
                node1_link,
                link.src_iface.clone(),
            )?;
            Self::attach_link(
                runtime,
                dst_node,
                node2_link,
                link.dst_iface.clone(),
            )?;
        }
        debug!("Setup complete");

        Ok(())
    }

    fn attach_link(
        runtime: &Runtime,
        node: &Node,
        current_link_name: String,
        new_link_name: String,
    ) -> NetResult<()> {
        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);
            match node {
                Node::Router(router) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(file_path) = &router.file_path
                    {
                        let file = File::open(file_path).map_err(|err| {
                            NamespaceError::FileOpen {
                                path: file_path.clone(),
                                source: err,
                            }
                        })?;
                        let message = LinkUnspec::new_with_index(index)
                            .setns_by_fd(file.as_raw_fd())
                            .build();
                        // Move router device to said namespace.
                        handle.link().set(message).execute().await.map_err(
                            |err| LinkError::ExecuteFailed {
                                operation:
                                    "attach-link->move-link-to-router-namespace"
                                        .to_string(),
                                source: err,
                            },
                        )?;

                        // Rename the interface to it's proper name.
                        router
                            .in_ns(move || async move {
                                let (conn, handle, _) = new_connection()
                                    .map_err(|err| {
                                        LinkError::ConnectionFailed {
                                            source: err,
                                        }
                                    })?;
                                tokio::spawn(conn);

                                // Rename the link from the name given to it
                                // at create_link and bring the link up.
                                let message = LinkUnspec::new_with_index(index)
                                    .name(new_link_name)
                                    .up()
                                    .build();

                                handle
                                    .link()
                                    .set(message)
                                    .execute()
                                    .await
                                    .map_err(|err| {
                                        LinkError::ExecuteFailed {
                                        operation:
                                            "attach-link->bring-interface-up"
                                                .to_string(),
                                        source: err,
                                    }
                                    })?;
                                Ok::<(), NetError>(())
                            })
                            .await??;
                        // Above: one '?' for the inner method, one for the
                        // 'in_ns' method.
                    }
                }
                Node::Switch(switch) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(ifindex) = switch.ifindex
                    {
                        // Rename the link from the name given to it
                        // at create_link and bring it up.
                        let message = LinkUnspec::new_with_index(index)
                            .name(new_link_name)
                            .up()
                            .build();
                        if let Err(err) =
                            handle.link().set(message).execute().await
                        {
                            error!(error = %err, "error changing name");
                        }

                        let message = LinkUnspec::new_with_index(index)
                            .controller(ifindex)
                            .build();
                        if let Err(err) =
                            handle.link().set(message).execute().await
                        {
                            error!(error = %err, "error changing controller");
                        }
                    }
                }
            }
            Ok(())
        })
    }
}

// ==== Link ====

#[derive(Debug, Clone)]
pub struct Link {
    pub src_device: String,
    pub src_iface: String,
    pub dst_device: String,
    pub dst_iface: String,
}

impl Link {
    pub fn src(&self) -> String {
        format!("{}:{}", self.src_device, self.src_iface)
    }

    pub fn dst(&self) -> String {
        format!("{}:{}", self.dst_device, self.dst_iface)
    }
}
