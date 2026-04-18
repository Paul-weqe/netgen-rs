pub mod devices;
pub mod error;
pub mod parser;
pub mod topology;

use std::fs::{File, create_dir_all, remove_dir_all};
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::{fs, io};

use error::{NamespaceError, NetError};
use nix::mount::{MsFlags, mount, umount};
use nix::sched::{CloneFlags, setns, unshare};
use nix::sys::signal::{Signal, kill};
use nix::unistd::{ForkResult, Pid, fork, pause};
use tracing::{debug, error};

pub type NetResult<T> = std::result::Result<T, error::NetError>;

pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
pub const DEVICES_NS_DIR: &str = "/tmp/netgen-rs/ns/devices";
pub const MAIN_NS_DIR: &str = "/tmp/netgen-rs/ns/main";

// Returns Ok(device_netns_path, device_pidns_path)
pub fn mount_device(
    device_name: Option<String>,
) -> NetResult<(String, String)> {
    let device = DeviceDetails::new(device_name.clone());
    let clone_flags = match device_name {
        Some(_) => CloneFlags::CLONE_NEWNET | CloneFlags::CLONE_NEWPID,
        None => CloneFlags::CLONE_NEWNET,
    };

    unshare(clone_flags).map_err(|err| {
        NetError::NamespaceError(NamespaceError::Unshare {
            ns_name: device.name.clone(),
            source: err,
        })
    })?;

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

    // During the creation of the main namespace, we wait for the mounting of
    // the namespace first.
    if device_name.is_none() {
        debug!("Starting main namespace");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    //Go back to main namespace
    enter_ns(None)?;

    Ok((device.netns_path(), device.pidns_path()))
}

pub fn mount_router_volumes(router: &devices::Router) -> NetResult<()> {
    for volume in &router.volumes {
        mount_volume(&router.name, &volume)?;
    }
    Ok(())
}

fn mount_volume(device_name: &str, volume: &devices::Volume) -> NetResult<()> {
    let src_path = Path::new(&volume.src);
    let dst_path = Path::new(&volume.dst);

    if !src_path.exists() {
        let err = NetError::NamespaceError(NamespaceError::MountSrcNotFound(
            volume.src.clone(),
        ));
        return Err(err);
    }

    // Derive a stable staging path from the destination.
    let binding = match src_path.file_name() {
        Some(src_path_str) => PathBuf::from(format!(
            "/tmp/netgen-rs/ns/devices/{device_name}/vols/{}",
            src_path_str.to_string_lossy()
        )),
        None => src_path.to_path_buf(),
    };

    let staging_path = binding.as_path();

    // Create staging path if it doesn't exist.
    if !staging_path.exists() {
        if src_path.is_file() {
            fs::copy(&src_path, staging_path).map_err(|err| {
                NetError::BasicError(format!(
                    "Unable to copy {src_path:?} to {staging_path:?}\n{err:?}"
                ))
            })?;
        } else if src_path.is_dir() {
            copy_dir_all(&src_path, staging_path).map_err(|err| {
                NetError::BasicError(format!(
                    "Unable to copy {src_path:?} to {staging_path:?}\n{err:?}"
                ))
            })?;
        }
    }

    // Create dst path if it doesn't exist.
    if !dst_path.exists() {
        if src_path.is_file() {
            let parent = dst_path.parent().unwrap();
            fs::create_dir_all(parent).map_err(|err| {
                NetError::BasicError(format!(
                    "Unable to create volume path {:?}: {err:?}",
                    &parent
                ))
            })?;
            File::create(dst_path).map_err(|err| {
                NetError::BasicError(format!(
                    "Unable to create volume file {:?}: {err:?}",
                    &dst_path
                ))
            })?;
        } else if src_path.is_dir() {
            fs::create_dir_all(dst_path).map_err(|err| {
                NetError::BasicError(format!(
                    "Unable to create volume path {:?}: {err:?}",
                    &dst_path
                ))
            })?;
        }
    }

    mount(
        Some(staging_path),
        dst_path,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .map_err(|err| {
        NetError::NamespaceError(NamespaceError::Mount {
            ns_type: "volume mount".to_string(),
            device: device_name.to_string(),
            source: err,
        })
    })?;

    Ok(())
}

fn copy_dir_all(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
) -> io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
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
            return Err(NamespaceError::Fork {
                fork_function: String::from("enter_ns"),
                source: err,
            }
            .into());
        }
    }

    // Only continue with the child process.
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

    pause();
    Ok(())
}

/// Kills the PIDs the device namespaces are running on,
/// then unmounts their mountpoints.
pub fn destroy_ns(device_name: Option<String>) -> NetResult<()> {
    let device = DeviceDetails::new(device_name.clone());

    if let Some(pid) = find_pid_from_mountpoint(&device.netns_path()) {
        kill_process(pid)?;
    }

    umount_ns(device_name)
}

pub fn kill_process(pid: i32) -> NetResult<()> {
    kill(Pid::from_raw(pid), Signal::SIGKILL).map_err(|err| {
        NetError::BasicError(format!(
            "Unable to kill process PID {pid} : {err:?}"
        ))
    })
}

/// Deletes the namespace created by the Router (if it exists)
/// If deleting the main namespace, we have device_name as None.
pub fn umount_ns(device_name: Option<String>) -> NetResult<()> {
    let device = DeviceDetails::new(device_name.clone());
    let net_ns_path = device.netns_path();
    let pid_ns_path = device.pidns_path();

    umount(net_ns_path.as_str()).map_err(|err| {
        error!(
            router = %device.name,
            error = %err,"issue unmounting namespace"
        );
        NamespaceError::Unmount {
            path: net_ns_path.clone(),
            source: err,
        }
    })?;

    umount(pid_ns_path.as_str()).map_err(|err| {
        error!(
            router = %device.name,
            error = %err,"issue unmounting namespace"
        );
        NamespaceError::Unmount {
            path: pid_ns_path.clone(),
            source: err,
        }
    })?;

    // Remove the files.
    remove_dir_all(&device.home_path).map_err(|err| {
        error!(router = %device.name, error = %err, dir=%device.home_path,
                    "problem removing directory");
        NetError::BasicError(format!(
            "Unable to remove directory {:?}: {err:?}",
            device.home_path
        ))
    })?;

    debug!(router = %device.name, "deleted");
    Ok(())
}

/// When devices are created they are created with '{device-dir}/net' and
/// '{device-dir}/pid' which are mount points for the network and pid namespaces.
///
/// We fetch the PID for the device by getting the associated PID of the network
/// namespace mount point, we can't use the PID mount point since not all of
/// the namespaces have them.
pub fn find_pid_from_mountpoint(mountpoint: &str) -> Option<i32> {
    let inode = fs::metadata(mountpoint).ok()?.ino();

    let proc = fs::read_dir("/proc").ok()?;
    for entry in proc.flatten() {
        let name = entry.file_name();
        let pid_str = name.to_str()?;

        // Ignore dirs & files that are not numbers.
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let ns_file = format!("/proc/{}/ns/net", pid_str);
        if let Ok(meta) = fs::metadata(&ns_file) {
            if meta.ino() == inode {
                return pid_str.parse().ok();
            }
        }
    }
    None
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
                home_path: MAIN_NS_DIR.to_string(),
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
