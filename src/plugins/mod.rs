use std::io::Result as IoResult;
use yaml_rust2::yaml::Hash;
use yaml_rust2::yaml::Yaml;

mod frr;
mod holo;

pub use frr::Frr;
pub use holo::Holo;

#[derive(Debug, Clone)]
pub enum Plugin {
    Holo(Holo),
    Frr(Frr),
}

#[derive(Debug)]
pub struct Config {
    pub(crate) plugins: Vec<Plugin>,
}

pub fn yaml_parse_config_contents(yaml_data: &Yaml) -> IoResult<Config> {
    let mut plugins_configs: Vec<Plugin> = vec![];
    if let Yaml::Hash(configs) = yaml_data {
        // look through the plugins
        if let Some((_config_name, devices)) =
            configs.get_key_value(&Yaml::String("plugins".to_string()))
        {
            if let Ok(plugins) = yaml_parse_plugins(devices) {
                plugins_configs = plugins;
            }
        }
    }
    let config = Config {
        plugins: plugins_configs,
    };
    Ok(config)
}

/// Fetches a list of all the plugins and
/// Parses each individual plugin to a
/// {plugin-name}_config() function
fn yaml_parse_plugins(yaml_devices: &Yaml) -> IoResult<Vec<Plugin>> {
    let mut plugins: Vec<Plugin> = vec![];
    if let Yaml::Hash(configured_plugins) = yaml_devices {
        for (plugin_name, plugin_config) in configured_plugins {
            if let Yaml::String(name) = plugin_name
                && let &Yaml::Hash(config) = &plugin_config
            {
                // TODO: throw an error for an invalid
                // plugin name
                if name == "holo" {
                    let holo_config = holo_config(config);
                    if let Some(holo_plugin_config) = holo_config {
                        plugins.push(holo_plugin_config);
                    }
                } else if name == "frr" {
                    let frr_config = frr_config(config);
                    if let Some(frr_plugin_config) = frr_config {
                        plugins.push(frr_plugin_config);
                    }
                }
            } else {
                // TODO: check for if the configs
                // for a plugin are not a Hash
            }
        }
    }
    Ok(plugins)
}

fn holo_config(config: &Hash) -> Option<Plugin> {
    let mut holo = Holo::default();

    // set holo-daemon path
    if let Some(daemon_path) = config.get(&Yaml::String(String::from("daemon"))) {
        holo.daemon_path = daemon_path.clone().into_string().unwrap();
    }

    // set holod cli path
    if let Some(cli_path) = config.get(&Yaml::String(String::from("cli-path"))) {
        holo.cli_path = cli_path.clone().into_string().unwrap();
    }

    // set holod sysconfdir
    if let Some(sysconfdir) = config.get(&Yaml::String(String::from("sysconfdir"))) {
        holo.sysconfdir = sysconfdir.clone().into_string().unwrap();
    }

    // set holod user
    if let Some(user) = config.get(&Yaml::String(String::from("user"))) {
        holo.user = user.clone().into_string().unwrap();
    }

    // set holod group
    if let Some(group) = config.get(&Yaml::String(String::from("group"))) {
        holo.group = group.clone().into_string().unwrap();
    }
    Some(Plugin::Holo(holo))
}

fn frr_config(config: &Hash) -> Option<Plugin> {
    let mut frr = Frr::default();
    // set frr daemon path
    if let Some(daemon_path) = config.get(&Yaml::String(String::from("daemon"))) {
        frr.daemon_path = daemon_path.clone().into_string().unwrap();
    }
    // set frr cli path
    if let Some(cli_path) = config.get(&Yaml::String(String::from("cli-path"))) {
        frr.cli_path = cli_path.clone().into_string().unwrap();
    }
    Some(Plugin::Frr(frr))
}
