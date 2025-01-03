use crate::Result;
use crate::PLUGIN_PIDS_FILE;

use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;

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

impl Holo {
    /// Runs the holo daemon in the node it is in.
    /// ....
    pub fn run(&self) -> Result<()> {
        let holod_path = format!("{}/holod", self.daemon_dir);
        let mut command = Command::new(holod_path.as_str());

        match command.spawn() {
            Ok(mut child) => {
                if let Ok(mut file) = OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(PLUGIN_PIDS_FILE)
                {
                    let _ = writeln!(file, "{}", child.id());
                }
                let _ = child.try_wait();
            }
            Err(_err) => {
                // TODO: handle when spawning holod command fails
            }
        }
        Ok(())
    }

    // Run the router's startup config
    pub fn run_startup_config(&self, startup_config: String) -> Result<()> {
        let cli_path = format!("{}/holo-cli", self.cli_dir);
        let mut command = Command::new(cli_path);
        command.args(["--file", &startup_config]);
        let _ = command.spawn();
        // TODO: Throw error in case running holo-cli failed

        Ok(())
    }
}
