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
    # Slices are split into sub-bels; count each slice once via its A6LUT.
    kinds = {1: "SLICEX", 2: "SLICEL", 3: "SLICEM"}
    for _, _, b in g.bels("SLICE_LUT"):
        if b["name"].endswith(".A6LUT"):
            counts[kinds[b["site_kind"]]] += 1
    if d["device"] == "xc6slx9":
        for k, v in EXPECTED_XC6SLX9.items():
            check(counts[k] == v, f"{k}: {counts[k]} (expected {v})")
        check(counts["CARRY4"] == 715, f"CARRY4: {counts['CARRY4']} (expected 715, one per SLICEL/SLICEM)")
        pkg = next(p for p in d["packages"] if p["name"] == "tqg144")
        n_io = sum(1 for p in pkg["pins"] if p.get("io"))
        check(n_io == EXPECTED_TQG144_USER_IO, f"TQG144 user I/O: {n_io} (expected {EXPECTED_TQG144_USER_IO})")

    def bel_at(x, y, name):
        return next(b for bx, by, b in g.bels() if (bx, by) == (x, y) and b["name"] == name)

    def pin(x, y, name, p):
        return g.pin_node(x, y, bel_at(x, y, name), p)

    def all_pins(bel_type, p, pred=lambda b: True):
        return [g.pin_node(x, y, b, p) for x, y, b in g.bels(bel_type) if pred(b) and any(q["name"] == p for q in b["pins"])]

    def path(msg, src, dsts):
        h = g.reachable(src, dsts if isinstance(dsts, list) else [dsts])
        check(h is not None, f"{msg} ({h} hops)")

    # Logic: pick the lowest and highest SLICEX on the die.
    xs = sorted((x, y) for x, y, b in g.bels("SLICE_LUT") if b["site_kind"] == 1 and b["name"] == "SLICE[1].A6LUT")
    (nx, ny), (fx, fy) = xs[0], xs[-1]
    path("LUT6 O -> own FF D (site mux)", pin(nx, ny, "SLICE[1].A6LUT", "O"), pin(nx, ny, "SLICE[1].AFF", "D"))
    path("LUT5 O -> own FF D (site mux)", pin(nx, ny, "SLICE[1].A5LUT", "O"), pin(nx, ny, "SLICE[1].AFF", "D"))
    path(f"FF Q X{nx}Y{ny} -> LUT A1 X{fx}Y{fy}", pin(nx, ny, "SLICE[1].AFF", "Q"), pin(fx, fy, "SLICE[1].B6LUT", "A1"))
    path("5FF Q -> fabric (via AMUX)", pin(nx, ny, "SLICE[1].A5FF", "Q"), pin(fx, fy, "SLICE[1].B6LUT", "A1"))

    # Carry: CO3 of one slice reaches CI of the slice above through the dedicated wire.
    cx, cy_ = next((x, y) for x, y, b in g.bels("CARRY4") if (x, y + 1) in g.tile_at
                   and any(bb["type"] == "CARRY4" for bb in g.tt[g.tile_at[(x, y + 1)]["type"]]["bels"]))
    above = g.tile_at[(cx, cy_ + 1)]
    check(above.get("carry_prev") == [cx, cy_], f"carry_prev of X{cx}Y{cy_ + 1} is X{cx}Y{cy_}")
    path("CARRY4 CO3 -> CARRY4 CI above", pin(cx, cy_, "SLICE[0].CARRY4", "CO3"), pin(cx, cy_ + 1, "SLICE[0].CARRY4", "CI"))
    check(g.reachable(pin(cx, cy_, "SLICE[0].CARRY4", "O0"), [pin(cx, cy_, "SLICE[0].AFF", "D")]) == 1,
          "CARRY4 O0 -> AFF D in one site pip")

    if d["device"] == "xc6slx9":
        pkg = next(p for p in d["packages"] if p["name"] == "tqg144")
        def io_pin(pkg_pin, p):
            io = next(q for q in pkg["pins"] if q["pin"] == pkg_pin)["io"]
            return pin(io["x"], io["y"], io["bel"], p)
        bufg_in = all_pins("BUFGMUX", "I0") + all_pins("BUFGMUX", "I1")
        path("P56 (Mojo clock) GCLK_I -> BUFGMUX", io_pin("P56", "GCLK_I"), bufg_in)
        path("P56 I (data path) -> LUT A1", io_pin("P56", "I"), pin(fx, fy, "SLICE[1].B6LUT", "A1"))
        path("FF Q -> P134 (Mojo LED0) O", pin(fx, fy, "SLICE[1].AFF", "Q"), io_pin("P134", "O"))
        path("FF Q -> P134 T", pin(fx, fy, "SLICE[1].AFF", "Q"), io_pin("P134", "T"))

    bufg = next(g.bels("BUFGMUX"))
    path("BUFGMUX O -> FF CK", g.pin_node(bufg[0], bufg[1], bufg[2], "O"), pin(fx, fy, "SLICE[1].AFF", "CK"))

    # Constants: LUT inputs must be tieable to VCC (needed when a LUT6 is split into two LUT5s).
    vcc = {n for (x, y), t in g.tile_at.items() for i, w in enumerate(g.tt[t["type"]]["wires"])
           if w.get("const_value") == "VCC" for n in [g.node((x, y, i))]}
    a6 = pin(fx, fy, "SLICE[1].A6LUT", "A6")
    check(any(g.reachable(v, [a6]) is not None for v in list(vcc)[:200]), "a VCC wire reaches LUT A6")

    # Graph hygiene: pins that can never carry a signal.
    in_nodes = collections.defaultdict(list)
    out_nodes = collections.defaultdict(list)
    for x, y, b in g.bels():
        for p in b["pins"]:
            n = g.pin_node(x, y, b, p["name"])
            (in_nodes if p["dir"] != "OUT" else out_nodes)[n].append((b["type"], p["name"]))
    undriven = collections.Counter(t for n, v in in_nodes.items()
                                   if n not in g.has_uphill and n not in out_nodes for t in v)
    # Only the first CARRY4 of each chain has nothing feeding CI.
    check(set(undriven) <= {("CARRY4", "CI")}, f"undriven bel inputs are only chain-bottom CARRY4.CI: {dict(undriven)}")
    unloaded = collections.Counter(t for n, v in out_nodes.items()
                                   if n not in g.has_downhill and n not in in_nodes for t in v)
    # Clock-pad and feedback outputs only connect on clock-capable pads; PLL TEST_CLKIN only fed test wiring.
    clock_only = {("IOB", "GCLK_I"), ("ILOGIC", "DFB"), ("ILOGIC", "CFB0"), ("ILOGIC", "CFB1"),
                  ("IODELAY", "DQSOUTP"), ("IODELAY", "DQSOUTN"),
                  ("PLL", "TEST_CLKIN")}  # only fed factory-test wiring, which the exporter drops
    check(set(unloaded) <= clock_only, f"unloaded bel outputs are only clock-pad paths: {dict(unloaded)}")

    print(f"\n{failures} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
