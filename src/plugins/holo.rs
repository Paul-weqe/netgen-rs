#[derive(Debug, Clone)]
pub struct Holo {
    pub daemon_path: String,
    pub cli_path: String,
    pub sysconfdir: String,
    pub user: String,
    pub group: String,
}

impl Default for Holo {
    fn default() -> Self {
        Self {
            daemon_path: String::from("/usr/bin/holod"),
            cli_path: String::from("/usr/bin/holo-cli"),
            sysconfdir: String::from("/etc/holod"),
            user: String::from("holo"),
            group: String::from("holo"),
        }
    }
}
