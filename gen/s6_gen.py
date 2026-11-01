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
    args = ap.parse_args()

    sys.path.insert(0, path.join(args.nextpnr, "himbaechel"))
    from himbaechel_dbgen.chip import Chip, NodeWire, PinType

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
            # Timing class = item kind for now; real delays are a separate project.
            tt.create_pip(names[p["src"]], names[p["dst"]], p["kind"])
        for b in tt_json["bels"]:
            bel = tt.create_bel(b["name"], b["type"], b["z"])
            for pin in b["pins"]:
                tt.add_bel_pin(bel, pin["name"], names[pin["wire"]], pin_dir[pin["dir"]])

    for t in dev["tiles"]:
        inst = ch.set_tile_type(t["x"], t["y"], dev["tile_types"][t["type"]]["name"])
        inst.name_prefix = ch.strs.id(t["name"])

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

    ch.write_bba(args.bba)
    n_pips = sum(len(t["pips"]) for t in dev["tile_types"])
    print(f"{dev['device']}: {len(dev['tile_types'])} tile types, {n_pips} pips per type set, "
          f"{len(dev['nodes'])} nodes -> {args.bba}")


if __name__ == "__main__":
    main()
