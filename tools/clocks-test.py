#!/usr/bin/env python3
"""Hardware check for tests/clocks (DCM_SP, PLL_BASE): measure each
generated clock against the 50 MHz board clock.

    tools/clocks-test.py [--port /dev/ttyACM0] [--runs 5] [--config 1|2]

Load the design first (make load TEST=clocks).
"""

import argparse
import struct
import sys
import time

import serial

WINDOW = 65536
CONFIGS = {
    1: [("DCM CLKFX", 70e6), ("PLL CLKOUT0", 80e6), ("PLL CLKOUT1", 400e6 / 12)],  # tests/clocks
    2: [("DCM CLKFX", 75e6), ("PLL CLKOUT0", 125e6), ("PLL CLKOUT1", 500e6 / 7)],  # tests/clocks2
}
# LOCKED bits expected per config (bit 0 DCM, bit 1 PLL). In config 2 the PLL
# runs at the right frequencies but never raises LOCKED; ISE's own build of
# the same design does exactly the same on the Mojo V3.
LOCKED = {1: 0b11, 2: 0b01}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--config", type=int, default=1, choices=sorted(CONFIGS), help="1: tests/clocks, 2: tests/clocks2")
    args = ap.parse_args()
    clocks = CONFIGS[args.config]

    bad = 0
    with serial.Serial(args.port, 115200, timeout=1) as port:
        time.sleep(0.5)
        port.reset_input_buffer()
        for run in range(args.runs):
            port.write(b"C")
            got = port.read(13)
            if len(got) != 13:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            counts = struct.unpack("<III", got[:12])
            locked = got[12]
            line = []
            for (name, f), n in zip(clocks, counts):
                want = WINDOW * f / 50e6
                ok = abs(n - want) <= 4
                bad += not ok
                line.append(f"{name} {n} ({n / WINDOW * 50:.3f} MHz){'' if ok else ' WRONG'}")
            if locked & LOCKED[args.config] != LOCKED[args.config]:
                bad += 1
                line.append(f"LOCKED {locked:#04b} WRONG")
            print(f"  run {run}: " + ", ".join(line))
    if bad:
        print(f"FAIL: {bad} wrong measurements")
        sys.exit(1)
    print("PASS: DCM and PLL outputs at the expected frequencies, LOCKED as expected")


if __name__ == "__main__":
    main()
