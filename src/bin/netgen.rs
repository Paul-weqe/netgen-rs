use netgen::topology::Topology;
use std::collections::BTreeSet;

pub(crate) type Index = libc::c_uint;

#[tokio::main]
async fn main() {
    // ------                   -------                    ------
    // | r1 | eth0 ------- eth1 | br1 | eth2 -------- eth3 | r2 |
    // ------                   -------                    ------
    //
    let routers = BTreeSet::from(["r1", "r2"]);
    let switches = BTreeSet::from(["br1"]);

    let links = vec![
        vec!["r1", "eth0", "br1", "eth1"],
        vec!["r2", "eth3", "br1", "eth2"],
    ];
    let mut topology = Topology::new();

    topology.add_routers(routers).await;
    topology.add_switches(switches).await;
    topology.add_links(links).await;
}
