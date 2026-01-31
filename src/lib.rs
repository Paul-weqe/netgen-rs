pub mod devices;
pub mod error;
pub mod topology;

use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{BufRead, Write};
use std::os::fd::AsFd;
use std::path::Path;

use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, setns, unshare};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{ForkResult, Pid, fork, pause};

pub type Result<T> = std::result::Result<T, error::Error>;

pub const ALLOWED_PLUGINS: [&str; 2] = ["holo", "frr"];
pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
pub const DEVICES_NS_DIR: &str = "/tmp/netgen-rs/ns/devices";
pub const PID_FILE: &str = "/tmp/netgen-rs/ns/main/.pid";

// If we are trying to mount the main pid, we leave device_name as None
pub fn mount_device(device_name: Option<String>, pid: Pid) -> Result<String> {
    let device = DeviceDetails::new(device_name);
    unshare(CloneFlags::CLONE_NEWNET).unwrap();

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            create_ns(&device)?;
        }
        Ok(ForkResult::Parent { .. }) => {
            // Waiting for child
        }
        Err(err) => {
            println!("unable to create a second child fork -> {err:?}")
        }
    }

    let main_net_path = format!("/proc/{}/ns/net", pid.as_raw());
    let main_net_file = File::open(main_net_path.as_str())
        .expect(format!("unable to open file {:?}", main_net_path).as_str());

    setns(main_net_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(|err| {
        error::Error::GeneralError(format!(
            "unable to move back to main namespace -> {err:?}"
        ))
    })?;

    //Go back to main namespace
    Ok(device.netns_path())
}

fn create_ns(device: &DeviceDetails) -> Result<()> {
    create_dir_all(&device.home_path).map_err(|e| {
        error::Error::GeneralError(format!(
            "unable to create namespace directory {}\n{}",
            &device.home_path,
            e.to_string()
        ))
    })?;

    match File::create(&device.netns_path()) {
        Ok(_) => {
            let proc_ns_path = format!("/proc/self/ns/net");
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
                    "unable to mound netns for {} on path -> {err:?}",
                    &device.name
                ))
            })?;

            // Path to the .pid file for the namespace.
            let pid_path = format!("{}/.pid", device.home_path);

            // Create PID file.
            if let Ok(mut f) = File::create(pid_path) {
                let _ = writeln!(f, "{}", Pid::this().as_raw());
            }
            pause();
        }
        Err(err) => {
            return Err(error::Error::GeneralError(format!(
                "unable to create path {} -> {err:?}",
                &device.netns_path()
            )));
        }
    }
    Ok(())
}

// Kills the process specified in the file.
// Mostly a .pid file.
pub fn kill_process(pid_file: &str) -> Result<()> {
    println!("killing {pid_file}");

    // Kills all the running plugin PIDs.
    if let Ok(file) = OpenOptions::new().read(true).open(pid_file) {
        let mut reader = std::io::BufReader::new(file);
        let mut pid = String::new();
        let _ = reader.read_line(&mut pid).unwrap();

        if let Ok(pid) = pid.trim().parse::<i32>() {
            kill(Pid::from_raw(pid), Signal::SIGKILL).unwrap();
        }
    }
    Ok(())
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

    // Network namespace Path.
    pub fn netns_path(&self) -> String {
        format!("{}/net", self.home_path)
    }

    // PID namespace Path.
    pub fn pidns_path(&self) -> String {
        format!("{}/pid", self.home_path)
    }
}
