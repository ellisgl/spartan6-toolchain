#!/usr/bin/env python3
"""Hardware check for tests/dsp (DSP48A1): signed 18x18 multiplies and a
48-bit multiply-accumulate over the Mojo's USB serial port.

    tools/dsp-test.py [--port /dev/ttyACM0] [--count 500]

Load the design first (make load TEST=dsp).
"""

import argparse
import random
import struct
import sys
import time

import serial


def s18(v):
    return v - (1 << 18) if v & (1 << 17) else v


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--count", type=int, default=500)
    args = ap.parse_args()

    edge = [0, 1, 2, 0x1FFFF, 0x20000, 0x3FFFF, 0x3FFFE, 0x15555, 0x2AAAA]
    pairs = [(a, b) for a in edge for b in edge]
    while len(pairs) < args.count:
        pairs.append((random.getrandbits(18), random.getrandbits(18)))
    bad = 0
    acc = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()

        def cmd(op, a, b, n):
            port.write(op + a.to_bytes(3, "little") + b.to_bytes(3, "little"))
            return port.read(n)

        port.write(b"Z")
        if port.read(1) != b"K":
            print("FAIL: no reply to Z")
            sys.exit(1)
        for a, b in pairs:
            prod = s18(a) * s18(b)
            got = cmd(b"M", a, b, 5)
            want = (prod & ((1 << 40) - 1)).to_bytes(5, "little")
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  M {s18(a)} * {s18(b)}: got {got.hex()} want {want.hex()}")
            acc = (acc + prod) & ((1 << 48) - 1)
            got = cmd(b"A", a, b, 6)
            want = acc.to_bytes(6, "little")
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  A += {s18(a)} * {s18(b)}: got {got.hex()} want {want.hex()}")
                acc = int.from_bytes(got, "little") if len(got) == 6 else acc
    if bad:
        print(f"FAIL: {bad} of {2 * len(pairs)} results wrong")
        sys.exit(1)
    print(f"PASS: {len(pairs)} multiplies and {len(pairs)} accumulates correct")


if __name__ == "__main__":
    main()
