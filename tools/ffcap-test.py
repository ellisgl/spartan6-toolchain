#!/usr/bin/env python3
"""Hardware check for tests/ffcap (7680 FFs, most of the 5FFs through pass-through LUT5s).

    tools/ffcap-test.py [--port /dev/ttyACM0] [--ops 3000]

Load the design first (make load TEST=ffcap). Every stage is checked: a
byte shifted in comes out of stage 479 after 480 shifts and stage 959 after
960, so --ops above 960 covers the whole chain.
"""

import argparse
import random
import sys
import time

import serial

DEPTH = 960


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--ops", type=int, default=3000)
    args = ap.parse_args()
    stages = [0] * DEPTH  # stage 0 = newest
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for i in range(args.ops):
            if i == 0 or random.random() < 0.002:
                s.write(b"R\x00")
                if s.read(1) != b"K":
                    print("FAIL: R not acknowledged")
                    sys.exit(1)
                stages = [0] * DEPTH
                continue
            d = random.getrandbits(8)
            s.write(bytes([ord("S"), d]))
            got = s.read(2)
            want = bytes([stages[DEPTH - 1], stages[DEPTH // 2 - 1]])
            if got != want:
                bad += 1
                if bad <= 8:
                    print(f"  S #{i}: got {got.hex()} want {want.hex()}")
            stages = [d] + stages[:-1]
    if bad:
        print(f"FAIL: {bad} replies wrong")
        sys.exit(1)
    print(f"PASS: {args.ops} operations through {DEPTH} x 8 stages")


if __name__ == "__main__":
    main()
