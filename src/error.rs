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
    LinkError(#[from] LinkError),
}

#[derive(Debug, ThisError)]
pub enum ConfigError {
    #[error("Topology file not configured.")]
    TopologyFileMissing,

    #[error("Link {src} <-> {dst} configured multiple times.")]
    DuplicateLink { src: String, dst: String },

    #[error("Node {0} has been configured multiple times.")]
    DuplicateNode(String),

    #[error("Field {field} has incorrect type. Expected type '{expected}'.")]
    IncorrectType { field: String, expected: String },

    #[error("Topology missing required field '{0}'.")]
    MissingField(String),

    #[error("Link references to unknown node {0}.")]
    UnknownNode(String),

    #[error("Invalid YAML Syntax {0}.")]
    YamlSyntax(#[from] ScanError),

    #[error(
        "Invalid {addr_type} address '{address}' for interface '{interface}'."
    )]
    InvalidAddress {
        addr_type: String,
        address: String,
        interface: String,
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
