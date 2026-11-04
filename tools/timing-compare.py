#!/usr/bin/env python3
"""Compare nextpnr's timing estimate with ISE's timing analyser (trce) on the
same placement and routing.

    tools/timing-compare.py alu rom fill ...
    tools/timing-compare.py --build build/xc6slx9-tqg144-3 alu ...   # another speed / device

Each design needs build/<name>/design_routed.ncd (make ise-check TEST=<name>)
and build/<name>/nextpnr.log. Prints, per design, nextpnr's worst
clock-to-clock period and ISE's.
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def nextpnr_period(log):
    """Worst register-to-register period (ns) from nextpnr's final report
    (the last value reported for each clock; the slowest clock wins)."""
    last = {}
    with open(log) as f:
        for line in f:
            m = re.search(r"Max frequency for clock\s+'(.*)': ([0-9.]+) MHz", line)
            if m:
                last[m.group(1)] = float(m.group(2))
    return 1000.0 / min(last.values()) if last else None


def ise_period(build):
    """Worst clock-to-setup as a clock period (ns) from trce's
    unconstrained data sheet."""
    twr = os.path.join(build, "trce_u.twr")
    subprocess.run(
        [os.path.join(ROOT, "tools/ise.sh"), "trce", "-v", "1", "-u", "1", "design_routed.ncd", "-o", "trce_u.twr"],
        cwd=build, env={**os.environ, "ISE_MOUNT": os.path.abspath(build)},
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True,
    )
    worst = None
    in_table = False
    with open(twr) as f:
        for line in f:
            if line.startswith("Clock to Setup on destination clock"):
                in_table = True
                continue
            if in_table:
                if not line.strip():
                    in_table = False
                    continue
                # Columns: Src:Rise/Dest:Rise, Fall/Rise, Rise/Fall, Fall/Fall.
                # trce gives raw path delays; a path between opposite edges
                # has half a period, so it needs twice that as a period.
                cols = [c.strip() for c in line.split("|")]
                for k, scale in ((1, 1.0), (2, 2.0), (3, 2.0), (4, 1.0)):
                    if len(cols) > k and re.fullmatch(r"[0-9.]+", cols[k]):
                        worst = max(worst or 0.0, scale * float(cols[k]))
    return worst


def main():
    args = sys.argv[1:]
    base = os.path.join(ROOT, "build")
    if args[:1] == ["--build"]:
        base, args = os.path.abspath(args[1]), args[2:]
    print(f"{'design':12} {'nextpnr':>10} {'ISE trce':>10} {'error':>8}")
    for name in args:
        build = os.path.join(base, name)
        n = nextpnr_period(os.path.join(build, "nextpnr.log"))
        i = ise_period(build)
        if n is None or i is None:
            print(f"{name:12} {'-' if n is None else f'{n:.2f} ns':>10} {'-' if i is None else f'{i:.2f} ns':>10}")
            continue
        print(f"{name:12} {n:7.2f} ns {i:7.2f} ns {100 * (n - i) / i:+7.1f}%")


if __name__ == "__main__":
    main()
