use std::io::Error as IoError;

use yaml_rust2::scanner::ScanError;

use crate::ALLOWED_PLUGINS;

#[derive(Debug)]
pub enum Error {
    GeneralError(String),
    IoError(IoError),
    ScanError(ScanError),

    // Vec<String> -> allowed names, String -> configured name
    InvalidPluginName(String),

    // 1st &str is name of field,2nd &str is type that has been configured
    IncorrectYamlType(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GeneralError(error) => write!(f, "{}", error.as_str()),
            Self::IoError(error) => std::fmt::Display::fmt(error, f),
            Self::ScanError(error) => std::fmt::Display::fmt(error, f),
            Self::InvalidPluginName(plugin_name) => {
                write!(
                    f,
                    "Invalid plugin name {plugin_name}. \nAllowed plugins => {ALLOWED_PLUGINS:?}",
                )
            }
            Self::IncorrectYamlType(name) => {
                write!(f, "Incorrect yaml type for field \"{name}\".")
            }
        }
    }
}

impl std::error::Error for Error {}
