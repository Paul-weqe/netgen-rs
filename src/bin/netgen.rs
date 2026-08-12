use std::fs::{self, File};
use std::path::Path;

use clap::{Args, Parser, Subcommand};
use netgen::error::{ConfigError, NamespaceError, NetError};
use netgen::node::Router;
use netgen::topology::{Topology, TopologyParser};
use netgen::{
    DEVICES_NS_DIR, MAIN_NS_DIR, NetResult, mount_device, mount_router_volumes,
};
use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, unshare};
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, Pid, execvp, fork};
use tracing::{Level, debug, error};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::Registry;

#[derive(Parser)]
#[command(name = "netgen")]
#[command(about = "Network topology management tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Starts the netgen setup")]
    #[command(
        long_about = "Powers on all routers and switches defined in the topology file."
    )]
    Start(StartStopArgs),

    #[command(about = "Stops the running netgen setup")]
    #[command(
        long_about = "Gracefully powers off all devices and cleans up network namespaces."
    )]
    Stop(StartStopArgs),

    #[command(about = "Logs into a device")]
    #[command(
        long_about = "Spawns a new shell inside the device's network and mount namespaces."
    )]
    Login(LoginArgs),
}

#[derive(Args)]
pub struct StartStopArgs {
    #[arg(short = 't', long = "topo", value_name = "yaml-file")]
    #[arg(help = "file containing the topology definition")]
    #[arg(default_value = "topo.yml")]
    pub topo_file: String,
}

#[derive(Args)]
pub struct LoginArgs {
    #[arg(short = 't', long = "topo", value_name = "yaml-file")]
    #[arg(help = "file containing the topology definition")]
    #[arg(default_value = "topo.yml")]
    pub topo_file: String,

    #[arg(short = 'd', long = "device", value_name = "device-name")]
    #[arg(help = "name of device")]
    pub device_name: Option<String>,
}

#[derive(Args)]
pub struct LsArgs {
    #[arg(short = 't', long = "topo", value_name = "yaml-file")]
    #[arg(help = "file with the topology")]
    pub topo_file: Option<String>,
}

fn main() {
    if let Err(err) = ngen_main() {
        error!(%err);
        std::process::exit(1);
    }
}

fn ngen_main() -> NetResult<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Commands::Start(args) => {
            let (mut topology, config_file_name) =
                parse_config_args(args.topo_file)?;

            if instance_running() {
                let err = NetError::BasicError(format!(
                    "Topology is currently running. \
                        Consider running 'netgen stop -t {config_file_name}' \
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
        Commands::Stop(args) => {
            let (topology, _config_file_name) =
                parse_config_args(args.topo_file)?;
            topology.power_off()?;
        }
        Commands::Login(args) => {
            let router = parse_login_args(args.topo_file, args.device_name)?;

            // Check if topology instance is running.
            if !instance_running() {
                let err = NetError::BasicError(
                    "No topology instance currently running.".to_string(),
                );

                error!(%err);
                std::process::exit(1);
            }

            // Check if device instance is running.
            if !device_running(&router.name) {
                let err = NetError::BasicError(
                    "Device instance not running. Ensure device has been started".to_string()
                );
                error!(%err);
                std::process::exit(1);
            }

            // Enter into the device's PID and network namespaces.
            netgen::enter_ns(Some(router.name.clone()))?;

            // unshare into the mount namespace.
            unshare(CloneFlags::CLONE_NEWNS).map_err(|err| {
                NetError::NamespaceError(NamespaceError::Unshare {
                    ns_name: router.name.clone(),
                    source: err,
                })
            })?;

            // Have procfs correctly mounted.
            mount(
                None::<&str>,
                "/",
                None::<&str>,
                MsFlags::MS_PRIVATE | MsFlags::MS_REC,
                None::<&str>,
            )
            .map_err(|err| {
                NetError::NamespaceError(NamespaceError::Mount {
                    ns_type: String::from("proc mount"),
                    device: router.name.clone(),
                    source: err,
                })
            })?;

            mount(
                Some("proc"),
                "/proc",
                Some("proc"),
                MsFlags::empty(),
                None::<&str>,
            )
            .map_err(|err| {
                NetError::NamespaceError(NamespaceError::Mount {
                    ns_type: String::from("mount"),
                    device: router.name.clone(),
                    source: err,
                })
            })?;

            // Mount the volumes.
            mount_router_volumes(&router)?;

            debug!("successfully logged in");

            let shell = std::ffi::CString::new("/bin/bash").unwrap();
            execvp(&shell, &[&shell]).map_err(|err| {
                NetError::BasicError(format!("execvp failed: {err}"))
            })?;
        }
    }
    Ok(())
}

/// Checks if the main directory exists indicating if there is an instance
/// running.
fn instance_running() -> bool {
    Path::new(MAIN_NS_DIR).exists()
}

/// Make sure both the net and pid files are present.
fn device_running(router_name: &str) -> bool {
    let net_path = format!("{DEVICES_NS_DIR}/{router_name}/net");
    let pid_path = format!("{DEVICES_NS_DIR}/{router_name}/pid");

    Path::new(&net_path).exists() && Path::new(&pid_path).exists()
}

// Powers on all the devices in the topology.
fn create_routers(topology: &mut Topology) -> NetResult<()> {
    let fork = unsafe { fork() };

    // Fork for creating the devices.
    match fork {
        Ok(ForkResult::Child) => {
            let _ = mount_device(None).map_err(|err| {
                error!(%err);
                std::process::exit(1);
            });

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

fn parse_config_args(topo_yml_file: String) -> NetResult<(Topology, String)> {
    let mut topo_file =
        File::open(&topo_yml_file).map_err(|err| NamespaceError::FileOpen {
            path: topo_yml_file.clone(),
            source: err,
        })?;

    let topology = TopologyParser::from_yaml_file(&mut topo_file)?;
    Ok((topology, topo_yml_file))
}

fn parse_login_args(
    topo_yml_file: String,
    router_name: Option<String>,
) -> NetResult<Router> {
    let router_name = router_name.unwrap_or_else(prompt_device);

    // Generate Topology.
    let mut topo_file =
        File::open(&topo_yml_file).map_err(|err| NamespaceError::FileOpen {
            path: topo_yml_file.clone(),
            source: err,
        })?;
    let topology = TopologyParser::from_yaml_file(&mut topo_file)?;

    // Fetch device.
    let router = topology
        .get_router(&router_name)
        .ok_or(ConfigError::UnknownNode(router_name))?;

    Ok(router)
}

fn prompt_device() -> String {
    println!("Device Name: ");
    let mut buf = String::new();
    let stdin = std::io::stdin();

    // TODO: Create an error class for below unwrap.
    stdin.read_line(&mut buf).unwrap();
    buf.trim().to_string()
}
