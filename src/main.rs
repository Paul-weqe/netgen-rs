mod devices;
mod topology;

use nix::sched::setns;
use nix::sched::CloneFlags;
use nix::unistd::gettid;
use std::fs::File;
use std::os::fd::AsFd;
use topology::Topology;

pub(crate) type Index = libc::c_uint;

#[tokio::main]
async fn main() {
    let (src_node, dst_node, src_iface, dst_iface) = ("ns1", "ns2", "eth-ns1", "eth-ns2");
    let topology = Topology::new();

    let _ = topology.add_router(src_node).await;
    let _ = topology.add_router(dst_node).await;

    let _ = topology
        .add_link(src_iface, src_node, dst_iface, dst_node)
        .await;
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
