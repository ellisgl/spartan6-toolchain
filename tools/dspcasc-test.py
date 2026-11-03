#!/usr/bin/env python3
"""Hardware check for tests/dspcasc (DSP48A1 PCIN/BCIN/CARRYIN cascades).

    tools/dspcasc-test.py [--port /dev/ttyACM0] [--ops 300]

Load the design first (make load TEST=dspcasc).
"""

import argparse
import random
import struct
import sys
import time

import serial

M48 = (1 << 48) - 1


def s18(v):
    v &= (1 << 18) - 1
    return v - (1 << 18) if v >> 17 else v


def model(a0, a1, a2, b):
    m = [(s18(a) * s18(b)) & M48 for a in (a0, a1, a2)]
    p0 = m[0]
    s1 = p0 + m[1]
    p1, co1 = s1 & M48, s1 >> 48
    p2 = (p1 + m[2] + co1) & M48
    return p2, co1


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=300)
    args = ap.parse_args()
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        cases = [(1, 2, 3, 4), (0x1FFFF, 0x1FFFF, 0x1FFFF, 0x1FFFF), (0x20000, 0x20000, 0x20000, 0x20000),
                 (0x3FFFF, 1, 0x3FFFF, 0x3FFFF)]
        cases += [tuple(random.getrandbits(18) for _ in range(4)) for _ in range(args.ops)]
        for a0, a1, a2, b in cases:
            s.write(b"L" + b"".join(struct.pack("<I", v)[:3] for v in (a0, a1, a2, b)))
            if s.read(1) != b"K":
                print("FAIL: L not acknowledged")
                sys.exit(1)
            s.write(b"R")
            got = s.read(7)
            if len(got) != 7:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            p2 = int.from_bytes(got[:6], "little")
            co1 = got[6] & 1
            want = model(a0, a1, a2, b)
            if (p2, co1) != want:
                bad += 1
                if bad <= 8:
                    print(f"  a=({a0:#x},{a1:#x},{a2:#x}) b={b:#x}: got P2={p2:#014x} co={co1}, "
                          f"want P2={want[0]:#014x} co={want[1]}")
    if bad:
        print(f"FAIL: {bad} of {len(cases)} wrong")
        sys.exit(1)
    print(f"PASS: {len(cases)} cascaded sums of products correct (PCIN, BCIN, CARRYIN)")


if __name__ == "__main__":
    main()
