#![feature(let_chains)]
mod devices;
mod topology;

use nix::sched::setns;
use nix::sched::CloneFlags;
use nix::unistd::gettid;
use std::collections::BTreeSet;
use std::fs::File;
use std::os::fd::AsFd;
use topology::Topology;

pub(crate) type Index = libc::c_uint;

#[tokio::main]
async fn main() {
    let routers = BTreeSet::from(["r1", "r2", "r3"]);
    let switches = BTreeSet::from(["sw"]);

    let links = vec![
        vec!["r1", "eth-r1", "sw", "eth0"],
        vec!["r2", "eth-r2", "sw", "eth1"],
        vec!["r3", "eth-r3", "sw", "eth2"],
    ];
    let mut topology = Topology::new();

    topology.add_routers(routers).await;
    topology.add_switches(switches).await;
    topology.add_links(links).await;
}

/// Runs the commands inside the
/// namespace specified.
fn _in_namespace<F, T>(ns_path: &str, f: F) -> std::io::Result<T>
where
    F: FnOnce() -> T,
{
    let current_thread_path = format!("/proc/self/task/{}/ns/net", gettid());
    let current_thread_file = File::open(&current_thread_path).unwrap();

    let ns_file = File::open(ns_path).unwrap();

    // move into namespace
    setns(ns_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();
    let result = f();

    // come back to default namespace
    setns(current_thread_file.as_fd(), CloneFlags::CLONE_NEWNET).unwrap();

    Ok(result)
}
