#!/usr/bin/env python3
"""Hardware check for tests/lvds (LVDS_33 OBUFDS and IBUFDS).

    tools/lvds-test.py [--port /dev/ttyACM0]

Needs the output pair looped onto the input pair with two jumper wires on
the Mojo's header: P41 -> P6 and P40 -> P5 (P to P, N to N). Load the design
first (make load TEST=lvds). The input must follow the output, both ways.
Without the jumpers the input floats; the script says so and exits 2.
"""

import argparse
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--count", type=int, default=200)
    args = ap.parse_args()
    bad = 0
    with serial.Serial(args.port, 115200, timeout=1) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for i in range(args.count):
            v = i & 1
            s.write(b"S" + bytes([v]))
            if s.read(1) != b"K":
                sys.exit("FAIL: no reply to S")
            s.write(b"R")
            r = s.read(1)
            if len(r) != 1:
                sys.exit("FAIL: no reply to R")
            o, i_read = r[0] >> 1 & 1, r[0] & 1
            if o != v:
                sys.exit(f"FAIL: output register reads {o}, set {v}")
            bad += i_read != v
    if bad == args.count // 2 or bad == args.count:
        print(f"no loopback: the input pair doesn't follow the output (jumper P41 -> P6, P40 -> P5)")
        sys.exit(2)
    if bad:
        sys.exit(f"FAIL: input wrong {bad} of {args.count} times")
    print(f"PASS: {args.count} levels through OBUFDS -> IBUFDS (LVDS_33)")


if __name__ == "__main__":
    main()
