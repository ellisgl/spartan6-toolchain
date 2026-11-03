#!/usr/bin/env python3
"""Hardware check for tests/phase (DCM fixed phase shift).

    make bit TEST=phase      # build once
    tools/phase-test.py [--port /dev/ttyACM0]

For each PHASE_SHIFT the value is rewritten in a copy of
build/phase/routed.json and s6-bitgen makes the bitstream (same placement and
routing), which is loaded into the FPGA's RAM. The shifted clock, sampled as
data on the unshifted board clock, must read low when shifted later (+) and
high when shifted earlier (-).
"""

import argparse
import glob
import json
import os
import struct
import subprocess
import sys
import tempfile
import time

import serial

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD = os.path.join(ROOT, "build")
SHIFTS = [64, -64, 96, -96]  # +-90 and +-135 degrees


def bitstream(shift, path):
    d = json.load(open(os.path.join(BUILD, "phase/routed.json")))
    mod = next(iter(d["modules"].values()))
    for c in mod["cells"].values():
        if c["type"] == "DCM":
            c["parameters"]["PHASE_SHIFT"] = format(shift & 0xFFFFFFFF, "032b")
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(d, f)
        tmp = f.name
    try:
        subprocess.run(
            [os.path.join(BUILD, "target/release/s6-bitgen"), "--db", glob.glob(os.path.join(BUILD, "spartan6-*.zstd"))[0],
             "gen", "--routed", tmp, "--chipdb-json", os.path.join(BUILD, "xc6slx9.json"), "--out", path],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        os.unlink(tmp)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    args = ap.parse_args()
    bad = False
    for shift in SHIFTS:
        with tempfile.TemporaryDirectory() as td:
            path = os.path.join(td, "phase.bin")
            bitstream(shift, path)
            subprocess.run([sys.executable, "-I", os.path.join(ROOT, "tools/mojo-load.py"), "--port", args.port, path],
                           check=True, stdout=subprocess.DEVNULL)
        with serial.Serial(args.port, 115200, timeout=2) as s:
            time.sleep(0.5)
            s.reset_input_buffer()
            s.write(b"P")
            got = s.read(5)
        if len(got) != 5:
            print(f"FAIL: short reply {got.hex()}")
            sys.exit(1)
        ones = struct.unpack("<I", got[:4])[0]
        frac = ones / 65536
        want_high = shift < 0
        ok = ((frac > 0.95) if want_high else (frac < 0.05)) and bool(got[4] & 1)
        bad |= not ok
        print(f"  PHASE_SHIFT {shift:+4d} ({shift * 360 / 256:+6.1f} deg): sampled high {100 * frac:5.1f}%, "
              f"LOCKED {got[4] & 1} -> {'pass' if ok else 'FAIL'}")
    if bad:
        print("FAIL")
        sys.exit(1)
    print("PASS: fixed phase shifts move the clock the right way")


if __name__ == "__main__":
    main()
