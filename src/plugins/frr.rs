#[derive(Debug, Clone)]
pub struct Frr {
    pub daemon_dir: String,
    pub cli_dir: String,
}

impl Default for Frr {
    fn default() -> Self {
        Self {
            daemon_dir: String::from("/usr/bin/frrd"),
            cli_dir: String::from("/usr/bin/frr-cli"),
        }
    }
}
