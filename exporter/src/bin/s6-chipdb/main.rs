//! s6-chipdb: build nextpnr's himbaechel chip database (.bba) for Spartan-6
//! from the exporter's JSON (was gen/s6_gen.py + gen/s6_timing.py, which used
//! nextpnr's Python himbaechel_dbgen; this writes the same .bba text).
//!
//!     s6-chipdb --json build/xc6slx9.json --constids <uarch>/constids.inc \
//!               [--timing data/timing-xc6slx9-2.json ...] --bba build/xc6slx9.bba

mod dbgen;
mod timing;

use std::path::PathBuf;

use clap::Parser;
use serde::Deserialize;

use dbgen::{BEL_FLAG_GLOBAL, Chip, PinDir};

const UARCH: &str = "spartan6";

#[derive(Parser)]
struct Args {
    /// Exporter output (s6-export).
    #[arg(long)]
    json: PathBuf,
    /// Output .bba.
    #[arg(long)]
    bba: PathBuf,
    /// The uarch's constids.inc to preload.
    #[arg(long)]
    constids: Option<PathBuf>,
    /// Delay model from s6-timing, one file per measured speed grade
    /// (data/timing-xc6slx9-<grade>.json); grades without one take the -2
    /// file's (or the first's) values.
    #[arg(long)]
    timing: Vec<String>,
}

#[derive(Deserialize)]
struct Dev {
    device: String,
    width: usize,
    height: usize,
    speed_grades: Vec<String>,
    tile_types: Vec<TileTypeJ>,
    tiles: Vec<TileJ>,
    nodes: Vec<Vec<[usize; 3]>>,
    packages: Vec<PackageJ>,
}

#[derive(Deserialize)]
struct TileTypeJ {
    name: String,
    wires: Vec<WireJ>,
    pips: Vec<PipJ>,
    bels: Vec<BelJ>,
}

#[derive(Deserialize)]
struct WireJ {
    name: String,
    kind: String,
    #[serde(default)]
    const_value: String,
}

#[derive(Deserialize)]
struct PipJ {
    src: usize,
    dst: usize,
    kind: String,
    timing_class: String,
    #[serde(default)]
    src_ref: Vec<String>,
}

#[derive(Deserialize)]
struct BelJ {
    name: String,
    #[serde(rename = "type")]
    typ: String,
    z: u32,
    #[serde(default)]
    site_kind: u32,
    pins: Vec<BelPinJ>,
}

#[derive(Deserialize)]
struct BelPinJ {
    name: String,
    wire: usize,
    dir: String,
}

#[derive(Deserialize)]
struct TileJ {
    x: usize,
    y: usize,
    #[serde(rename = "type")]
    typ: usize,
}

#[derive(Deserialize)]
struct PackageJ {
    name: String,
    pins: Vec<PadJ>,
}

#[derive(Deserialize)]
struct PadJ {
    pin: String,
    io: Option<IoJ>,
}

#[derive(Deserialize)]
struct IoJ {
    x: usize,
    y: usize,
    bel: String,
    bank: u32,
    #[serde(default)]
    vref: bool,
}

fn base(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// General interconnect (not the clock network).
fn is_general(name: &str) -> bool {
    let b = base(name);
    ["IMUX_LOGICIN", "IMUX_GFAN", "LOGICIN", "SNG_", "DBL_", "QUAD_", "OUT["].iter().any(|p| b.starts_with(p))
}

/// Clock network wires: clock input muxes, CMT outputs, row/tile clocks.
fn is_clock_net(name: &str) -> bool {
    let b = base(name);
    ["IMUX_CLK", "CMT_OUT", "HCLK", "GCLK"].iter().any(|p| b.starts_with(p))
}

fn main() {
    let args = Args::parse();
    let dev: Dev = serde_json::from_str(&std::fs::read_to_string(&args.json).expect("read json")).expect("parse json");
    let mut ch = Chip::new(UARCH, &dev.device, dev.width, dev.height);
    if let Some(c) = &args.constids {
        ch.strs.read_constids(&std::fs::read_to_string(c).expect("read constids"));
    }
    ch.set_speed_grades(&dev.speed_grades);

    for tj in &dev.tile_types {
        let tt = ch.create_tile_type(&tj.name);
        let names: Vec<&str> = tj.wires.iter().map(|w| w.name.as_str()).collect();
        for w in &tj.wires {
            ch.create_wire(tt, &w.name, &w.kind, &w.const_value);
        }
        for p in &tj.pips {
            // Delay class: the class of the driven wire, or the slice mux.
            let pip = ch.create_pip(tt, names[p.src], names[p.dst], &p.timing_class);
            let mut flags = 0;
            // Flag 1: a LUT's DI1 from the letter above's data (MUX_<L>DI1 =
            // ALT, not the MC31 cascade). ISE allows it only for RAM, not
            // SRLs; the uarch checks.
            let r = &p.src_ref;
            if p.kind == "SITE"
                && r.len() >= 4
                && ["MUX_ADI1", "MUX_BDI1", "MUX_CDI1"].contains(&r[2].as_str())
                && r[3] == "ALT"
                && !names[p.src].contains("MC31")
            {
                flags = 1;
            }
            // Flag 2: from general routing into the clock network. The uarch
            // keeps global clock nets off these.
            if is_general(names[p.src]) && is_clock_net(names[p.dst]) {
                flags |= 2;
            }
            // Flag 4: through an unused ILOGIC (clock to CFB0, for a PLL's
            // BUFG feedback); only if the ILOGIC's I/O isn't an input.
            if p.kind == "RT_ILOGIC" {
                flags |= 4;
            }
            ch.tile_types[tt].pips[pip].flags = flags;
        }
        for b in &tj.bels {
            let bel = ch.create_bel(tt, &b.name, &b.typ, b.z);
            // Slice sub-bels: 1 = SLICEX, 2 = SLICEL, 3 = SLICEM (read by the uarch).
            ch.tile_types[tt].bels[bel].site = b.site_kind;
            for pin in &b.pins {
                let dir = match pin.dir.as_str() {
                    "IN" => PinDir::Input,
                    "OUT" => PinDir::Output,
                    _ => PinDir::Inout,
                };
                ch.add_bel_pin(tt, bel, &pin.name, names[pin.wire], dir);
            }
        }
    }

    // Global constant drivers. router2 routes constant nets back to any wire
    // whose const_value matches (TIE_0/TIE_1 and the slices' CONST_* wires),
    // so these bels only give the GND/VCC nets a driver to be placed on.
    let corner = &dev.tiles[0];
    assert_eq!((corner.x, corner.y), (0, 0));
    assert_eq!(dev.tiles.iter().filter(|t| t.typ == corner.typ).count(), 1, "corner tile type is shared");
    for (i, (name, value)) in [("PSEUDO_GND", "GND"), ("PSEUDO_VCC", "VCC")].into_iter().enumerate() {
        let wire = format!("{name}.Y");
        ch.create_wire(corner.typ, &wire, "CONST", value);
        let bel = ch.create_bel(corner.typ, name, name, 0x1F0 + i as u32);
        ch.tile_types[corner.typ].bels[bel].flags |= BEL_FLAG_GLOBAL;
        ch.add_bel_pin(corner.typ, bel, "Y", &wire, PinDir::Output);
    }

    // Tile names are "X{x}Y{y}": himbaechel appends that to an empty prefix.
    for t in &dev.tiles {
        ch.set_tile_type(t.x, t.y, &dev.tile_types[t.typ].name);
    }
    for node in &dev.nodes {
        ch.add_node(node);
    }
    // Pad functions, as "IO[_GCLK][_VREF]_<bank>": GCLK when the pad's
    // dedicated clock output reaches a BUFGMUX (as the packer decides),
    // VREF for the package's VREF pins.
    let gclk = clock_capable_pads(&dev);
    println!("{}: {} clock-capable IOBs", dev.device, gclk.len());
    for pj in &dev.packages {
        let pkg = ch.create_package(&pj.name.to_uppercase());
        for pin in &pj.pins {
            if let Some(io) = &pin.io {
                let mut function = "IO".to_string();
                if gclk.contains(&(io.x, io.y, io.bel.clone())) {
                    function += "_GCLK";
                }
                if io.vref {
                    function += "_VREF";
                }
                function += &format!("_{}", io.bank);
                ch.create_pad(pkg, &pin.pin, &format!("X{}Y{}", io.x, io.y), &io.bel, &function, io.bank);
            }
        }
    }
    if !args.timing.is_empty() {
        let classes: Vec<String> =
            dev.tile_types.iter().flat_map(|t| t.pips.iter().map(|p| p.timing_class.clone())).collect();
        timing::add_timing(&mut ch, &dev.speed_grades, &classes, &args.timing);
    }

    let n_pips: usize = dev.tile_types.iter().map(|t| t.pips.len()).sum();
    let text = ch.write_bba();
    std::fs::write(&args.bba, text).expect("write bba");
    println!(
        "{}: {} tile types, {n_pips} pips per type set, {} nodes -> {}",
        dev.device,
        dev.tile_types.len(),
        dev.nodes.len(),
        args.bba.display()
    );
}

/// IOBs whose GCLK_I (the dedicated clock path) reaches a BUFGMUX input
/// within a few hops: the clock-capable pads.
fn clock_capable_pads(dev: &Dev) -> std::collections::HashSet<(usize, usize, String)> {
    use std::collections::{HashMap, HashSet};
    type W = (usize, usize, usize);
    let mut canon: HashMap<W, W> = HashMap::new();
    for n in &dev.nodes {
        let root = (n[0][0], n[0][1], n[0][2]);
        for &[x, y, w] in n {
            canon.insert((x, y, w), root);
        }
    }
    let c = |w: W| *canon.get(&w).unwrap_or(&w);
    let mut fwd: HashMap<W, Vec<W>> = HashMap::new();
    let mut members: HashMap<W, Vec<W>> = HashMap::new();
    let mut bufg_in: HashSet<W> = HashSet::new();
    let mut gclk_pins: Vec<(W, (usize, usize, String))> = vec![];
    for t in &dev.tiles {
        let tt = &dev.tile_types[t.typ];
        for p in &tt.pips {
            fwd.entry((t.x, t.y, p.src)).or_default().push(c((t.x, t.y, p.dst)));
        }
        for w in 0..tt.wires.len() {
            members.entry(c((t.x, t.y, w))).or_default().push((t.x, t.y, w));
        }
        for b in &tt.bels {
            for p in &b.pins {
                if b.typ == "BUFGMUX" && (p.name == "I0" || p.name == "I1") {
                    bufg_in.insert(c((t.x, t.y, p.wire)));
                }
                if b.typ == "IOB" && p.name == "GCLK_I" {
                    gclk_pins.push((c((t.x, t.y, p.wire)), (t.x, t.y, b.name.clone())));
                }
            }
        }
    }
    let mut out = HashSet::new();
    for (start, iob) in gclk_pins {
        let mut seen = HashSet::from([start]);
        let mut frontier = vec![start];
        'search: for _ in 0..10 {
            let mut next = vec![];
            for n in &frontier {
                if bufg_in.contains(n) {
                    out.insert(iob.clone());
                    break 'search;
                }
                for m in members.get(n).into_iter().flatten() {
                    for &d in fwd.get(m).into_iter().flatten() {
                        if seen.insert(d) {
                            next.push(d);
                        }
                    }
                }
            }
            frontier = next;
        }
    }
    out
}
