#!/usr/bin/env python3
"""Hardware check for tests/ram32 (RAM32X1S, RAM32X1D).

    tools/ram32-test.py [--port /dev/ttyACM0] [--ops 1500]

Load the design first (make load TEST=ram32). Checks the initial contents
first, so run it right after loading.
"""

import argparse
import random
import sys
import time

import serial

SEED = 0x9E3779B9


def rotl(v, n):
    n %= 32
    return ((v << n) | (v >> (32 - n))) & 0xFFFFFFFF if n else v


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=1500)
    args = ap.parse_args()
    inits = [rotl(SEED, 4 * i) for i in range(8)]
    ms = [sum(((inits[i] >> a) & 1) << i for i in range(8)) for a in range(32)]
    md = [sum(((~inits[i] >> a) & 1) << i for i in range(8)) for a in range(32)]
    wa = 0
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()

        def read(a):
            nonlocal bad
            s.write(bytes([ord("R"), a]))
            got = s.read(3)
            want = bytes([ms[a % 32], md[wa % 32], md[a % 32]])
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  R {a:#04x} (wa {wa:#04x}): got {got.hex()} want {want.hex()}")

        for a in range(32):
            read(a)
        if bad:
            print(f"FAIL: {bad} initial reads wrong")
            sys.exit(1)
        print("initial contents correct")
        for _ in range(args.ops):
            if random.random() < 0.5:
                a, d = random.getrandbits(8), random.getrandbits(8)
                s.write(bytes([ord("W"), a, d]))
                if s.read(1) != b"K":
                    print("FAIL: W not acknowledged")
                    sys.exit(1)
                wa = a
                ms[a % 32] = d
                md[a % 32] = d
            else:
                read(random.getrandbits(8))
    if bad:
        print(f"FAIL: {bad} reads wrong")
        sys.exit(1)
    print(f"PASS: {args.ops} operations on RAM32X1S and RAM32X1D correct")


if __name__ == "__main__":
    main()
