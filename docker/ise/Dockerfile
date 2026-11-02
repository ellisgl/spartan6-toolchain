# Runtime for Xilinx ISE 14.7 command-line tools (xdl, bitgen, drc, map, par).
#
# ISE itself is NOT in this image: it is installed once into a host directory
# (tools/ise-install.sh) and mounted at /opt/Xilinx. This keeps the image to a
# few hundred MB instead of ~25 GB.
#
# Ubuntu 14.04 is the newest LTS that still ships libXp.so.6, which ISE needs.
# Package list from https://github.com/codepainters/ise14 (scripts/install.sh).
FROM ubuntu:14.04

RUN export DEBIAN_FRONTEND=noninteractive \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      libncurses5 libstdc++5 \
      libx11-6 libglib2.0-0 libsm6 libxi6 libxm4 libxp6 \
      libxrender1 libxrandr2 libfreetype6 libfontconfig1 \
      libxcursor1 libxtst6 libusb-0.1-4 \
 && rm -rf /var/lib/apt/lists/*
