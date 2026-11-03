#!/usr/bin/env python3
"""Hardware check for tests/pll2 (PLL_BASE with default
SYSTEM_SYNCHRONOUS compensation and direct feedback).

    tools/dll-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=pll2).
"""

import argparse
import struct
import sys
import time

import serial

WINDOW = 65536
EXPECT = {"CLKOUT0": 125.0, "CLKOUT1": 500.0 / 7, "CLKOUT2": 50.0}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--runs", type=int, default=3)
    args = ap.parse_args()
    bad = False
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for r in range(args.runs):
            s.write(b"C")
            got = s.read(13)
            if len(got) != 13:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            counts = struct.unpack("<3I", got[:12])
            locked = got[12] & 1
            line = []
            for (name, mhz), n in zip(EXPECT.items(), counts):
                want = WINDOW * mhz / 50.0
                ok = abs(n - want) <= 4
                bad |= not ok
                line.append(f"{name} {n * 50.0 / WINDOW:.2f} MHz{'' if ok else ' (want %.2f)' % mhz}")
            bad |= not locked
            print(f"  run {r}: " + ", ".join(line) + f", LOCKED {locked}")
    if bad:
        print("FAIL")
        sys.exit(1)
    print("PASS: PLL (default compensation) outputs at the expected frequencies, LOCKED")


if __name__ == "__main__":
    main()
