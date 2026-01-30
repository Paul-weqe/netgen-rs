use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Error as IoError, Read};
use std::os::fd::AsRawFd;

use nix::net::if_::if_nametoindex;
use rand::Rng;
use rand::distributions::Alphanumeric;
use rtnetlink::{LinkUnspec, LinkVeth, new_connection};
use tokio;
use tokio::runtime::Runtime;
use tracing::{debug, debug_span, error};
use yaml_rust2::YamlLoader;
use yaml_rust2::yaml::Yaml;

use crate::Result;
use crate::devices::{Link, Node, Router, Switch};
use crate::error::Error;

#[derive(Debug)]
pub struct Topology {
    // String holds the nodename(),
    // Node holds the node object.
    nodes: BTreeMap<String, Node>,
    links: Vec<Link>,
    runtime: Runtime,
}

impl Topology {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            links: vec![],
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    pub fn from_yaml_file(file: &mut File) -> Result<Self> {
        let mut contents = String::new();
        let _ = file.read_to_string(&mut contents);
        Self::from_yaml_str(contents.as_str())
    }

    pub fn from_yaml_str(yaml_str: &str) -> Result<Self> {
        let mut topology = Self::new();
        let yaml_content =
            YamlLoader::load_from_str(yaml_str).map_err(Error::ScanError)?;
        for yaml_group in yaml_content {
            // TODO: handle the unwrap() below
            topology.parse_topology_config(&yaml_group).unwrap();
        }
        Ok(topology)
    }

    pub fn parse_topology_config(&mut self, yaml_data: &Yaml) -> Result<()> {
        if let Yaml::Hash(topo_config_group) = yaml_data {
            // fetch the routers
            if let Some(routers_configs) =
                topo_config_group.get(&Yaml::String(String::from("routers")))
                && let Ok(routers) = self.parse_router_configs(routers_configs)
            {
                for router in routers {
                    // check if router exists
                    if self.nodes.contains_key(&router.name) {
                        let err = format!(
                            "Node {} has been configured more than once",
                            router.name
                        );
                        let io_err = IoError::other(err.as_str());
                        return Err(Error::IoError(io_err));
                    }
                    self.nodes
                        .insert(router.name.clone(), Node::Router(router));
                }
            } else {
                // TODO: handle errors thrown when fetching the routers.
            }

            // fetch the switches
            if let Some(switches_configs) =
                topo_config_group.get(&Yaml::String(String::from("switches")))
            {
                if let Ok(switches) =
                    self.parse_switch_configs(switches_configs)
                {
                    for switch in switches {
                        if self.nodes.contains_key(&switch.name) {
                            let err = format!(
                                "Node {} has been configured more than once",
                                switch.name
                            );
                            let io_err = IoError::other(err.as_str());
                            return Err(Error::IoError(io_err));
                        }
                        self.nodes
                            .insert(switch.name.clone(), Node::Switch(switch));
                    }
                } else {
                    // TODO: handle errors thrown when fetching the switches
                }
            }

            // fetch the links
            if let Some(links_configs) =
                topo_config_group.get(&Yaml::String(String::from("links")))
                && let Ok(links) = self.parse_links_configs(links_configs)
            {
                for link in links {
                    // check if link devices exist in config
                    if !self.nodes.contains_key(&link.src_device) {
                        let err = format!(
                            "src node name {} configured in {:?} does not exist",
                            link.src_device, link
                        );
                        let io_err = IoError::other(err.as_str());
                        return Err(Error::IoError(io_err));
                    }
                    if !self.nodes.contains_key(&link.dst_device) {
                        let err = format!(
                            "dst node name {} configured in {:?} does not exist",
                            link.dst_device, link
                        );
                        let io_err = IoError::other(err.as_str());
                        return Err(Error::IoError(io_err));
                    }

                    // check if link has already been configured.
                    if self.link_exists(&link) {
                        let err = format!(
                            "link {} <-> {} has been configured more than once",
                            link.src(),
                            link.dst()
                        );
                        let io_err = IoError::other(err.as_str());
                        return Err(Error::IoError(io_err));
                    }
                    self.links.push(link);
                }
            }
        }
        Ok(())
    }

    pub fn parse_router_configs(
        &self,
        routers_config: &Yaml,
    ) -> Result<Vec<Router>> {
        let mut routers: Vec<Router> = vec![];
        if let Yaml::Hash(configs) = routers_config {
            for (router_name, router_config) in configs {
                if let Yaml::String(router_name) = router_name
                    && let Yaml::Hash(router_config) = router_config
                {
                    if let Ok(router) =
                        Router::from_yaml_config(router_name, router_config)
                    {
                        routers.push(router);
                    }
                } else {
                    return Err(Error::IncorrectYamlType(format!(
                        "{router_name:?}",
                    )));
                }
            }
        }
        Ok(routers)
    }

    pub fn parse_switch_configs(
        &self,
        switches_configs: &Yaml,
    ) -> Result<Vec<Switch>> {
        let mut switches: Vec<Switch> = vec![];
        if let Yaml::Hash(configs) = switches_configs {
            for (switch_name, switch_config) in configs {
                if let Yaml::String(switch_name) = switch_name
                    && let Yaml::Hash(switch_config) = switch_config
                {
                    if let Ok(switch) =
                        Switch::from_yaml_config(switch_name, switch_config)
                    {
                        switches.push(switch);
                    } else {
                        // TODO: handle a case where switch_name is not a string
                        // or the switch config is not a Yaml::Hash
                    }
                }
            }
        }
        Ok(switches)
    }

    pub fn parse_links_configs(
        &self,
        links_configs: &Yaml,
    ) -> Result<Vec<Link>> {
        let mut links: Vec<Link> = vec![];
        if let Yaml::Array(configs) = links_configs {
            for link_config in configs {
                if let Yaml::Hash(link_config) = link_config {
                    if let Some(Yaml::String(src)) = link_config
                        .get(&Yaml::String(String::from("src-device")))
                        && let Some(Yaml::String(src_iface)) = link_config
                            .get(&Yaml::String(String::from("src-iface")))
                        && let Some(Yaml::String(dst)) = link_config
                            .get(&Yaml::String(String::from("dst-device")))
                        && let Some(Yaml::String(dst_iface)) = link_config
                            .get(&Yaml::String(String::from("dst-iface")))
                    {
                        let link = Link {
                            src_device: src.to_string(),
                            src_iface: src_iface.to_string(),
                            dst_device: dst.to_string(),
                            dst_iface: dst_iface.to_string(),
                        };
                        links.push(link);
                    } else {
                        // TODO: throw error when either of the link configs is off
                    }
                }
            }
        }
        Ok(links)
    }

    pub fn link_exists(&self, link: &Link) -> bool {
        for link2 in &self.links {
            if (link.src() == link2.src()) && (link.dst() == link2.dst())
                || ((link.src() == link2.dst()) && (link.dst() == link2.src()))
            {
                return true;
            }
        }
        false
    }

    /// "Powers on" all the devices in the network.
    ///
    /// For switches, this is reating a bridged interface
    ///     and making sure it's administrative state is "up"
    ///
    /// For routers, this is creating a new namespace and making sure
    ///     the relevant interfaces are brought up.
    pub fn power_on(&mut self) -> Result<()> {
        let power_on_span = debug_span!("net-init");
        let _span_guard = power_on_span.enter();

        // powers on all the nodes
        for (_, node) in self.nodes.iter_mut() {
            //node.power_on(&runtime)?;
            node.power_on(&self.runtime)?;
        }

        // sets up all the links
        self.setup_links()?;
        Ok(())
    }

    /// "Powers off" all the devices in the network.
    ///
    /// For switches, this means deleting the bridged interface
    ///
    /// For routers, this is deleting the respective namespaces
    pub fn power_off(&mut self) {
        let power_off_span = debug_span!("net-stop");
        let _span_guard = power_off_span.enter();
        // Powers off all the nodes
        for (_, node) in self.nodes.iter_mut() {
            node.power_off();
        }
    }

    pub fn setup_links(&self) -> Result<()> {
        // create links.
        for link in &self.links {
            self.create_link(&link)?;
        }

        // Add addresss for links in the router nodes.
        for (_, node) in self.nodes.iter() {
            if let Node::Router(router) = node {
                let _ = router.add_iface_addresses(&self.runtime);
            }
        }
        Ok(())
    }

    /// creates a link between two nodes.
    pub fn create_link(&self, link: &Link) -> Result<()> {
        let runtime = &self.runtime;
        let src_iface = format!("{}:{}", link.src_device, link.src_iface);
        let dst_iface = format!("{}:{}", link.dst_device, link.dst_iface);
        let link_span = debug_span!("link-setup", %src_iface, %dst_iface);
        let _span_guard = link_span.enter();
        debug!("setting up");

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
            let (connection, handle, _) = new_connection().unwrap();
            tokio::spawn(connection);
            let request = handle.link().add(
                LinkVeth::new(node1_link.as_str(), node2_link.as_str()).build(),
            );
            //request.message_mut().header.flags.push(LinkFlag::Up);
            request.execute().await.unwrap();
        });
        if let Some(src_node) = self.nodes.get(&link.src_device)
            && let Some(dst_node) = self.nodes.get(&link.dst_device)
        {
            // attaches the links to their respective nodes
            self.attach_link(src_node, node1_link, link.src_iface.clone())?;
            self.attach_link(dst_node, node2_link, link.dst_iface.clone())?;
        }
        debug!("setup complete");

        Ok(())
    }

    fn attach_link(
        &self,
        node: &Node,
        current_link_name: String,
        new_link_name: String,
    ) -> Result<()> {
        let runtime = &self.runtime;
        runtime.block_on(async {
            let (connection, handle, _) = new_connection().unwrap();
            tokio::spawn(connection);
            match node {
                Node::Router(router) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(file_path) = &router.file_path
                        && let Ok(file) = File::open(file_path)
                    {
                        let message = LinkUnspec::new_with_index(index)
                            .setns_by_fd(file.as_raw_fd())
                            .build();
                        // move router device to said namespace
                        handle.link().set(message).execute().await.unwrap();

                        // rename the interface to it's proper name
                        router
                            .in_ns(move || async move {
                                let (conn, handle, _) =
                                    new_connection().unwrap();
                                tokio::spawn(conn);

                                let message = LinkUnspec::new_with_index(index)
                                    .name(new_link_name)
                                    .up()
                                    .build();
                                // Bring the interface up and give it the proper
                                // name.
                                if let Err(err) =
                                    handle.link().set(message).execute().await
                                {
                                    error!(error = %err, "error coming up");
                                }
                            })
                            .await
                            .unwrap();
                    }
                }
                Node::Switch(switch) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(ifindex) = switch.ifindex
                    {
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

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}
