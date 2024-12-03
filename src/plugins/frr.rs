#[derive(Debug, Clone)]
pub struct Frr {
    pub daemon_path: String,
    pub cli_path: String,
}

impl Default for Frr {
    fn default() -> Self {
        Self {
            daemon_path: String::from("/usr/bin/frrd"),
            cli_path: String::from("/usr/bin/frr-cli"),
        }
    }
}
