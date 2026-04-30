# netgen-rs

A network simulator written in Rust, built on top of Linux network namespaces.

The project takes inspiration from
[netgen](https://github.com/rwestphal/netgen/) and
[holo-munet](https://github.com/holo-routing/holo-munet-topologies),
both excellent tools but aims to push further in terms of scale and
accessibility. The long-term vision includes support for larger topologies,
a proper terminal interface for accessing simulated devices, and eventually
a web UI for sharing simulations with people who don't live in the terminal.

---

## Why bother with a network simulator

The old advice still holds: never test in production. In networking especially,
a seemingly small misconfiguration can cascade into something far more
serious. You need a sandbox.

What makes netgen-rs lightweight enough to actually be practical is that
it leans entirely on Linux **namespaces** for isolation; no VMs,
no heavy hypervisors. Each simulated device lives in its own network
namespace, which means you get real network stack isolation without the
overhead. If you've never dug into namespaces before, [this blog series](https://www.redhat.com/en/blog/7-linux-namespaces) is worth your time.

---

## Installation

Clone the repository and build the binary. You'll need Rust installed.

```sh
git clone https://github.com/Paul-weqe/netgen-rs
cd netgen-rs
cargo build --bin netgen
```

Copy the binary somewhere on your PATH:

```sh
cp ./target/debug/netgen /usr/bin/netgen
```

---

## Defining a topology

Topologies are defined in YAML. The structure is straightforward: you declare
your devices (routers and/or switches), their interfaces with IP addresses,
and then the links connecting them.

Here's a three-router setup to illustrate:

```
                                          +---------------------+
                                          :       RT-A          :
              eth0 - 192.168.0.1/24       :                     : eth1 - 192.168.1.1/24
                                          :                     :
          ------------------------------> :                     : <----------
          |                               +---------------------+           |
          |                                                                 |
eth0      |                                                                 | eth0
192.168.0.2/24                                                    192.168.1.2/24
          v                                                                 v
+---------------------+                                         +---------------------+
:       RT-B          : <-------------------------------------->:       RT-C          :
:                     : eth1                                eth1:                     :
:                     : 192.168.2.1/24            192.168.2.2/24:                     :
+---------------------+                                         +---------------------+
```

The corresponding `topology.yml`:

```yaml
routers:
  RT-A:
    interfaces:
      eth0:
        - 192.168.0.1/24
        - 2001:db8:a::1/64
      eth1:
        - 192.168.1.1/24

  RT-B:
    interfaces:
      eth0:
        - 192.168.0.2/24
      eth1:
        - 192.168.2.1/24
        - 2001:db8:b::1/64
    volumes:
      - /tmp/src:/tmp/dst

  RT-C:
    interfaces:
      eth0:
        - 192.168.1.2/24
      eth1:
        - 192.168.2.2/24

links:
  - src-device: RT-A
    src-iface: eth0
    dst-device: RT-B
    dst-iface: eth0

  - src-device: RT-A
    src-iface: eth1
    dst-device: RT-C
    dst-iface: eth0

  - src-device: RT-B
    src-iface: eth1
    dst-device: RT-C
    dst-iface: eth1
```

Switches are also supported. Add them under a `switches` key in the same file,
and link them to routers the same way you'd link two routers.

---

## Running a simulation

```sh
netgen start --topo topology.yml
```

You'll see output like this as the devices and links come up:

```
2025-08-17T19:40:43.222180Z DEBUG net-init: powered on router=RT-A
2025-08-17T19:40:43.224728Z DEBUG net-init: powered on router=RT-B
2025-08-17T19:40:43.227207Z DEBUG net-init: powered on router=RT-C
2025-08-17T19:40:43.227500Z DEBUG net-init:link-setup{src_iface=RT-A:eth0 dst_iface=RT-B:eth0}: setting up
2025-08-17T19:40:43.288303Z DEBUG net-init:link-setup{src_iface=RT-A:eth0 dst_iface=RT-B:eth0}: setup complete
...
```

Once it finishes, press `Ctrl+C`. The simulation keeps running in the
background — the process exiting is expected.

---

## Accessing a device

Use the `login` subcommand, passing the topology file and the name of the
device you want to enter:

```sh
netgen login --topo topology.yml --device RT-A
```

This drops you into a shell inside RT-A's network namespace, with procfs
correctly mounted and any volumes you've defined for that router already
bind-mounted in. From there you can use standard tools as you normally would:

```sh
# RT-B's eth0 — directly connected, should work
ping 192.168.0.2

# RT-C's eth0 — also directly connected
ping 192.168.1.2

# RT-C's eth1 — not directly reachable without routing configured
ping 192.168.2.2
# ping: connect: Network is unreachable
```

Press `Ctrl+D` or type `logout` to return to your host shell.

---

## Stopping the simulation

```sh
netgen stop --topo topology.yml
```

This tears down all the devices defined in the topology file.

---

## Volumes

Routers support bind-mounting host directories or files into the simulated
device. This is useful if you want to share config files or logs between
the host and a router's namespace.

```yaml
routers:
  rt1:
    interfaces:
      eth0:
        - 10.0.1.1/24
    volumes:
      - /tmp/router-configs:/etc/frr
```

---

With these concepts in place, you should be able to run your networking
softwares in these isolated simulations.

---

## Current limitations and what's next

The project is still early. A few things are partially implemented or not yet working as intended:

- **Multiple simulation support:** Currently, you can only have a single netgen
  simulation running at a time. We should enable the running of multiple
  simulations, each with their individual names, and a datastore for the 
  different simulation metadata.
- **More Interactive UI:** Instead of having the user being forced to run the
  commands, we can prompt the user to enter details, step by step of the
  simulation. The login subcommand for example, might become very verbose when
  the multiple simulations are supported.
- **Web UI:** for sharing and viewing simulations without needing terminal access.

If any of these feel approachable, PRs are welcome.
