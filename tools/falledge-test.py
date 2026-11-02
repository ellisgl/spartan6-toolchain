#!/usr/bin/env python3
"""Hardware check for tests/falledge (falling-edge FFs, SRL and RAM: the
slice's inverted clock).

    tools/falledge-test.py [--port /dev/ttyACM0] [--runs 3]

Load the design first (make load TEST=falledge). Each run checks 2^22 clock
cycles; every count must be 0.
"""

import argparse
import struct
import sys
import time

import serial


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
            s.write(b"T")
            got = s.read(12)
            if len(got) != 12:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            e_n, e_sr, e_ram = struct.unpack("<III", got)
            print(f"  run {r}: mismatches FF {e_n}, SRL {e_sr}, RAM {e_ram}")
            bad |= bool(e_n or e_sr or e_ram)
    if bad:
        print("FAIL: falling-edge cells don't behave as falling-edge")
        sys.exit(1)
    print(f"PASS: falling-edge FF, SRL and RAM correct over {args.runs} x 2^22 cycles")


if __name__ == "__main__":
    main()
