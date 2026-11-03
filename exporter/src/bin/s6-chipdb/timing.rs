//! Delay model for the chip database (was gen/s6_timing.py).
//!
//! Reads the delays fitted by `s6-timing` (data/timing-xc6slx9-2.json: per
//! pip class, from ISE's timing simulation SDF for our test designs) and the
//! cell delays ISE reports, and adds them to the chip:
//!
//! - pip classes: one delay each (the fit is per class of driven wire);
//! - cell types: LUT, FF, carry, wide mux, RAM/SRL, BRAM, DSP and BUFGMUX
//!   arcs, mapped from ISE's simulation primitives (X_LUT6, X_SFF, ...) to
//!   our cell types and pin names.
//!
//! Only the -2 speed grade was measured; the other grades get the same values
//! (pessimistic for -3).

use std::collections::{BTreeSet, HashMap};

use regex::Regex;
use serde::Deserialize;

use crate::dbgen::{Chip, ClockEdge};

/// Unmeasured routing (clock spines, I/O, ...): a typical routing hop.
const DEFAULT_ROUTING_PS: f64 = 250.0;
/// Interface-tile classes ("<class>@INTF") with fewer samples than this take
/// their plain class's value.
const MIN_SPLIT_SAMPLES: u64 = 10;

#[derive(Deserialize)]
struct PipClass {
    delay_ps: f64,
    #[serde(default)]
    samples: u64,
}

#[derive(Deserialize)]
struct Arc {
    celltype: String,
    from: String,
    to: String,
    delay_ps: f64,
}

#[derive(Deserialize)]
struct Check {
    celltype: String,
    data: String,
    clock: String,
    setup_ps: f64,
    hold_ps: f64,
}

#[derive(Deserialize)]
struct Data {
    pip_classes: HashMap<String, PipClass>,
    cell_delays: Vec<Arc>,
    cell_checks: Vec<Check>,
}

/// Picoseconds, rounded as Python's round() does (half to even).
fn ps(v: f64) -> i64 {
    (v.round_ties_even() as i64).max(0)
}

fn full(re: &str) -> Regex {
    Regex::new(&format!("^(?:{re})$")).expect("regex")
}

/// Python's `a or b` for an optional delay (0 counts as missing).
fn or(a: Option<f64>, b: impl FnOnce() -> Option<f64>) -> Option<f64> {
    match a {
        Some(v) if v != 0.0 => Some(v),
        _ => b(),
    }
}

struct Cells<'a>(&'a Data);

impl Cells<'_> {
    fn delay(&self, celltype: &str, from: &str, to: &str) -> Option<f64> {
        let (f, t) = (full(from), full(to));
        self.0
            .cell_delays
            .iter()
            .filter(|a| a.celltype == celltype && f.is_match(&a.from) && t.is_match(&a.to))
            .map(|a| a.delay_ps)
            .reduce(f64::max)
    }

    fn check(&self, celltype: &str, data: &str, clock: &str) -> Option<(f64, f64)> {
        let (d, c) = (full(data), full(clock));
        let v: Vec<_> = self
            .0
            .cell_checks
            .iter()
            .filter(|x| x.celltype == celltype && d.is_match(&x.data) && c.is_match(&x.clock))
            .collect();
        if v.is_empty() {
            return None;
        }
        Some((
            v.iter().map(|x| x.setup_ps).reduce(f64::max).unwrap(),
            v.iter().map(|x| x.hold_ps).reduce(f64::max).unwrap(),
        ))
    }
}

pub fn add_timing(ch: &mut Chip, speed_grades: &[String], pip_classes: &[String], path: &str) {
    let data: Data = serde_json::from_str(&std::fs::read_to_string(path).expect("read timing data")).expect("timing data");
    let mut fitted: HashMap<String, f64> = data.pip_classes.iter().map(|(k, v)| (k.clone(), v.delay_ps)).collect();
    for (k, v) in &data.pip_classes {
        if let Some((base, _)) = k.split_once('@')
            && v.samples < MIN_SPLIT_SAMPLES
            && let Some(&d) = data.pip_classes.get(base).map(|p| &p.delay_ps)
        {
            fitted.insert(k.clone(), d);
        }
    }
    let classes: BTreeSet<&String> = pip_classes.iter().collect();
    let cd = Cells(&data);
    let mut unmeasured = 0;
    for (gi, grade) in speed_grades.iter().enumerate() {
        for &cls in &classes {
            if !fitted.contains_key(cls)
                && let Some((base, _)) = cls.split_once('@')
                && let Some(&d) = fitted.get(base)
            {
                fitted.insert(cls.clone(), d);
            }
            let d = if let Some(&d) = fitted.get(cls) {
                d
            } else if cls.starts_with("SITE:") || cls == "BELOUT" || cls == "PERMABUF" {
                0.0
            } else {
                if gi == 0 {
                    unmeasured += 1;
                }
                DEFAULT_ROUTING_PS
            };
            ch.set_pip_class(grade, cls, ps(d));
        }
        add_cells(ch, grade, &cd);
    }
    println!(
        "timing: {} pip classes ({} measured or zero), {} speed grades from {path}",
        classes.len(),
        classes.len() - unmeasured,
        speed_grades.len()
    );
}

fn add_cells(ch: &mut Chip, grade: &str, cd: &Cells) {
    use ClockEdge::{Falling, Rising};
    let comb = |ch: &mut Chip, cell, from: &str, to: &str, v: Option<f64>| {
        if let Some(v) = v {
            ch.add_comb_arc(cell, from, to, ps(v));
        }
    };
    let clk_out = |ch: &mut Chip, cell, clk: &str, out: &str, v: Option<f64>, edge| {
        if let Some(v) = v {
            ch.add_clock_out(cell, clk, out, edge, ps(v));
        }
    };
    let setup_hold = |ch: &mut Chip, cell, clk: &str, pin: &str, sh: Option<(f64, f64)>, edge| {
        if let Some((s, h)) = sh {
            ch.add_setup_hold(cell, clk, pin, edge, ps(s), ps(h));
        }
    };

    // LUTs: A1..A6 -> O (ISE ADR0..ADR5).
    let lut = ch.add_cell_variant(grade, "SLICE_LUT");
    for i in 0..6 {
        comb(ch, lut, &format!("A{}", i + 1), "O", cd.delay("X_LUT6", &format!("ADR{i}"), "O"));
    }
    let lut5 = ch.add_cell_variant(grade, "SLICE_LUT5");
    for i in 0..5 {
        comb(ch, lut5, &format!("A{}", i + 1), "O", cd.delay("X_LUT5", &format!("ADR{i}"), "O"));
    }

    // Flip-flops: CK -> Q, setup/hold of D, CE, SR. "_N": the slice's clock
    // inverted (CLKINV), the same numbers on the falling edge.
    for ty in ["SLICE_FF", "SLICE_FF5"] {
        for (sfx, edge) in [("", Rising), ("_N", Falling)] {
            let ff = ch.add_cell_variant(grade, &format!("{ty}{sfx}"));
            clk_out(ch, ff, "CK", "Q", or(cd.delay("X_SFF", "CLK", "O"), || cd.delay("X_FF", "CLK", "O")), edge);
            setup_hold(ch, ff, "CK", "D", cd.check("X_SFF", "I", "CLK"), edge);
            setup_hold(ch, ff, "CK", "CE", cd.check("X_SFF", "CE", "CLK"), edge);
            setup_hold(ch, ff, "CK", "SR", cd.check("X_SFF", "SRST|SSET", "CLK"), edge);
        }
    }

    // Carry chain: our CI is ISE's CI or CYINIT (one CYINIT mux in front).
    let carry = ch.add_cell_variant(grade, "CARRY4");
    for i in 0..4 {
        for out in ["O", "CO"] {
            let to = format!("{out}{i}");
            comb(ch, carry, "CI", &to, cd.delay("X_CARRY4", "CI|CYINIT", &format!(r"{out}\[{i}\]")));
            for j in 0..=i {
                comb(ch, carry, &format!("S{j}"), &to, cd.delay("X_CARRY4", &format!(r"S\[{j}\]"), &format!(r"{out}\[{i}\]")));
                comb(ch, carry, &format!("DI{j}"), &to, cd.delay("X_CARRY4", &format!(r"DI\[{j}\]"), &format!(r"{out}\[{i}\]")));
            }
        }
    }

    // Wide muxes (ISE X_MUX2: IA/IB data, SEL select).
    for ty in ["SLICE_F7MUX", "SLICE_F8MUX"] {
        let mux = ch.add_cell_variant(grade, ty);
        comb(ch, mux, "I0", "O", cd.delay("X_MUX2", "IA|IB", "O"));
        comb(ch, mux, "I1", "O", cd.delay("X_MUX2", "IA|IB", "O"));
        comb(ch, mux, "S", "O", cd.delay("X_MUX2", "SEL", "O"));
    }

    // Distributed RAM and shift registers: reads combinational from the
    // address, writes/shifts clocked.
    for (sfx, edge) in [("", Rising), ("_N", Falling)] {
        let lr = ch.add_cell_variant(grade, &format!("SLICE_LUTRAM{sfx}"));
        let rd = or(cd.delay("X_RAMD64_ADV", r"RADR\d", "O"), || cd.delay("X_SRLC32E", r"A\[\d\]", "Q"));
        for i in 0..6 {
            comb(ch, lr, &format!("A{}", i + 1), "O", rd);
            comb(ch, lr, &format!("A{}", i + 1), "O5", rd);
        }
        // max(filter(None, [...]), default=None): 0 and missing drop out.
        let co = [cd.delay("X_RAMD64_ADV", "CLK", "O"), cd.delay("X_SRLC32E", "CLK", "Q")]
            .into_iter()
            .flatten()
            .filter(|&v| v != 0.0)
            .reduce(f64::max);
        clk_out(ch, lr, "CLK", "O", co, edge);
        for pin in ["DI1", "DI2"] {
            setup_hold(ch, lr, "CLK", pin, cd.check("X_RAMD64_ADV", "I", "CLK"), edge);
        }
        for i in 0..6 {
            setup_hold(ch, lr, "CLK", &format!("WA{}", i + 1), cd.check("X_RAMD64_ADV", &format!("WADR{i}"), "CLK"), edge);
        }
        setup_hold(ch, lr, "CLK", "WE", cd.check("X_SRLC32E", "CE", "CLK"), edge);
    }

    // Block RAM (both halves the same): clock-to-out, setup/hold.
    let bram = ch.add_cell_variant(grade, "BRAM");
    for ab in ["A", "B"] {
        let clk = format!("CLK{ab}");
        let co = cd.delay("X_RAMB16BWER", &clk, &format!(r"DO{ab}\[\d+\]|DOP{ab}\[\d+\]"));
        for i in 0..16 {
            clk_out(ch, bram, &clk, &format!("DO{ab}{i}"), co, Rising);
        }
        for i in 0..2 {
            clk_out(ch, bram, &clk, &format!("DOP{ab}{i}"), co, Rising);
        }
        let sh = cd.check("X_RAMB16BWER", &format!(r"(ADDR|DI|DIP|WE){ab}\[\d+\]|(EN|REGCE|RST){ab}"), &clk);
        for i in 0..13 {
            setup_hold(ch, bram, &clk, &format!("ADDR{ab}{i}"), sh, Rising);
        }
        for i in 0..16 {
            setup_hold(ch, bram, &clk, &format!("DI{ab}{i}"), sh, Rising);
        }
        for i in 0..2 {
            setup_hold(ch, bram, &clk, &format!("DIP{ab}{i}"), sh, Rising);
            setup_hold(ch, bram, &clk, &format!("WE{ab}{i}"), sh, Rising);
        }
        for pin in ["EN", "REGCE", "RST"] {
            setup_hold(ch, bram, &clk, &format!("{pin}{ab}"), sh, Rising);
        }
    }

    // DSP48A1: one variant per pipeline-register configuration, DSP_<groups>
    // (registered groups in the order A B C D M P O(PMODE); DSP_NONE when
    // none), as measured by tests/dsp_char*; with OPMODE tied to a measured
    // constant, DSP_<groups>_<hex>. ISE times whole buses (A[17:0] ->
    // P[47:0]), so each arc is stored once, on bit 0 (A0 -> P0); the uarch
    // maps every bus pin onto bit 0. M is looked up as P.
    let bus = Regex::new(r"^(\w+?)\[\d+(:\d+)?\]$").unwrap();
    let dsp_pin = |name: &str| match bus.captures(name) {
        Some(c) => format!("{}0", &c[1]),
        None => name.to_string(),
    };
    let is_cascade = |pin: &str| {
        let bus = pin.split('[').next().unwrap_or(pin);
        ["PCIN", "PCOUT", "BCIN", "BCOUT", "CARRYIN", "CARRYOUT", "CARRYOUTF"].contains(&bus)
    };
    let cfgs: BTreeSet<&str> = cd
        .0
        .cell_delays
        .iter()
        .map(|a| a.celltype.as_str())
        .chain(cd.0.cell_checks.iter().map(|c| c.celltype.as_str()))
        .filter_map(|t| t.strip_prefix("X_DSP48A1:"))
        .collect();
    for cfg in cfgs {
        let (regs, op) = match cfg.split_once('_') {
            Some((r, o)) => (r, format!("_{o}")),
            None => (cfg, String::new()),
        };
        let dsp = ch.add_cell_variant(grade, &format!("DSP_{}{op}", if regs.is_empty() { "NONE" } else { regs }));
        let key = format!("X_DSP48A1:{cfg}");
        // Cascades were only measured with OPMODE from logic
        // (tests/dsp_casc_char*): a constant-OPMODE variant takes its
        // cascade-pin arcs from there.
        let logic_key = format!("X_DSP48A1:{regs}");
        let theirs = |celltype: &str, pins: [&str; 2]| {
            celltype == key || (!op.is_empty() && celltype == logic_key && pins.iter().any(|p| is_cascade(p)))
        };
        for a in cd.0.cell_delays.iter().filter(|a| theirs(&a.celltype, [&a.from, &a.to])) {
            if a.from == "CLK" {
                clk_out(ch, dsp, "CLK", &dsp_pin(&a.to), Some(a.delay_ps), Rising);
            } else {
                comb(ch, dsp, &dsp_pin(&a.from), &dsp_pin(&a.to), Some(a.delay_ps));
            }
        }
        for c in cd.0.cell_checks.iter().filter(|c| theirs(&c.celltype, [&c.data, ""]) && c.clock == "CLK") {
            setup_hold(ch, dsp, "CLK", &dsp_pin(&c.data), Some((c.setup_ps, c.hold_ps)), Rising);
        }
    }

    // Global clock buffer.
    let bufg = ch.add_cell_variant(grade, "BUFGMUX");
    comb(ch, bufg, "I0", "O", cd.delay("X_CKBUF", "I", "O"));
    comb(ch, bufg, "I1", "O", cd.delay("X_CKBUF", "I", "O"));
}
