use std::fmt;

use ipnetwork::IpNetwork;
use thiserror::Error as ThisError;
use yaml_rust2::scanner::ScanError;

#[derive(Debug, ThisError)]
pub enum NetError {
    #[error("{0}")]
    BasicError(String),

    #[error(transparent)]
    ConfigError(#[from] ConfigError),

    #[error(transparent)]
    NamespaceError(#[from] NamespaceError),

    #[error(transparent)]
    LoginError(#[from] LoginError),

    #[error(transparent)]
    LinkError(#[from] LinkError),
}

// TODO: Look into customizing the LoginErrors. Currently mushed
// together with ConfigErrors. Improved partitioning of the two.
#[derive(Debug, ThisError)]
pub enum LoginError {
    #[error("Unable to reach {0}, make sure device has been turned on.")]
    HostUnreachable(String),
}

#[derive(Debug, ThisError)]
pub enum ConfigError {
    #[error("Topology file not configured.")]
    TopologyFileMissing,

    #[error("Sim name is not configured.")]
    SimNameMissing,

    #[error("Device name is not configured.")]
    DeviceNameMissing,

    #[error("Link {src} <-> {dst} configured multiple times.")]
    DuplicateLink { src: String, dst: String },

    #[error("Node {0} has been configured multiple times.")]
    DuplicateNode(String),

    #[error("Field has incorrect type. Expected '{expected}':\n{path}")]
    IncorrectType { path: YamlPath, expected: String },

    #[error("Required field is missing:\n{path}")]
    MissingField { path: YamlPath },

    #[error("Link references to unknown node {0}.")]
    UnknownNode(String),

    #[error("Invalid YAML Syntax {0}.")]
    YamlSyntax(#[from] ScanError),

    #[error("Invalid address '{address}' for interface:\n{path}")]
    InvalidAddress {
        address: String,
        path: YamlPath,
        #[source]
        source: ipnetwork::IpNetworkError,
    },
}

#[derive(Debug, ThisError)]
pub enum NamespaceError {
    // Directory/filesystem operations
    #[error("Failed to create namespace path '{path}': {source}.")]
    PathCreation {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Mount source {0} not found.")]
    MountSrcNotFound(String),

    // Namespace mounting
    #[error(
        "Failed to mount {ns_type} namespace for device '{device}': {source}."
    )]
    Mount {
        ns_type: String,
        device: String,
        #[source]
        source: nix::Error,
    },

    #[error("Failed to unmount namespace at '{path}': {source}")]
    Unmount {
        path: String,
        #[source]
        source: nix::Error,
    },

    // Namespace entry/switching
    #[error("Failed to enter namespace for device '{device}': {source}")]
    Entry {
        device: String,
        #[source]
        source: nix::Error,
    },

    #[error("Failed to return to main namespace: {source}")]
    ReturnToMain {
        #[source]
        source: nix::Error,
    },

    // Namespace not found.
    #[error("Namespace fd for device '{device}' not found.")]
    NotFound { device: String },

    #[error("Main namespace path '{0}' not found.")]
    MainNotFound(String),

    // Fork/process errors
    #[error(
        "Failed to fork {fork_function} process for namespace creation: {source}"
    )]
    Fork {
        fork_function: String,
        #[source]
        source: nix::Error,
    },

    #[error("Failed to create new network namespace {ns_name}: {source}")]
    Unshare {
        ns_name: String,
        #[source]
        source: nix::Error,
    },

    #[error("Failed to open file '{path}': {source}")]
    FileOpen {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, ThisError)]
pub enum LinkError {
    #[error("Interface {iface} not found: {source}")]
    NoInterface {
        iface: String,
        #[source]
        source: nix::Error,
    },

    #[error("Unable to add address {addr}: {source}")]
    AddressAdd {
        iface: String,
        addr: IpNetwork,
        #[source]
        source: rtnetlink::Error,
    },

    #[error(
        "Unable to change interface:{ifindex} on device:{device} state to up: {source}"
    )]
    ChangeStateUp {
        device: String,
        ifindex: u32,
        #[source]
        source: rtnetlink::Error,
    },

    #[error("Unable to create a new netlink connection: {source}")]
    ConnectionFailed {
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to execute netlink operation '{operation}': {source}")]
    ExecuteFailed {
        operation: String,
        #[source]
        source: rtnetlink::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct YamlPath {
    segments: Vec<PathSegment>,
}

#[derive(Debug, Clone)]
enum PathSegment {
    Key(String), // a named key:  "routers:"
    Unknown,     // bad/missing:  "???"
}

impl YamlPath {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a known key to the path.
    pub fn key(&mut self, k: impl Into<String>) -> Self {
        self.segments.push(PathSegment::Key(k.into()));
        self.clone()
    }

    /// Append an "unknown/bad value" marker at the end.
    pub fn unknown(&mut self) -> Self {
        self.segments.push(PathSegment::Unknown);
        self.clone()
    }
}

impl fmt::Display for YamlPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (depth, segment) in self.segments.iter().enumerate() {
            let indent = "  ".repeat(depth);
            match segment {
                PathSegment::Key(k) => writeln!(f, "{indent}{k}:")?,
                PathSegment::Unknown => write!(f, "{indent}???")?,
            }
        }
        Ok(())
    }
}
