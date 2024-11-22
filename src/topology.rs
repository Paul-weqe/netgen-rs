use crate::Index;
use netlink_packet_route::link::LinkFlag;
use nix::net::if_::if_nametoindex;
use nix::sched::setns;
use nix::sched::CloneFlags;
use nix::unistd::gettid;
use rtnetlink::{new_connection, Handle, NetworkNamespace, NETNS_PATH};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::os::fd::AsFd;
use std::process::Command;
use tokio;

#[derive(Debug, Clone)]
pub struct Router {
    name: String,
    file_path: String,
}

impl Router {
    /// creates a new namespace that will represent the
    /// router
    async fn new(name: &str) -> IoResult<Self> {
        let mut ns_path = String::new();
        if let Err(err) = NetworkNamespace::add(name.to_string()).await {
            let e = format!("unable to create namespace\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        ns_path.push_str(NETNS_PATH);
        ns_path.push_str(name);
        let router = Self {
            name: name.to_string(),
            file_path: ns_path,
        };

        // make sure the loopback interface of the router up
        let _ = router.up(String::from("lo")).await;
        Ok(router)
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
    ///         // If no modifications have been made,
    ///         // should only show the loopback ("lo")
    ///         // interface
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

        let ns_file = File::open(self.file_path.as_str()).unwrap();

        // move into router namespace
        setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();
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
    async fn up(&self, iface_name: String) -> IoResult<()> {
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
        return result?;
    }
}

#[derive(Debug, Clone)]
pub struct Switch {
    name: String,
    ifindex: Index,
}

impl Switch {
    async fn new(handle: &Handle, name: &str) -> IoResult<Self> {
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
        })
    }

    async fn up(&self, handle: &Handle) -> IoResult<()> {
        let _ = handle.link().set(self.ifindex).up().execute().await;
        // TODO: error handling for when bringing the interface up does not work.
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum Node {
    Router(Router),
    Switch(Switch),
}

impl Node {
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

pub struct Link {
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

    /// Given a list of router names and need to create a new namespace
    /// for each one of them.
    ///
    /// We will be giving each router an id after creating it's
    /// namespace and populating the router's path.
    ///
    /// use of BTreeSet is to ensure each of the strings is unique
    pub async fn add_routers(&mut self, routers: BTreeSet<&str>) {
        println!("adding routers...");
        for name in routers {
            let router = Router::new(name).await.unwrap();
            self.nodes.insert(name.to_string(), Node::Router(router));
        }
    }

    pub async fn add_switches(&mut self, switches: BTreeSet<&str>) {
        println!("adding switches....");
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
            let _ = self.add_link(&link[0], &link[1], &link[2], &link[3]).await;
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
    fn node_exists(&self, node_name: &str) -> Option<&Node> {
        // look through the switches and routers
        if let Some(node) = self.nodes.get(node_name) {
            match node {
                Node::Router(_) => return Some(&node),
                Node::Switch(_) => return Some(&node),
            }
        }
        None
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
            let _ = node_1.up(&self.handle, src_iface.to_string()).await;
            let _ = node_2.up(&self.handle, dst_iface.to_string()).await;

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
