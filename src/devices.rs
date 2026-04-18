use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

use ipnetwork::IpNetwork;
use nix::fcntl::{OFlag, open};
use nix::net::if_::if_nametoindex;
use nix::sched::{CloneFlags, setns};
use nix::sys::stat::Mode;
use nix::unistd::{ForkResult, dup2, fork, setsid};
use rand::Rng;
use rand::distributions::Alphanumeric;
use rtnetlink::{Handle, LinkBridge, LinkUnspec, LinkVeth, new_connection};
use tokio::runtime::Runtime;
use tracing::{debug, debug_span, error, warn, warn_span};

use crate::error::{LinkError, NamespaceError, NetError};
use crate::{NS_DIR, NetResult, mount_device};

// ==== struct Interface ====

#[derive(Debug, Clone)]
pub struct Interface {
    pub name: String,
    pub addresses: Vec<IpNetwork>,
}

impl Interface {
    pub(crate) fn new(name: String) -> Self {
        Self {
            name,
            addresses: vec![],
        }
    }

    async fn add_addresses(&self, handle: &Handle) -> NetResult<()> {
        let ifindex = match if_nametoindex(self.name.as_str()) {
            Ok(ifindex) => ifindex,
            Err(_) => {
                warn!(
                    "Address not added. Interfaces without attached links not added."
                );
                return Ok(());
            }
        };

        for addr in &self.addresses {
            let request =
                handle.address().add(ifindex, addr.ip(), addr.prefix());
            request.execute().await.map_err(|err| {
                error!(addr=%addr ,"Unable to add address");
                NetError::LinkError(LinkError::AddressAdd {
                    iface: self.name.clone(),
                    addr: *addr,
                    source: err,
                })
            })?;
        }
        Ok(())
    }
}

// ==== Node ====

#[derive(Debug, Clone)]
pub enum Node {
    Router(Router),
    Switch(Switch),
}

impl Node {
    pub fn power_off(&self) -> NetResult<()> {
        match self {
            Self::Router(router) => router.power_off(),
            Self::Switch(_) => Ok(()), // briges are cleaned up via destroy_ns.
        }
    }
}

// ==== Volume ====
#[derive(Clone, Debug, Default)]
pub(crate) struct Volume {
    pub(crate) src: String,
    pub(crate) dst: String,
}

// ==== Router =====

#[derive(Clone, Debug, Default)]
pub struct Router {
    pub name: String,
    pub(crate) net_path: Option<String>,
    pub(crate) pid_path: Option<String>,
    pub(crate) interfaces: Vec<Interface>,
    pub(crate) volumes: Vec<Volume>,
    pub(crate) scripts: Vec<String>,
}

impl Router {
    /// Creates a Router object that will represent the
    /// router
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    /// Creates a namespace representing the router and turns on the
    /// loopback interface.
    pub fn power_on(&mut self) -> NetResult<()> {
        let (net_path, pid_path) = mount_device(Some(self.name.clone()))?;
        self.net_path = Some(net_path);
        self.pid_path = Some(pid_path);

        debug!(router=%self.name, "Powered on");
        Ok(())
    }

    /// Change interface state to up.
    pub fn iface_up(&self, ifindex: u32, runtime: &Runtime) -> NetResult<()> {
        let router_name = self.name.clone();
        runtime.block_on(async {
            self.in_ns(false, move || async move {
                let (connection, handle, _) =
                    new_connection().map_err(|err| {
                        LinkError::ConnectionFailed { source: err }
                    })?;

                tokio::spawn(connection);

                let message = LinkUnspec::new_with_index(ifindex).up().build();

                handle.link().set(message).execute().await.map_err(|err| {
                    error!(router=%router_name, ifindex=%ifindex,
                        "problem bringing up"
                    );
                    NetError::LinkError(LinkError::ChangeStateUp {
                        device: router_name.clone(),
                        ifindex,
                        source: err,
                    })
                })?;
                Ok::<(), NetError>(())
            })
            .await?
        })
    }

    /// Deletes the namespace created by the Router (if it exists)
    pub fn power_off(&self) -> NetResult<()> {
        crate::destroy_ns(Some(self.name.clone()))?;
        Ok(())
    }

    /// Executes instructions inside the
    /// router's namespace.
    ///
    /// ```no_run
    /// use std::process::Command;
    ///
    /// use topology::Router;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let router = Router::new("r1");
    ///     router
    ///         .in_ns(false, move || async move {
    ///             let output = Command::new("ip").args(vec!["link"]).output();
    ///
    ///             // this will show you the output
    ///             // of the `ip link` in the router.
    ///             // If no modifications have been made
    ///             // to the namespace, should only show
    ///             // the loopback ("lo") interface
    ///             println!("{#:?}", output);
    ///         })
    ///         .await;
    /// }
    /// ```
    pub async fn in_ns<Fut, T, R>(&self, is_mount: bool, f: Fut) -> NetResult<R>
    where
        Fut: FnOnce() -> T + Send + 'static,
        T: Future<Output = R> + Send,
    {
        match (&self.net_path, &self.pid_path) {
            (Some(net_path), Some(pid_path)) => {
                // Move into the Router namespace.
                let netns_file =
                    File::open(net_path.as_str()).map_err(|err| {
                        NamespaceError::FileOpen {
                            path: net_path.clone(),
                            source: err,
                        }
                    })?;

                let pidns_file =
                    File::open(pid_path.as_str()).map_err(|err| {
                        NamespaceError::FileOpen {
                            path: net_path.clone(),
                            source: err,
                        }
                    })?;

                setns(netns_file.as_fd(), CloneFlags::CLONE_NEWNET).map_err(
                    |err| NamespaceError::Entry {
                        device: self.name.clone(),
                        source: err,
                    },
                )?;

                setns(pidns_file.as_fd(), CloneFlags::CLONE_NEWPID).map_err(
                    |err| NamespaceError::Entry {
                        device: self.name.clone(),
                        source: err,
                    },
                )?;

                if is_mount {
                    // Unshare into a new mount namespace so our mounts
                    // don't leak to the host.
                    nix::sched::unshare(CloneFlags::CLONE_NEWNS).map_err(
                        |err| NamespaceError::Unshare {
                            ns_name: self.name.clone(),
                            source: err,
                        },
                    )?;

                    // Make all mounts private before remounting proc,
                    // same as what login does.
                    nix::mount::mount(
                        None::<&str>,
                        "/",
                        None::<&str>,
                        nix::mount::MsFlags::MS_PRIVATE
                            | nix::mount::MsFlags::MS_REC,
                        None::<&str>,
                    )
                    .map_err(|err| {
                        NamespaceError::Mount {
                            ns_type: "private remount".to_string(),
                            device: self.name.clone(),
                            source: err,
                        }
                    })?;

                    // Remount /proc so it reflects the router's PID namespace.
                    nix::mount::mount(
                        Some("proc"),
                        "/proc",
                        Some("proc"),
                        nix::mount::MsFlags::empty(),
                        None::<&str>,
                    )
                    .map_err(|err| {
                        NamespaceError::Mount {
                            ns_type: "proc".to_string(),
                            device: self.name.clone(),
                            source: err,
                        }
                    })?;

                    crate::mount_router_volumes(self)?;
                }

                let result = (f)().await;

                // Go back to the main namespace.
                let main_net_path = format!("{NS_DIR}/main/net");
                let main_pid_path = format!("{NS_DIR}/main/pid");

                let main_net_file =
                    File::open(&main_net_path).map_err(|err| {
                        NetError::BasicError(format!(
                            "Unable to open file {main_net_path}: {err:?}"
                        ))
                    })?;

                let main_pid_file =
                    File::open(&main_pid_path).map_err(|err| {
                        NetError::BasicError(format!(
                            "Unable to open file {main_pid_path}: {err:?}"
                        ))
                    })?;

                setns(main_net_file.as_fd(), CloneFlags::CLONE_NEWNET)
                    .map_err(|err| {
                        NetError::NamespaceError(NamespaceError::ReturnToMain {
                            source: err,
                        })
                    })?;

                setns(main_pid_file.as_fd(), CloneFlags::CLONE_NEWPID)
                    .map_err(|err| {
                        NetError::NamespaceError(NamespaceError::ReturnToMain {
                            source: err,
                        })
                    })?;

                Ok(result)
            }
            (_, _) => Err(NamespaceError::NotFound {
                device: self.name.clone(),
            }
            .into()),
        }
    }

    /// adds the addresses of the said router as
    /// per the topology yaml file.
    ///
    /// Example:
    /// ```yaml
    /// 
    /// rt2:
    ///   interfaces:
    ///     lo:
    ///       ipv4:
    ///       - 2.2.2.2/32
    ///     eth-sw1:
    ///       ipv4:
    ///       - 10.0.1.2/24
    /// ```
    /// Above yaml config in topo file will add the address
    /// 10.0.1.2/24 to the eth-sw1 interface and 2.2.2.2/32
    /// to the lo address
    pub fn add_iface_addresses(&self, runtime: &Runtime) -> NetResult<()> {
        let interfaces = self.interfaces.clone();
        let router_name = self.name.clone();

        runtime.block_on(async {
            self.in_ns(false, move || async move {
                let (connection, handle, _) =
                    new_connection().map_err(|err| {
                        LinkError::ConnectionFailed { source: err }
                    })?;
                tokio::spawn(connection);
                for iface in interfaces {
                    let iface_name = iface.name.clone();
                    let add_iface_addr_span =
                        warn_span!("add-address", %iface_name, %router_name);
                    let _span_guard = add_iface_addr_span.enter();
                    iface.add_addresses(&handle).await?;
                }
                Ok(())
            })
            .await?
        })
    }

    pub fn run_scripts(&self, runtime: &Runtime) -> NetResult<()> {
        if self.scripts.is_empty() {
            return Ok(());
        }

        let scripts = self.scripts.clone();
        //let volumes = self.volumes.clone();
        let router_name = self.name.clone();

        runtime.block_on(async {
            self.in_ns(true, move || async move {
                for script in &scripts {
                    debug!(
                        router = %router_name,
                        script = %script,
                        "Running script"
                    );

                    let mut parts: Vec<&str> =
                        script.split_whitespace().collect();

                    // Commands that are meant to run in background end with '&',
                    // We run the commands in background by default, so no need
                    // for that.
                    if parts.last() == Some(&"&") {
                        parts.pop();
                    }

                    // Check if it is meant to be a background task.
                    if parts.is_empty() {
                        continue;
                    };
                    let executable = parts[0];
                    Self::spawn_detached(executable, &parts[1..])?;

                    debug!(
                        router = %router_name,
                        script = %script,
                        "Script completed"
                    );
                }
                Ok::<(), NetError>(())
            })
            .await?
        })
    }

    fn spawn_detached(cmd: &str, args: &[&str]) -> NetResult<()> {
        match unsafe { fork() } {
            Ok(ForkResult::Parent { .. }) => {
                // Parent: just return immediately
                return Ok(());
            }

            Ok(ForkResult::Child) => {
                // Detach from terminal & session.
                setsid().map_err(|err|
                    NetError::BasicError(
                        format!("Unable to detach from terminal for {cmd} {args:?} -> {err:?}")
                    )
                )?;

                match unsafe { fork() } {
                    Ok(ForkResult::Parent { .. }) => {
                        // Exits to prevent zombies.
                        std::process::exit(0);
                    }

                    Ok(ForkResult::Child) => {
                        // Redirect stdio → /dev/null
                        let devnull: RawFd =
                            open("/dev/null", OFlag::O_RDWR, Mode::empty())
                                .unwrap();

                        // TODO: Handle these unwraps
                        dup2(devnull, 0).unwrap(); // stdin
                        dup2(devnull, 1).unwrap(); // stdout
                        dup2(devnull, 2).unwrap(); // stderr

                        // Execute command (no extra process layer!)
                        let _ = Command::new(cmd).args(args).exec();

                        unreachable!();
                    }
                    Err(err) => {
                        return Err(NetError::BasicError(format!(
                            "Problem creating detached command {cmd} {args:?} : {err:?}"
                        )));
                    }
                }
            }
            Err(err) => {
                return Err(NetError::BasicError(format!(
                    "Problem creating spawn_detached {err:?}"
                )))
                .into();
            }
        }
    }
}

// ==== Switch ====
#[derive(Debug, Clone)]
pub struct Switch {
    pub name: String,
    pub ifindex: Option<u32>,
    pub interfaces: Vec<Interface>,
}

impl Switch {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ifindex: None,
            interfaces: vec![],
        }
    }

    /// Initializes a network bridge representing the switch.
    pub fn power_on(&mut self, runtime: &Runtime) -> NetResult<()> {
        let name = self.name.as_str();

        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);

            let message = LinkBridge::new(name).up().build();
            let request = handle.link().add(message);

            request.execute().await.map_err(|e| {
                NetError::BasicError(format!(
                    "Failed to create bridge {name}: {e}",
                ))
            })?;

            if let Ok(ifindex) = if_nametoindex(name) {
                self.ifindex = Some(ifindex);
                debug!(switch = %self.name, "Powered on");
            }

            Ok(())
        })
    }
}

// ==== LinkManager ====

pub(crate) struct LinkManager;

impl LinkManager {
    pub(crate) fn setup_all(
        &self,
        runtime: &Runtime,
        nodes: &BTreeMap<String, Node>,
        links: &[Link],
    ) -> NetResult<()> {
        // Bring up the Routers' loopback interfaces.
        for node in nodes.values() {
            if let Node::Router(router) = node {
                router.iface_up(1, runtime)?;
            }
        }

        for link in links {
            Self::create_link(runtime, nodes, link)?;
        }

        // Add addresses for links in the router nodes.
        for node in nodes.values() {
            if let Node::Router(router) = node {
                router.add_iface_addresses(runtime)?;
            }
        }

        // Scripts run after addresses in case any of them needs the address or
        // a running & reachable network interface.
        for node in nodes.values() {
            if let Node::Router(router) = node {
                router.run_scripts(runtime)?;
            }
        }
        Ok(())
    }

    fn create_link(
        runtime: &Runtime,
        nodes: &BTreeMap<String, Node>,
        link: &Link,
    ) -> NetResult<()> {
        let src_iface = format!("{}:{}", link.src_device, link.src_iface);
        let dst_iface = format!("{}:{}", link.dst_device, link.dst_iface);
        let link_span = debug_span!("link-setup", %src_iface, %dst_iface);
        let _span_guard = link_span.enter();
        debug!("Setting up");

        // generate random names for veth link
        // we do this to avoid conflict in the
        // parent device of interface names.
        let mut link_name: String;
        link_name = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(4)
            .map(char::from)
            .collect();

        let node1_link = format!("eth-{link_name}");

        link_name = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(4)
            .map(char::from)
            .collect();
        let node2_link = format!("eth-{link_name}");

        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);
            let request = handle.link().add(
                LinkVeth::new(node1_link.as_str(), node2_link.as_str()).build(),
            );

            //request.message_mut().header.flags.push(LinkFlag::Up);
            request.execute().await.map_err(|err| {
                LinkError::ExecuteFailed {
                    operation: "create_link".to_string(),
                    source: err,
                }
            })?;

            Ok::<(), NetError>(())
        })?;
        if let Some(src_node) = nodes.get(&link.src_device)
            && let Some(dst_node) = nodes.get(&link.dst_device)
        {
            // attaches the links to their respective nodes
            Self::attach_link(
                runtime,
                src_node,
                node1_link,
                link.src_iface.clone(),
            )?;
            Self::attach_link(
                runtime,
                dst_node,
                node2_link,
                link.dst_iface.clone(),
            )?;
        }
        debug!("Setup complete");

        Ok(())
    }

    fn attach_link(
        runtime: &Runtime,
        node: &Node,
        current_link_name: String,
        new_link_name: String,
    ) -> NetResult<()> {
        runtime.block_on(async {
            let (connection, handle, _) = new_connection()
                .map_err(|err| LinkError::ConnectionFailed { source: err })?;
            tokio::spawn(connection);
            match node {
                Node::Router(router) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(net_path) = &router.net_path
                    {
                        let file = File::open(net_path).map_err(|err| {
                            NamespaceError::FileOpen {
                                path: net_path.clone(),
                                source: err,
                            }
                        })?;
                        let message = LinkUnspec::new_with_index(index)
                            .setns_by_fd(file.as_raw_fd())
                            .build();
                        // Move router device to said namespace.
                        handle.link().set(message).execute().await.map_err(
                            |err| LinkError::ExecuteFailed {
                                operation:
                                    "attach-link->move-link-to-router-namespace"
                                        .to_string(),
                                source: err,
                            },
                        )?;

                        // Rename the interface to it's proper name.
                        router
                            .in_ns(false, move || async move {
                                let (conn, handle, _) = new_connection()
                                    .map_err(|err| {
                                        LinkError::ConnectionFailed {
                                            source: err,
                                        }
                                    })?;
                                tokio::spawn(conn);

                                // Rename the link from the name given to it
                                // at create_link and bring the link up.
                                let message = LinkUnspec::new_with_index(index)
                                    .name(new_link_name)
                                    .up()
                                    .build();

                                handle
                                    .link()
                                    .set(message)
                                    .execute()
                                    .await
                                    .map_err(|err| {
                                        LinkError::ExecuteFailed {
                                        operation:
                                            "attach-link->bring-interface-up"
                                                .to_string(),
                                        source: err,
                                    }
                                    })?;
                                Ok::<(), NetError>(())
                            })
                            .await??;
                        // Above: one '?' for the inner method, one for the
                        // 'in_ns' method.
                    }
                }
                Node::Switch(switch) => {
                    if let Ok(index) =
                        if_nametoindex(current_link_name.as_str())
                        && let Some(ifindex) = switch.ifindex
                    {
                        // Rename the link from the name given to it
                        // at create_link and bring it up.
                        let message = LinkUnspec::new_with_index(index)
                            .name(new_link_name)
                            .up()
                            .build();
                        if let Err(err) =
                            handle.link().set(message).execute().await
                        {
                            error!(error = %err, "error changing name");
                        }

                        let message = LinkUnspec::new_with_index(index)
                            .controller(ifindex)
                            .build();
                        if let Err(err) =
                            handle.link().set(message).execute().await
                        {
                            error!(error = %err, "error changing controller");
                        }
                    }
                }
            }
            Ok(())
        })
    }
}

// ==== Link ====

#[derive(Debug, Clone)]
pub struct Link {
    pub src_device: String,
    pub src_iface: String,
    pub dst_device: String,
    pub dst_iface: String,
}

impl Link {
    pub fn src(&self) -> String {
        format!("{}:{}", self.src_device, self.src_iface)
    }

    pub fn dst(&self) -> String {
        format!("{}:{}", self.dst_device, self.dst_iface)
    }
}
