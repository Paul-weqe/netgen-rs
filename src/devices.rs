use std::fs::File;
use std::future::Future;
use std::os::fd::AsFd;

use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use nix::mount::umount;
use nix::net::if_::if_nametoindex;
use nix::sched::{CloneFlags, setns};
use nix::unistd::Pid;
use rtnetlink::{Handle, LinkBridge, new_connection};
use tokio::runtime::Runtime;
use tracing::{debug, error};
use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::error::Error;
use crate::{DEVICES_NS_DIR, NS_DIR, Result, mount_device};

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
            let request =
                handle.address().add(ifindex, addr.ip(), addr.prefix());
            request.execute().await.map_err(|_| {
                let err_msg = format!("Unable to add address {addr}");
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
        if let Some(ipv4_addresses) =
            yaml_config.get(&Yaml::String(String::from("ipv4")))
        {
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
        if let Some(ipv6_addresses) =
            yaml_config.get(&Yaml::String(String::from("ipv6")))
        {
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

// ==== Node ====

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Router(Router),
    Switch(Switch),
}

impl Node {
    pub fn power_on(&mut self, runtime: &Runtime) -> Result<()> {
        match self {
            Self::Router(router) => router.power_on(),
            Self::Switch(switch) => switch.power_on(runtime),
        }
    }

    pub fn power_off(&mut self) {
        match self {
            Self::Router(router) => router.power_off(),
            Self::Switch(switch) => switch.power_off(),
        }
    }
}

// ==== Router =====
#[derive(Debug, Clone)]
pub struct Router {
    pub name: String,
    pub file_path: Option<String>,
    pub interfaces: Vec<Interface>,
    pub pid: Option<Pid>,

    // This will be run when the startup is run
    pub startup_config: Option<String>,
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
            startup_config: None,
        }
    }

    pub fn from_yaml_config(name: &str, router_config: &Hash) -> Result<Self> {
        let mut router = Self::new(name);

        // == Interface configs ==
        if let Some(Yaml::Hash(interfaces_config)) =
            router_config.get(&Yaml::String(String::from("interfaces")))
        {
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    if let Ok(interface) =
                        Interface::from_yaml_config(iface_name, iface_config)
                    {
                        router.interfaces.push(interface);
                    }
                } else {
                    return Err(Error::GeneralError(String::from(
                        "Interface content for 'interfaces' not a dictionary",
                    )));
                }
            }
        }

        // Get the startup config
        if let Some(startup_config) =
            router_config.get(&Yaml::String(String::from("startup-config")))
        {
            if let Yaml::String(startup_config) = startup_config {
                router.startup_config = Some(startup_config.to_string());
            } else {
                return Err(Error::IncorrectYamlType(String::from(
                    "startup-config",
                )));
            }
        }
        Ok(router)
    }

    /// Creates a namespace representing the router
    /// and turns on the loopback interface.
    pub fn power_on(&mut self) -> Result<()> {
        let file_path = mount_device(Some(self.name.clone()), Pid::this())?;
        self.file_path = Some(file_path);
        debug!(router=%self.name, "powered on");
        Ok(())
    }

    /// Deletes the namespace created by the Router (if it exists)
    pub fn power_off(&mut self) {
        // create the file that will be hooked to the router's namespace.
        let ns_path = format!("{DEVICES_NS_DIR}/{}", self.name);

        if let Err(err) = umount(ns_path.as_str()) {
            error!(router = %self.name, error = %err,"issue unmounting namespace");
        }

        // Remove the files.
        if let Err(err) = std::fs::remove_file(ns_path.as_str()) {
            error!(router = %self.name, error = %err,"issue removing namespace file");
        } else {
            debug!(router = %self.name, "deleted");
        }
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
    ///     let router = Router::new("r1").await.unwrap();
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
    pub async fn in_ns<Fut, T, R>(&self, f: Fut) -> Result<R>
    where
        Fut: FnOnce() -> T + Send + 'static,
        T: Future<Output = R> + Send,
    {
        match &self.file_path {
            Some(file_path) => {
                // Move into the Router namespace.
                let ns_file = File::open(file_path.as_str()).unwrap();

                setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(
                    |err| {
                        let err = format!(
                            "Unable to enter into {} namespace {:#?}",
                            self.name, err
                        );
                        Error::GeneralError(err)
                    },
                )?;

                let result = (f)().await;

                // Go back to the main namespace.
                let main_namespace_path = format!("{NS_DIR}/main");

                if let Ok(main_file) = File::open(main_namespace_path.as_str())
                    && let Ok(_) =
                        setns(main_file.as_fd(), CloneFlags::CLONE_NEWNET)
                {
                    Ok(result)
                } else {
                    Err(Error::GeneralError(
                        "unable to move back to main namespace".to_string(),
                    ))
                }
            }
            None => Err(Error::GeneralError(format!(
                "namespace fd {:?} not found",
                self.file_path
            ))),
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
    pub fn add_iface_addresses(&self, runtime: &Runtime) -> Result<()> {
        let interfaces = self.interfaces.clone();

        runtime.block_on(async {
            self.in_ns(move || async move {
                let (connection, handle, _) = new_connection().unwrap();
                tokio::spawn(connection);
                for iface in interfaces {
                    iface.add_addresses(&handle).await?;
                }
                Ok(())
            })
            .await?
        })
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
    pub fn from_yaml_config(
        switch_name: &str,
        switch_config: &Hash,
    ) -> Result<Self> {
        let mut switch = Self::new(switch_name);

        if let Some(Yaml::Hash(interfaces_config)) =
            switch_config.get(&Yaml::String(String::from("interfaces")))
        {
            for (iface_name, iface_config) in interfaces_config {
                if let Yaml::String(iface_name) = iface_name
                    && let Yaml::Hash(iface_config) = iface_config
                {
                    let interface =
                        Interface::from_yaml_config(iface_name, iface_config)?;
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
    pub fn power_on(&mut self, runtime: &Runtime) -> Result<()> {
        let name = self.name.as_str();

        runtime.block_on(async {
            let (connection, handle, _) = new_connection().unwrap();
            tokio::spawn(connection);

            let message = LinkBridge::new(name).up().build();
            let request = handle.link().add(message);

            request.execute().await.map_err(|e| {
                Error::GeneralError(format!(
                    "Failed to create bridge {name}: {e}",
                ))
            })?;

            if let Ok(ifindex) = if_nametoindex(name) {
                self.ifindex = Some(ifindex as u32);
                debug!(switch = %self.name, "powered on");
            }

            Ok(())
        })
    }

    /// Switch does not run in dedicated namespace thus no need to unmount.
    pub fn power_off(&mut self) {
        // Powering off...I guess.
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
