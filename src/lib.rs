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
    let device = DeviceDetails::new(device_name);

    std::fs::create_dir_all(&device.home_path).map_err(|e| {
        error::Error::GeneralError(format!(
            "unable to create namespace directory {}\n{}",
            &device.home_path,
            e.to_string()
        ))
    })?;

    match std::fs::File::create(&device.netns_path()) {
        Ok(_) => {
            let _ = unshare(CloneFlags::CLONE_NEWNET);
            let proc_ns_path = format!("/proc/{}/ns/net", pid.as_raw());
            let net_path = &device.netns_path();
            let target_path = Path::new(net_path);

            mount(
                Some(proc_ns_path.as_str()),
                target_path.as_os_str(),
                None::<&str>,
                MsFlags::MS_BIND,
                None::<&str>,
            )
            .map_err(|err| {
                error::Error::GeneralError(format!(
                    "unable to mount PID {} on {proc_ns_path} -> {err:?}",
                    &device.netns_path()
                ))
            })?;
        }
        Err(err) => {
            return Err(error::Error::GeneralError(format!(
                "unable to create path {} -> {err:?}",
                &device.netns_path()
            )));
        }
    }

    //Go back to main namespace
    let main_net_path = format!("{NS_DIR}/main/net");

    if let Ok(main_file) = File::open(main_net_path.as_str())
        && let Ok(_) = setns(main_file.as_fd(), CloneFlags::CLONE_NEWNET)
    {
        Ok(device.netns_path())
    } else {
        Err(error::Error::GeneralError(
            "unable to move back to main namespace".to_string(),
        ))
    }
}

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
                name: format!("main"),
                home_path: format!("{NS_DIR}/main"),
            },
        }
    }

    pub fn netns_path(&self) -> String {
        format!("{}/net", self.home_path)
    }
}
