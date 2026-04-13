use std::collections::BTreeMap;

use ipnetwork::IpNetwork;
use yaml_rust2::yaml::{Hash, Yaml};

use crate::NetResult;
use crate::devices::{Interface, Router, Switch, Volume};
use crate::error::{ConfigError, NetError, YamlPath};

// ==== trait FromYamlConfig ====

pub(crate) trait FromYamlConfig: Sized {
    fn from_yaml_config(
        name: &str,
        config: &Yaml,
        context: BTreeMap<&str, &str>,
    ) -> NetResult<Self>;
}

// ==== impl Router ====

impl FromYamlConfig for Router {
    fn from_yaml_config(
        name: &str,
        router_config: &Yaml,
        _router_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        let router_config = if let Yaml::Hash(router_config) = router_config {
            router_config
        } else {
            return Err(ConfigError::IncorrectType {
                path: YamlPath::new().key("routers").key(name).unknown(),
                expected: "hash".to_string(),
            }
            .into());
        };

        let mut router = Self::new(name);
        // Router Interface Configurations.
        match router_config.get(&Yaml::String(String::from("interfaces"))) {
            Some(Yaml::Hash(interfaces_config)) => {
                for (iface_name, iface_config) in interfaces_config {
                    let iface_name = match iface_name {
                        Yaml::String(iface_name) => iface_name,
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                path: YamlPath::new()
                                    .key("routers")
                                    .key(name)
                                    .key("interfaces")
                                    .unknown(),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    };

                    let interface = Interface::from_yaml_config(
                        &iface_name,
                        iface_config,
                        BTreeMap::from([
                            ("device_name", name),
                            ("device_type", "router"),
                        ]),
                    )?;
                    router.interfaces.push(interface);
                }
            }
            Some(Yaml::Null) | None => {
                /* Ignore router interfaces config. */
            }
            Some(_) => {
                return Err(ConfigError::IncorrectType {
                    path: YamlPath::new()
                        .key("routers")
                        .key(name)
                        .key("interfaces")
                        .unknown(),
                    expected: "hash".to_string(),
                }
                .into());
            }
        }

        // Router Volume Configurations.
        match router_config.get(&Yaml::String(String::from("volumes"))) {
            Some(Yaml::Array(volumes_configs)) => {
                for volume_config in volumes_configs {
                    let volume = Volume::from_yaml_config(
                        router.name.as_str(),
                        volume_config,
                        BTreeMap::new(),
                    )?;
                    router.volumes.push(volume);
                }
            }
            Some(Yaml::Null) | None => { /* Ignore volume configs. */ }
            Some(_) => {
                return Err(ConfigError::IncorrectType {
                    path: YamlPath::new()
                        .key("routers")
                        .key(name)
                        .key("volumes")
                        .unknown(),
                    expected: "hash".to_string(),
                }
                .into());
            }
        }

        // Router Scripts Configurations.
        match router_config.get(&Yaml::String(String::from("scripts"))) {
            Some(Yaml::Array(script_configs)) => {
                for script in script_configs {
                    if let Yaml::String(path) = script {
                        router.scripts.push(path.clone());
                    }
                }
            }
            Some(Yaml::Null) | None => {}
            Some(_) => {
                return Err(ConfigError::IncorrectType {
                    path: YamlPath::new()
                        .key("routers")
                        .key(name)
                        .key("scripts")
                        .unknown(),

                    expected: "array".to_string(),
                }
                .into());
            }
        }
        Ok(router)
    }
}

// ==== impl Switch ====

impl FromYamlConfig for Switch {
    /// Handles config that is in the form of:
    ///
    /// ```yaml
    /// sw1:
    ///   interfaces:
    ///     eth0:
    ///       ipv4:
    ///         - 192.168.100.20/24
    ///       ipv6:
    ///         - 2001:db8::/96
    /// ```
    /// converted into a yaml_rust2::yaml::Hash;
    fn from_yaml_config(
        switch_name: &str,
        switch_config: &Yaml,
        _switch_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        let switch_config = if let Yaml::Hash(switch_config) = switch_config {
            switch_config
        } else {
            return Err(ConfigError::IncorrectType {
                path: YamlPath::new()
                    .key("switches")
                    .key(switch_name)
                    .unknown(),
                expected: "hash".to_string(),
            }
            .into());
        };

        let mut switch = Self::new(switch_name);

        match switch_config.get(&Yaml::String(String::from("interfaces"))) {
            Some(Yaml::Hash(interfaces_config)) => {
                for (iface_name, iface_config) in interfaces_config {
                    match iface_name {
                        Yaml::String(iface_name) => {
                            let interface = Interface::from_yaml_config(
                                &iface_name,
                                iface_config,
                                BTreeMap::from([
                                    ("device_name", switch_name),
                                    ("device_type", "switch"),
                                ]),
                            )?;
                            switch.interfaces.push(interface);
                        }
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                path: YamlPath::new()
                                    .key("switches")
                                    .key(switch_name)
                                    .key("interfaces")
                                    .unknown(),
                                expected: "string".to_string(),
                            }
                            .into());
                        }
                    }
                }
                Ok(switch)
            }
            _ => Err(ConfigError::IncorrectType {
                path: YamlPath::new()
                    .key("switches")
                    .key(switch_name)
                    .unknown(),
                expected: "hash".to_string(),
            }
            .into()),
        }
    }
}

// ==== impl Interface ====

impl FromYamlConfig for Interface {
    fn from_yaml_config(
        iface_name: &str,
        iface_config: &Yaml,
        iface_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        let yaml_path = match (
            iface_ctx.get("device_name"),
            iface_ctx.get("device_type"),
        ) {
            (Some(device_name), Some(device_type)) => {
                if *device_type == "switch" {
                    YamlPath::new()
                        .key("switches")
                        .key(*device_name)
                        .key("interfaces")
                        .key(iface_name)
                } else if *device_type == "router" {
                    YamlPath::new()
                        .key("routers")
                        .key(*device_name)
                        .key("interfaces")
                        .key(iface_name)
                } else {
                    return Err(NetError::BasicError(format!(
                        "Unidentified device type {device_type}"
                    )));
                }
            }
            (_, _) => {
                // TODO: Add more specified error checks.
                return Err(NetError::BasicError(format!(
                    "device_name or device_type not specified for iface {iface_name}"
                )));
            }
        };

        // Confirm Yaml config type.
        let mut interface = Interface::new(iface_name.to_string());
        let addr_array = match iface_config {
            Yaml::Array(addr_array) => addr_array,
            Yaml::Null => {
                return Ok(interface);
            }
            _ => {
                return Err(ConfigError::IncorrectType {
                    path: yaml_path.clone(),
                    expected: "array".to_string(),
                }
                .into());
            }
        };

        for addr in addr_array {
            if let Yaml::String(addr_str) = addr {
                let ip: IpNetwork =
                    addr_str.as_str().parse().map_err(|err| {
                        ConfigError::InvalidAddress {
                            address: addr_str.to_string(),
                            path: yaml_path.clone(),
                            source: err,
                        }
                    })?;
                interface.addresses.push(ip);
            } else {
                return Err(ConfigError::IncorrectType {
                    path: yaml_path,
                    expected: "string".to_string(),
                }
                .into());
            }
        }

        Ok(interface)
    }
}

// ==== impl Volume ====

impl FromYamlConfig for Volume {
    fn from_yaml_config(
        router_name: &str,
        volume_config: &Yaml,
        _router_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        if let Yaml::Hash(map) = volume_config {
            if let Some((Yaml::String(src), Yaml::String(dst))) =
                map.iter().next()
            {
                return Ok(Volume {
                    src: src.to_string(),
                    dst: dst.to_string(),
                });
            } else {
                return Err(ConfigError::IncorrectType {
                    path: YamlPath::new()
                        .key("routers")
                        .key(router_name)
                        .key("volumes")
                        .unknown(),
                    expected: "'string':'string'".to_string(),
                }
                .into());
            }
        } else {
            return Err(ConfigError::IncorrectType {
                path: YamlPath::new()
                    .key("routers")
                    .key(router_name)
                    .key("volumes")
                    .unknown(),
                expected: "array".to_string(),
            }
            .into());
        }
    }
}

// Get field value from Yaml Hash.
// TODO: have this better positioned with other YANG configs methods.
pub(crate) fn get_string_field(
    config: &Hash,
    field: &str,
) -> NetResult<String> {
    let field_value =
        config
            .get(&Yaml::String(field.to_string()))
            .ok_or_else(|| ConfigError::MissingField {
                path: YamlPath::new().key(field).unknown(),
            })?;

    match field_value {
        Yaml::String(value) => Ok(value.to_string()),
        _ => Err(ConfigError::IncorrectType {
            path: YamlPath::new().key(field).unknown(),
            expected: "string".to_string(),
        }
        .into()),
    }
}
