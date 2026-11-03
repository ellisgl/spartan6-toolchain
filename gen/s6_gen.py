#!/usr/bin/env python3
"""Build a nextpnr himbaechel chip database (.bba) for Spartan-6.

Input is the JSON written by the Rust exporter (exporter/, `s6-export`).
Uses nextpnr's own himbaechel_dbgen library, so the .bba layout always
matches the nextpnr checkout it is built against.

    python3 s6_gen.py --nextpnr /path/to/nextpnr --json xc6slx9.json --bba xc6slx9.bba
"""

import argparse
import json
import sys
from os import path

UARCH = "spartan6"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--nextpnr", required=True, help="nextpnr source tree (for himbaechel_dbgen)")
    ap.add_argument("--json", required=True, help="exporter output")
    ap.add_argument("--bba", required=True, help="output .bba path")
    ap.add_argument("--constids", help="uarch constids.inc to preload, if any")
    ap.add_argument("--timing", help="delay model from s6-timing (data/timing-xc6slx9-2.json)")
    args = ap.parse_args()

    sys.path.insert(0, path.join(args.nextpnr, "himbaechel"))
    from himbaechel_dbgen.chip import BEL_FLAG_GLOBAL, Chip, ClockEdge, NodeWire, PinType, TimingValue

    with open(args.json) as f:
        dev = json.load(f)

    ch = Chip(UARCH, dev["device"], dev["width"], dev["height"])
    if args.constids:
        ch.strs.read_constids(args.constids)
    ch.set_speed_grades(dev["speed_grades"])

    pin_dir = {"IN": PinType.INPUT, "OUT": PinType.OUTPUT, "INOUT": PinType.INOUT}
    for tt_json in dev["tile_types"]:
        tt = ch.create_tile_type(tt_json["name"])
        names = [w["name"] for w in tt_json["wires"]]
        for w in tt_json["wires"]:
            tt.create_wire(w["name"], w["kind"], w.get("const_value", ""))
        for p in tt_json["pips"]:
            # Delay class: the class of the driven wire, or the slice mux.
            pip = tt.create_pip(names[p["src"]], names[p["dst"]], p["timing_class"])
            # Flag 1: the C LUT's DI1 from DX (MUX_CDI1 = ALT, not the MC31
            # cascade). ISE allows it only for RAM (RAM128X1S), not SRLs; the
            # uarch checks.
            ref = p.get("src_ref", [])
            if p["kind"] == "SITE" and ref[2:4] == ["MUX_CDI1", "ALT"] and "MC31" not in names[p["src"]]:
                pip.flags = 1
            # Flag 2: a clock input mux (IMUX_CLK) fed from general routing,
            # not a tile clock (HCLK). The uarch keeps global clock nets off
            # these, so their clock pins are reached through the clock
            # network (router2 otherwise sometimes finishes through the
            # fabric, adding ~1 ns of skew).
            if names[p["dst"]].startswith("IMUX_CLK") and not names[p["src"]].startswith("HCLK"):
                pip.flags |= 2
        for b in tt_json["bels"]:
            bel = tt.create_bel(b["name"], b["type"], b["z"])
            # Slice sub-bels: 1 = SLICEX, 2 = SLICEL, 3 = SLICEM (read by the uarch).
            bel.site = b.get("site_kind", 0)
            for pin in b["pins"]:
                tt.add_bel_pin(bel, pin["name"], names[pin["wire"]], pin_dir[pin["dir"]])

    # Global constant drivers. router2 routes constant nets back to any wire
    # whose const_value matches (TIE_0/TIE_1 and the slices' CONST_* wires),
    # so these bels only give the GND/VCC nets a driver to be placed on.
    corner = dev["tiles"][0]
    assert (corner["x"], corner["y"]) == (0, 0)
    assert sum(1 for t in dev["tiles"] if t["type"] == corner["type"]) == 1, \
        "corner tile type is shared; pseudo constant bels would be duplicated"
    tt = ch.tile_types[corner["type"]]
    for i, (name, value) in enumerate([("PSEUDO_GND", "GND"), ("PSEUDO_VCC", "VCC")]):
        tt.create_wire(f"{name}.Y", "CONST", const_value=value)
        bel = tt.create_bel(name, name, z=0x1F0 + i)
        bel.flags |= BEL_FLAG_GLOBAL
        tt.add_bel_pin(bel, "Y", f"{name}.Y", PinType.OUTPUT)

    # Tile names are "X{x}Y{y}": himbaechel appends that to name_prefix, which stays empty.
    for t in dev["tiles"]:
        ch.set_tile_type(t["x"], t["y"], dev["tile_types"][t["type"]]["name"])

    for node in dev["nodes"]:
        ch.add_node([NodeWire(x, y, w) for x, y, w in node])

    for pkg_json in dev["packages"]:
        pkg = ch.create_package(pkg_json["name"].upper())
        for pin in pkg_json["pins"]:
            io = pin.get("io")
            if io is None:
                continue
            tile = f"X{io['x']}Y{io['y']}"
            pkg.create_pad(pin["pin"], tile, io["bel"], "", io["bank"])

    if args.timing:
        sys.path.insert(0, path.dirname(path.abspath(__file__)))
        import s6_timing

        s6_timing.add_timing(ch, dev, args.timing, TimingValue, ClockEdge)

    ch.write_bba(args.bba)
    n_pips = sum(len(t["pips"]) for t in dev["tile_types"])
    print(f"{dev['device']}: {len(dev['tile_types'])} tile types, {n_pips} pips per type set, "
          f"{len(dev['nodes'])} nodes -> {args.bba}")


if __name__ == "__main__":
    main()
