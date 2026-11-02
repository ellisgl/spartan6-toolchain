#!/usr/bin/env python3
"""Hardware check for tests/bram (block RAM): read every word's initial
value, then write random data and read it back.

    tools/bram-test.py [--port /dev/ttyACM0] [--writes 2000]

Load the design first (make load TEST=bram).
"""

import argparse
import os
import random
import struct
import sys
import time

import serial

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bram_data import MEMS, init  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--writes", type=int, default=2000)
    args = ap.parse_args()

    mem = [init(i, depth, width) for i, (_, depth, width) in enumerate(MEMS)]
    bad = 0
    reads = 0

    def check(port, sel, addr, what):
        nonlocal bad, reads
        port.write(b"R" + struct.pack("<HB", addr, sel))
        got = port.read(4)
        reads += 1
        want = struct.pack("<I", mem[sel][addr])
        if got != want:
            bad += 1
            if bad <= 8:
                print(f"  {what} {MEMS[sel][0]}[{addr}]: got {got.hex()} want {want.hex()}")
            if not got:
                print("FAIL: no reply")
                sys.exit(1)

    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()
        for sel, (_, depth, _) in enumerate(MEMS):
            for a in range(depth):
                check(port, sel, a, "init")
        init_bad = bad
        for _ in range(args.writes):
            sel = random.randrange(len(MEMS))
            _, depth, width = MEMS[sel]
            a = random.randrange(depth)
            v = random.getrandbits(width)
            port.write(b"W" + struct.pack("<HBI", a, sel, v))
            if port.read(1) != b"K":
                print("FAIL: write not acknowledged")
                sys.exit(1)
            mem[sel][a] = v
            check(port, sel, a, "write")
            # And a random other word, to catch writes landing elsewhere.
            check(port, sel, random.randrange(depth), "other")
    if bad:
        print(f"FAIL: {bad} of {reads} reads wrong ({init_bad} initial values)")
        sys.exit(1)
    print(f"PASS: {reads} reads correct ({sum(d for _, d, _ in MEMS)} initial words, {args.writes} writes)")


if __name__ == "__main__":
    main()
