#!/usr/bin/env python3
"""Hardware check for tests/srlq31 (SRLC32E using both Q and Q31).

    tools/srlq31-test.py [--port /dev/ttyACM0] [--ops 600]

Load the design first (make load TEST=srlq31).
"""

import argparse
import random
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=600)
    args = ap.parse_args()
    stages = [0] * 64  # stage 0 = newest; INIT 0
    a = 0
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for _ in range(args.ops):
            if random.random() < 0.2:
                a = random.getrandbits(5)
                s.write(bytes([ord("A"), a]))
                if s.read(1) != b"K":
                    print("FAIL: A not acknowledged")
                    sys.exit(1)
                continue
            d = random.getrandbits(8)
            s.write(bytes([ord("S"), d]))
            got = s.read(2)
            want = bytes([stages[a], stages[63]])
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  S (a={a}): got {got.hex()} want {want.hex()}")
            stages = [d] + stages[:-1]
    if bad:
        print(f"FAIL: {bad} replies wrong")
        sys.exit(1)
    print(f"PASS: {args.ops} operations; the tap (Q) and the cascade (Q31) both correct")


if __name__ == "__main__":
    main()
