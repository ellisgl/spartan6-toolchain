#!/bin/bash
# One-time install of ISE 14.7 WebPACK into a host directory.
#
#   tools/ise-install.sh <extracted Xilinx_ISE_DS_Lin_14.7_1015_1 dir>
#
# Installs into $ISE_ROOT (default ~/Xilinx), about 18 GB. Afterwards the
# installer directory and tarball can be deleted. Runs as the container's
# root, which rootless Docker maps to you, so the files are owned by you.
set -euo pipefail

SRC=${1:?usage: $0 <extracted Xilinx_ISE_DS_Lin_14.7_1015_1 dir>}
SRC=$(cd "$SRC" && pwd)
ISE_ROOT=${ISE_ROOT:-$HOME/Xilinx}
IMAGE=${ISE_IMAGE:-ise-runtime:14.04}
HERE=$(cd "$(dirname "$0")/.." && pwd)

[ -x "$SRC/bin/lin64/batchxsetup" ] || { echo "no bin/lin64/batchxsetup in $SRC" >&2; exit 1; }
if [ -d "$ISE_ROOT/14.7/ISE_DS" ]; then
    echo "ISE already installed in $ISE_ROOT/14.7/ISE_DS" >&2
    exit 1
fi
mkdir -p "$ISE_ROOT"

docker build -t "$IMAGE" "$HERE/docker/ise"

# batchxsetup asks to accept the licence agreements; `yes` answers them.
# It refuses to start without TERM set.
docker run --rm -e TERM=xterm \
    -v "$SRC:/xilinx:ro" \
    -v "$ISE_ROOT:/opt/Xilinx" \
    -v "$HERE/docker/ise/install.cfg:/install.cfg:ro" \
    "$IMAGE" \
    bash -c 'yes | /xilinx/bin/lin64/batchxsetup --batch /install.cfg'

echo "Installed: $ISE_ROOT/14.7/ISE_DS ($(du -sh "$ISE_ROOT" | cut -f1))"
