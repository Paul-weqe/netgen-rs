use std::fs::File;
use std::io::{Read, Result as IoResult};
use yaml_rust2::yaml::Hash;
use yaml_rust2::yaml::Yaml;
use yaml_rust2::YamlLoader;

mod frr;
mod holo;

pub use frr::Frr;
pub use holo::Holo;

#[derive(Debug, Clone)]
pub enum Plugin {
    Holo(Holo),
    Frr(Frr),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub(crate) plugins: Vec<Plugin>,
}

impl Config {
    pub fn from_yaml_file(file: &mut File) -> IoResult<Self> {
        let mut contents = String::new();
        let _ = file.read_to_string(&mut contents);
        Self::from_yaml_str(contents.as_str())
    }

    pub fn from_yaml_str(yaml_str: &str) -> IoResult<Self> {
        let data = YamlLoader::load_from_str(yaml_str).unwrap();
        Self::from_yaml(&data)
    }

    pub fn from_yaml(yaml_data: &Vec<Yaml>) -> IoResult<Self> {
        let mut plugins_configs: Vec<Plugin> = vec![];

        for single_config in yaml_data {
            if let Yaml::Hash(configs) = single_config {
                // look through the plugins
                if let Some(plugin_params) = configs.get(&Yaml::String("plugins".to_string()))
                    && let Ok(plugins) = Self::yaml_parse_plugins(plugin_params)
                {
                    plugins_configs = plugins;
                }
            }
        }
        let config = Self {
            plugins: plugins_configs,
        };
        Ok(config)
    }

    /// Fetches a list of all the plugins and
    /// Parses each individual plugin to a
    /// {plugin-name}_config() function
    /// e.g holo_plugin(), frr_plugin() etc...
    fn yaml_parse_plugins(plugin_configs: &Yaml) -> IoResult<Vec<Plugin>> {
        let mut plugins: Vec<Plugin> = vec![];
        if let Yaml::Hash(configured_plugins) = plugin_configs {
            for (plugin_name, plugin_config) in configured_plugins {
                if let Yaml::String(name) = plugin_name
                    && let &Yaml::Hash(config) = &plugin_config
                {
                    match name.as_str() {
                        "holo" => {
                            let holo_config = Self::holo_config(config);
                            if let Some(holo_plugin_config) = holo_config {
                                plugins.push(holo_plugin_config);
                            }
                        }
                        "frr" => {
                            let frr_config = Self::frr_config(config);
                            if let Some(frr_plugin_config) = frr_config {
                                plugins.push(frr_plugin_config);
                            }
                        }
                        _ => {
                            // TODO: throw an error for an invalid
                            // plugin name
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
        if let Some(daemon_dir) = config.get(&Yaml::String(String::from("daemon-dir"))) {
            holo.daemon_dir = daemon_dir.clone().into_string().unwrap();
        }

        // set holod cli path
        if let Some(cli_dir) = config.get(&Yaml::String(String::from("cli-dir"))) {
            holo.cli_dir = cli_dir.clone().into_string().unwrap();
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
        if let Some(daemon_dir) = config.get(&Yaml::String(String::from("daemon-dir"))) {
            frr.daemon_dir = daemon_dir.clone().into_string().unwrap();
        }
        // set frr cli path
        if let Some(cli_dir) = config.get(&Yaml::String(String::from("cli-dir"))) {
            frr.cli_dir = cli_dir.clone().into_string().unwrap();
        }
        Some(Plugin::Frr(frr))
    }
}
