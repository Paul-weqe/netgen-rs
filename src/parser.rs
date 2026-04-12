use std::collections::BTreeMap;

use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};
use yaml_rust2::Yaml;
use yaml_rust2::yaml::Hash;

use crate::NetResult;
use crate::devices::{Interface, Router, Switch, Volume};
use crate::error::{ConfigError, NetError, YamlPath};

// ==== trait FromYamlConfig ====

pub(crate) trait FromYamlConfig: Sized {
    fn from_yaml_config(
        name: &str,
        config: &Hash,
        context: BTreeMap<&str, &str>,
    ) -> NetResult<Self>;
}

// ==== impl Router ====

impl FromYamlConfig for Router {
    fn from_yaml_config(
        name: &str,
        router_config: &Hash,
        _router_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
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

                    match iface_config {
                        Yaml::Hash(iface_config) => {
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
                        Yaml::Null => {
                            // TODO: Make sure interface come up successfully
                            // on this config branch.
                            let interface =
                                Interface::new(iface_name.to_string());
                            router.interfaces.push(interface);
                        }
                        _ => {
                            return Err(ConfigError::IncorrectType {
                                path: YamlPath::new()
                                    .key("routers")
                                    .key(name)
                                    .key("interfaces")
                                    .key(iface_name)
                                    .unknown(),
                                expected: "hash".to_string(),
                            }
                            .into());
                        }
                    }
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
                for config in volumes_configs {
                    if let Yaml::Hash(config) = config {
                        let src = get_string_field(&config, "src")?;
                        let dst = get_string_field(&config, "dst")?;

                        router.volumes.push(Volume { src, dst });
                    }
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
        switch_config: &Hash,
        _switch_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        let mut switch = Self::new(switch_name);

        match switch_config.get(&Yaml::String(String::from("interfaces"))) {
            Some(Yaml::Hash(interfaces_config)) => {
                for (iface_name, iface_config) in interfaces_config {
                    match iface_name {
                        Yaml::String(iface_name) => match iface_config {
                            Yaml::Hash(iface_config) => {
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
                                    expected: "hash".to_string(),
                                }
                                .into());
                            }
                        },
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
        yaml_config: &Hash,
        iface_ctx: BTreeMap<&str, &str>,
    ) -> NetResult<Self> {
        let mut interface = Interface::new(iface_name.to_string());
        let mut yaml_path = match (
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

        // Get IPV4 Addresses.
        interface
            .addresses
            .extend(parse_ip_addresses::<Ipv4Network>(
                yaml_config,
                "ipv4",
                &yaml_path.key("ipv4").unknown(),
            )?);

        // Get Ipv6 Addresses.
        interface
            .addresses
            .extend(parse_ip_addresses::<Ipv6Network>(
                yaml_config,
                "ipv6",
                &yaml_path.key("ipv6").unknown(),
            )?);
        Ok(interface)
    }
}

fn parse_ip_addresses<N>(
    yaml_config: &Hash,
    key: &str,
    path: &YamlPath,
) -> NetResult<Vec<IpNetwork>>
where
    N: std::str::FromStr<Err = ipnetwork::IpNetworkError>,
    IpNetwork: From<N>,
{
    let Some(addr_list) = yaml_config.get(&Yaml::String(String::from(key)))
    else {
        return Ok(vec![]);
    };

    match addr_list {
        Yaml::Array(entries) => entries
            .iter()
            .filter_map(|y| {
                if let Yaml::String(s) = y {
                    Some(s)
                } else {
                    None
                }
            })
            .map(|addr_str| {
                addr_str.parse::<N>().map(IpNetwork::from).map_err(|err| {
                    ConfigError::InvalidAddress {
                        addr_type: key.to_string(),
                        address: addr_str.to_string(),
                        path: path.clone(),
                        source: err,
                    }
                    .into()
                })
            })
            .collect(),
        Yaml::Null => Ok(vec![]),
        _ => Err(ConfigError::IncorrectType {
            path: path.clone(),
            expected: "array".to_string(),
        }
        .into()),
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
