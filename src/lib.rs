pub mod devices;
pub mod error;
pub mod topology;

use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{BufRead, Write};
use std::os::fd::AsFd;
use std::path::Path;

use error::{NamespaceError, NetError};
use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, setns, unshare};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{ForkResult, Pid, fork, pause};
use tracing::{debug, error};

pub type NetResult<T> = std::result::Result<T, error::NetError>;

pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
pub const DEVICES_NS_DIR: &str = "/tmp/netgen-rs/ns/devices";
pub const PID_FILE: &str = "/tmp/netgen-rs/ns/main/.pid";

// If we are trying to mount the main pid, we leave device_name as None
pub fn mount_device(device_name: Option<String>) -> NetResult<String> {
    let device = DeviceDetails::new(device_name.clone());

    unshare(CloneFlags::CLONE_NEWNET | CloneFlags::CLONE_NEWPID).map_err(
        |err| {
            NetError::NamespaceError(NamespaceError::Unshare {
                ns_name: device.name.clone(),
                source: err,
            })
        },
    )?;

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            create_ns(&device)?;
        }
        Ok(ForkResult::Parent { .. }) => {
            // Waiting for child
        }
        Err(err) => {
            error!(
                device = %device.name,
                "unable to create a second child fork: {err:?}"
            );
        }
    }

    // If this is the main namespace being created, we may have to wait for
    // the mounting process.
    if device_name.is_none() {
        debug!("mounting main namespace");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    //Go back to main namespace
    enter_ns(None)?;

    Ok(device.netns_path())
}

/// When we want to enter the main namespace, the `device_name` is None.
/// If not, specify the name of the device e.g Some("router-1").
pub fn enter_ns(device_name: Option<String>) -> NetResult<()> {
    let device = DeviceDetails::new(device_name.clone());
    let device_net_path = device.netns_path();
    let device_pid_path = device.pidns_path();

    let device_net_file =
        File::open(device_net_path.as_str()).map_err(|err| {
            NamespaceError::FileOpen {
                path: device_net_path.clone(),
                source: err,
            }
        })?;

    let device_pid_file =
        File::open(device_pid_path.as_str()).map_err(|err| {
            NamespaceError::FileOpen {
                path: device_pid_path.clone(),
                source: err,
            }
        })?;

    setns(device_net_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(
        |source| {
            NetError::NamespaceError(NamespaceError::ReturnToMain { source })
        },
    )?;

    // If we are forking from / to main, we don't CLONE_NEWPID.
    setns(device_pid_file.as_fd(), CloneFlags::CLONE_NEWPID).map_err(
        |source| {
            NetError::NamespaceError(NamespaceError::ReturnToMain { source })
        },
    )?;

    let pid = Pid::this();
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            // Just forking so we can actually enter the PID namespace.
        }
        Ok(ForkResult::Parent { child }) => {
            nix::sys::wait::waitpid(child, None).map_err(|err| {
                NamespaceError::Fork {
                    fork_function: String::from("enter_ns"),
                    source: err,
                }
            })?;
        }
        Err(err) => {
            return Err(NetError::NamespaceError(NamespaceError::Fork {
                fork_function: String::from("enter_ns"),
                source: err,
            }));
        }
    }

    if Pid::this() == pid {
        std::process::exit(0);
    }

    Ok(())
}

fn create_ns(device: &DeviceDetails) -> NetResult<()> {
    create_dir_all(&device.home_path).map_err(|e| {
        NetError::NamespaceError(NamespaceError::PathCreation {
            path: device.home_path.clone(),
            source: e,
        })
    })?;

    // Create net namespace.
    File::create(device.netns_path()).map_err(|err| {
        NetError::BasicError(format!(
            "unable to create path {} -> {err:?}",
            &device.netns_path()
        ))
    })?;
    let proc_net_ns_path = "/proc/self/ns/net".to_string();
    let net_path = &device.netns_path();
    let target_net_path = Path::new(net_path);

    mount(
        Some(proc_net_ns_path.as_str()),
        target_net_path.as_os_str(),
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )
    .map_err(|err| {
        NetError::NamespaceError(NamespaceError::Mount {
            ns_type: String::from("network"),
            device: device.name.clone(),
            source: err,
        })
    })?;

    // Create PID namespace.
    File::create(device.pidns_path()).map_err(|err| {
        NetError::BasicError(format!(
            "unable to create path {} -> {err:?}",
            &device.pidns_path()
        ))
    })?;

    let proc_pid_ns_path = "/proc/self/ns/pid".to_string();
    let pid_path = &device.pidns_path();
    let target_pid_path = Path::new(pid_path);

    mount(
        Some(proc_pid_ns_path.as_str()),
        target_pid_path.as_os_str(),
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    )
    .map_err(|err| {
        NetError::NamespaceError(NamespaceError::Mount {
            ns_type: String::from("pid"),
            device: device.name.clone(),
            source: err,
        })
    })?;

    // Path to the .pid file for the namespace.
    let pid_path = format!("{}/.pid", device.home_path);

    // Create PID file.
    if let Ok(mut f) = File::create(pid_path) {
        let _ = writeln!(f, "{}", Pid::this().as_raw());
    }

    pause();
    Ok(())
}

/// Kills the process specified in the file.
/// Mostly a .pid file.
pub fn kill_process(pid_file: &str) -> NetResult<()> {
    // Kills all the running plugin PIDs.
    if let Ok(file) = OpenOptions::new().read(true).open(pid_file) {
        let mut reader = std::io::BufReader::new(file);
        let mut pid = String::new();
        let _ = reader.read_line(&mut pid).map_err(|err| {
            NetError::BasicError(format!(
                "Unable to read file:{pid_file} : {err:?}"
            ))
        })?;

        if let Ok(pid) = pid.trim().parse::<i32>() {
            kill(Pid::from_raw(pid), Signal::SIGKILL).map_err(|err| {
                NetError::BasicError(format!(
                    "Issue killing process PID {pid} : {err:?}"
                ))
            })?;
        }
    }
    Ok(())
}

// ==== struct DeviceDetails ====

pub struct DeviceDetails {
    pub name: String,
    pub home_path: String,
}

impl DeviceDetails {
    pub fn new(name: Option<String>) -> DeviceDetails {
        match name {
            Some(name) => Self {
                name: name.clone(),
                home_path: format!("{DEVICES_NS_DIR}/{name}"),
            },
            None => Self {
                name: "main".to_string(),
                home_path: format!("{NS_DIR}/main"),
            },
        }
    }

    // Network namespace Path.
    pub fn netns_path(&self) -> String {
        format!("{}/net", self.home_path)
    }

    // PID namespace Path.
    pub fn pidns_path(&self) -> String {
        format!("{}/pid", self.home_path)
    }
}
