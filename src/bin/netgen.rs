use std::fs::{self, File};
use std::io::Write;
use std::os::fd::AsFd;

use clap::{Arg, ArgMatches, command};
use netgen::error::{ConfigError, NamespaceError, NetError};
use netgen::topology::{Topology, TopologyParser};
use netgen::{
    DEVICES_NS_DIR, NS_DIR, NetResult, PID_FILE, kill_process, mount_device,
};
use nix::mount::umount;
use nix::sched::{CloneFlags, setns};
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
            // If the PID file exists, then there is a netgen instance already running.
            if File::open(PID_FILE).is_ok() {
                return Err(NetError::BasicError(String::from(
                    "topology is currently running. Consider running 'netgen stop -t my-topo.yml' before starting again",
                )));
            }

            let pid = Pid::this();

            // Create the directory storing our namespaces if it doesn't exists
            let _ = fs::create_dir_all(DEVICES_NS_DIR);

            let mut topology = parse_config_args(start_args)?;
            create_routers(&mut topology)?;

            // Check if this is the child process.
            if Pid::this() == pid {
                return Ok(());
            }

            // For for setting vEth and bridges up for the devices.
            add_switches_and_links(&mut topology)?;
        }
        Some(("stop", stop_args)) => {
            if let Ok(mut topology) = parse_config_args(stop_args) {
                topology.power_off()?;
            }

            // Kill the main process.
            kill_process(PID_FILE)?;

            // Unmount the main task.
            let main_mount_dir = format!("{NS_DIR}/main/net");

            if let Err(err) = umount(main_mount_dir.as_str()) {
                error!(%main_mount_dir, error = %err, "error umounting");
            }

            // Delete the PID file.
            let _ = fs::remove_file(PID_FILE);
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

            let _ = mount_device(None)?;

            if let Ok(mut f) = File::create(PID_FILE) {
                let _ = writeln!(f, "{}", pid.as_raw());
            }

            // Creates required namespaces for the routing devices.
            topology.power_routers_on()?;
            debug!("devices powered on");
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

// Adds links to all the devices that have been created.
fn add_switches_and_links(topology: &mut Topology) -> NetResult<()> {
    let fork = unsafe { fork() };

    match fork {
        Ok(ForkResult::Child) => {
            // Enter the main namespace.
            netgen::enter_ns(None)?;
            topology.power_switches_on()?;
            topology.setup_links()?;
        }
        Ok(ForkResult::Parent { child }) => {
            waitpid(child, None).map_err(|err| {
                NetError::NamespaceError(NamespaceError::Fork {
                    fork_function: String::from("add_device_links"),
                    source: err,
                })
            })?;
        }
        Err(err) => {
            return Err(NamespaceError::Fork {
                fork_function: String::from("add_device_links"),
                source: err,
            }
            .into());
        }
    }
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

fn parse_config_args(config_args: &ArgMatches) -> NetResult<Topology> {
    match config_args.get_one::<String>("Topo File") {
        Some(topo_file) => {
            let mut topo_file = File::open(topo_file).map_err(|err| {
                NamespaceError::FileOpen {
                    path: topo_file.clone(),
                    source: err,
                }
            })?;
            let topology = TopologyParser::from_yaml_file(&mut topo_file)?;
            Ok(topology)
        }
        None => Err(ConfigError::TopologyFileMissing.into()),
    }
}
