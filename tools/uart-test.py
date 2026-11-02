#!/usr/bin/env python3
"""Hardware check for tests/uart_echo: send bytes over the Mojo's USB serial
port and check that every reply is the byte + 1.

    tools/uart-test.py [--port /dev/ttyACM0] [--count 256]

Load the design first (make load TEST=uart_echo). Opening the port asserts
DTR, which the Mojo's AVR needs before it forwards FPGA data to USB. (Five
DTR toggles within 250 ms would put it back in loader mode; a single open
does not.)
"""

import argparse
import os
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--count", type=int, default=256)
    args = ap.parse_args()

    data = os.urandom(args.count)
    with serial.Serial(args.port, 115200, timeout=0.5) as port:  # USB side baud is ignored
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()
        bad = 0
        got = bytearray()
        # Send in small chunks so the FPGA's one-byte buffer keeps up
        # (each reply takes 20 us at 500 kbaud).
        for i in range(0, len(data), 16):
            chunk = data[i : i + 16]
            port.write(chunk)
            port.flush()
            got += port.read(len(chunk))
        expected = bytes((b + 1) & 0xFF for b in data)
        if len(got) != len(expected):
            print(f"FAIL: sent {len(data)} bytes, got {len(got)} back")
            sys.exit(1)
        for i, (g, e) in enumerate(zip(got, expected)):
            if g != e:
                bad += 1
                if bad <= 5:
                    print(f"  byte {i}: sent {data[i]:#04x}, got {g:#04x}, expected {e:#04x}")
        if bad:
            print(f"FAIL: {bad} of {len(data)} replies wrong")
            sys.exit(1)
        print(f"PASS: {len(data)} bytes echoed as byte+1")


if __name__ == "__main__":
    main()
