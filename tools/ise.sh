#!/bin/bash
# Run an ISE 14.7 command-line tool in the runtime container.
#
#   tools/ise.sh xdl -xdl2ncd design.xdl design.ncd
#   tools/ise.sh bitgen -w design.ncd
#
# $ISE_MOUNT (default: the current directory) is mounted at the same path and
# the tool runs in the current directory, so relative paths work. Files must
# live under $ISE_MOUNT. ISE comes from $ISE_ROOT (default ~/Xilinx),
# the licence from $XILINXD_LICENSE_FILE (default ~/.Xilinx/Xilinx.lic).
set -euo pipefail

ISE_ROOT=${ISE_ROOT:-$HOME/Xilinx}
IMAGE=${ISE_IMAGE:-ise-runtime:14.04}
LIC=${XILINXD_LICENSE_FILE:-$HOME/.Xilinx/Xilinx.lic}
MOUNT=$(realpath "${ISE_MOUNT:-$PWD}")
case "$PWD/" in "$MOUNT"/*) ;; *) echo "current directory is not under ISE_MOUNT=$MOUNT" >&2; exit 1 ;; esac

[ -d "$ISE_ROOT/14.7/ISE_DS" ] || { echo "ISE not found in $ISE_ROOT; run tools/ise-install.sh" >&2; exit 1; }
[ $# -gt 0 ] || { echo "usage: $0 <ise tool> [args...]" >&2; exit 1; }

# ISE's FlexNet licensing code (libSecurity_FNP.so, used by map/par/bitgen)
# overflows on the long "flags" line of modern CPUs' /proc/cpuinfo and
# segfaults. Give the container a copy of the first CPU's entry with a short
# flags line.
CPUINFO=${XDG_CACHE_HOME:-$HOME/.cache}/xc6slx9-pnr/cpuinfo
mkdir -p "$(dirname "$CPUINFO")"
awk '/^processor/ { n++ } n > 1 { exit }
     /^flags/ { print "flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ht syscall nx lm constant_tsc pni ssse3 cx16 sse4_1 sse4_2 popcnt"; next }
     /^(vmx flags|bugs)/ { next }
     { print }' /proc/cpuinfo > "$CPUINFO"

# settings64.sh takes its first positional argument as the install path, so
# pass it explicitly (otherwise it would see the tool command).
docker run --rm -i \
    -v "$ISE_ROOT:/opt/Xilinx:ro" \
    -v "$LIC:/lic/Xilinx.lic:ro" \
    -v "$CPUINFO:/proc/cpuinfo:ro" \
    -e XILINXD_LICENSE_FILE=/lic/Xilinx.lic \
    -e HOME=/tmp \
    -v "$MOUNT:$MOUNT" -w "$PWD" \
    "$IMAGE" \
    bash -c '. /opt/Xilinx/14.7/ISE_DS/settings64.sh /opt/Xilinx/14.7/ISE_DS >/dev/null && exec "$@"' ise "$@"
