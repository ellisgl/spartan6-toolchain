#!/usr/bin/env python3
"""Hardware check for tests/alu (carry chains): send pairs of 32-bit numbers
over the Mojo's USB serial port and check the sum, difference, a*3+1 and
comparison flags the FPGA sends back.

    tools/alu-test.py [--port /dev/ttyACM0] [--count 300]

Load the design first (make load TEST=alu).
"""

import argparse
import random
import struct
import sys
import time

import serial

M = 0xFFFFFFFF


def expected(a, b):
    s = a + b
    sa = a - (1 << 32) if a & 0x80000000 else a
    sb = b - (1 << 32) if b & 0x80000000 else b
    flags = 0xA0 | (s >> 32) << 3 | (sa < sb) << 2 | (a == b) << 1 | (a < b)
    return struct.pack("<IIIB", s & M, (a - b) & M, (a * 3 + 1) & M, flags)


def cases(n):
    edge = [0, 1, 2, 0x7FFFFFFF, 0x80000000, 0xFFFFFFFE, M, 0x0000FFFF, 0xFFFF0000, 0x55555555, 0xAAAAAAAA]
    out = [(a, b) for a in edge for b in edge]
    out += [(x, x) for x in edge]
    while len(out) < n:
        a = random.getrandbits(32)
        b = random.choice([random.getrandbits(32), a, (a + 1) & M, (a - 1) & M, (~a) & M])
        out.append((a, b))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--count", type=int, default=300)
    args = ap.parse_args()

    bad = 0
    tests = cases(args.count)
    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()
        for a, b in tests:
            port.write(struct.pack("<II", a, b))
            got = port.read(13)
            want = expected(a, b)
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  a={a:#010x} b={b:#010x}: got {got.hex()} want {want.hex()}")
                if not got:
                    print("FAIL: no reply")
                    sys.exit(1)
    if bad:
        print(f"FAIL: {bad} of {len(tests)} wrong")
        sys.exit(1)
    print(f"PASS: {len(tests)} add/sub/mul3/compare results correct")


if __name__ == "__main__":
    main()
