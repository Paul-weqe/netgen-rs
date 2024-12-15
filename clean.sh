#!/bin/bash 

sudo ip l delete sw1
sudo ip netns delete rt1
sudo ip netns delete rt2
sudo ip netns delete rt3
