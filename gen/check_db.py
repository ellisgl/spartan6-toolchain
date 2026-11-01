#!/usr/bin/env python3
"""Sanity checks on the exporter's JSON before building a chip database.

Checks bel counts against the datasheet, and runs path searches through the
routing graph (pips + nodes) for the connections a Blinky design needs.

    python3 -I check_db.py xc6slx9.json
"""

import collections
import json
import sys

# DS160 Table 1 (XC6SLX9), plus resources visible in TQG144.
EXPECTED_XC6SLX9 = {
    "SLICEX": 715, "SLICEL": 355, "SLICEM": 360,
    "DSP": 16, "BUFGMUX": 16, "DCM": 4, "PLL": 2,
    "BRAM": 64,  # 32 RAMB16BWER, each modelled as two RAMB8BWER-sized bels
}
EXPECTED_TQG144_USER_IO = 102


class Graph:
    def __init__(self, d):
        self.d = d
        self.tt = d["tile_types"]
        self.tile_at = {(t["x"], t["y"]): t for t in d["tiles"]}
        # (x, y, wire) -> node id; wires not in a multi-wire node are their own node.
        self.node_of = {}
        for i, node in enumerate(d["nodes"]):
            for x, y, w in node:
                self.node_of[(x, y, w)] = ("N", i)
        self.fwd = collections.defaultdict(set)
        self.has_uphill = set()
        self.has_downhill = set()
        for (x, y), t in self.tile_at.items():
            for p in self.tt[t["type"]]["pips"]:
                s = self.node((x, y, p["src"]))
                dn = self.node((x, y, p["dst"]))
                self.fwd[s].add(dn)
                self.has_downhill.add(s)
                self.has_uphill.add(dn)

    def node(self, xyw):
        return self.node_of.get(xyw, xyw)

    def bels(self, bel_type=None):
        for (x, y), t in self.tile_at.items():
            for b in self.tt[t["type"]]["bels"]:
                if bel_type is None or b["type"] == bel_type:
                    yield x, y, b

    def pin_node(self, x, y, bel, pin):
        for p in bel["pins"]:
            if p["name"] == pin:
                return self.node((x, y, p["wire"]))
        raise KeyError(f"{bel['name']} has no pin {pin}")

    def reachable(self, src, targets):
        """BFS from src; returns hop count to the first target reached, or None."""
        targets = set(targets)
        seen = {src}
        frontier = [src]
        hops = 0
        while frontier:
            if any(n in targets for n in frontier):
                return hops
            nxt = []
            for n in frontier:
                for m in self.fwd.get(n, ()):
                    if m not in seen:
                        seen.add(m)
                        nxt.append(m)
            frontier = nxt
            hops += 1
        return None


def main():
    d = json.load(open(sys.argv[1]))
    g = Graph(d)
    failures = 0

    def check(ok, msg):
        nonlocal failures
        print(("PASS " if ok else "FAIL ") + msg)
        if not ok:
            failures += 1

    counts = collections.Counter(b["type"] for _, _, b in g.bels())
    if d["device"] == "xc6slx9":
        for k, v in EXPECTED_XC6SLX9.items():
            check(counts[k] == v, f"{k}: {counts[k]} (expected {v})")
        pkg = next(p for p in d["packages"] if p["name"] == "tqg144")
        n_io = sum(1 for p in pkg["pins"] if p.get("io"))
        check(n_io == EXPECTED_TQG144_USER_IO, f"TQG144 user I/O: {n_io} (expected {EXPECTED_TQG144_USER_IO})")

    # Path checks. Bels are not route-throughs, so each hop through a bel
    # (IOB -> BUFIO2 -> BUFGMUX) is checked separately.
    pins = lambda t, pin: [g.pin_node(x, y, b, pin) for x, y, b in g.bels(t) if any(p["name"] == pin for p in b["pins"])]

    slices = sorted(g.bels("SLICEX"), key=lambda e: (e[0], e[1]))
    near, far = slices[0], slices[-1]
    h = g.reachable(g.pin_node(near[0], near[1], near[2], "AQ"), [g.pin_node(far[0], far[1], far[2], "A1")])
    check(h is not None, f"SLICEX X{near[0]}Y{near[1]} AQ -> SLICEX X{far[0]}Y{far[1]} A1 ({h} hops)")

    if d["device"] == "xc6slx9":
        pkg = next(p for p in d["packages"] if p["name"] == "tqg144")
        p56 = next(p for p in pkg["pins"] if p["pin"] == "P56")["io"]
        iob = next(b for x, y, b in g.bels("IOB") if (x, y) == (p56["x"], p56["y"]) and b["name"] == p56["bel"])
        src = g.pin_node(p56["x"], p56["y"], iob, "I")
        h = g.reachable(src, pins("BUFIO2", "I"))
        check(h is not None, f"P56 (Mojo clock) IOB.I -> BUFIO2.I ({h} hops)")
        h = g.reachable(src, pins("BUFGMUX", "I0") + pins("BUFGMUX", "I1"))
        print(f"INFO P56 IOB.I -> BUFGMUX directly: {'yes, ' + str(h) + ' hops' if h is not None else 'no'}")

    bufio_out = pins("BUFIO2", "DIVCLK")
    hits = sum(1 for s in bufio_out if g.reachable(s, pins("BUFGMUX", "I0") + pins("BUFGMUX", "I1")) is not None)
    check(hits > 0, f"BUFIO2.DIVCLK -> BUFGMUX input: {hits}/{len(bufio_out)} BUFIO2s")

    bufg = next(g.bels("BUFGMUX"))
    h = g.reachable(g.pin_node(bufg[0], bufg[1], bufg[2], "O"), [g.pin_node(far[0], far[1], far[2], "CLK")])
    check(h is not None, f"BUFGMUX O -> SLICEX X{far[0]}Y{far[1]} CLK ({h} hops)")

    # Graph hygiene: nodes that can never carry a signal.
    out_pin_nodes = {g.pin_node(x, y, b, p["name"]) for x, y, b in g.bels() for p in b["pins"] if p["dir"] != "IN"}
    in_pin_nodes = {g.pin_node(x, y, b, p["name"]) for x, y, b in g.bels() for p in b["pins"] if p["dir"] != "OUT"}
    undriven_inputs = [n for n in in_pin_nodes if n not in g.has_uphill and n not in out_pin_nodes]
    print(f"INFO bel input nodes with no driver at all: {len(undriven_inputs)} of {len(in_pin_nodes)}")

    print(f"\n{failures} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
