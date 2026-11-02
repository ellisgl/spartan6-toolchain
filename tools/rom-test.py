#!/usr/bin/env python3
"""Hardware check for tests/rom (MUXF7/MUXF8): read every ROM address over
the Mojo's USB serial port.

    tools/rom-test.py [--port /dev/ttyACM0] [--rounds 4]

Load the design first (make load TEST=rom).
"""

import argparse
import os
import random
import sys
import time

import serial

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rom_data import rom7_bytes, rom_bytes  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--rounds", type=int, default=4)
    args = ap.parse_args()

    rom = rom_bytes()
    rom7 = rom7_bytes()
    bad = 0
    n = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as port:
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()
        for _ in range(args.rounds):
            addrs = list(range(256))
            random.shuffle(addrs)
            for a in addrs:
                port.write(bytes([a]))
                got = port.read(3)
                want = bytes([rom[a], rom[a ^ 0x5A], rom7[a & 0x7F]])
                n += 1
                if got != want:
                    bad += 1
                    if bad <= 8:
                        print(f"  addr {a:#04x}: got {got.hex()} want {want.hex()}")
                    if not got:
                        print("FAIL: no reply")
                        sys.exit(1)
    if bad:
        print(f"FAIL: {bad} of {n} reads wrong")
        sys.exit(1)
    print(f"PASS: {n} ROM reads correct")


if __name__ == "__main__":
    main()
