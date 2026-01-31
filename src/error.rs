use std::io::Error as IoError;

use thiserror::Error as ThisError;
use yaml_rust2::scanner::ScanError;

#[derive(Debug)]
pub enum Error {
    GeneralError(String),
    IoError(IoError),
    ScanError(ScanError),

    // 1st &str is name of field,2nd &str is type that has been configured
    IncorrectYamlType(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GeneralError(error) => write!(f, "{}", error.as_str()),
            Self::IoError(error) => std::fmt::Display::fmt(error, f),
            Self::ScanError(error) => std::fmt::Display::fmt(error, f),
            Self::IncorrectYamlType(name) => {
                write!(f, "Incorrect yaml type for field \"{name}\".")
            }
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, ThisError)]
pub enum NetError {
    #[error("{0}")]
    BasicError(String),

    #[error(transparent)]
    ConfigError(#[from] ConfigError),
}

#[derive(Debug, ThisError)]
pub enum ConfigError {
    #[error("Link {src} <-> {dst} configured multiple times")]
    DuplicateLink { src: String, dst: String },

    #[error("Node {0} has been configured multiple times")]
    DuplicateNode(String),

    #[error("Field '{field}' has incorrect type (expected {expected})")]
    IncorrectType { field: String, expected: String },

    #[error("missing required field {0}")]
    MissingField(String),

    #[error("Link references to unknown node {0}")]
    UnknownNode(String),

    #[error("Invalid YAML Syntax {0}")]
    YamlSyntax(#[from] ScanError),

    #[error(
        "Invalid {addr_type} address '{address}' for interface '{interface}'"
    )]
    InvalidAddress {
        addr_type: String,
        address: String,
        interface: String,
    },
}
