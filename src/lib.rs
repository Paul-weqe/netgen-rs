#![feature(let_chains)]
pub mod devices;
pub mod error;
pub mod plugins;
pub mod topology;

use std::path::Path;

use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, unshare};
use nix::unistd::Pid;

pub type Result<T> = std::result::Result<T, error::Error>;

pub const ALLOWED_PLUGINS: [&str; 2] = ["holo", "frr"];
pub const PLUGIN_PIDS_FILE: &str = ".plugin-pids";

pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
pub const DEVICES_NS_DIR: &str = "/tmp/netgen-rs/ns/devices";

// If we are trying to mount the main pid, we leave device_name as None
pub fn mount_device(device_name: Option<String>, pid: Pid) -> Result<String> {
    let ns_path = match device_name {
        Some(device_name) => format!("{DEVICES_NS_DIR}/{device_name}"),
        None => format!("{NS_DIR}/main"),
    };

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
        return Err(error::Error::GeneralError(format!(
            "unable to create path {ns_path}"
        )));
    }

    Ok(ns_path)
}
