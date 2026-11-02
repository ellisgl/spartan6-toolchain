#!/usr/bin/env python3
"""Load a bitstream onto a Mojo V3 through its AVR (USB serial).

    tools/mojo-load.py build/cylon-ise/cylon.bin            # RAM: lost on power cycle
    tools/mojo-load.py --flash build/cylon-ise/cylon.bin    # flash, verify, then load

Follows the protocol of the official loader (embmicro/mojo-loader,
MojoLoader.java): reset the AVR by toggling DTR, send the command byte
('R' RAM, 'F' flash, 'V' flash + verify), expect 'R', send the file length as
4 bytes little-endian, expect 'O', send the raw .bin, expect 'D'. For verify:
send 'S', expect 0xAA, a 4-byte LE length (file length + 5), then the file
bytes echoed back. Finally 'L' loads the FPGA from flash and expects 'D'.
"""

import argparse
import sys
import time

import serial


def expect(port, want, timeout, what):
    port.timeout = timeout
    got = port.read(1)
    if got != want:
        sys.exit(f"error: {what} (expected {want!r}, got {got!r})")


def restart(port):
    # Same DTR pattern as MojoLoader.restartMojo(); ends with DTR high.
    port.dtr = False
    time.sleep(0.005)
    for _ in range(5):
        port.dtr = False
        time.sleep(0.005)
        port.dtr = True
        time.sleep(0.005)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("bin", help="raw bitstream from bitgen -g Binary:Yes")
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--flash", action="store_true", help="write to flash (verified) instead of RAM")
    args = ap.parse_args()

    data = open(args.bin, "rb").read()
    if data.find(b"\xaa\x99\x55\x66") < 0:
        sys.exit("error: no Spartan-6 sync word in file; pass the .bin, not the .bit")

    with serial.Serial(args.port, 115200, timeout=2) as port:
        restart(port)
        port.reset_input_buffer()
        port.write(b"V" if args.flash else b"R")
        expect(port, b"R", 2, "Mojo did not respond; is this the right port?")
        port.write(len(data).to_bytes(4, "little"))
        expect(port, b"O", 10, "Mojo did not acknowledge the transfer size")
        t0 = time.time()
        port.write(data)
        port.flush()
        expect(port, b"D", 10, "Mojo did not acknowledge the transfer")
        print(f"sent {len(data)} bytes in {time.time() - t0:.1f} s")

        if args.flash:
            port.write(b"S")
            expect(port, b"\xaa", 2, "flash does not contain a valid start byte")
            port.timeout = 2
            size = int.from_bytes(port.read(4), "little")
            if size != len(data) + 5:
                sys.exit(f"error: flash size mismatch ({size} != {len(data) + 5})")
            port.timeout = 10
            back = port.read(len(data))
            if back != data:
                bad = next((i for i, (a, b) in enumerate(zip(back, data)) if a != b), len(back))
                sys.exit(f"error: verification failed at byte {bad} of {len(data)}")
            print("flash verified")
            port.write(b"L")
            expect(port, b"D", 5, "could not load the FPGA from flash")

    print("done: FPGA configured" + (" from flash" if args.flash else " (RAM; lost on power cycle)"))


if __name__ == "__main__":
    main()
