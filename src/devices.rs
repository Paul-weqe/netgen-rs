use crate::plugins::{Holo, Plugin};
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
        let router = Self {
            name: name.to_string(),
            file_path: None,
            plugin: None,
            interfaces: vec![],
            startup_config: None,
        };

        router
    }

    pub fn new_from_yaml_config(name: &str, router_config: &Hash) -> IoResult<Self> {
        let mut router = Self::new(name);

        // == plugin configs ==
        if let Some(plugin_config) = router_config.get(&Yaml::String(String::from("plugin"))) {
            if let Yaml::String(plugin_name) = plugin_config {
                match plugin_name.as_str() {
                    "holo" => {
                        router.plugin = Some(Plugin::Holo(Holo::default()));
                    }
                    _ => {
                        // TODO: handler when the plugin mentioned does not exist
                    }
                }
            } else {
                // TODO: return an error in the case the plugin config has not
                // been configured as a yaml string
            }
        }

        // == interface configs ==
        if let Some(Yaml::Hash(interfaces_config)) =
            router_config.get(&Yaml::String(String::from("interfaces")))
        {
            //
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    let mut interface = Interface {
                        name: iface_name.to_string(),
                        addresses: vec![],
                    };

                    // --- get the interface's Ipv4 addresses ---
                    if let Some(Yaml::Array(ipv4_addresses)) =
                        iface_config.get(&Yaml::String(String::from("ipv4")))
                    {
                        let mut addr_iter = ipv4_addresses.into_iter();
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
                        iface_config.get(&Yaml::String(String::from("ipv6")))
                    {
                        let mut addr_iter = ipv6_addresses.into_iter();
                        while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                            if let Ok(ip_net) = addr_str.parse::<Ipv6Network>() {
                                interface.addresses.push(IpNetwork::V6(ip_net));
                            }
                        }
                    } else {
                        // TODO: handle cases for when the user has not configured ipv4 as an array
                    }
                    router.interfaces.push(interface);
                } else {
                    // TODO: handle when iface name is not a string
                    // or iface config is not a Hash
                }
            }
        }
        Ok(router)
    }

    /// Creates a namespace and turns on the loopback interface.
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
    pub ifindex: u32,
    pub interfaces: Vec<Interface>,
}

impl Switch {
    pub async fn new(handle: &Handle, name: &str) -> IoResult<Self> {
        let mut request = handle.link().add().bridge(name.into());
        request.message_mut().header.flags.push(LinkFlag::Up);
        if let Err(err) = request.execute().await {
            let e = format!("problem creating bridge {name}\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        let ifindex = if_nametoindex(name)?;
        Ok(Self {
            name: name.to_string(),
            ifindex,
            interfaces: vec![],
        })
    }

    pub async fn up(&self, handle: &Handle) -> IoResult<()> {
        let _ = handle.link().set(self.ifindex).up().execute().await;
        // TODO: error handling for when bringing the interface up does not work.
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
    /// Brings the device up.
    ///
    /// if it is a router, we specify the interface name that is
    /// to be brought up. the handle will be created custom for
    /// the router's namespace.
    ///
    /// If it is a switch, we specify the handle, but since the switch
    /// is a bridge device and we already have the interface name, we
    /// only use that for the switch.
    pub async fn _up(&self, handle: &Handle, iface_name: String) -> IoResult<()> {
        match self {
            Self::Router(router) => router.up(iface_name).await,
            Self::Switch(switch) => switch.up(handle).await,
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
    pub fn to_vec(&self) -> Vec<String> {
        vec![
            self.src_name.clone(),
            self.src_iface.clone(),
            self.dst_name.clone(),
            self.dst_iface.clone(),
        ]
    }
}
