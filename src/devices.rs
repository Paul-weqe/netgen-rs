use crate::plugins::{Config, Holo, Plugin};

use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use netlink_packet_route::link::LinkFlag;
use nix::net::if_::if_nametoindex;
use nix::sched::{setns, CloneFlags};
use nix::unistd::gettid;
use rtnetlink::{new_connection, Handle, NetworkNamespace, NETNS_PATH};
use std::fs::File;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
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
    fn from_yaml_config(name: &str, yaml_config: &Hash) -> IoResult<Self> {
        let mut interface = Interface {
            name: name.to_string(),
            addresses: vec![],
        };

        // --- get the interface's Ipv4 addresses ---
        if let Some(Yaml::Array(ipv4_addresses)) =
            yaml_config.get(&Yaml::String(String::from("ipv4")))
        {
            let mut addr_iter = ipv4_addresses.iter();
            while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                if let Ok(ip_net) = addr_str.parse::<Ipv4Network>() {
                    interface.addresses.push(IpNetwork::V4(ip_net));
                }
            }
        } else {
            // TODO: handle cases for when the user has not configured ipv4 as an array
        }

        // --- get the interface's Ipv6 addresses ---
        if let Some(Yaml::Array(ipv6_addresses)) =
            yaml_config.get(&Yaml::String(String::from("ipv6")))
        {
            let mut addr_iter = ipv6_addresses.iter();
            while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                if let Ok(ip_net) = addr_str.parse::<Ipv6Network>() {
                    interface.addresses.push(IpNetwork::V6(ip_net));
                }
            }
        } else {
            // TODO: handle cases for when the user has not configured ipv4 as an array
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
    ) -> IoResult<Self> {
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
                _ => {
                    // TODO: handler when the plugin mentioned does not exist
                }
            }
        } else {

            // TODO: return an error in the case the plugin config has not
            // been configured as a yaml string
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
                    // TODO: handle when iface name is not a string
                    // or iface config is not a Hash
                }
            }
        }
        Ok(router)
    }

    /// Creates a namespace representing the router
    /// and turns on the loopback interface.
    pub async fn power_on(&mut self) -> IoResult<()> {
        if let Err(err) = NetworkNamespace::add(self.name.clone()).await {
            let e = format!("unable to create namespace\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        let mut ns_path = String::new();
        ns_path.push_str(NETNS_PATH);
        ns_path.push_str(self.name.as_str());
        self.file_path = Some(ns_path);

        // make sure the loopback interface of the router is up
        let _ = self.up(String::from("lo")).await;

        // add the environment variables for the necessary binaries
        Ok(())
    }

    /// Deletes the namespace created by the Router (if it exists)
    pub async fn power_off(&mut self) {
        //
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
    pub async fn run<Fut, T, R>(&self, f: Fut) -> IoResult<R>
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
    pub async fn up(&self, iface_name: String) -> IoResult<()> {
        let result = self
            .run(move || async move {
                if let Ok(ifindex) = if_nametoindex(iface_name.as_str())
                    && let Ok((connection, handle, _)) = new_connection()
                {
                    tokio::spawn(connection);
                    let _ = handle.link().set(ifindex).up().execute().await;
                }
                // TODO: add error catcher in case
                // of problems when bringing the interface up
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
    pub fn from_yaml_config(switch_name: &str, switch_config: &Hash) -> IoResult<Self> {
        let mut switch = Self::new(switch_name);

        if let Some(Yaml::Hash(interfaces_config)) =
            switch_config.get(&Yaml::String(String::from("interfaces")))
        {
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    if let Ok(interface) = Interface::from_yaml_config(iface_name, iface_config) {
                        switch.interfaces.push(interface);
                    }
                } else {
                    // TODO: handle when iface name is not a string
                    // or iface config is not a Hash
                }
            }
        }
        Ok(switch)
    }

    /// Initializes a network bridge representing the switch.
    pub async fn power_on(&mut self, handle: &Handle) -> IoResult<()> {
        let name = self.name.as_str();
        let mut request = handle.link().add().bridge(name.into());
        request.message_mut().header.flags.push(LinkFlag::Up);
        request.message_mut().header.flags.push(LinkFlag::Multicast);
        if let Err(err) = request.execute().await {
            let e = format!("problem creating bridge {}\n {:#?}", &self.name, err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        // TODO: error handling for when bringing the interface up does not work.
        let ifindex = if_nametoindex(name)?;
        self.ifindex = Some(ifindex);
        handle.link().set(ifindex).up().execute().await.unwrap();
        Ok(())
    }

    /// changes the admin state of the interface to up
    pub async fn up(&mut self, handle: &Handle) -> IoResult<()> {
        if let Some(ifindex) = self.ifindex {
            let _ = handle.link().set(ifindex).up().execute().await;
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
    pub async fn power_on(&mut self, handle: &Handle) -> IoResult<()> {
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
