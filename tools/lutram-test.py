#!/usr/bin/env python3
"""Hardware check for tests/lutram (distributed RAM, shift registers).

    tools/lutram-test.py [--port /dev/ttyACM0] [--ops 1500]

Load the design first (make load TEST=lutram).
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

    # Initial contents (see tests/lutram/lutram.v); checked first, so run
    # this right after loading the design.
    m64 = [(37 * i + 11) & 0xFF for i in range(64)]
    m32 = [(91 * i + 5) & 0xFF for i in range(32)]
    sr = [(53 * i + 7) & 0xFF for i in range(24)]
    md = [(29 * i + 3) & 0xFF for i in range(64)]
    mb = [(7 * i + 1) & 3 for i in range(128)]
    lsr = [None] * 70  # unknown until shifted in
    raddr = [0]
    pat = 0xA5C3
    bad = 0
    n = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        time.sleep(0.5)
        port.reset_input_buffer()

        def write(a, d):
            port.write(bytes([ord("W"), a, d]))
            if port.read(1) != b"K":
                print("FAIL: write not acknowledged")
                sys.exit(1)
            m64[a % 64] = d
            m32[a % 32] = d
            md[a % 64] = d
            mb[a % 128] = d & 3

        def read(a):
            """(got, want) for 'R' a; md comes from the previous R's address."""
            port.write(bytes([ord("R"), a]))
            got = port.read(4)
            want = bytes([m64[a % 64], m32[a % 32], md[raddr[0]], mb[a % 128]])
            raddr[0] = a % 64
            return got, want

        # Initial contents: read every address, then shift the initial
        # shift register contents out.
        init_bad = 0
        for a in list(range(128)) + [0]:
            got, want = read(a)
            if got != want:
                init_bad += 1
                if init_bad <= 8:
                    print(f"  initial R {a:#04x}: got {got.hex()} want {want.hex()}")
        for i in range(24):
            d = random.getrandbits(8)
            port.write(bytes([ord("S"), d]))
            got = port.read(1)
            want = sr[-1]
            sr = [d] + sr[:-1]
            if got != bytes([want]):
                init_bad += 1
                if init_bad <= 8:
                    print(f"  initial S #{i}: got {got.hex()} want {want:02x}")
        if init_bad:
            print(f"FAIL: {init_bad} initial values wrong")
            sys.exit(1)
        print("initial RAM and shift register contents correct")
        for op in range(args.ops):
            kind = random.choice("WRSPL")
            n += 1
            if kind == "W":
                write(random.getrandbits(8), random.getrandbits(8))
            elif kind == "R":
                a = random.getrandbits(8)
                got, want = read(a)
                if got != want:
                    bad += 1
                    if bad <= 8:
                        print(f"  R {a:#04x}: got {got.hex()} want {want.hex()}")
            elif kind == "P":
                port.write(b"P")
                got = port.read(1)
                want = bytes([pat >> 15 & 1])
                pat = (pat << 1 | pat >> 15) & 0xFFFF
                if got != want:
                    bad += 1
                    if bad <= 8:
                        print(f"  P: got {got.hex()} want {want.hex()}")
            elif kind == "L":
                d = random.getrandbits(8)
                port.write(bytes([ord("L"), d]))
                got = port.read(1)
                want = lsr[-1]
                lsr = [d] + lsr[:-1]
                if not got:
                    print("FAIL: no reply to L")
                    sys.exit(1)
                if want is not None and got != bytes([want]):
                    bad += 1
                    if bad <= 8:
                        print(f"  L: got {got.hex()} want {want:02x}")
            else:
                d = random.getrandbits(8)
                port.write(bytes([ord("S"), d]))
                got = port.read(1)
                want = sr[-1]
                sr = [d] + sr[:-1]
                if want is not None and got != bytes([want]):
                    bad += 1
                    if bad <= 8:
                        print(f"  S: got {got.hex()} want {want:02x}")
                elif not got:
                    print("FAIL: no reply to S")
                    sys.exit(1)
    if bad:
        print(f"FAIL: {bad} of {n} operations wrong")
        sys.exit(1)
    print(f"PASS: {n} RAM/shift-register operations correct")


if __name__ == "__main__":
    main()
