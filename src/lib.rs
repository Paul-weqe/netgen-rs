#![feature(let_chains)]
pub mod devices;
pub mod error;
pub mod topology;

use std::fs::File;
use std::os::fd::AsFd;
use std::path::Path;

use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, setns, unshare};
use nix::unistd::Pid;

pub type Result<T> = std::result::Result<T, error::Error>;

pub const ALLOWED_PLUGINS: [&str; 2] = ["holo", "frr"];

pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
pub const DEVICES_NS_DIR: &str = "/tmp/netgen-rs/ns/devices";
pub const PID_FILE: &str = "/tmp/netgen-rs/ns/.pid";

// If we are trying to mount the main pid, we leave device_name as None
pub fn mount_device(device_name: Option<String>, pid: Pid) -> Result<String> {
    let ns_path = match device_name {
        Some(device_name) => format!("{DEVICES_NS_DIR}/{device_name}"),
        None => format!("{NS_DIR}/main"),
    };

    match std::fs::File::create(ns_path.as_str()) {
        Ok(_) => {
            let _ = unshare(CloneFlags::CLONE_NEWNET);
            let proc_ns_path = format!("/proc/{}/ns/net", pid.as_raw());
            let target_path = Path::new(&ns_path);

            mount(
                Some(proc_ns_path.as_str()),
                target_path.as_os_str(),
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            )
            .map_err(|err| {
                error::Error::GeneralError(format!(
                    "unable to mount PID {ns_path} on {proc_ns_path} -> {err:?}"
                ))
            })?;
        }
        Err(err) => {
            return Err(error::Error::GeneralError(format!(
                "unable to create path {ns_path} -> {err:?}"
            )));
        }
    }

    if std::fs::File::create(ns_path.as_str()).is_ok() {
        let _ = unshare(CloneFlags::CLONE_NEWNET);
        let proc_ns_path = format!("/proc/{}/ns/net", pid.as_raw());
        let target_path = Path::new(&ns_path);

        mount(
            Some(proc_ns_path.as_str()),
            target_path.as_os_str(),
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        )
        .map_err(|err| {
            error::Error::GeneralError(format!(
                "unable to mount PID {ns_path} on {proc_ns_path} -> {err:?}",
            ))
        })?;
    } else {
        eprintln!("unable to create file {:?}", ns_path.as_str());
    }

    // Go back to main namespace
    let main_ns_path = format!("{NS_DIR}/main");
    if let Ok(main_file) = File::open(main_ns_path.as_str())
        && let Ok(_) = setns(main_file.as_fd(), CloneFlags::CLONE_NEWNET)
    {
        Ok(ns_path)
    } else {
        Err(error::Error::GeneralError(
            "unable to move back to main namespace".to_string(),
        ))
    }
}
