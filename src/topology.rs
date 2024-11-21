use crate::Index;
use enum_as_inner::EnumAsInner;
use nix::net::if_::if_nametoindex;
use rtnetlink::{new_connection, Handle, LinkBridge, LinkVeth, NetworkNamespace, NETNS_PATH};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::process::Command;
use tokio;

#[derive(PartialEq, EnumAsInner)]
enum NodeType {
    Router,
    Switch,
}

#[derive(Debug, Clone)]
pub(crate) struct Router {
    name: String,
    fd: RawFd,
}

#[derive(Debug, Clone)]
pub(crate) struct Switch {
    name: String,
    ifindex: Index,
}

#[derive(Debug, Clone)]
pub(crate) enum LinkType {
    RouterToRouter,
    RouterToSwitch,
    SwitchToRouter,
    SwitchToSwitch,
}
pub(crate) struct Link {
    src_name: String,
    src_iface: String,
    dst_name: String,
    dst_iface: String,
    link_type: LinkType,
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

pub struct Topology {
    handle: Handle,
    routers: BTreeMap<String, Router>,
    switches: BTreeMap<String, Switch>,
    links: Vec<Link>,
}

impl Topology {
    pub fn new() -> Self {
        let (connection, handle, _) = new_connection().unwrap();
        tokio::spawn(connection);
        Self {
            handle,
            routers: BTreeMap::new(),
            switches: BTreeMap::new(),
            links: vec![],
        }
    }

    /// Given a list of router names and need to create a new namespace
    /// for each one of them.
    ///
    /// We will be giving each router an id after creating it's
    /// namespace and populating the router's path.
    ///
    /// use of BTreeSet is to ensure each of the strings is unique
    pub(crate) async fn add_routers(&mut self, routers: BTreeSet<&str>) {
        for name in routers {
            let router = self.add_router(name).await.unwrap();
            self.routers.insert(name.to_string(), router);
        }
    }

    pub(crate) async fn add_switches(&mut self, switches: BTreeSet<&str>) {
        println!("adding switches....");
        for name in switches {
            let switch = self.add_switch(name).await.unwrap();
            self.switches.insert(name.to_string(), switch);
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
    ///     // [ src_node, src_iface, dst_node, dst_iface ]
    ///  ]
    ///
    pub(crate) async fn add_links(&mut self, links: Vec<Vec<&str>>) {
        for link in links {
            // make sure that the link does not exist
            if self.link_exists(link.clone()) {
                println!(
                    "Problem creating link [{}:{}]-[{}:{}]. Link already exists",
                    link[0], link[1], link[2], link[3]
                );
                continue;
            }

            // make sure the hosts exist
            let node_1_exists = self.node_exists(&link[0]).await;
            let node_2_exists = self.node_exists(&link[2]).await;

            if let Some(node_type1) = &node_1_exists
                && let Some(node_type2) = &node_2_exists
            {
                let mut link_type: Option<LinkType> = None;
                if node_type1.is_router() && node_type2.is_switch() {
                    link_type = Some(LinkType::RouterToRouter);
                } else if node_type1.is_router() && node_type2.is_switch() {
                    link_type = Some(LinkType::RouterToSwitch);
                } else if node_type1.is_switch() && node_type2.is_router() {
                    link_type = Some(LinkType::SwitchToRouter);
                } else if node_type1.is_switch() && node_type2.is_switch() {
                    link_type = Some(LinkType::SwitchToSwitch);
                }

                if let Some(link_type) = link_type {
                    let link = self
                        .add_link(&link[0], &link[1], &link[2], &link[3], link_type)
                        .await
                        .unwrap();
                    self.links.push(link);
                    continue;
                }
            }

            // print out when either the first node does not exist
            // or the second node does not exist
            if node_1_exists.is_none() {
                println!(
                    "Problem creating link [{}:{}]-[{}:{}]. Node {} does not exist",
                    link[0], link[1], link[2], link[3], link[0]
                );
                continue;
            } else if node_2_exists.is_none() {
                println!(
                    "Problem creating link [{}:{}]-[{}:{}]. Node {} does not exist",
                    link[0], link[1], link[2], link[3], link[3]
                );
                continue;
            }
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
    async fn node_exists(&self, node_name: &str) -> Option<NodeType> {
        // look through the switches and routers
        if self.routers.get(node_name).is_some() {
            return Some(NodeType::Router);
        }
        if self.switches.get(node_name).is_some() {
            return Some(NodeType::Switch);
        }
        None
    }

    /// Creates a Node's namespace and all it's details.
    /// Returns a string that is the node's path.
    pub(crate) async fn add_router(&self, name: &str) -> IoResult<Router> {
        let mut ns_path = String::new();
        if let Err(err) = NetworkNamespace::add(name.to_string()).await {
            let e = format!("unable to create namespace\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        ns_path.push_str(NETNS_PATH);
        ns_path.push_str(name);
        let f = File::open(ns_path)?;
        Ok(Router {
            name: name.to_string(),
            fd: f.as_raw_fd(),
        })
    }

    pub(crate) async fn add_switch(&self, name: &str) -> IoResult<Switch> {
        if let Err(err) = self
            .handle
            .link()
            .add(LinkBridge::new(name).build())
            .execute()
            .await
        {
            let e = format!("problem creating bridge {name}\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        let ifindex = if_nametoindex(name)?;
        Ok(Switch {
            name: name.to_string(),
            ifindex,
        })
    }

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
        link_type: LinkType,
    ) -> IoResult<Link> {
        // create the link
        if let Err(err) = self
            .handle
            .link()
            .add(LinkVeth::new(src_iface, dst_iface).build())
            .execute()
            .await
        {
            let e = format!(
                "problem creating veth link [{src_node}:{src_iface} - {dst_node}:{dst_iface}]\n {:#?}",
                err
            );
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }

        let lt1: &str;
        let lt2: &str;

        match link_type {
            LinkType::RouterToRouter => {
                lt1 = "netns";
                lt2 = "netns";
            }
            LinkType::RouterToSwitch => {
                lt1 = "netns";
                lt2 = "master";
            }
            LinkType::SwitchToRouter => {
                lt1 = "master";
                lt2 = "netns";
            }
            LinkType::SwitchToSwitch => {
                lt1 = "master";
                lt2 = "master";
            }
        }

        let _ = Command::new("ip")
            .args(["link", "set", src_iface, lt1, src_node])
            .output();

        let _ = Command::new("ip")
            .args(["link", "set", dst_iface, lt2, dst_node])
            .output();

        Ok(Link {
            src_name: src_node.to_string(),
            src_iface: src_iface.to_string(),
            dst_name: dst_node.to_string(),
            dst_iface: dst_iface.to_string(),
            link_type,
        })
    }
}
