#!/usr/bin/env python3
"""Hardware check for tests/tristate: drive or release two header pins over
the Mojo's USB serial port and check what they read back.

    tools/tristate-test.py [--port /dev/ttyACM0] [--rounds 8]

Load the design first (make load TEST=tristate); nothing may be connected to
P51 (pa, pull-up) or P50 (pb, pull-down). Each command byte's low nibble is
{pb value, pb enable, pa value, pa enable}; the reply is 0xA0 | pb << 1 | pa,
where an enabled pin reads its own value and a released one its pull.
Getting any reply at all also shows avr_rx (an OBUFT) came out of tristate.
"""

import argparse
import random
import sys
import time

import serial


def expected(cmd):
    pa = (cmd >> 1) & 1 if cmd & 1 else 1  # pull-up
    pb = (cmd >> 3) & 1 if cmd & 4 else 0  # pull-down
    return 0xA0 | pb << 1 | pa


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--rounds", type=int, default=8, help="passes over all 16 pin states")
    args = ap.parse_args()

    cmds = []
    for _ in range(args.rounds):
        r = list(range(16))
        random.shuffle(r)
        cmds += r
    bad = 0
    with serial.Serial(args.port, 115200, timeout=0.5) as port:  # USB side baud is ignored
        # The AVR sends a few bytes while its bridge starts up; let the
        # design answer them, then drop the answers.
        time.sleep(0.5)
        port.reset_input_buffer()
        for i, cmd in enumerate(cmds):
            port.write(bytes([cmd]))
            got = port.read(1)
            if not got:
                print(f"FAIL: no reply to command {i} ({cmd:#04x})")
                sys.exit(1)
            if got[0] != expected(cmd):
                bad += 1
                if bad <= 8:
                    print(f"  cmd {cmd:#06b}: got {got[0]:#04x}, expected {expected(cmd):#04x}")
    if bad:
        print(f"FAIL: {bad} of {len(cmds)} replies wrong")
        sys.exit(1)
    print(f"PASS: {len(cmds)} pin states read back as expected (driven 0/1, pull-up, pull-down)")


if __name__ == "__main__":
    main()
