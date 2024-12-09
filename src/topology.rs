use crate::devices::{Link, Node, Router, Switch};
use crate::plugins::Config;

use netlink_packet_route::link::LinkFlag;
use nix::net::if_::if_nametoindex;
use rtnetlink::{new_connection, Handle};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::os::fd::AsRawFd;
use tokio;
use yaml_rust2::yaml::Yaml;
use yaml_rust2::YamlLoader;

#[derive(Debug)]
pub struct Topology {
    handle: Handle,
    // String holds the nodename(),
    // Node holds the node object.
    nodes: BTreeMap<String, Node>,
    links: Vec<Link>,
}

impl Topology {
    pub fn new() -> Self {
        let (connection, handle, _) = new_connection().unwrap();
        tokio::spawn(connection);
        Self {
            handle,
            nodes: BTreeMap::new(),
            links: vec![],
        }
    }

    pub fn from_yaml_file(file: &mut File, config: Option<Config>) -> IoResult<Self> {
        let mut contents = String::new();
        let _ = file.read_to_string(&mut contents);
        Self::from_yaml_str(contents.as_str(), config)
    }

    pub fn from_yaml_str(yaml_str: &str, config: Option<Config>) -> IoResult<Self> {
        let (connection, handle, _) = new_connection().unwrap();
        tokio::spawn(connection);
        let mut topology = Self {
            handle,
            nodes: BTreeMap::new(),
            links: vec![],
        };

        // TODO: handle the unwrap() below
        let yaml_content = YamlLoader::load_from_str(yaml_str).unwrap();
        for yaml_group in yaml_content {
            topology.parse_topology_config(&yaml_group, &config)?;
        }
        Ok(topology)
    }

    pub fn parse_topology_config(
        &mut self,
        yaml_data: &Yaml,
        config: &Option<Config>,
    ) -> IoResult<()> {
        if let Yaml::Hash(topo_config_group) = yaml_data {
            // fetch the routers
            if let Some(routers_configs) =
                topo_config_group.get(&Yaml::String(String::from("routers")))
            {
                // TODO: handle the unwrap below
                if let Ok(routers) = self.parse_router_configs(routers_configs, config) {
                    for router in routers {
                        // check if router exists
                        if self.nodes.contains_key(&router.name) {
                            let err =
                                format!("Node {} has been configured more than once", router.name);
                            return Err(IoError::new(ErrorKind::Other, err.as_str()));
                        }
                        self.nodes.insert(router.name.clone(), Node::Router(router));
                    }
                } else {
                    // TODO: handle errors thrown when fetching the routers.
                }
            }

            // fetch the switches
            if let Some(switches_configs) =
                topo_config_group.get(&Yaml::String(String::from("switches")))
            {
                // TODO: handle the unwrap below
                if let Ok(switches) = self.parse_switch_configs(switches_configs) {
                    for switch in switches {
                        if self.nodes.contains_key(&switch.name) {
                            let err =
                                format!("Node {} has been configured more than once", switch.name);
                            return Err(IoError::new(ErrorKind::Other, err.as_str()));
                        }
                        self.nodes.insert(switch.name.clone(), Node::Switch(switch));
                    }
                } else {
                    // TODO: handle errors thrown when fetching the switches
                }
            }

            // fetch the links
            if let Some(links_configs) = topo_config_group.get(&Yaml::String(String::from("links")))
            {
                if let Ok(links) = self.parse_links_configs(links_configs) {
                    for link in links {
                        // check if link devices exist in config
                        if !self.nodes.contains_key(&link.src_name) {
                            let err = format!(
                                "src node name {} configured in link {:?} does not exist",
                                link.src_name, link
                            );
                            return Err(IoError::new(ErrorKind::Other, err.as_str()));
                        }
                        if !self.nodes.contains_key(&link.dst_name) {
                            let err = format!(
                                "src node name {} configured in link {:?} does not exist",
                                link.src_name, link
                            );
                            return Err(IoError::new(ErrorKind::Other, err.as_str()));
                        }

                        // check if link has already been configured.
                        if self.link_exists(&link) {
                            let err = format!(
                                "link {} <-> {} has been configured more than once",
                                link.src(),
                                link.dst()
                            );
                            return Err(IoError::new(ErrorKind::Other, err.as_str()));
                        }
                        self.links.push(link);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn parse_router_configs(
        &self,
        routers_config: &Yaml,
        config: &Option<Config>,
    ) -> IoResult<Vec<Router>> {
        let mut routers: Vec<Router> = vec![];
        if let Yaml::Hash(configs) = routers_config {
            for (router_name, router_config) in configs {
                if let Yaml::String(router_name) = router_name
                    && let Yaml::Hash(router_config) = router_config
                {
                    if let Ok(router) = Router::from_yaml_config(router_name, router_config, config)
                    {
                        routers.push(router);
                    }
                } else {
                    // TODO: handle a case where the router_name is not a string
                    // or the router_config is not a Yaml::Hash
                }
            }
        }
        Ok(routers)
    }

    pub fn parse_switch_configs(&self, switches_configs: &Yaml) -> IoResult<Vec<Switch>> {
        let mut switches: Vec<Switch> = vec![];
        if let Yaml::Hash(configs) = switches_configs {
            for (switch_name, switch_config) in configs {
                if let Yaml::String(switch_name) = switch_name
                    && let Yaml::Hash(switch_config) = switch_config
                {
                    if let Ok(switch) = Switch::from_yaml_config(switch_name, switch_config) {
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

    pub fn parse_links_configs(&self, links_configs: &Yaml) -> IoResult<Vec<Link>> {
        let mut links: Vec<Link> = vec![];
        if let Yaml::Array(configs) = links_configs {
            for link_config in configs {
                if let Yaml::Hash(link_config) = link_config {
                    if let Some(Yaml::String(src)) =
                        link_config.get(&Yaml::String(String::from("src")))
                        && let Some(Yaml::String(src_iface)) =
                            link_config.get(&Yaml::String(String::from("src-iface")))
                        && let Some(Yaml::String(dst)) =
                            link_config.get(&Yaml::String(String::from("dst")))
                        && let Some(Yaml::String(dst_iface)) =
                            link_config.get(&Yaml::String(String::from("dst-iface")))
                    {
                        let link = Link {
                            src_name: src.to_string(),
                            src_iface: src_iface.to_string(),
                            dst_name: dst.to_string(),
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

    /// Given a list of router names and need to create a new namespace
    /// for each one of them.
    ///
    /// We will be giving each router an id after creating it's
    /// namespace and populating the router's path.
    ///
    /// use of BTreeSet is to ensure each of the strings is unique
    pub async fn add_routers(&mut self, routers: BTreeSet<&str>) {
        for name in routers {
            let router = Router::new(name);
            self.nodes.insert(name.to_string(), Node::Router(router));
        }
    }

    /// Cretes the beidges for the respective switches.  
    pub async fn add_switches(&mut self, switches: BTreeSet<&str>) {
        for name in switches {
            let switch = Switch::new(name);
            self.nodes.insert(name.to_string(), Node::Switch(switch));
        }
    }

    /// Tells you whether a node of a specific name exists
    /// If it exists, it lets you know what the type of the node is
    /// Creates a link that will be used between two nodes:
    ///
    /// src_name |===============================| dst_name
    ///
    /// Note that the src_iface and dst_iface should not have the same name
    pub async fn add_link(
        &self,
        src_node: &str,
        src_iface: &str,
        dst_node: &str,
        dst_iface: &str,
    ) -> IoResult<Link> {
        // create the link
        let mut request = self
            .handle
            .link()
            .add()
            .veth(src_iface.into(), dst_iface.into());
        request.message_mut().header.flags.push(LinkFlag::Up);

        if let Err(err) = request.execute().await {
            let e = format!(
                "problem creating veth link [{src_node}:{src_iface} - {dst_node}:{dst_iface}]\n {:#?}",
                err
            );
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }

        if let Some(node_1) = self.nodes.get(src_node)
            && let Some(node_2) = self.nodes.get(dst_node)
        {
            match node_1 {
                // attaches the first link to its respective interface
                // device
                Node::Router(router) => {
                    if let Ok(index) = if_nametoindex(src_iface) {
                        if let Some(file_path) = &router.file_path {
                            let file = File::open(file_path).unwrap();
                            self.handle
                                .link()
                                .set(index)
                                .setns_by_fd(file.as_raw_fd())
                                .execute()
                                .await
                                .unwrap();
                            let _ = router.up(src_iface.to_string()).await;
                        }
                    }
                }
                Node::Switch(switch) => {
                    if let Ok(index) = if_nametoindex(src_iface)
                        && let Some(ifindex) = switch.ifindex
                    {
                        self.handle.link().set(index).up().execute().await.unwrap();
                        self.handle
                            .link()
                            .set(index)
                            .controller(ifindex)
                            .execute()
                            .await
                            .unwrap();
                        // TODO: Handle the error in case of a problem while setting the MASTER
                    }
                }
            }

            // attaches the second link to its respective interface
            // device.
            // Also, during the bringing up of the veth interface above,
            // only the src_iface changes the admin state to up. So we
            // manually have to set dst_iface state as up
            match node_2 {
                Node::Router(router) => {
                    if let Ok(index) = if_nametoindex(dst_iface) {
                        if let Some(file_path) = &router.file_path {
                            let file = File::open(file_path).unwrap();
                            self.handle
                                .link()
                                .set(index)
                                .setns_by_fd(file.as_raw_fd())
                                .execute()
                                .await
                                .unwrap();
                            let _ = router.up(dst_iface.to_string()).await;
                        }
                    }
                }
                Node::Switch(switch) => {
                    if let Ok(index) = if_nametoindex(dst_iface)
                        && let Some(ifindex) = switch.ifindex
                    {
                        // bring up the link endpoint
                        self.handle.link().set(index).up().execute().await.unwrap();
                        self.handle
                            .link()
                            .set(index)
                            .controller(ifindex)
                            .execute()
                            .await
                            .unwrap();
                        // TODO: Handle the error in case of a problem while setting the MASTER
                    }
                }
            }

            Ok(Link {
                src_name: src_node.to_string(),
                src_iface: src_iface.to_string(),
                dst_name: dst_node.to_string(),
                dst_iface: dst_iface.to_string(),
            })
        } else {
            let err = format!(
                "One of the nodes [{}] or [{}] does not exist in the topology",
                src_node, dst_node
            );
            Err(IoError::new(ErrorKind::Other, err.as_str()))
        }
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}
