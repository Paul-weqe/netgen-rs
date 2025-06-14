#![feature(let_chains)]
use std::fs::{File, OpenOptions};
use std::io::BufRead;

use clap::{Arg, ArgMatches, command};
use netgen::error::Error;
use netgen::plugins::Config;
use netgen::topology::Topology;
use netgen::{PLUGIN_PIDS_FILE, Result};
use sysinfo::{Pid, System};

#[tokio::main]
async fn main() -> Result<()> {
    let app_match = command!("netgen")
        .subcommand(
            command!("start")
                .args(config_args())
                .about("starts the netgen setup"),
        )
        .subcommand(
            command!("stop")
                .args(config_args())
                .about("stops the running netgen setup"),
        )
        .get_matches();

    match app_match.subcommand() {
        Some(("start", start_args)) => {
            let mut topology = parse_config_args(start_args)?;

            // If the file exists, then the topology is currently running
            if File::open(PLUGIN_PIDS_FILE).is_ok() {
                return Err(Error::GeneralError(String::from(
                    "topology is currently running. Consider running 'netgen stop -t my-topo.yml' before starting again",
                )));
            }

            File::create(PLUGIN_PIDS_FILE).unwrap();

            // "powers on" all the devices and sets up all the required links
            topology.power_on().await?;

            // Runs the plugins in the routers.
            // and initiates their startup-config (if present)
            topology.run().await?;
        }
        Some(("stop", stop_args)) => {
            let mut topology = parse_config_args(stop_args)?;

            // Turns off all the nodes
            // as a result also deletes any dangling veth link
            topology.power_off().await;

            // Kills all the running plugin PIDs
            if let Ok(file) =
                OpenOptions::new().read(true).open(PLUGIN_PIDS_FILE)
            {
                let reader = std::io::BufReader::new(file);
                let system = System::new_all();

                for line in reader.lines() {
                    if let Ok(line) = line
                        && let Ok(pid) = line.parse::<u32>()
                        && let Some(process) =
                            system.process(Pid::from_u32(pid))
                    {
                        process.kill();
                    }
                }
            }

            // Delete the PID file.
            let _ = std::fs::remove_file(PLUGIN_PIDS_FILE);
        }
        _ => {
            // Probably "help"
        }
    }
    Ok(())
}

fn config_args() -> Vec<Arg> {
    vec![
        Arg::new("Config File")
            .short('c')
            .long("config")
            .value_name("yaml-file")
            .help("the file with plugin configs"),
        Arg::new("Topo File")
            .short('t')
            .long("topo")
            .value_name("yaml-file")
            .help("file with the topology"),
    ]
}

fn parse_config_args(config_args: &ArgMatches) -> Result<Topology> {
    let config = match config_args.get_one::<String>("Config File") {
        Some(config_file) => {
            let mut config_file = File::open(config_file).unwrap();
            Config::from_yaml_file(Some(&mut config_file))?
        }
        None => Config::from_yaml_file(None)?,
    };

    let topo_file = match config_args.get_one::<String>("Topo File") {
        Some(topo_file) => topo_file,
        None => {
            return Err(Error::GeneralError(String::from(
                "topolofy file not configured",
            )));
        }
    };

    let mut topo_file = File::open(topo_file).unwrap();
    let topology =
        Topology::from_yaml_file(&mut topo_file, config.clone()).unwrap();
    Ok(topology)
}
