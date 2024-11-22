use netgen::topology::Topology;
use std::collections::BTreeSet;

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
