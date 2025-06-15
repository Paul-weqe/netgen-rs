#![feature(let_chains)]
pub mod devices;
pub mod error;
pub mod plugins;
pub mod topology;

pub type Result<T> = std::result::Result<T, error::Error>;

pub const ALLOWED_PLUGINS: [&str; 2] = ["holo", "frr"];
pub const PLUGIN_PIDS_FILE: &str = ".plugin-pids";

pub const NS_DIR: &str = "/tmp/netgen-rs/ns";
