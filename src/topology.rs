use crate::Index;
use nix::net::if_::if_nametoindex;
use rtnetlink::{
    new_connection, Error as RtError, Handle, LinkBridge, LinkVeth, NetworkNamespace, NETNS_PATH,
};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::process::Command;
use tokio;

pub(crate) trait NetworkDevice {
    // add code here
}

pub(crate) struct Router {
    id: u32,
    name: String,
    fd: RawFd,
}
impl NetworkDevice for Router {}

pub(crate) struct Switch {
    id: u32,
    name: String,
    ifindex: Index,
}
impl NetworkDevice for Switch {}

pub(crate) struct Link {
    src_id: u32,
    dst_id: u32,
    src_iface: String,
    dst_iface: String,
}

pub(crate) struct Topology {
    handle: Handle,
    next_id: u32,
    routers: Vec<Router>,
    switches: Vec<Switch>,
    links: Vec<Link>,
}

impl Topology {
    pub fn new() -> Self {
        let (connection, handle, _) = new_connection().unwrap();
        tokio::spawn(connection);
        Self {
            handle,
            next_id: 1,
            routers: vec![],
            switches: vec![],
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
    pub(crate) async fn add_routers(&mut self, routers: BTreeSet<String>) {
        println!("adding routers....");
        for name in routers {
            let fd = self.add_router(&name).await.unwrap();
            let router = Router {
                id: self.next_id,
                name,
                fd,
            };
            self.routers.push(router);
            self.next_id += 1;
        }
    }

    pub(crate) async fn add_switches(&mut self, switches: BTreeSet<String>) {
        println!("adding switches....");
        for name in switches {
            let ifindex = self.add_switch(&name).await.unwrap();
            let switch = Switch {
                id: self.next_id,
                name,
                ifindex,
            };
            self.switches.push(switch);
            self.next_id += 1;
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
    pub(crate) async fn add_links(&mut self, links: Vec<Vec<String>>) {
        for link_group in links {
            let link = self
                .add_link(
                    &link_group[0],
                    &link_group[1],
                    &link_group[2],
                    &link_group[3],
                )
                .await
                .unwrap();
        }
    }

    fn find_device_id(&self, name: &str) -> Option<u32> {
        if let Some(id) = self.find_router_id(name) {
            return Some(id);
        }
        if let Some(id) = self.find_switch_id(name) {
            return Some(id);
        }
        None
    }

    fn find_router_id(&self, name: &str) -> Option<u32> {
        None
    }

    fn find_switch_id(&self, name: &str) -> Option<u32> {
        None
    }

    /// Creates a Node's namespace and all it's details.
    /// Returns a string that is the node's path.
    pub(crate) async fn add_router(&self, name: &str) -> IoResult<RawFd> {
        let mut ns_path = String::new();
        if let Err(err) = NetworkNamespace::add(name.to_string()).await {
            let e = format!("unable to create namespace\n {:#?}", err);
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }
        ns_path.push_str(NETNS_PATH);
        ns_path.push_str(name);
        let f = File::open(ns_path)?;
        Ok(f.as_raw_fd())
    }

    pub(crate) async fn add_switch(&self, name: &str) -> IoResult<Index> {
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
        Ok(ifindex)
    }

    /// Creates a link that will be used between two nodes:
    ///
    /// src_iface |===============================| dst_iface
    ///
    /// Note that the src_iface and dst_iface should not have the same name
    pub(crate) async fn add_link(
        &self,
        src_iface: &str,
        src_node: &str,
        dst_iface: &str,
        dst_node: &str,
    ) -> IoResult<()> {
        // create the link
        if let Err(err) = self
            .handle
            .link()
            .add(LinkVeth::new(src_iface, dst_iface).build())
            .execute()
            .await
        {
            let e = format!(
                "problem creating link [{src_node}:{src_iface} - {dst_node}:{dst_iface}]\n {:#?}",
                err
            );
            return Err(IoError::new(ErrorKind::Other, e.as_str()));
        }

        // TODO: change the following commands to rtnetlink calls
        //
        // connect the link's endpoints to a namespace
        let _ = Command::new("ip")
            .args(["link", "set", src_iface, "netns", src_node])
            .output();

        let _ = Command::new("ip")
            .args(["link", "set", dst_iface, "netns", dst_node])
            .output();

        Ok(())
    }
}
