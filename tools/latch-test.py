#!/usr/bin/env python3
"""Hardware check for tests/latch (slice latches: LDCE, LDPE, inverted gate).

    tools/latch-test.py [--port /dev/ttyACM0] [--ops 2000]

Load the design first (make load TEST=latch).
"""

import argparse
import random
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=2000)
    args = ap.parse_args()
    data, gate, clr = 0, 0, 0
    qa = qb = qn = None  # unknown until first written

    def settle():
        nonlocal qa, qb, qn
        if clr:
            qa, qb = 0x00, 0xFF
        elif gate:
            qa, qb = data, data
        if not gate:
            qn = data

    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()

        def cmd(c, v):
            s.write(bytes([ord(c), v]))
            if s.read(1) != b"K":
                print(f"FAIL: {c} not acknowledged")
                sys.exit(1)

        # Known start: clear/preset, then open and close the gate.
        cmd("C", 1); clr = 1; settle()
        cmd("C", 0); clr = 0
        cmd("G", 0); gate = 0; settle()
        for i in range(args.ops):
            k = random.choice("DDGGCQQ")
            if k == "D":
                data = random.getrandbits(8); cmd("D", data)
            elif k == "G":
                gate = random.getrandbits(1); cmd("G", gate)
            elif k == "C":
                clr = 1 if random.random() < 0.3 else 0; cmd("C", clr)
            settle()
            if k == "Q":
                s.write(b"Q")
                got = s.read(3)
                want = bytes([qa, qb, qn])
                if got != want:
                    bad += 1
                    if bad <= 8:
                        print(f"  Q (data {data:02x} gate {gate} clr {clr}): got {got.hex()} want {want.hex()}")
    if bad:
        print(f"FAIL: {bad} reads wrong")
        sys.exit(1)
    print(f"PASS: {args.ops} latch operations correct (LDCE, LDPE, inverted gate)")


if __name__ == "__main__":
    main()
