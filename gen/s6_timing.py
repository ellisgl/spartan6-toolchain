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
    classes = set()
    for tt in dev["tile_types"]:
        for p in tt["pips"]:
            classes.add(p["timing_class"])
    cd = CellData(data)
    unmeasured = []
    for grade in dev["speed_grades"]:
        for cls in sorted(classes):
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

    def clk_out(cell, clk, out, v):
        if v is not None:
            cell.add_clock_out(clk, out, R, TimingValue(ps(v)))

    def setup_hold(cell, clk, pin, sh):
        if sh is not None:
            cell.add_setup_hold(clk, pin, R, TimingValue(ps(sh[0])), TimingValue(ps(sh[1])))

    # LUTs: A1..A6 -> O (ISE ADR0..ADR5).
    lut = ch.timing.add_cell_variant(grade, "SLICE_LUT")
    for i in range(6):
        comb(lut, f"A{i + 1}", "O", cd.delay("X_LUT6", f"ADR{i}", "O"))
    lut5 = ch.timing.add_cell_variant(grade, "SLICE_LUT5")
    for i in range(5):
        comb(lut5, f"A{i + 1}", "O", cd.delay("X_LUT5", f"ADR{i}", "O"))

    # Flip-flops: CK -> Q, setup/hold of D, CE, SR.
    for ty in ("SLICE_FF", "SLICE_FF5"):
        ff = ch.timing.add_cell_variant(grade, ty)
        clk_out(ff, "CK", "Q", cd.delay("X_SFF", "CLK", "O") or cd.delay("X_FF", "CLK", "O"))
        setup_hold(ff, "CK", "D", cd.check("X_SFF", "I", "CLK"))
        setup_hold(ff, "CK", "CE", cd.check("X_SFF", "CE", "CLK"))
        setup_hold(ff, "CK", "SR", cd.check("X_SFF", "SRST|SSET", "CLK"))

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
    lr = ch.timing.add_cell_variant(grade, "SLICE_LUTRAM")
    rd = cd.delay("X_RAMD64_ADV", r"RADR\d", "O") or cd.delay("X_SRLC32E", r"A\[\d\]", "Q")
    for i in range(6):
        comb(lr, f"A{i + 1}", "O", rd)
        comb(lr, f"A{i + 1}", "O5", rd)
    clk_out(lr, "CLK", "O", max(filter(None, [cd.delay("X_RAMD64_ADV", "CLK", "O"), cd.delay("X_SRLC32E", "CLK", "Q")]), default=None))
    for pin in ("DI1", "DI2"):
        setup_hold(lr, "CLK", pin, cd.check("X_RAMD64_ADV", "I", "CLK"))
    for i in range(6):
        setup_hold(lr, "CLK", f"WA{i + 1}", cd.check("X_RAMD64_ADV", f"WADR{i}", "CLK"))
    setup_hold(lr, "CLK", "WE", cd.check("X_SRLC32E", "CE", "CLK"))

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

    # DSP48A1: three variants by pipeline registers (the uarch picks one per
    # cell): DSP_P (PREG: P registered), DSP_M (MREG), DSP_IN (only input
    # registers; C -> P is then combinational).
    sh = cd.check("X_DSP48A1:P", r"[ABCD]\[.*\]|OPMODE.*", "CLK") or cd.check("X_DSP48A1", r"[ABCD]\[.*\]|OPMODE.*", "CLK")
    for variant, cfg in (("DSP_P", "P"), ("DSP_M", "M"), ("DSP_IN", "IN")):
        dsp = ch.timing.add_cell_variant(grade, variant)
        co = cd.delay(f"X_DSP48A1:{cfg}", "CLK", r"P\[.*\]|M\[.*\]")
        for i in range(48):
            clk_out(dsp, "CLK", f"P{i}", co)
        for i in range(36):
            clk_out(dsp, "CLK", f"M{i}", co)
        comb_c = cfg != "P" and cd.delay("X_DSP48A1:IN", r"C\[.*\]", r"P\[.*\]")
        for bus, n in (("A", 18), ("B", 18), ("C", 48), ("D", 18), ("OPMODE", 8)):
            for i in range(n):
                if bus == "C" and comb_c:
                    for j in range(48):
                        comb(dsp, f"C{i}", f"P{j}", comb_c)
                else:
                    setup_hold(dsp, "CLK", f"{bus}{i}", sh)

    # Global clock buffer.
    bufg = ch.timing.add_cell_variant(grade, "BUFGMUX")
    comb(bufg, "I0", "O", cd.delay("X_CKBUF", "I", "O"))
    comb(bufg, "I1", "O", cd.delay("X_CKBUF", "I", "O"))
