#!/bin/sh
# The packages a Linux build of gitclient needs on Debian.
set -e
apt-get update
apt-get install -y --no-install-recommends jq pkg-config libxcb1-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libfontconfig1-dev libfreetype-dev libwayland-dev
rm -rf /var/lib/apt/lists/*
