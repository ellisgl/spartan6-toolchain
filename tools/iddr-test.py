#!/usr/bin/env python3
"""Hardware check for tests/iddr (IDDR2: DDR input).

    tools/iddr-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=iddr). The pin is driven from the
fabric and read back by an IDDR2: held low/high both samples follow it;
toggled every cycle, the rising- and falling-edge samples always differ.
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
    bad = False
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for mode, name in [(0, "held low"), (1, "held high"), (2, "toggling")]:
            s.write(bytes([ord("S"), mode]))
            if s.read(1) != b"K":
                print("FAIL: S not acknowledged")
                sys.exit(1)
            s.write(b"C")
            got = s.read(12)
            if len(got) != 12:
                print(f"FAIL: short reply {got.hex()}")
                sys.exit(1)
            n0, n1, nd = struct.unpack("<III", got)
            if mode == 2:
                ok = nd == N and abs(n0 - N // 2) <= 2 and abs(n1 - N // 2) <= 2
            else:
                ok = n0 == n1 == mode * N and nd == 0
            bad |= not ok
            print(f"  pin {name:9}: Q0 high {100 * n0 / N:5.1f}%, Q1 high {100 * n1 / N:5.1f}%, "
                  f"Q0 != Q1 {100 * nd / N:5.1f}% -> {'pass' if ok else 'FAIL'}")
    if bad:
        print("FAIL")
        sys.exit(1)
    print("PASS: IDDR2 captures the pin on both clock edges")


if __name__ == "__main__":
    main()
