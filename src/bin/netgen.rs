use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use clap::{Arg, ArgMatches, command};
use netgen::error::{ConfigError, NamespaceError, NetError};
use netgen::topology::{Topology, TopologyParser};
use netgen::{DEVICES_NS_DIR, NetResult, PID_FILE, mount_device};
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, Pid, fork};
use tracing::{Level, debug, error};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::Registry;

fn main() -> NetResult<()> {
    init_tracing();
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
            let config_details = parse_config_args(start_args)
                .map_err(|err| {
                    error!(%err);
                    std::process::exit(1);
                })
                .unwrap();
            let mut topology = config_details.0;
            let config_file_name = config_details.1;

            // If the PID file exists, then there is a netgen instance already
            // running.
            if Path::new(PID_FILE).exists() {
                let err = NetError::BasicError(format!(
                    "Topology is currently running. \
                    Consider running 'netgen stop -t {config_file_name}', \
                    then try again.",
                ));
                error!(%err);
                std::process::exit(1);
            }

            let pid = Pid::this();

            // Create the directory storing our namespaces if it doesn't exists.
            let _ = fs::create_dir_all(DEVICES_NS_DIR);

            create_routers(&mut topology).map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });

            // Check if this is the child process.
            if Pid::this() == pid {
                return Ok(());
            }

            // For for setting vEth and bridges up for the devices.
            add_switches_and_links(&mut topology).map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });
        }
        Some(("stop", stop_args)) => {
            let topology = parse_config_args(stop_args)
                .map_err(|err| {
                    error!(%err);
                    std::process::exit(1);
                })
                .unwrap()
                .0;
            topology.power_off().map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });
        }
        _ => {
            // Probably "help"
        }
    }
    Ok(())
}

// Powers on all the devices in the topology.
fn create_routers(topology: &mut Topology) -> NetResult<()> {
    let fork = unsafe { fork() };

    // Fork for creating the devices.
    match fork {
        Ok(ForkResult::Child) => {
            let pid = Pid::this();

            let _ = mount_device(None).map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });

            if let Ok(mut f) = File::create(PID_FILE) {
                let _ = writeln!(f, "{}", pid.as_raw());
            }

            // Creates required namespaces for the routing devices.
            topology.power_routers_on().map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });
            debug!("Devices powered on");
        }
        Ok(ForkResult::Parent { child }) => {
            waitpid(child, None).map_err(|err| {
                NetError::NamespaceError(NamespaceError::Fork {
                    fork_function: String::from("create_routers"),
                    source: err,
                })
            })?;
        }
        Err(err) => {
            return Err(NamespaceError::Fork {
                fork_function: String::from("create_routers"),
                source: err,
            }
            .into());
        }
    }
    Ok(())
}

fn add_switches_and_links(topology: &mut Topology) -> NetResult<()> {
    netgen::enter_ns(None)?;
    topology.power_switches_on()?;
    topology.setup_links()?;

    Ok(())
}

fn init_tracing() {
    let level_filter = LevelFilter::from_level(Level::TRACE);
    let layer = tracing_subscriber::fmt::layer().with_target(false);
    let layer = layer.with_filter(level_filter);
    let subscriber = Registry::default().with(layer);
    let _ = tracing::subscriber::set_global_default(subscriber).map_err(|_| {
        eprintln!("unable to initialize tracing");
    });
}

fn config_args() -> Vec<Arg> {
    vec![
        Arg::new("Topo File")
            .short('t')
            .long("topo")
            .value_name("yaml-file")
            .help("file with the topology"),
    ]
}

/// Returns Result<(topology_object, config_file_path)>
fn parse_config_args(
    config_args: &ArgMatches,
) -> NetResult<(Topology, String)> {
    match config_args.get_one::<String>("Topo File") {
        Some(topo_yml_file) => {
            let mut topo_file = File::open(topo_yml_file).map_err(|err| {
                NamespaceError::FileOpen {
                    path: topo_yml_file.to_string(),
                    source: err,
                }
            })?;
            let topology = TopologyParser::from_yaml_file(&mut topo_file)?;
            Ok((topology, topo_yml_file.to_string()))
        }
        None => {
            let err = ConfigError::TopologyFileMissing;
            error!(%err, "configuration issues");
            std::process::exit(1);
        }
    }
}
