#!/usr/bin/env python3
"""On-chip speed test: sweep tests/speed's clock and find where its timed
path starts failing on the board, next to nextpnr's and ISE's predictions.

    make bit TEST=speed            # build once (placement and routing fixed)
    tools/speed-test.py [--port /dev/ttyACM0] [--max 375]

For each frequency the PLL's CLKFBOUT_MULT / CLKOUT0_DIVIDE are rewritten in
a copy of build/speed/routed.json and s6-bitgen makes the bitstream, so every
point tests the same placement and routing. Each bitstream is loaded into the
FPGA's RAM (about 13 s each).

The predictions are worst case (85 C, 1.14 V, -2): a board at room
temperature should pass well above them.
"""

import argparse
import glob
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import time

import serial

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD = os.path.join(ROOT, "build")
sys.path.insert(0, os.path.join(ROOT, "tools"))


def candidates(fmin, fmax):
    """{freq MHz: (mult, divide)} for 50 MHz * M / D, VCO 400..1000 MHz."""
    out = {}
    for m in range(8, 21):
        for d in range(1, 33):
            f = 50.0 * m / d
            if fmin <= f <= fmax:
                key = round(f, 3)
                out.setdefault(key, (m, d))
    return dict(sorted(out.items()))


def bitstream(base, mult, div, path):
    """s6-bitgen on a copy of the routed design with new PLL settings."""
    d = json.load(open(base))
    mod = next(iter(d["modules"].values()))
    pll = [c for c in mod["cells"].values() if c["type"] == "PLL"]
    assert len(pll) == 1, "expected one PLL"
    p = pll[0]["parameters"]
    p["CLKFBOUT_MULT"] = format(mult, "032b")
    p["CLKOUT0_DIVIDE"] = format(div, "032b")
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(d, f)
        tmp = f.name
    try:
        subprocess.run(
            [os.path.join(BUILD, "target/release/s6-bitgen"), "--db", glob.glob(os.path.join(BUILD, "spartan6-*.zstd"))[0],
             "gen", "--routed", tmp, "--chipdb-json", os.path.join(BUILD, "xc6slx9.json"), "--out", path],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        os.unlink(tmp)


def load(path, port):
    subprocess.run([os.path.join(ROOT, "build/target/release/s6-load"), "--port", port, path],
                   check=True, stdout=subprocess.DEVNULL)


def measure(port, runs):
    """[(cycles, errors, locked)] for `runs` test windows."""
    res = []
    with serial.Serial(port, 115200, timeout=2) as s:
        time.sleep(0.5)
        s.reset_input_buffer()
        for _ in range(runs):
            s.write(b"T")
            got = s.read(9)
            if len(got) != 9:
                raise RuntimeError(f"short reply {got.hex()}")
            cycles, errors = struct.unpack("<II", got[:8])
            res.append((cycles, errors, got[8] & 1))
    return res


def predictions():
    out = {}
    log = os.path.join(BUILD, "speed/nextpnr.log")
    last = {}
    for line in open(log):
        m = re.search(r"Max frequency for clock\s+'(.*)': ([0-9.]+) MHz", line)
        if m:
            last[m.group(1)] = float(m.group(2))
    if "tclk" in last:
        out["nextpnr"] = last["tclk"]
    try:
        import importlib.util

        spec = importlib.util.spec_from_file_location("tc", os.path.join(ROOT, "tools/timing-compare.py"))
        tc = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(tc)
        if os.path.exists(os.path.join(BUILD, "speed/design_routed.ncd")):
            out["ISE trce"] = 1000.0 / tc.ise_period(os.path.join(BUILD, "speed"))
    except Exception as e:  # ISE is optional here
        print(f"(no ISE prediction: {e})")
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", default="/dev/ttyACM0")
    ap.add_argument("--min", type=float, default=80)
    ap.add_argument("--max", type=float, default=375, help="upper limit (MHz); BUFG tops out around here")
    ap.add_argument("--step", type=float, default=0.08, help="coarse step (fraction)")
    ap.add_argument("--runs", type=int, default=3, help="test windows per frequency")
    args = ap.parse_args()

    base = os.path.join(BUILD, "speed/routed.json")
    pred = predictions()
    for k, v in pred.items():
        print(f"predicted Fmax ({k}, worst case): {v:.1f} MHz")
    cands = candidates(args.min, args.max)
    freqs = list(cands)

    results = {}

    def test(f):
        m, d = cands[f]
        with tempfile.TemporaryDirectory() as td:
            path = os.path.join(td, "speed.bin")
            bitstream(base, m, d, path)
            load(path, args.port)
        r = measure(args.port, args.runs)
        meas = sum(c for c, _, _ in r) / len(r) / (1 << 22) * 50.0
        errs = sum(e for _, e, _ in r)
        locked = all(lk for _, _, lk in r)
        ok = errs == 0 and abs(meas - f) / f < 0.01
        results[f] = (meas, errs, locked, ok)
        print(f"  {f:7.2f} MHz (x{m}/{d}): measured {meas:7.2f} MHz, errors {errs:9d}, "
              f"{'locked' if locked else 'NOT locked'} -> {'pass' if ok else 'FAIL'}", flush=True)
        return ok

    # Coarse sweep up until two failures in a row.
    last_pass, first_fail, fails = None, None, 0
    f_next = freqs[0]
    for f in freqs:
        if f < f_next:
            continue
        if test(f):
            last_pass, fails = f, 0
        else:
            first_fail = first_fail or f
            fails += 1
            if fails >= 2:
                break
        f_next = f * (1 + args.step)
    # Refine between the last pass and the first failure.
    if last_pass is not None and first_fail is not None:
        lo, hi = last_pass, first_fail
        between = [f for f in freqs if lo < f < hi]
        while between:
            f = between[len(between) // 2]
            if test(f):
                lo = f
                between = [x for x in between if x > f]
            else:
                hi = f
                between = [x for x in between if x < f]
        print(f"\nhighest passing: {lo:.2f} MHz, lowest failing: {hi:.2f} MHz")
        for k, v in pred.items():
            print(f"  vs {k} {v:.1f} MHz: board passes up to {lo / v:.2f}x the worst-case prediction")
    elif first_fail is None:
        print(f"\nno failures up to {freqs[-1]:.2f} MHz (raise --max, or the path is too short)")
    else:
        print("\nfails at the lowest frequency tried (lower --min)")


if __name__ == "__main__":
    main()
