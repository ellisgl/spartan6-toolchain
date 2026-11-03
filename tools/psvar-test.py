#!/usr/bin/env python3
"""Hardware check for tests/psvar (DCM variable phase shift).

    tools/psvar-test.py [--port /dev/ttyACM0] [--steps 400] [--every 50]

Steps the DCM's phase later (PSINCDEC = 1) and then earlier, sampling the
shifted clock on the unshifted board clock every few steps. Every step must
end with PSDONE; the sampled level must read mostly low after shifting later
and mostly high after shifting earlier (as tests/phase does with fixed
shifts). Load the design first (make load TEST=psvar).
"""

import argparse
import struct
import sys
import time

import serial


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--steps", type=int, default=400, help="steps each way")
    ap.add_argument("--every", type=int, default=50, help="sample every this many steps")
    args = ap.parse_args()

    with serial.Serial(args.port, 115200, timeout=1) as port:
        time.sleep(0.5)
        port.reset_input_buffer()

        def sample():
            port.write(b"P")
            r = port.read(5)
            if len(r) != 5:
                sys.exit(f"FAIL: short reply to P ({r.hex()})")
            ones, flags = struct.unpack("<IB", r)
            if not flags & 1:
                sys.exit("FAIL: DCM not locked")
            return ones / 65536

        def step(cmd, n):
            for i in range(n):
                port.write(cmd)
                r = port.read(1)
                if r != b"D":
                    sys.exit(f"FAIL: step {i} ({cmd.decode()}): {r!r} instead of PSDONE")

        pos = 0
        trace = [(0, sample())]
        # Later, then back past the start to earlier.
        for cmd, sign, total in ((b"+", 1, args.steps), (b"-", -1, 2 * args.steps)):
            done = 0
            while done < total:
                n = min(args.every, total - done)
                step(cmd, n)
                done += n
                pos += sign * n
                trace.append((pos, sample()))
        for p, f in trace:
            print(f"  phase {p:+5d} steps: sampled high {f:6.1%}")
        late = trace[[p for p, _ in trace].index(args.steps)][1]
        early = trace[-1][1]
        if late < 0.1 and early > 0.9:
            print(f"PASS: {4 * args.steps} phase steps with PSDONE; later reads low ({late:.1%}), earlier high ({early:.1%})")
        else:
            sys.exit(f"FAIL: expected low after +{args.steps} and high after -{args.steps} steps")


if __name__ == "__main__":
    main()
