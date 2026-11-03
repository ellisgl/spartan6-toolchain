#!/usr/bin/env python3
"""Hardware check for tests/oddr (ODDR2: DDR output / clock forwarding).

    tools/oddr-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=oddr). For each (D0, D1) the pin, read
back through its own input buffer, must show D0 when sampled on the clock's
falling edge and D1 on its rising edge, every cycle.
"""

import argparse
import struct
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    args = ap.parse_args()
    bad = False
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for d0, d1 in [(1, 0), (0, 1), (1, 1), (0, 0)]:
            s.write(bytes([ord("S"), d0 | d1 << 1]))
            if s.read(1) != b"K":
                print("FAIL: S not acknowledged")
                sys.exit(1)
            s.write(b"C")
            got = s.read(8)
            if len(got) != 8:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            on, op = struct.unpack("<II", got)
            ok = on == 65536 * d0 and op == 65536 * d1
            bad |= not ok
            print(f"  D0={d0} D1={d1}: high at falling edge {100 * on / 65536:5.1f}%, "
                  f"at rising edge {100 * op / 65536:5.1f}% -> {'pass' if ok else 'FAIL'}")
    if bad:
        print("FAIL")
        sys.exit(1)
    print("PASS: ODDR2 drives D0 in the first half of each cycle and D1 in the second")


if __name__ == "__main__":
    main()
