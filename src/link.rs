use std::collections::BTreeMap;
use std::fs::File;
use std::os::fd::AsRawFd;

use nix::net::if_::if_nametoindex;
use rand::Rng;
use rand::distributions::Alphanumeric;
use rtnetlink::{LinkUnspec, LinkVeth, new_connection};
use tokio::runtime::Runtime;
use tracing::{debug, debug_span, error};

use crate::NetResult;
use crate::error::{LinkError, NamespaceError, NetError};
use crate::node::Node;

// ==== Link ====

#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub src_device: String,
    pub src_iface: String,
    pub dst_device: String,
    pub dst_iface: String,
}

impl Link {
    pub(crate) fn src(&self) -> String {
        format!("{}:{}", self.src_device, self.src_iface)
    }

    pub(crate) fn dst(&self) -> String {
        format!("{}:{}", self.dst_device, self.dst_iface)
    }
}

// ==== LinkManager ====

pub(crate) struct LinkManager;

impl LinkManager {
    pub(crate) fn setup_all(
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
