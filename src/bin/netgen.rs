#![feature(let_chains)]
use netgen::plugins::Config;
use netgen::topology::Topology;
use std::fs::File;

#[tokio::main]
async fn main() {
    if let Ok(mut config_file) = File::open("./assets/config.yml")
        && let Ok(mut topo_file) = File::open("./assets/sample-top.yml")
    {
        // load the base configuration
        let config = match Config::from_yaml_file(&mut config_file) {
            Ok(config) => Some(config),
            Err(_err) => None,
        };

        // load the topology configuration
        match Topology::from_yaml_file(&mut topo_file, config) {
            Ok(topo) => {
                println!("{:?}", topo);
            }
            Err(err) => {
                println!("{:#?}", err);
            }
        }
    }
}
