#!/usr/bin/env python3
"""Hardware check for tests/oddrtsr (ODDR2 set/reset from logic, shared by a
pad's data and tristate ODDR2s).

    tools/oddrtsr-test.py [--port /dev/ttyACM0]

Load the design first (make load TEST=oddrtsr). pa's signal sets its data
register and resets its tristate one: the pin is driven 1 whatever the
data and T. pb's resets its data and sets its tristate: the pin is released
(its pull-down reads 0). With both off the pins follow the data as in
tools/oddrt-test.py.
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

        def cmd(op, x):
            s.write(op + bytes([x]))
            if s.read(1) != b"K":
                sys.exit(f"FAIL: no reply to {op!r}")

        def sample():
            time.sleep(0.01)
            s.write(b"C")
            r = s.read(16)
            if len(r) != 16:
                sys.exit("FAIL: short reply to C")
            return [v / 65536 for v in struct.unpack("<4I", r)]

        # x: pa D0 D1 T (bits 0..2), pb D0 D1 T0 T1 (bits 3..6)
        for x, label in [(0b0100100, "both released"), (0b0000000, "both driving 0"), (0b0011011, "pa 1, pb 1")]:
            cmd(b"S", x)
            for sr in (0, 1, 2, 3):
                cmd(b"R", sr)
                an, ap_, bn, bp = sample()
                a_t, b_t = x >> 2 & 1, x >> 5 & 1
                a_d, b_d = x & 1, x >> 3 & 1
                want_a = 1.0 if sr & 1 else (1.0 if a_t else float(a_d))
                want_b = 0.0 if sr & 2 else (0.0 if b_t else float(b_d))
                ok = all(abs(v - w) < 0.01 for v, w in ((an, want_a), (ap_, want_a), (bn, want_b), (bp, want_b)))
                n += 1
                bad += not ok
                print(f"  {label:15} R={sr:02b}: pa {an:.2f}/{ap_:.2f} (want {want_a:.0f}), pb {bn:.2f}/{bp:.2f} (want {want_b:.0f})"
                      f" -> {'pass' if ok else 'FAIL'}")
        cmd(b"R", 0)
    if bad:
        sys.exit(f"FAIL: {bad} of {n}")
    print(f"PASS: {n} set/reset cases on the data and tristate registers")


if __name__ == "__main__":
    main()
