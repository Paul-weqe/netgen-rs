# Netgen-rs

Netgen-rs is a network simulator built in Rust that uses network namespaces for
this functionality.

Though there is still lots of work to be done, the project gets its motivation
from [netgen](https://github.com/rwestphal/netgen/) and
[munet](https://github.com/holo-routing/holo-munet-topologies). The two are
brilliant pieces of technology but I saw a need for creating something that
may handle way larger networks. Also, despite the fact they both have means of
accessing the devices that are being simulated, there is still a space for
creating something that can be accessed via the terminal and maybe accessed via
web for instance.

## Why need a network simulator

What's your elders' favourite quote ? <b>Never test in Production.</b>
This is an essential piece of advice more so in computer networks, something
that the whole world runs on. I mean, testing in production is cute until a
tiny bug snowballs into a global catastrophe all because Chad can’t post
his brunch pics. We therefore need a sandbox we can carry out experiments in,
that is where netgen-rs comes in.

Netgen-rs offers a lightweight means of creating these network simulations. The
magic bullet ? 

<b>Namespaces</b>. Namespaces are essentially a means to
isolate specific items in your Linux based system. Read more
[here](https://www.redhat.com/en/blog/7-linux-namespaces), I promise you'll
enjoy it. If possible, follow through the entire blog series on building a
container by hand.

## Getting started.

We need to first have the binary in our device. This can be done via cloning
the repository and building it. This assumes you already have Rust installed.

Here's how:

```sh
$ git clone https://github.com/Paul-weqe/netgen-rs
$ cd netgen-rs
$ cargo build --bin netgen
```

Now we can now copy the resulting binary to the /usr/bin directory:

```sh
$ cp -r ./target/debug/netgen /usr/bin/netgen
```


## Outlining our topology.

Now that we have our binary accessible from our terminal, we create a topology
definition file. In netgen, this is defined in a .yml file (more options may be
introduced in the future).


We want to simulate a three router network:

```
                                              +---------------------+
                                              :       Device RT-A   :
                    eth0 - 192.168.0.1/24     :                     : eth1 - 192.168.1.1/24
                                              :   eth0 -----> RT-B  :
                ----------------------------> :   eth1 -----> RT-C  : <------------
                |                             +---------------------+             |
                |                                                                 |
eth0            |                                                                 | eth0
192.168.0.2/24  |                                                                 | 192.168.1.2/24
                v                                                                 v

+---------------------+                                               +---------------------+
:       Device RT-B   :<----------------------------------------------:       Device RT-C   :
:                     : eth1                      eth1                :
:                     : 192.168.2.1/24            192.168.2.2/24      :
:                     :                                               :                     :
:   eth0 -----> RT-A  :                                               :   eth0 -----> RT-A  :
:   eth1 -----> RT-C  :                                               :   eth1 -----> RT-B  :
+---------------------+                                               +---------------------+
```

To create this, out topology.yml file will look like the following:

```yaml
routers:
  RT-A:
    interfaces:
      eth0:
        ipv4:
        - 192.168.0.1/24
      eth1:
        ipv4:
        - 192.168.1.1/24

  RT-B:
    interfaces:
      eth0:
        ipv4:
        - 192.168.0.2/24

      eth1:
        ipv4:
        - 192.168.2.1/24

  RT-C:
    interfaces:
      eth0:
        ipv4:
        - 192.168.1.2/24

      eth1:
        ipv4:
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

We can save that as my-topo.yml

That should be basic enough. Plans to simplify the `links` fields is underway
but for now, kindly bear with us. 

## Running our simulation.

Running the simulation is pretty simple since we already have netgen-rs
in out `/usr/bin` directory. This is how we get our simulation to run:

```sh
$ netgen start --topo my-topo.yml
```

This should bring out logs that look like the following:

```
2025-08-17T19:40:43.222180Z DEBUG net-init: powered on router=RT-A
2025-08-17T19:40:43.224728Z DEBUG net-init: powered on router=RT-B
2025-08-17T19:40:43.227207Z DEBUG net-init: powered on router=RT-C
2025-08-17T19:40:43.227500Z DEBUG net-init:link-setup{src_iface=RT-A:eth0 dst_iface=RT-B:eth0}: setting up
2025-08-17T19:40:43.288303Z DEBUG net-init:link-setup{src_iface=RT-A:eth0 dst_iface=RT-B:eth0}: setup complete
2025-08-17T19:40:43.288521Z DEBUG net-init:link-setup{src_iface=RT-A:eth1 dst_iface=RT-C:eth0}: setting up
2025-08-17T19:40:43.337055Z DEBUG net-init:link-setup{src_iface=RT-A:eth1 dst_iface=RT-C:eth0}: setup complete
2025-08-17T19:40:43.337242Z DEBUG net-init:link-setup{src_iface=RT-B:eth1 dst_iface=RT-C:eth1}: setting up
2025-08-17T19:40:43.383368Z DEBUG net-init:link-setup{src_iface=RT-B:eth1 dst_iface=RT-C:eth1}: setup complete
```

Look at that beauty. At this point, when we press `CTRL+C` we should be back to our terminal. Don't worry, everything is running okay. 

Our devices are now up. We should be ready to access them. 

## Accessing the devices.

It pains me to write this but netgen does not yet have a subcommand to acess the
routers. This is a major work in progress. For now we will do this manually.
This is how we access RT-A:

```
$ nsenter --net=/tmp/netgen-rs/ns/devices/RT-A
```

So we're now logged into RT-A. We should try to ping RT-B(eth0) and RT-C(eth0)
which should all be reachable. Then we try and ping RTC-(eth1) which is clearly
not reachable unless routing is configured:

```
$ ping 192.168.0.2
PING 192.168.0.2 (192.168.0.2) 56(84) bytes of data.
64 bytes from 192.168.0.2: icmp_seq=1 ttl=64 time=0.063 ms
64 bytes from 192.168.0.2: icmp_seq=2 ttl=64 time=0.068 ms
^C
--- 192.168.0.2 ping statistics ---
2 packets transmitted, 2 received, 0% packet loss, time 1030ms
rtt min/avg/max/mdev = 0.063/0.065/0.068/0.002 ms
$
$ ping 192.168.1.2
PING 192.168.1.2 (192.168.1.2) 56(84) bytes of data.
64 bytes from 192.168.1.2: icmp_seq=1 ttl=64 time=0.123 ms
64 bytes from 192.168.1.2: icmp_seq=2 ttl=64 time=0.062 ms
^C
--- 192.168.1.2 ping statistics ---
2 packets transmitted, 2 received, 0% packet loss, time 1033ms
rtt min/avg/max/mdev = 0.062/0.092/0.123/0.030 ms
$ ping 192.168.2.2
ping: connect: Network is unreachable
$
```

That went fairly well for our first attempt. Type `logout` in your terminal or
press `CTRL+D` to get out of the router and back to our device. 

## Stoping our simulation

We stop our simulation by running the following command:

```sh
$ netgen stop --topo my-topo.yml
```

This will stop all the devices in the topology. 

## TODOs

It's easier to list what we have done that what we haven't. Anyway, what's on
the plan:

1. PID namespaces and mount namespaces are still being worked on. If you've read
   the codebase they are present but not working as intended.
2. Improve the subcommands and add more commands e.g to access a specific
   device's terminal etc...
3. Have a more user friendly UI e.g a web UI. For us to be able to easily share
   these simulations with people less comfortable with the terminal.

Those are top of the list, together with some refactoring. If you find like you
can take on any of these, feel free to open a PR, my discord and email are on my
profile. 

Happy hacking :)
