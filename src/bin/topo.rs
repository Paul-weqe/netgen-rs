#![feature(let_chains)]
use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use netgen::devices::{Interface, Link, Router, Switch};
use netgen::plugins::{Holo, Plugin};
use rtnetlink::{new_connection, Handle};
use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use yaml_rust2::yaml::Yaml;
use yaml_rust2::YamlLoader;

#[tokio::main]
async fn main() {
    new_from_yaml_file("./assets/sample-top.yml").await;
}

pub async fn new_from_yaml_file(yaml_file_path: &str) {
    let mut f = File::open(yaml_file_path).unwrap();
    let mut contents = String::new();
    let _ = f.read_to_string(&mut contents);
    new_from_yaml_str(&contents).await;
}

pub async fn new_from_yaml_str(yaml_str: &str) {
    let (connection, handle, _) = new_connection().unwrap();
    tokio::spawn(connection);

    // TODO: handle the unwrap() below.
    let yaml_content = YamlLoader::load_from_str(yaml_str).unwrap();
    for x in yaml_content {
        parse_topology_config(&x, &handle).await;
    }
}

pub async fn parse_topology_config(yaml_data: &Yaml, handle: &Handle) {
    match yaml_data {
        // topo config groups can be 'routers',
        // 'switches' or 'links'
        Yaml::Hash(topo_config_group) => {
            // fetch the routers
            if let Some(routers_configs) =
                topo_config_group.get(&Yaml::String("routers".to_string()))
            {
                let routers = parse_routers_config(routers_configs).await;
            }
            // fetch the switches
            if let Some(switches_configs) =
                topo_config_group.get(&Yaml::String("switches".to_string()))
            {
                let switches = parse_switches_config(switches_configs, handle).await;
            }

            // fetch the links
            if let Some(links_configs) = topo_config_group.get(&Yaml::String("links".to_string())) {
                let links = parse_links_config(links_configs).await;
                println!("{:#?}", links);
            }
        }
        _ => {
            // TODO: handle a case where the
            // configuration on the file is not a hash.
        }
    }
}

/// Parses the configurations handled under the routers
/// group in the yaml file
/// Example below:
///
/// ```yaml
/// routers:
///   rt1:
///     plugin: holo
///     interfaces:
///       eth0:
///         ipv4:
///           - 192.168.100.10/24
///           - 192.168.20.1/24
/// ```
pub async fn parse_routers_config(routers_config: &Yaml) -> Vec<Router> {
    let mut routers: Vec<Router> = vec![];
    if let Yaml::Hash(configs) = routers_config {
        for (router_name, router_config) in configs {
            if let Yaml::Hash(router_config) = router_config {
                if let Yaml::String(name) = router_name {
                    let mut router = Router::new(name.as_str());

                    // fetch the plugin if the router has
                    // any configured
                    if let Some(plugin_config) =
                        router_config.get(&Yaml::String(String::from("plugin")))
                    {
                        if let Yaml::String(name) = plugin_config {
                            match name.as_str() {
                                "holo" => {
                                    router.plugin = Some(Plugin::Holo(Holo::default()));
                                }
                                _ => {
                                    // TODO: handler when the plugin does not exist
                                }
                            }
                        } else {
                            // TODO: handle case when user does not put
                            // plugin: as a Yaml::string
                        }
                    }

                    // fetch interfaces configs
                    if let Some(Yaml::Hash(interfaces_config)) =
                        router_config.get(&Yaml::String(String::from("interfaces")))
                    {
                        for (iface_name, iface_config) in interfaces_config {
                            if let Yaml::String(iface_name) = iface_name
                                && let Yaml::Hash(iface_config) = iface_config
                            {
                                let mut interface = Interface {
                                    name: iface_name.to_string(),
                                    addresses: vec![],
                                };

                                // get the interface's ipv4 addresses
                                if let Some(Yaml::Array(ipv4_addresses)) =
                                    iface_config.get(&Yaml::String(String::from("ipv4")))
                                {
                                    let mut addr_iter = ipv4_addresses.into_iter();
                                    while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                                        if let Ok(net) = addr_str.parse::<Ipv4Network>() {
                                            interface.addresses.push(IpNetwork::V4(net));
                                        }
                                    }
                                } else {
                                    // TODO: handle for when the ipv4 is not an array
                                }
                                // get the interface's ipv6 addresses
                                if let Some(Yaml::Array(ipv6_addresses)) =
                                    iface_config.get(&Yaml::String(String::from("ipv6")))
                                {
                                    let mut addr_iter = ipv6_addresses.into_iter();
                                    while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                                        if let Ok(net) = addr_str.parse::<Ipv6Network>() {
                                            interface.addresses.push(IpNetwork::V6(net));
                                        }
                                    }
                                } else {
                                    // TODO: handle for when the ipv6 is not an array
                                }

                                router.interfaces.push(interface);
                            } else {
                                // TODO: handle for when the interface name
                                // has not been put as string
                            }
                        }
                    }

                    routers.push(router);
                }
            }
        }
    }
    routers
}

/// Parses the configurations handled under the switches
/// group in the yaml file
/// Example below:
///
/// ```yaml
/// switches:
///   sw1:
///     interfaces:
///       eth0:
///         ipv4:
///           - 192.168.100.11/24
///           - 192.168.20.1/24
/// ```
pub async fn parse_switches_config(switches_configs: &Yaml, handle: &Handle) -> Vec<Switch> {
    let mut switches: Vec<Switch> = vec![];
    if let Yaml::Hash(configs) = switches_configs {
        for (switch_name, switch_config) in configs {
            if let Yaml::Hash(switch_config) = switch_config
                && let Yaml::String(switch_name) = switch_name
            {
                let mut switch = Switch::new(handle, switch_name).await.unwrap();
                // fetch interfaces config
                if let Some(Yaml::Hash(interfaces_config)) =
                    switch_config.get(&Yaml::String(String::from("interfaces")))
                {
                    for (iface_name, iface_config) in interfaces_config {
                        if let Yaml::String(iface_name) = iface_name
                            && let Yaml::Hash(iface_config) = iface_config
                        {
                            //
                            let mut interface = Interface {
                                name: iface_name.to_string(),
                                addresses: vec![],
                            };

                            // get iface's ipv4 addresses
                            if let Some(Yaml::Array(ipv4_addresses)) =
                                iface_config.get(&Yaml::String(String::from("ipv4")))
                            {
                                let mut addr_iter = ipv4_addresses.into_iter();
                                while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                                    if let Ok(net) = addr_str.parse::<Ipv4Network>() {
                                        interface.addresses.push(IpNetwork::V4(net));
                                    }
                                }
                            } else {
                                // TODO: handle for when the ipv4 is not an array
                            }

                            // get iface's ipv6 address
                            if let Some(Yaml::Array(ipv6_addresses)) =
                                iface_config.get(&Yaml::String(String::from("ipv6")))
                            {
                                let mut addr_iter = ipv6_addresses.into_iter();
                                while let Some(Yaml::String(addr_str)) = addr_iter.next() {
                                    if let Ok(net) = addr_str.parse::<Ipv6Network>() {
                                        interface.addresses.push(IpNetwork::V6(net));
                                    }
                                }
                            } else {
                                // TODO: handle for when ipv6 is not as an array
                            }

                            switch.interfaces.push(interface);
                        }
                    }
                }

                switches.push(switch);
            } else {
                // TODO: handle when either a switch config's
                // is not a hash
            }
        }
    }
    switches
}

/// Parses the configurations under the links
/// group in the yaml file.
/// Example below:
/// ```yaml
/// links:
///   - src: rt1
///     src-iface: eth0
///     dst: rt2
///     dst-iface: eth1
/// ```
async fn parse_links_config(links_configs: &Yaml) -> Vec<Link> {
    let mut links: Vec<Link> = vec![];
    if let Yaml::Array(configs) = links_configs {
        for link_config in configs {
            if let Yaml::Hash(link_config) = link_config {
                if let Some(Yaml::String(src)) = link_config.get(&Yaml::String(String::from("src")))
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
    links
}
