#[derive(Debug, Clone)]
pub struct Holo {
    pub daemon_dir: String,
    pub cli_dir: String,
    pub sysconfdir: String,
    pub user: String,
    pub group: String,
}

impl Default for Holo {
    fn default() -> Self {
        Self {
            daemon_dir: String::from("/usr/bin/"),
            cli_dir: String::from("/usr/bin/"),
            sysconfdir: String::from("/etc/holod"),
            user: String::from("holo"),
            group: String::from("holo"),
        }
    }
}
