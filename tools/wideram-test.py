#!/usr/bin/env python3
"""Hardware check for tests/wideram (RAM64X1D, RAM128X1D, RAM256X1S).

    tools/wideram-test.py [--port /dev/ttyACM0] [--ops 1500]

Load the design first (make load TEST=wideram). Checks the initial contents
first, so run it right after loading.
"""

import argparse
import random
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=1500)
    args = ap.parse_args()
    m64 = [(13 * i + 5) & 0xFF for i in range(64)]
    mq = [((7 * i + 3) >> 2) & 1 for i in range(128)]
    mr = [((5 * i + 1) >> 1) & 1 for i in range(128)]
    ms = [((11 * i + 7) >> 3) & 1 for i in range(256)]
    wa = 0
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()

        def read(a):
            nonlocal bad
            s.write(bytes([ord("R"), a]))
            got = s.read(3)
            bits = (mq[wa % 128] | mq[a % 128] << 1 | mr[wa % 128] << 2 | mr[a % 128] << 3 | ms[wa] << 4)
            want = bytes([m64[wa % 64], m64[a % 64], bits])
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  R {a:#04x} (wa {wa:#04x}): got {got.hex()} want {want.hex()}")

        def write(a, d):
            nonlocal wa
            s.write(bytes([ord("W"), a, d]))
            if s.read(1) != b"K":
                print("FAIL: write not acknowledged")
                sys.exit(1)
            wa = a
            m64[a % 64] = d
            mq[a % 128] = d & 1
            mr[a % 128] = d >> 1 & 1
            ms[a] = d >> 2 & 1

        # Initial contents, without writing: wa stays 0, read every address.
        for a in range(256):
            read(a)
        if bad:
            print(f"FAIL: {bad} initial reads wrong")
            sys.exit(1)
        print("initial contents correct")
        for _ in range(args.ops):
            if random.random() < 0.5:
                write(random.getrandbits(8), random.getrandbits(8))
            else:
                read(random.getrandbits(8))
    if bad:
        print(f"FAIL: {bad} reads wrong")
        sys.exit(1)
    print(f"PASS: {args.ops} wide-RAM operations correct (RAM64X1D, RAM128X1D, RAM256X1S)")


if __name__ == "__main__":
    main()
