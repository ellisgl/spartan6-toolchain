#!/usr/bin/env python3
"""Compare nextpnr's timing estimate with ISE's timing analyser (trce) on the
same placement and routing.

    tools/timing-compare.py alu rom fill ...

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
    """Worst clock-to-setup (ns) from trce's unconstrained data sheet."""
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
                cols = [c.strip() for c in line.split("|")]
                if len(cols) > 1 and re.fullmatch(r"[0-9.]+", cols[1]):
                    worst = max(worst or 0.0, float(cols[1]))
    return worst


def main():
    print(f"{'design':12} {'nextpnr':>10} {'ISE trce':>10} {'error':>8}")
    for name in sys.argv[1:]:
        build = os.path.join(ROOT, "build", name)
        n = nextpnr_period(os.path.join(build, "nextpnr.log"))
        i = ise_period(build)
        if n is None or i is None:
            print(f"{name:12} {'-' if n is None else f'{n:.2f} ns':>10} {'-' if i is None else f'{i:.2f} ns':>10}")
            continue
        print(f"{name:12} {n:7.2f} ns {i:7.2f} ns {100 * (n - i) / i:+7.1f}%")


if __name__ == "__main__":
    main()
