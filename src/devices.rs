use crate::plugins::{Config, Holo, Plugin};
use crate::{error::Error, Result};

use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use netlink_packet_route::link::LinkFlag;
use nix::net::if_::if_nametoindex;
use nix::sched::{setns, CloneFlags};
use nix::unistd::gettid;
use rtnetlink::{new_connection, Handle, NetworkNamespace, NETNS_PATH};
use std::fs::File;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind};
use std::os::fd::AsFd;
use yaml_rust2::yaml::Hash;
use yaml_rust2::Yaml;

// ==== Interface ====
#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub addresses: Vec<IpNetwork>,
}

impl Interface {
    async fn add_addresses(&self, handle: &Handle) -> Result<()> {
        let ifindex = if_nametoindex(self.name.as_str()).map_err(|_| {
            let err_msg = format!("Interface {:?} not found", self.name);
            Error::GeneralError(err_msg)
        })?;

        for addr in &self.addresses {
            let request = handle.address().add(ifindex, addr.ip(), addr.prefix());
            request.execute().await.map_err(|_| {
                let err_msg = format!("Unable to add address {:?}", addr);
                Error::GeneralError(err_msg)
            })?;
        }
        Ok(())
    }

    fn from_yaml_config(name: &str, yaml_config: &Hash) -> Result<Self> {
        let mut interface = Interface {
            name: name.to_string(),
            addresses: vec![],
        };

        // --- Get the interface's Ipv4 addresses ---
        if let Some(ipv4_addresses) = yaml_config.get(&Yaml::String(String::from("ipv4"))) {
            if let Yaml::Array(ipv4_addresses) = ipv4_addresses {
                let mut addr_iter = ipv4_addresses.iter();
                while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                    if let Ok(ip_net) = addr_str.parse::<Ipv4Network>() {
                        interface.addresses.push(IpNetwork::V4(ip_net));
                    }
                }
            } else {
                // When ipv4 is not an array
                return Err(Error::IncorrectYamlType(String::from("ipv4")));
            }
        }

        // --- Get the interface's Ipv6 addresses ---
        if let Some(ipv6_addresses) = yaml_config.get(&Yaml::String(String::from("ipv6"))) {
            if let Yaml::Array(ipv6_addresses) = ipv6_addresses {
                let mut addr_iter = ipv6_addresses.iter();
                while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                    if let Ok(ip_net) = addr_str.parse::<Ipv6Network>() {
                        interface.addresses.push(IpNetwork::V6(ip_net));
                    }
                }
            } else {
                // When ipv4 is not an array
                return Err(Error::IncorrectYamlType(String::from("ipv6")));
            }
        }
        Ok(interface)
    }
}

// ==== Router =====
#[derive(Debug, Clone)]
pub struct Router {
    pub name: String,
    pub file_path: Option<String>,
    pub plugin: Option<Plugin>,
    pub interfaces: Vec<Interface>,

    // this will be run when the startup is run
    pub startup_config: Option<String>,
}

impl Router {
    /// creates a Router object that will represent the
    /// router
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            file_path: None,
            plugin: None,
            interfaces: vec![],
            startup_config: None,
        }
    }

    pub fn from_yaml_config(
        name: &str,
        router_config: &Hash,
        config: &Option<Config>,
    ) -> Result<Self> {
        let mut router = Self::new(name);

        // == plugin configs ==
        if let Some(plugin_config) = router_config.get(&Yaml::String(String::from("plugin")))
            && let Yaml::String(plugin_name) = plugin_config
        {
            match plugin_name.as_str() {
                "holo" => {
                    if let Some(config) = config {
                        for plugin in &config.plugins {
                            if let Plugin::Holo(_) = plugin {
                                router.plugin = Some(plugin.clone());
                            }
                        }
                    } else {
                        router.plugin = Some(Plugin::Holo(Holo::default()));
                    }
                }
                _ => return Err(Error::InvalidPluginName(plugin_name.to_string())),
            }
        } else {
            return Err(Error::GeneralError(String::from(
                "field 'plugin' required for device",
            )));
        }

        // == interface configs ==
        if let Some(Yaml::Hash(interfaces_config)) =
            router_config.get(&Yaml::String(String::from("interfaces")))
        {
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    if let Ok(interface) = Interface::from_yaml_config(iface_name, iface_config) {
                        router.interfaces.push(interface);
                    }
                } else {
                    return Err(Error::GeneralError(String::from(
                        "Interface content for '{:?}' not a dictionary",
                    )));
                }
            }
        }
        Ok(router)
    }

    /// Creates a namespace representing the router
    /// and turns on the loopback interface.
    pub async fn power_on(&mut self) -> Result<()> {
        if let Err(err) = NetworkNamespace::add(self.name.clone()).await {
            let err_msg = format!("unable to create namespace\n {:?}", err);
            let io_err = IoError::new(ErrorKind::Other, err_msg.as_str());
            return Err(Error::IoError(io_err));
        }
        let mut ns_path = String::new();
        ns_path.push_str(NETNS_PATH);
        ns_path.push_str(self.name.as_str());
        self.file_path = Some(ns_path);

        // make sure the loopback interface of the router is up
        self.up(String::from("lo")).await?;

        // add the environment variables for the necessary binaries
        Ok(())
    }

    /// Deletes the namespace created by the Router (if it exists)
    pub async fn power_off(&mut self) {
        // TODO: add power_off functionality
    }

    /// Runs the commands inside the
    /// router namespace.
    ///
    /// ```no_run
    /// use topology::Router;
    /// use std::process::Command;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let router = Router::new("r1").await.unwrap();
    ///     router.run(move || async move {
    ///         let output = Command::new("ip")
    ///             .args(vec!["link"]).output();
    ///         
    ///         // this will show you the output
    ///         // of the `ip link` in the router.
    ///         // If no modifications have been made
    ///         // to the namespace, should only show
    ///         // the loopback ("lo") interface
    ///         println!("{#:?}", output);
    ///     }).await;
    /// }
    /// ```
    pub async fn run<Fut, T, R>(&self, f: Fut) -> Result<R>
    where
        Fut: FnOnce() -> T + Send + 'static,
        T: Future<Output = R> + Send,
    {
        let current_thread_path = format!("/proc/self/task/{}/ns/net", gettid());
        let current_thread_file = File::open(&current_thread_path).unwrap();

        // move into namespace if it has already been created
        if let Some(file_path) = &self.file_path {
            let ns_file = File::open(file_path.as_str()).unwrap();
            setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();
        }
        let result = (f)().await;

        // come back to parent namespace
        setns(current_thread_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();

        Ok(result)
    }

    /// Brings a specific interface of the router up
    /// administratively.
    ///
    /// ```no_run
    /// // here, we create the br1 bridge
    /// // We will first create the interface
    /// // then bring it up
    ///
    /// use topology::Router;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let router = Router::new("r1").await.unwrap();
    ///     // brings up the loopback interface
    ///     router.up("lo").await;
    /// }
    /// ```
    pub async fn up(&self, iface_name: String) -> Result<()> {
        let router_name = self.name.clone();
        let result = self
            .run(move || async move {
                let (connection, handle, _) = new_connection().unwrap();
                let ifindex = if_nametoindex(iface_name.as_str()).map_err(|_| {
                    let err_msg = format!("Router interface '{}:{}' not found", router_name, iface_name);
                    Error::GeneralError(err_msg)
                })?;

                tokio::spawn(connection);

                handle
                    .link()
                    .set(ifindex)
                    .up()
                    .execute()
                    .await
                    .map_err(|err| {
                        let err_msg = format!(
                            "Unable to change Router '{}' interface '{}' state to up. Netlink Error: {:?}",
                            router_name, iface_name, err
                        );
                        Error::GeneralError(err_msg)
                    })?;
                Ok(())
            })
            .await;
        result?
    }

    /// adds the addresses of the said router as
    /// per the topology yaml file.
    ///
    /// Example:
    /// ```yaml
    ///
    /// rt2:
    ///   plugin: holo
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
    pub async fn add_iface_addresses(&self) -> Result<()> {
        let interfaces = self.interfaces.clone();
        let result = self
            .run(move || async move {
                let (connection, handle, _) = new_connection().unwrap();
                tokio::spawn(connection);
                for iface in interfaces {
                    iface.add_addresses(&handle).await?;
                }
                Ok(())
            })
            .await;
        result?
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
    pub fn from_yaml_config(switch_name: &str, switch_config: &Hash) -> Result<Self> {
        let mut switch = Self::new(switch_name);

        if let Some(Yaml::Hash(interfaces_config)) =
            switch_config.get(&Yaml::String(String::from("interfaces")))
        {
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    let interface = Interface::from_yaml_config(iface_name, iface_config)?;
                    switch.interfaces.push(interface);
                } else {
                    return Err(Error::IncorrectYamlType(String::from(
                        "interfaces['value']",
                    )));
                }
            }
        } else {
            return Err(Error::IncorrectYamlType(String::from("interfaces")));
        }
        Ok(switch)
    }

    /// Initializes a network bridge representing the switch.
    pub async fn power_on(&mut self, handle: &Handle) -> Result<()> {
        let name = self.name.as_str();
        let mut request = handle.link().add().bridge(name.into());
        request.message_mut().header.flags.push(LinkFlag::Up);
        request.message_mut().header.flags.push(LinkFlag::Multicast);
        if let Err(err) = request.execute().await {
            let e = format!("problem creating bridge {}\n {:#?}", &self.name, err);
            let io_err = IoError::new(ErrorKind::Other, e.as_str());
            return Err(Error::IoError(io_err));
        }

        let ifindex = if_nametoindex(name).map_err(|_| {
            let err_msg = format!("interface {} not found", name);
            Error::GeneralError(err_msg)
        })?;

        self.ifindex = Some(ifindex);
        Ok(())
    }

    /// changes the admin state of the interface to up
    pub async fn up(&mut self, handle: &Handle) -> Result<()> {
        if let Some(ifindex) = self.ifindex {
            handle
                .link()
                .set(ifindex)
                .up()
                .execute()
                .await
                .map_err(|err| {
                    let err_msg = format!(
                        "Unable to change '{}' admin state to up.\n Netlink Error: {:?}",
                        self.name, err
                    );
                    Error::GeneralError(err_msg)
                })?;
        }
        Ok(())
    }
}

// ==== Node ====

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Router(Router),
    Switch(Switch),
}

impl Node {
    pub async fn power_on(&mut self, handle: &Handle) -> Result<()> {
        match self {
            Self::Router(router) => router.power_on().await,
            Self::Switch(switch) => switch.power_on(handle).await,
        }
    }
}

// ==== Link ====

#[derive(Debug, Clone)]
pub struct Link {
    pub src_name: String,
    pub src_iface: String,
    pub dst_name: String,
    pub dst_iface: String,
}

impl Link {
    pub fn src(&self) -> String {
        format!("{}:{}", self.src_name, self.src_iface)
    }

    pub fn dst(&self) -> String {
        format!("{}:{}", self.dst_name, self.dst_iface)
    }
}
