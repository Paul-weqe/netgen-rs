#!/bin/bash

umount /tmp/netgen-rs/ns/main/net
umount /tmp/netgen-rs/ns/devices/rt1/net
umount /tmp/netgen-rs/ns/devices/rt2/net
umount /tmp/netgen-rs/ns/devices/rt3/net

rm -rf /tmp/netgen-rs
