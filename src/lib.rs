#![feature(let_chains)]
pub mod devices;
pub mod error;
pub mod plugins;
pub mod topology;

pub type Result<T> = std::result::Result<T, error::Error>;

pub const ALLOWED_PLUGINS: [&str; 1] = ["cool"];
