#!/bin/bash 

cargo +nightly build 
sudo ./target/debug/netgen-rs
