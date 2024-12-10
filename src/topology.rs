use crate::devices::{Link, Node, Router, Switch};
use crate::plugins::Config;

use rtnetlink::{new_connection, Handle};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
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
                                "dst node name {} configured in link {:?} does not exist",
                                link.dst_name, link
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

    /// "Powers on" all the devices in the network.
    ///
    /// For switches, this is reating a bridged interface
    ///     and making sure it's administrative state is "up"
    ///
    /// For routers, this is creating a new namespace and making sure
    ///     the relevant interfaces are brought up.
    pub async fn power_on(&mut self) -> IoResult<()> {
        for (_, node) in self.nodes.iter_mut() {
            node.power_on(&self.handle).await?;
            // ...
        }
        Ok(())
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}
