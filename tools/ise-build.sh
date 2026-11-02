#!/bin/bash
# Reference build with the full ISE flow: XST -> ngdbuild -> map -> par -> bitgen.
#
#   tools/ise-build.sh <top> <ucf> <outdir> <verilog files...>
#
# Writes <outdir>/<top>.bit and <outdir>/<top>.bin (for the Mojo loader).
# Run from the repo root; all paths must be under it.
set -euo pipefail

TOP=${1:?usage: $0 <top> <ucf> <outdir> <verilog...>}
UCF=${2:?}
OUT=${3:?}
shift 3
PART=xc6slx9-2-tqg144
ISE="$(cd "$(dirname "$0")" && pwd)/ise.sh"
# Mount the whole working tree, since the tools run inside $OUT.
export ISE_MOUNT=${ISE_MOUNT:-$PWD}

mkdir -p "$OUT"
: > "$OUT/$TOP.prj"
for f in "$@"; do
    echo "verilog work \"$(realpath "$f")\"" >> "$OUT/$TOP.prj"
done
cat > "$OUT/$TOP.xst" <<EOF
run
-ifn $TOP.prj
-ifmt mixed
-ofn $TOP.ngc
-ofmt NGC
-p $PART
-top $TOP
-opt_mode Speed
-opt_level 1
EOF
UCF=$(realpath "$UCF")

cd "$OUT"
"$ISE" xst -ifn "$TOP.xst" -ofn "$TOP.syr"
"$ISE" ngdbuild -p "$PART" -uc "$UCF" "$TOP.ngc" "$TOP.ngd"
"$ISE" map -p "$PART" -w -o "${TOP}_map.ncd" "$TOP.ngd" "$TOP.pcf"
"$ISE" par -w "${TOP}_map.ncd" "$TOP.ncd" "$TOP.pcf"
# Binary:Yes also writes the headerless .bin the Mojo loader sends.
# UnusedPin:Pullnone leaves unused pins (incl. the AVR's SPI/serial lines) floating.
"$ISE" bitgen -w -g Binary:Yes -g UnusedPin:Pullnone "$TOP.ncd" "$TOP.bit" "$TOP.pcf"
ls -l "$TOP.bit" "$TOP.bin"
