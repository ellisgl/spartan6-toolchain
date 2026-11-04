#!/usr/bin/env python3
"""Hardware check for tests/plldrp (PLL_ADV dynamic reconfiguration port).

    tools/plldrp-test.py [--port /dev/ttyACM0] [--rounds 3]

Load the design first (make load TEST=plldrp). It is built with
CLKOUT0_DIVIDE = 4 (125 MHz). Over the DRP, with the PLL held in reset, the
test writes the two words that differ in the same design built with
CLKOUT0_DIVIDE = 5 (read from that build on the board: 0x06 and 0x0d),
reads them back, releases the reset and checks
CLKOUT0 is now 100 MHz; then restores the original words and checks
125 MHz again. Every other DRP word must be unchanged throughout.
"""


import argparse
import struct
import sys
import time

import serial

WINDOW = 65536


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--rounds", type=int, default=3)
    args = ap.parse_args()
    with serial.Serial(args.port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()

        def rd(a):
            s.write(bytes([ord("R"), a]))
            r = s.read(2)
            if len(r) != 2:
                print(f"FAIL: no reply to DRP read of {a:#04x}")
                sys.exit(1)
            return r[0] | r[1] << 8

        def wr(a, v):
            s.write(bytes([ord("W"), a, v & 0xFF, v >> 8]))
            if s.read(1) != b"K":
                print(f"FAIL: DRP write of {a:#04x} not acknowledged")
                sys.exit(1)

        def rst(r):
            s.write(bytes([ord("X"), r]))
            if s.read(1) != b"K":
                print("FAIL: X not acknowledged")
                sys.exit(1)

        def measure():
            s.write(b"C")
            r = s.read(5)
            if len(r) != 5:
                print("FAIL: no reply to C")
                sys.exit(1)
            n, locked = struct.unpack("<IB", r)
            return n * 50.0 / WINDOW, locked & 1

        # The words for CLKOUT0_DIVIDE = 4 (as built) and 5.
        DIV4 = {0x06: 0x1101, 0x0D: 0x2008}
        DIV5 = {0x06: 0x1105, 0x0D: 0x2028}
        bad = False

        def check(want_mhz, words, what):
            nonlocal bad
            got = {a: rd(a) for a in words}
            f, locked = measure()
            ok = locked and abs(f - want_mhz) < 0.1 and got == words
            bad |= not ok
            print(f"  {what}: CLKOUT0 {f:.2f} MHz (want {want_mhz:.0f}), LOCKED {locked}, "
                  + " ".join(f"{a:#04x}={v:04x}" for a, v in got.items()) + ("" if ok else "  <-- wrong"))

        def load(words):
            rst(1)
            for a, v in words.items():
                wr(a, v)
            rst(0)
            time.sleep(0.05)

        before = [rd(a) for a in range(32)]
        print("  DRP:", " ".join(f"{v:04x}" for v in before))
        check(125.0, DIV4, "as built")
        for r in range(args.rounds):
            load(DIV5)
            check(100.0, DIV5, f"round {r}, divide 5")
            load(DIV4)
            check(125.0, DIV4, f"round {r}, divide 4")
        after = [rd(a) for a in range(32)]
        if after != before:
            print("FAIL: DRP contents differ from before the test")
            sys.exit(1)
        if bad:
            print("FAIL: wrong frequency, DRP readback or LOCKED")
            sys.exit(1)
    print("PASS: CLKOUT0 reprogrammed over the DRP (125 -> 100 -> 125 MHz), read back, LOCKED")


if __name__ == "__main__":
    main()
