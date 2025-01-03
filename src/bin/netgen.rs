#![feature(let_chains)]
use netgen::plugins::Config;
use netgen::topology::Topology;
use netgen::{error::Error, Result};

use std::fs::File;

use clap::{command, Arg};

#[tokio::main]
async fn main() -> Result<()> {
    let matches = command!("netgen")
        .arg(
            Arg::new("Config File")
                .short('c')
                .long("config")
                .value_name("yaml-file")
                .help("the file with plugin configs"),
        )
        .arg(
            Arg::new("Topo File")
                .short('t')
                .long("topo")
                .value_name("yaml-file")
                .help("file with the topology"),
        )
        .get_matches();
    let config = match matches.get_one::<String>("Config File") {
        Some(config_file) => {
            let mut config_file = File::open(config_file).unwrap();
            Config::from_yaml_file(Some(&mut config_file))?
        }
        None => Config::from_yaml_file(None)?,
    };

    let topo_file = match matches.get_one::<String>("Topo File") {
        Some(topo_file) => topo_file,
        None => {
            return Err(Error::GeneralError(String::from(
                "topolofy file not configured",
            )))
        }
    };

    let mut topo_file = File::open(topo_file).unwrap();
    let mut topology = Topology::from_yaml_file(&mut topo_file, config).unwrap();

    // "powers on" all the devices and sets up all the required links
    topology.power_on().await?;

    // runs the plugins in the routers.
    // and initiates their startup-config (if present)
    topology.run().await?;

    Ok(())
}
