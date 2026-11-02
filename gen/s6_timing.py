"""Delay model for the Spartan-6 chip database.

Reads the delays fitted by `s6-timing` (data/timing-xc6slx9-2.json: per pip
class, from ISE's timing simulation SDF for our test designs) and the cell
delays ISE reports, and adds them to a himbaechel `Chip`:

- pip classes: one delay each (the fit is per class of driven wire);
- cell types: LUT, FF, carry, wide mux, RAM/SRL, BRAM, DSP and BUFGMUX arcs,
  mapped from ISE's simulation primitives (X_LUT6, X_SFF, ...) to our cell
  types and pin names.

Only the -2 speed grade was measured; the other grades get the same values
(pessimistic for -3).
"""

import json
import re

# Unmeasured routing (clock spines, I/O, ...): a typical routing hop.
DEFAULT_ROUTING_PS = 250
MIN_SPLIT_SAMPLES = 10


def ps(v):
    return max(0, int(round(v)))


def load(path):
    with open(path) as f:
        return json.load(f)


class CellData:
    """ISE's cell delays and checks, looked up by primitive and pin regex."""

    def __init__(self, data):
        self.arcs = data["cell_delays"]
        self.checks = data["cell_checks"]

    def delay(self, celltype, frm, to):
        vals = [a["delay_ps"] for a in self.arcs
                if a["celltype"] == celltype and re.fullmatch(frm, a["from"]) and re.fullmatch(to, a["to"])]
        return max(vals) if vals else None

    def check(self, celltype, data, clock):
        vals = [(c["setup_ps"], c["hold_ps"]) for c in self.checks
                if c["celltype"] == celltype and re.fullmatch(data, c["data"]) and re.fullmatch(clock, c["clock"])]
        if not vals:
            return None
        return max(v[0] for v in vals), max(v[1] for v in vals)


def add_timing(ch, dev, timing_path, TimingValue, ClockEdge):
    data = load(timing_path)
    fitted = {k: v["delay_ps"] for k, v in data["pip_classes"].items()}
    # Interface-tile input muxes (block RAM / DSP columns, "<class>@INTF")
    # with too few samples to trust take their plain class's value.
    for k, v in data["pip_classes"].items():
        base = k.split("@")[0]
        if "@" in k and v.get("samples", 0) < MIN_SPLIT_SAMPLES and base in fitted:
            fitted[k] = fitted[base]
    classes = set()
    for tt in dev["tile_types"]:
        for p in tt["pips"]:
            classes.add(p["timing_class"])
    cd = CellData(data)
    unmeasured = []
    for grade in dev["speed_grades"]:
        for cls in sorted(classes):
            if cls not in fitted and "@" in cls and cls.split("@")[0] in fitted:
                fitted[cls] = fitted[cls.split("@")[0]]
            if cls in fitted:
                d = fitted[cls]
            elif cls.startswith("SITE:") or cls in ("BELOUT", "PERMABUF"):
                d = 0
            else:
                d = DEFAULT_ROUTING_PS
                if grade == dev["speed_grades"][0]:
                    unmeasured.append(cls)
            ch.timing.set_pip_class(grade, cls, TimingValue(ps(d)))
        add_cells(ch, grade, cd, TimingValue, ClockEdge)
    print(f"timing: {len(classes)} pip classes ({len(classes) - len(unmeasured)} measured or zero), "
          f"{len(dev['speed_grades'])} speed grades from {timing_path}")


def add_cells(ch, grade, cd, TimingValue, ClockEdge):
    R = ClockEdge.RISING

    def comb(cell, frm, to, v):
        if v is not None:
            cell.add_comb_arc(frm, to, TimingValue(ps(v)))

    def clk_out(cell, clk, out, v, edge=R):
        if v is not None:
            cell.add_clock_out(clk, out, edge, TimingValue(ps(v)))

    def setup_hold(cell, clk, pin, sh, edge=R):
        if sh is not None:
            cell.add_setup_hold(clk, pin, edge, TimingValue(ps(sh[0])), TimingValue(ps(sh[1])))

    # LUTs: A1..A6 -> O (ISE ADR0..ADR5).
    lut = ch.timing.add_cell_variant(grade, "SLICE_LUT")
    for i in range(6):
        comb(lut, f"A{i + 1}", "O", cd.delay("X_LUT6", f"ADR{i}", "O"))
    lut5 = ch.timing.add_cell_variant(grade, "SLICE_LUT5")
    for i in range(5):
        comb(lut5, f"A{i + 1}", "O", cd.delay("X_LUT5", f"ADR{i}", "O"))

    # Flip-flops: CK -> Q, setup/hold of D, CE, SR. "_N" variants: the
    # slice's clock inverted (CLKINV), same numbers on the falling edge; the
    # uarch picks them for cells with CLK_INV.
    for ty in ("SLICE_FF", "SLICE_FF5"):
        for sfx, edge in (("", R), ("_N", ClockEdge.FALLING)):
            ff = ch.timing.add_cell_variant(grade, ty + sfx)
            clk_out(ff, "CK", "Q", cd.delay("X_SFF", "CLK", "O") or cd.delay("X_FF", "CLK", "O"), edge)
            setup_hold(ff, "CK", "D", cd.check("X_SFF", "I", "CLK"), edge)
            setup_hold(ff, "CK", "CE", cd.check("X_SFF", "CE", "CLK"), edge)
            setup_hold(ff, "CK", "SR", cd.check("X_SFF", "SRST|SSET", "CLK"), edge)

    # Carry chain: our CI is ISE's CI or CYINIT (one CYINIT mux in front).
    carry = ch.timing.add_cell_variant(grade, "CARRY4")
    for i in range(4):
        for out in ("O", "CO"):
            comb(carry, "CI", f"{out}{i}", cd.delay("X_CARRY4", "CI|CYINIT", fr"{out}\[{i}\]"))
            for j in range(i + 1):
                comb(carry, f"S{j}", f"{out}{i}", cd.delay("X_CARRY4", fr"S\[{j}\]", fr"{out}\[{i}\]"))
                comb(carry, f"DI{j}", f"{out}{i}", cd.delay("X_CARRY4", fr"DI\[{j}\]", fr"{out}\[{i}\]"))

    # Wide muxes (ISE X_MUX2: IA/IB data, SEL select).
    for ty in ("SLICE_F7MUX", "SLICE_F8MUX"):
        mux = ch.timing.add_cell_variant(grade, ty)
        comb(mux, "I0", "O", cd.delay("X_MUX2", "IA|IB", "O"))
        comb(mux, "I1", "O", cd.delay("X_MUX2", "IA|IB", "O"))
        comb(mux, "S", "O", cd.delay("X_MUX2", "SEL", "O"))

    # Distributed RAM and shift registers: reads are combinational from the
    # address, writes/shifts clocked.
    for sfx, edge in (("", R), ("_N", ClockEdge.FALLING)):
        lr = ch.timing.add_cell_variant(grade, "SLICE_LUTRAM" + sfx)
        rd = cd.delay("X_RAMD64_ADV", r"RADR\d", "O") or cd.delay("X_SRLC32E", r"A\[\d\]", "Q")
        for i in range(6):
            comb(lr, f"A{i + 1}", "O", rd)
            comb(lr, f"A{i + 1}", "O5", rd)
        clk_out(lr, "CLK", "O", max(filter(None, [cd.delay("X_RAMD64_ADV", "CLK", "O"), cd.delay("X_SRLC32E", "CLK", "Q")]), default=None), edge)
        for pin in ("DI1", "DI2"):
            setup_hold(lr, "CLK", pin, cd.check("X_RAMD64_ADV", "I", "CLK"), edge)
        for i in range(6):
            setup_hold(lr, "CLK", f"WA{i + 1}", cd.check("X_RAMD64_ADV", f"WADR{i}", "CLK"), edge)
        setup_hold(lr, "CLK", "WE", cd.check("X_SRLC32E", "CE", "CLK"), edge)

    # Block RAM (both halves use the same numbers): clock-to-out, setup/hold.
    bram = ch.timing.add_cell_variant(grade, "BRAM")
    for ab in "AB":
        co = cd.delay("X_RAMB16BWER", f"CLK{ab}", fr"DO{ab}\[\d+\]|DOP{ab}\[\d+\]")
        for i in range(16):
            clk_out(bram, f"CLK{ab}", f"DO{ab}{i}", co)
        for i in range(2):
            clk_out(bram, f"CLK{ab}", f"DOP{ab}{i}", co)
        sh = cd.check("X_RAMB16BWER", fr"(ADDR|DI|DIP|WE){ab}\[\d+\]|(EN|REGCE|RST){ab}", f"CLK{ab}")
        for i in range(13):
            setup_hold(bram, f"CLK{ab}", f"ADDR{ab}{i}", sh)
        for i in range(16):
            setup_hold(bram, f"CLK{ab}", f"DI{ab}{i}", sh)
        for i in range(2):
            setup_hold(bram, f"CLK{ab}", f"DIP{ab}{i}", sh)
            setup_hold(bram, f"CLK{ab}", f"WE{ab}{i}", sh)
        for pin in ("EN", "REGCE", "RST"):
            setup_hold(bram, f"CLK{ab}", f"{pin}{ab}", sh)

    # DSP48A1: one variant per pipeline-register configuration, named
    # DSP_<groups> with the registered groups in the order A B C D M P
    # O(PMODE) ("DSP_ACP"; "DSP_NONE" when none), as measured by
    # tests/dsp_char*. With OPMODE tied to a constant that was measured, the
    # variant is DSP_<groups>_<hex> (ISE drops paths that mode can't use);
    # other modes use the plain variant, measured with OPMODE from logic. ISE times whole buses (A[17:0] -> P[47:0]), so each
    # arc is stored once, on bit 0 of the buses (A0 -> P0); the uarch maps
    # every bus pin onto bit 0 when nextpnr asks. M isn't measured (ISE
    # doesn't allow M and P together) and is looked up as P.
    def dsp_pin(name):
        m = re.fullmatch(r"(\w+?)\[\d+(:\d+)?\]", name)
        return f"{m.group(1)}0" if m else name

    dsp_cfgs = sorted({a["celltype"].split(":", 1)[1] for a in cd.arcs + cd.checks
                       if a["celltype"].startswith("X_DSP48A1:")})
    for cfg in dsp_cfgs:
        regs, sep, op = cfg.partition("_")
        dsp = ch.timing.add_cell_variant(grade, f"DSP_{regs or 'NONE'}{sep}{op}")
        key = f"X_DSP48A1:{cfg}"
        for a in cd.arcs:
            if a["celltype"] != key:
                continue
            if a["from"] == "CLK":
                clk_out(dsp, "CLK", dsp_pin(a["to"]), a["delay_ps"])
            else:
                comb(dsp, dsp_pin(a["from"]), dsp_pin(a["to"]), a["delay_ps"])
        for c in cd.checks:
            if c["celltype"] == key and c["clock"] == "CLK":
                setup_hold(dsp, "CLK", dsp_pin(c["data"]), (c["setup_ps"], c["hold_ps"]))

    # Global clock buffer.
    bufg = ch.timing.add_cell_variant(grade, "BUFGMUX")
    comb(bufg, "I0", "O", cd.delay("X_CKBUF", "I", "O"))
    comb(bufg, "I1", "O", cd.delay("X_CKBUF", "I", "O"))
