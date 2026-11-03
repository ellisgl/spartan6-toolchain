#!/usr/bin/env python3
"""Hardware check for tests/oddrt (tristated DDR outputs: ODDR2 data and an
ODDR2 on T, packed into one OLOGIC).

    tools/oddrt-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=oddrt). For each setting the two pins,
read back through their own input buffers, must show the ODDR2 data (D0 on
the clock's falling edge, D1 on its rising edge) while driven, and their pull
(pa up, pb down) while released. With T driving for one half of the cycle
only, the weak pull can't move the pin in the other half: it keeps the value
driven (D0 with T0/T1 = 0/1, D1 with 1/0), which checks both T phases.
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
    bad = 0
    n = 0
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()

        def run(x):
            s.write(b"S" + bytes([x]))
            if s.read(1) != b"K":
                sys.exit("FAIL: no reply to S")
            time.sleep(0.01)
            s.write(b"C")
            r = s.read(16)
            if len(r) != 16:
                sys.exit(f"FAIL: short reply to C ({r.hex()})")
            return [v / 65536 for v in struct.unpack("<4I", r)]

        # pa: x[0] D0, x[1] D1, x[2] T; pb: x[3] D0, x[4] D1, x[5] T0, x[6] T1
        for ta in (0, 1):
            for tb in ((0, 0), (1, 1), (0, 1), (1, 0)):
                for d in range(4):
                    d0, d1 = d & 1, d >> 1
                    # pb's data the other way round, so the pins differ
                    x = d0 | d1 << 1 | ta << 2 | (1 - d0) << 3 | (1 - d1) << 4 | tb[0] << 5 | tb[1] << 6
                    an, apos, bn, bpos = run(x)
                    want_a = (1.0, 1.0) if ta else (d0, d1)
                    got_a = (an, apos)
                    ok_a = all(abs(g - w) < 0.01 for g, w in zip(got_a, want_a))
                    b0, b1 = 1 - d0, 1 - d1
                    want_b = {(0, 0): (b0, b1), (1, 1): (0, 0), (0, 1): (b0, b0), (1, 0): (b1, b1)}[tb]
                    ok_b = all(abs(g - w) < 0.01 for g, w in zip((bn, bpos), want_b))
                    n += 1
                    bad += not ok_b
                    if not ok_b:
                        print(f"  pb T0/T1={tb} D=({b0},{b1}): read {bn:.2f}/{bpos:.2f}, want {want_b}")
                    n += 1
                    bad += not ok_a
                    if not ok_a:
                        print(f"  pa T={ta} D=({d0},{d1}): read {an:.2f}/{apos:.2f}, want {want_a}")
    if bad:
        sys.exit(f"FAIL: {bad} of {n} checks wrong")
    print(f"PASS: {n} driven/released checks (DDR data, ODDR2 tristate, pulls)")


if __name__ == "__main__":
    main()
