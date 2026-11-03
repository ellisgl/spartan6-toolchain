#!/usr/bin/env python3
"""Hardware check for tests/iddrsr (IDDR2 with its reset R from logic).

    tools/iddrsr-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=iddrsr). With the pin held high, Q0
and Q1 must read 1 with R off and 0 with R on (synchronous reset, held); the
pin toggling with R off must still give Q0 != Q1 every cycle (as
tools/iddr-test.py checks).
"""

import argparse
import struct
import sys
import time

import serial

N = 65536


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    args = ap.parse_args()
    bad = 0
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        # (mode, R, name, expected (n0, n1, nd))
        cases = [
            (1, 0, "held high, R off", (N, N, 0)),
            (1, 1, "held high, R on ", (0, 0, 0)),
            (0, 1, "held low,  R on ", (0, 0, 0)),
            (1, 0, "held high, R off", (N, N, 0)),
            (2, 0, "toggling,  R off", (N // 2, N // 2, N)),
        ]
        for mode, r, name, want in cases:
            s.write(bytes([ord("S"), mode | r << 2]))
            if s.read(1) != b"K":
                sys.exit("FAIL: S not acknowledged")
            time.sleep(0.01)
            s.write(b"C")
            got = s.read(12)
            if len(got) != 12:
                sys.exit("FAIL: short reply to C")
            n0, n1, nd = struct.unpack("<3I", got)
            ok = (n0, n1, nd) == want if mode != 2 else nd == N and abs(n0 - N // 2) <= 1 and abs(n1 - N // 2) <= 1
            bad += not ok
            print(f"  {name}: Q0 high {n0 / N:6.1%}, Q1 high {n1 / N:6.1%}, Q0 != Q1 {nd / N:6.1%} -> {'pass' if ok else 'FAIL'}")
    if bad:
        sys.exit("FAIL")
    print("PASS: IDDR2 reset from logic clears both halves; captures normally when released")


if __name__ == "__main__":
    main()
