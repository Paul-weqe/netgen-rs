use crate::devices::{Link, Node, Router, Switch};

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

    pub async fn from_yaml_file(file_path: &str) -> IoResult<Self> {
        let mut f = File::open(file_path).unwrap();
        let mut contents = String::new();
        let _ = f.read_to_string(&mut contents);
        Self::from_yaml_str(contents.as_str()).await
    }

    pub async fn from_yaml_str(yaml_str: &str) -> IoResult<Self> {
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
            topology.parse_topology_config(&yaml_group).await;
        }
        Ok(topology)
    }

    pub async fn parse_topology_config(&mut self, yaml_data: &Yaml) {
        if let Yaml::Hash(topo_config_group) = yaml_data {
            // fetch the routers
            if let Some(routers_configs) =
                topo_config_group.get(&Yaml::String(String::from("routers")))
            {
                // TODO: handle the unwrap below
                let routers = self.parse_router_configs(routers_configs).await.unwrap();
            }

            // fetch the switches
            if let Some(switches_configs) =
                topo_config_group.get(&Yaml::String(String::from("switches")))
            {
                // TODO: handle the unwrap below
                let switches = self.parse_switch_configs(switches_configs).await.unwrap();
            }

            // fetch the links
            if let Some(links_configs) = topo_config_group.get(&Yaml::String(String::from("links")))
            {
                // TODO: handle the unwrap below
                let links = self.parse_links_configs(links_configs).await.unwrap();
            }
        }
    }

    pub async fn parse_router_configs(&self, routers_config: &Yaml) -> IoResult<Vec<Router>> {
        let mut routers: Vec<Router> = vec![];
        if let Yaml::Hash(configs) = routers_config {
            for (router_name, router_config) in configs {
                if let Yaml::String(router_name) = router_name
                    && let Yaml::Hash(router_config) = router_config
                {
                    let router = Router::new_from_yaml_config(&router_name, &router_config);
                    println!("{:#?}", router);
                } else {
                    // TODO: handle a case where the router_name is not a string
                    // or the router_config is not a Yaml::Hash
                }
            }
        }
        Ok(routers)
    }

    pub async fn parse_switch_configs(&self, switches_configs: &Yaml) -> IoResult<Vec<Switch>> {
        let mut switches: Vec<Switch> = vec![];
        Ok(switches)
    }

    pub async fn parse_links_configs(&self, links_configs: &Yaml) -> IoResult<Vec<Link>> {
        let mut links: Vec<Link> = vec![];
        Ok(links)
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
            let switch = Switch::new(&self.handle, name).await.unwrap();
            self.nodes.insert(name.to_string(), Node::Switch(switch));
        }
    }

    /// adds the links to the already created routers and switches.
    ///
    /// `links` argument is basically a list of all the links that should
    /// be created. E.g
    ///
    /// ------                         -------
    /// | s1 | eth0 ============= eth1 |  s2 |
    /// ------                         -------
    /// Will be in the form of:
    ///  vec![
    ///     vec![ "s1", "eth0", "s2", "eth1" ]
    ///  ]
    /// where
    /// [ src_node, src_iface, dst_node, dst_iface ]
    ///
    pub async fn add_links(&mut self, links: Vec<Vec<&str>>) {
        for link in links {
            // make sure that the link does not exist
            if self.link_exists(link.clone()) {
                println!(
                    "Problem creating link [{}:{}]-[{}:{}]. Link already exists",
                    link[0], link[1], link[2], link[3]
                );
                continue;
            }
            let _ = self.add_link(link[0], link[1], link[2], link[3]).await;
        }
    }

    fn link_exists(&self, link: Vec<&str>) -> bool {
        for created_link in &self.links {
            let created_link = created_link.to_vec();
            if created_link == link {
                return true;
            }

            let l_pair1 = (link[0], link[1]);
            let l_pair2 = (link[2], link[3]);

            let cl_pair1 = (created_link[0].as_str(), created_link[1].as_str());
            let cl_pair2 = (created_link[2].as_str(), created_link[3].as_str());

            if (l_pair1 == cl_pair2) || (l_pair2 == cl_pair1) {
                return true;
            }
        }
        false
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
                    if let Ok(index) = if_nametoindex(src_iface) {
                        self.handle.link().set(index).up().execute().await.unwrap();
                        self.handle
                            .link()
                            .set(index)
                            .controller(switch.ifindex)
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
                    if let Ok(index) = if_nametoindex(dst_iface) {
                        // bring up the link endpoint
                        self.handle.link().set(index).up().execute().await.unwrap();
                        self.handle
                            .link()
                            .set(index)
                            .controller(switch.ifindex)
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
