#!/usr/bin/env python3
"""Hardware check for tests/preadd (DSP48A1 pre-adder): a * (d + b) and
a * (d - b) with 18-bit wrapping sums, over the Mojo's USB serial port.

    tools/preadd-test.py [--port /dev/ttyACM0] [--count 500]

Load the design first (make load TEST=preadd).
"""

import argparse
import random
import sys
import time

import serial


def s18(v):
    v &= (1 << 18) - 1
    return v - (1 << 18) if v & (1 << 17) else v


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--count", type=int, default=500)
    args = ap.parse_args()

    edge = [0, 1, 0x1FFFF, 0x20000, 0x3FFFF, 0x15555]
    cases = [(d, b, a) for d in edge for b in edge for a in edge]
    while len(cases) < args.count:
        cases.append(tuple(random.getrandbits(18) for _ in range(3)))
    bad = 0
    mask = (1 << 48) - 1
    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        # The AVR sends a few bytes while its bridge starts up; the design
        # ignores them.
        time.sleep(0.5)
        port.reset_input_buffer()
        for d, b, a in cases:
            port.write(b"P" + d.to_bytes(3, "little") + b.to_bytes(3, "little") + a.to_bytes(3, "little"))
            got = port.read(12)
            want = ((s18(a) * s18(d + b)) & mask).to_bytes(6, "little") + \
                   ((s18(a) * s18(d - b)) & mask).to_bytes(6, "little")
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  d={s18(d)} b={s18(b)} a={s18(a)}: got {got.hex()} want {want.hex()}")
    if bad:
        print(f"FAIL: {bad} of {len(cases)} cases wrong")
        sys.exit(1)
    print(f"PASS: {len(cases)} pre-add and pre-subtract products correct")


if __name__ == "__main__":
    main()
