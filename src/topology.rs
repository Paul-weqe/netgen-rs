use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;

use tokio;
use tokio::runtime::Runtime;
use tracing::debug_span;
use yaml_rust2::YamlLoader;
use yaml_rust2::yaml::{Hash, Yaml};

use crate::NetResult;
use crate::devices::{FromYamlConfig, Link, LinkManager, Node, Router, Switch};
use crate::error::{ConfigError, NetError};

// struct TopologyParser ====

pub struct TopologyParser;

impl TopologyParser {
    pub fn from_yaml_file(file: &mut File) -> NetResult<Topology> {
        let mut contents = String::new();
        let _ = file.read_to_string(&mut contents);
        Self::from_yaml_str(contents.as_str())
    }

    pub fn from_yaml_str(yaml_str: &str) -> NetResult<Topology> {
        let mut topology = Topology::new()?;
        let yaml_content =
            YamlLoader::load_from_str(yaml_str).map_err(|err| {
                NetError::ConfigError(ConfigError::YamlSyntax(err))
            })?;

        for yaml_group in yaml_content {
            Self::parse_topology_config(&yaml_group, &mut topology)?;
        }
        Ok(topology)
    }

    pub fn parse_topology_config(
        yaml_data: &Yaml,
        topology: &mut Topology,
    ) -> NetResult<()> {
        if let Yaml::Hash(topo_config_group) = yaml_data {
            // Fetch the routers.
            if let Some(routers_configs) =
                topo_config_group.get(&Yaml::String(String::from("routers")))
            {
                let routers = Self::parse_router_configs(routers_configs)?;
                for router in routers {
                    // Check if router exists.
                    if topology.nodes.contains_key(&router.name) {
                        return Err(
                            ConfigError::DuplicateNode(router.name).into()
                        );
                    }
                    topology
                        .nodes
                        .insert(router.name.clone(), Node::Router(router));
                }
            }

            // Fetch switches.
            if let Some(switches_configs) =
                topo_config_group.get(&Yaml::String(String::from("switches")))
            {
                let switches = Self::parse_switch_configs(switches_configs)?;
                for switch in switches {
                    if topology.nodes.contains_key(&switch.name) {
                        return Err(
                            ConfigError::DuplicateNode(switch.name).into()
                        );
                    }
                    topology
                        .nodes
                        .insert(switch.name.clone(), Node::Switch(switch));
                }
            }

            // Fetch the links
            if let Some(links_configs) =
                topo_config_group.get(&Yaml::String(String::from("links")))
            {
                let yaml_links = Self::parse_links_configs(links_configs)?;

                for link in yaml_links {
                    if !topology.nodes.contains_key(&link.src_device) {
                        return Err(
                            ConfigError::UnknownNode(link.src_device).into()
                        );
                    }

                    if !topology.nodes.contains_key(&link.dst_device) {
                        return Err(
                            ConfigError::UnknownNode(link.dst_device).into()
                        );
                    }

                    // Check if link has already been added to the links vector.
                    for link2 in topology.links.as_slice() {
                        if (link.src() == link2.src())
                            && (link.dst() == link2.dst())
                            || ((link.src() == link2.dst())
                                && (link.dst() == link2.src()))
                        {
                            // Link exists.
                            return Err(ConfigError::DuplicateLink {
                                src: link.src(),
                                dst: link.dst(),
                            }
                            .into());
                        }
                    }
                    topology.links.push(link);
                }
            }
        }
        Ok(())
    }

    pub fn parse_router_configs(
        routers_config: &Yaml,
    ) -> NetResult<Vec<Router>> {
        let mut routers: Vec<Router> = vec![];

        match routers_config {
            Yaml::Hash(configs) => {
                for (router_name, router_config) in configs {
                    let router_name = match router_name {
                        Yaml::String(name) => name,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: "routers.router[name??]".to_string(),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    };

                    let router_config = match router_config {
                        Yaml::Hash(router_config) => router_config,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: "routers.router[router_name][config??]"
                                    .to_string(),
                                expected: "hash".to_string(),
                            }
                            .into());
                        }
                    };

                    let router =
                        Router::from_yaml_config(router_name, router_config)?;
                    routers.push(router);
                }
                Ok(routers)
            }
            _ => Err(ConfigError::IncorrectType {
                field: "routers[config??]".to_string(),
                expected: "hash".to_string(),
            }
            .into()),
        }
    }

    pub fn parse_switch_configs(
        switches_configs: &Yaml,
    ) -> NetResult<Vec<Switch>> {
        let mut switches: Vec<Switch> = vec![];
        match switches_configs {
            Yaml::Hash(configs) => {
                for (switch_name, switch_config) in configs {
                    let switch_name = match switch_name {
                        Yaml::String(name) => name,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: "switches.switch[name??]".to_string(),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    };

                    let switch_config = match switch_config {
                        Yaml::Hash(switch_config) => switch_config,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                field: format!(
                                    "switches.switch[{switch_name}][config??]"
                                ),
                                expected: "hash".to_string(),
                            }
                            .into());
                        }
                    };

                    let switch =
                        Switch::from_yaml_config(switch_name, switch_config)?;
                    switches.push(switch);
                }
            }
            _ => {
                return Err(ConfigError::IncorrectType {
                    field: "switches[config??]".to_string(),
                    expected: "hash".to_string(),
                }
                .into());
            }
        }
        Ok(switches)
    }

    pub fn parse_links_configs(links_configs: &Yaml) -> NetResult<Vec<Link>> {
        let mut links: Vec<Link> = vec![];
        if let Yaml::Array(configs) = links_configs {
            for link_config in configs {
                if let Yaml::Hash(link_config) = link_config {
                    let link = Link {
                        src_device: Self::get_string_field(
                            link_config,
                            "src-device",
                        )?,
                        src_iface: Self::get_string_field(
                            link_config,
                            "src-iface",
                        )?,
                        dst_device: Self::get_string_field(
                            link_config,
                            "dst-device",
                        )?,
                        dst_iface: Self::get_string_field(
                            link_config,
                            "dst-iface",
                        )?,
                    };
                    links.push(link);
                }
            }
        } else {
            return Err(ConfigError::IncorrectType {
                field: "links[config??]".to_string(),
                expected: "array".to_string(),
            }
            .into());
        }
        Ok(links)
    }

    // Get field value from Yaml list.
    fn get_string_field(config: &Hash, field: &str) -> NetResult<String> {
        let field_value = config
            .get(&Yaml::String(field.to_string()))
            .ok_or_else(|| ConfigError::MissingField(field.to_string()))?;

        match field_value {
            Yaml::String(value) => Ok(value.to_string()),
            _ => Err(ConfigError::IncorrectType {
                field: field.to_string(),
                expected: "string".to_string(),
            }
            .into()),
        }
    }
}

// ==== struct Topology ====

#[derive(Debug)]
pub struct Topology {
    // String holds the nodename(),
    // Node holds the node object.
    nodes: BTreeMap<String, Node>,
    links: Vec<Link>,
    runtime: Runtime,
}

impl Topology {
    pub fn new() -> NetResult<Self> {
        Ok(Self {
            nodes: BTreeMap::new(),
            links: vec![],
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    NetError::BasicError(format!(
                        "Failed to create tokio runtime: {err:?}"
                    ))
                })?,
        })
    }

    /// We power on the switches.
    ///
    /// This is by creating a bridged device and making sure its administrative
    ///     state is "up".This is done in the main namespace.
    pub fn power_switches_on(&mut self) -> NetResult<()> {
        let power_on_span = debug_span!("switch-power-on");
        let _span_guard = power_on_span.enter();

        for node in self.nodes.values_mut() {
            if let Node::Switch(switch) = node {
                switch.power_on(&self.runtime)?;
            }
        }

        Ok(())
    }

    /// Powers on Routers.
    ///
    /// This is done by creating a new namespace. The adding of the relevant
    ///     interfaces is done elsewhere .
    pub fn power_routers_on(&mut self) -> NetResult<()> {
        let power_on_span = debug_span!("router-power-on");
        let _span_guard = power_on_span.enter();

        for node in self.nodes.values_mut() {
            if let Node::Router(router) = node {
                router.power_on()?;
            }
        }
        Ok(())
    }

    /// "Powers off" all the devices in the network.
    ///
    /// For switches, this means deleting the bridged interface
    ///
    /// For routers, this is deleting the respective namespaces
    pub fn power_off(&mut self) -> NetResult<()> {
        let power_off_span = debug_span!("net-stop");
        let _span_guard = power_off_span.enter();
        // Powers off all the nodes
        for node in self.nodes.values_mut() {
            node.power_off()?;
        }
        Ok(())
    }

    pub fn setup_links(&self) -> NetResult<()> {
        let link_manager = LinkManager {};
        link_manager.setup_all(
            &self.runtime,
            &self.nodes,
            self.links.as_slice(),
        )
    }
}
