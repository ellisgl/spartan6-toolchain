#!/bin/bash
# Run an ISE 14.7 command-line tool in the runtime container.
#
#   tools/ise.sh xdl -xdl2ncd design.xdl design.ncd
#   tools/ise.sh bitgen -w design.ncd
#
# The current directory is mounted at the same path, so relative paths work
# (files must live under it). ISE comes from $ISE_ROOT (default ~/Xilinx),
# the licence from $XILINXD_LICENSE_FILE (default ~/.Xilinx/Xilinx.lic).
set -euo pipefail

ISE_ROOT=${ISE_ROOT:-$HOME/Xilinx}
IMAGE=${ISE_IMAGE:-ise-runtime:14.04}
LIC=${XILINXD_LICENSE_FILE:-$HOME/.Xilinx/Xilinx.lic}

[ -d "$ISE_ROOT/14.7/ISE_DS" ] || { echo "ISE not found in $ISE_ROOT; run tools/ise-install.sh" >&2; exit 1; }
[ $# -gt 0 ] || { echo "usage: $0 <ise tool> [args...]" >&2; exit 1; }

# settings64.sh takes its first positional argument as the install path, so
# pass it explicitly (otherwise it would see the tool command).
docker run --rm -i \
    -v "$ISE_ROOT:/opt/Xilinx:ro" \
    -v "$LIC:/lic/Xilinx.lic:ro" \
    -e XILINXD_LICENSE_FILE=/lic/Xilinx.lic \
    -e HOME=/tmp \
    -v "$PWD:$PWD" -w "$PWD" \
    "$IMAGE" \
    bash -c '. /opt/Xilinx/14.7/ISE_DS/settings64.sh /opt/Xilinx/14.7/ISE_DS >/dev/null && exec "$@"' ise "$@"
