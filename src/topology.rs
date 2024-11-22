use crate::Index;
use enum_as_inner::EnumAsInner;
use nix::net::if_::if_nametoindex;
use nix::sched::setns;
use nix::sched::CloneFlags;
use nix::unistd::gettid;
use rtnetlink::{new_connection, Handle, LinkBridge, LinkVeth, NetworkNamespace, NETNS_PATH};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::os::fd::AsFd;
use std::process::Command;
use tokio;

pub trait BaseNode {
    /// method is used to bring up a device's interface.
    /// if it is a switch, the iface_name will be irrelevant, the
    /// switche's name will be used.
    ///
    /// If it is a router, the handle will not be important
    /// since we will go into the routers namespace and create
    /// a custom handle in there.
    async fn up(&self, handle: &Handle, iface_name: String) -> IoResult<()>;
}

#[derive(Debug, Clone)]
pub struct Router {
    name: String,
    file_path: String,
}

impl Router {
    /// Runs the commands inside the
    /// router namespace.
    async fn run<F, T, R>(&self, f: F) -> IoResult<R>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Future<Output = R> + Send,
    {
        let current_thread_path = format!("/proc/self/task/{}/ns/net", gettid());
        let current_thread_file = File::open(&current_thread_path).unwrap();

        let ns_file = File::open(self.file_path.as_str()).unwrap();

        // move into router namespace
        setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();
        let result = (f)().await;

        // come back to parent namespace
        setns(current_thread_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();

        Ok(result)
    }

    async fn up(&self, iface_name: String) -> IoResult<()> {
        let result = self
            .run(move || async move {
                if let Ok((connection, handle, _)) = new_connection() {
                    tokio::spawn(connection);
                    let request = handle
                        .link()
                        .set(LinkBridge::new(&iface_name).up().build())
                        .execute()
                        .await;
                }
                // TODO: add error catcher in case
                // of problems when bringing the interface up
                Ok(())
            })
            .await;
        return result?;
    }
}

#[derive(Debug, Clone)]
pub struct Switch {
    name: String,
    ifindex: Index,
}

impl Switch {
    async fn up(&self, handle: &Handle) -> IoResult<()> {
        handle
            .link()
            .set(LinkBridge::new(&self.name).up().build())
            .execute()
            .await;
        // TODO: error handling for when bringing the interface up does not work.
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum NodeType {
    Router(Router),
    Switch(Switch),
}

impl NodeType {
    /// gets the link types that will be used to create
    /// the nodes.
    ///
    /// e.g for router the link type will be netns
    /// since it's own namespace will be created
    pub fn link_type(&self) -> String {
        match self {
            Self::Router(_) => String::from("netns"),
            Self::Switch(_) => String::from("master"),
        }
    }

    /// Brings the device up.
    ///
    /// if it is a router, we specify the interface name that is
    /// to be brought up. the handle will be created custom for
    /// the router's namespace.
    ///
    /// If it is a switch, we specify the handle, but since the switch
    /// is a bridge device and we already have the interface name, we
    /// only use that for the switch.
    pub async fn up(&self, handle: &Handle, iface_name: String) -> IoResult<()> {
        match self {
            Self::Router(router) => router.up(iface_name).await,
            Self::Switch(switch) => switch.up(handle).await,
        }
    }
}

pub(crate) struct Link {
    src_name: String,
    src_iface: String,
    dst_name: String,
    dst_iface: String,
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
    nodes: BTreeMap<String, NodeType>,
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

    /// Given a list of router names and need to create a new namespace
    /// for each one of them.
    ///
    /// We will be giving each router an id after creating it's
    /// namespace and populating the router's path.
    ///
    /// use of BTreeSet is to ensure each of the strings is unique
    pub(crate) async fn add_routers(&mut self, routers: BTreeSet<&str>) {
        println!("adding routers...");
        for name in routers {
            let router = self.add_router(name).await.unwrap();
            self.nodes
                .insert(name.to_string(), NodeType::Router(router));
        }
    }

    pub(crate) async fn add_switches(&mut self, switches: BTreeSet<&str>) {
        println!("adding switches....");
        for name in switches {
            let switch = self.add_switch(name).await.unwrap();
            self.nodes
                .insert(name.to_string(), NodeType::Switch(switch));
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
            self.add_link(&link[0], &link[1], &link[2], &link[3]).await;
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
    fn node_exists(&self, node_name: &str) -> Option<&NodeType> {
        // look through the switches and routers
        if let Some(node) = self.nodes.get(node_name) {
            match node {
                NodeType::Router(_) => return Some(&node),
                NodeType::Switch(_) => return Some(&node),
            }
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
        let router = Router {
            name: name.to_string(),
            file_path: ns_path,
        };

        // bring the loopback interface of the router up
        router.up(String::from("lo")).await;
        Ok(router)
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

        if let Some(node_1) = self.node_exists(src_node)
            && let Some(node_2) = self.node_exists(dst_node)
        {
            // set the link types
            let lt1: &str = &node_1.link_type();
            let lt2: &str = &node_2.link_type();

            let _ = Command::new("ip")
                .args(["link", "set", src_iface, lt1, src_node])
                .output();
            let _ = Command::new("ip")
                .args(["link", "set", dst_iface, lt2, dst_node])
                .output();

            // ensure the interfaces are in the up state
            node_1.up(&self.handle, src_iface.to_string()).await;
            node_2.up(&self.handle, dst_iface.to_string()).await;

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
            return Err(IoError::new(ErrorKind::Other, err.as_str()));
        }
    }
}
