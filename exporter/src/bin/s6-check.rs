//! s6-check: sanity checks on the exporter's JSON (was gen/check_db.py).
//!
//! Bel counts against the datasheet, path searches through the routing graph
//! (pips + nodes) for the connections a Blinky design needs, and graph
//! hygiene (inputs nothing can drive, outputs nothing can load).
//!
//!     s6-check build/xc6slx9.json

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;

#[derive(Deserialize)]
struct Dev {
    device: String,
    tile_types: Vec<TileType>,
    tiles: Vec<Tile>,
    nodes: Vec<Vec<[usize; 3]>>,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct TileType {
    wires: Vec<Wire>,
    pips: Vec<Pip>,
    bels: Vec<Bel>,
}

#[derive(Deserialize)]
struct Wire {
    #[serde(default)]
    const_value: String,
}

#[derive(Deserialize)]
struct Pip {
    src: usize,
    dst: usize,
}

#[derive(Deserialize)]
struct Bel {
    name: String,
    #[serde(rename = "type")]
    typ: String,
    #[serde(default)]
    site_kind: u32,
    pins: Vec<BelPin>,
}

#[derive(Deserialize)]
struct BelPin {
    name: String,
    wire: usize,
    dir: String,
}

#[derive(Deserialize)]
struct Tile {
    x: usize,
    y: usize,
    #[serde(rename = "type")]
    typ: usize,
    #[serde(default)]
    carry_prev: Option<[usize; 2]>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    pins: Vec<PkgPin>,
}

#[derive(Deserialize)]
struct PkgPin {
    pin: String,
    io: Option<Io>,
}

#[derive(Deserialize)]
struct Io {
    x: usize,
    y: usize,
    bel: String,
}

/// A routing node: a multi-wire node's index, or a lone tile wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Node {
    Multi(usize),
    Wire(usize, usize, usize),
}

struct Graph<'a> {
    d: &'a Dev,
    tile_at: HashMap<(usize, usize), &'a Tile>,
    node_of: HashMap<(usize, usize, usize), usize>,
    fwd: HashMap<Node, Vec<Node>>,
    has_uphill: HashSet<Node>,
    has_downhill: HashSet<Node>,
}

impl<'a> Graph<'a> {
    fn new(d: &'a Dev) -> Self {
        let tile_at = d.tiles.iter().map(|t| ((t.x, t.y), t)).collect();
        let mut node_of = HashMap::new();
        for (i, n) in d.nodes.iter().enumerate() {
            for &[x, y, w] in n {
                node_of.insert((x, y, w), i);
            }
        }
        let mut g = Graph { d, tile_at, node_of, fwd: HashMap::new(), has_uphill: HashSet::new(), has_downhill: HashSet::new() };
        for t in &d.tiles {
            for p in &d.tile_types[t.typ].pips {
                let (s, dn) = (g.node(t.x, t.y, p.src), g.node(t.x, t.y, p.dst));
                g.fwd.entry(s).or_default().push(dn);
                g.has_downhill.insert(s);
                g.has_uphill.insert(dn);
            }
        }
        g
    }

    fn node(&self, x: usize, y: usize, w: usize) -> Node {
        self.node_of.get(&(x, y, w)).map_or(Node::Wire(x, y, w), |&i| Node::Multi(i))
    }

    fn bels(&self) -> impl Iterator<Item = (usize, usize, &'a Bel)> + '_ {
        self.d.tiles.iter().flat_map(|t| self.d.tile_types[t.typ].bels.iter().map(move |b| (t.x, t.y, b)))
    }

    fn pin_node(&self, x: usize, y: usize, bel: &Bel, pin: &str) -> Node {
        let p = bel.pins.iter().find(|p| p.name == pin).unwrap_or_else(|| panic!("{} has no pin {pin}", bel.name));
        self.node(x, y, p.wire)
    }

    fn pin(&self, x: usize, y: usize, name: &str, pin: &str) -> Node {
        let bel = self.bels().find(|&(bx, by, b)| (bx, by) == (x, y) && b.name == name).map(|b| b.2);
        self.pin_node(x, y, bel.unwrap_or_else(|| panic!("no bel {name} at X{x}Y{y}")), pin)
    }

    /// Hops from `src` to the first of `targets`, by breadth-first search.
    fn reachable(&self, src: Node, targets: &[Node]) -> Option<usize> {
        self.reachable_from(&[src], targets)
    }

    /// Hops from the nearest of `srcs` to the first of `targets`.
    fn reachable_from(&self, srcs: &[Node], targets: &[Node]) -> Option<usize> {
        let targets: HashSet<Node> = targets.iter().copied().collect();
        let mut seen: HashSet<Node> = srcs.iter().copied().collect();
        let mut frontier: Vec<Node> = seen.iter().copied().collect();
        let mut hops = 0;
        while !frontier.is_empty() {
            if frontier.iter().any(|n| targets.contains(n)) {
                return Some(hops);
            }
            let mut next = vec![];
            for n in &frontier {
                for &m in self.fwd.get(n).into_iter().flatten() {
                    if seen.insert(m) {
                        next.push(m);
                    }
                }
            }
            frontier = next;
            hops += 1;
        }
        None
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: s6-check <device.json>");
    let d: Dev = serde_json::from_str(&std::fs::read_to_string(&path).expect("read json")).expect("parse json");
    let g = Graph::new(&d);
    let mut failures = 0;
    let mut check = |ok: bool, msg: String| {
        println!("{} {msg}", if ok { "PASS" } else { "FAIL" });
        failures += !ok as usize;
    };

    // Bel counts (slices are split into sub-bels: count each via its A6LUT).
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, b) in g.bels() {
        *counts.entry(b.typ.clone()).or_default() += 1;
        if b.typ == "SLICE_LUT" && b.name.ends_with(".A6LUT") {
            let kind = ["", "SLICEX", "SLICEL", "SLICEM"][b.site_kind as usize];
            *counts.entry(kind.into()).or_default() += 1;
        }
    }
    let count = |k: &str| counts.get(k).copied().unwrap_or(0);
    if d.device == "xc6slx9" {
        // DS160 Table 1 (XC6SLX9), plus resources visible in TQG144.
        for (k, v) in [("SLICEX", 715), ("SLICEL", 355), ("SLICEM", 360), ("DSP", 16), ("BUFGMUX", 16), ("DCM", 4), ("PLL", 2), ("BRAM", 64)] {
            check(count(k) == v, format!("{k}: {} (expected {v})", count(k)));
        }
        check(count("CARRY4") == 715, format!("CARRY4: {} (expected 715, one per SLICEL/SLICEM)", count("CARRY4")));
        let pkg = d.packages.iter().find(|p| p.name == "tqg144").expect("tqg144");
        let n_io = pkg.pins.iter().filter(|p| p.io.is_some()).count();
        check(n_io == 102, format!("TQG144 user I/O: {n_io} (expected 102)"));
    }

    let mut path = |msg: &str, src: Node, dsts: &[Node]| {
        let h = g.reachable(src, dsts);
        check(h.is_some(), format!("{msg} ({} hops)", h.map_or("-".into(), |h| h.to_string())));
    };

    // Logic: the lowest and highest SLICEX on the die.
    let mut xs: Vec<(usize, usize)> = g
        .bels()
        .filter(|(_, _, b)| b.typ == "SLICE_LUT" && b.site_kind == 1 && b.name == "SLICE[1].A6LUT")
        .map(|(x, y, _)| (x, y))
        .collect();
    xs.sort();
    let ((nx, ny), (fx, fy)) = (xs[0], *xs.last().unwrap());
    path("LUT6 O -> own FF D (site mux)", g.pin(nx, ny, "SLICE[1].A6LUT", "O"), &[g.pin(nx, ny, "SLICE[1].AFF", "D")]);
    path("LUT5 O -> own FF D (site mux)", g.pin(nx, ny, "SLICE[1].A5LUT", "O"), &[g.pin(nx, ny, "SLICE[1].AFF", "D")]);
    path(&format!("FF Q X{nx}Y{ny} -> LUT A1 X{fx}Y{fy}"), g.pin(nx, ny, "SLICE[1].AFF", "Q"), &[g.pin(fx, fy, "SLICE[1].B6LUT", "A1")]);
    path("5FF Q -> fabric (via AMUX)", g.pin(nx, ny, "SLICE[1].A5FF", "Q"), &[g.pin(fx, fy, "SLICE[1].B6LUT", "A1")]);

    // Carry: CO3 reaches CI of the slice above through the dedicated wire.
    let has_carry = |x: usize, y: usize| {
        g.tile_at.get(&(x, y)).is_some_and(|t| d.tile_types[t.typ].bels.iter().any(|b| b.typ == "CARRY4"))
    };
    let (cx, cy) = g.bels().filter(|(x, y, b)| b.typ == "CARRY4" && has_carry(*x, y + 1)).map(|(x, y, _)| (x, y)).next().unwrap();
    path("CARRY4 CO3 -> CARRY4 CI above", g.pin(cx, cy, "SLICE[0].CARRY4", "CO3"), &[g.pin(cx, cy + 1, "SLICE[0].CARRY4", "CI")]);
    drop(path);
    check(g.tile_at[&(cx, cy + 1)].carry_prev == Some([cx, cy]), format!("carry_prev of X{cx}Y{} is X{cx}Y{cy}", cy + 1));
    check(
        g.reachable(g.pin(cx, cy, "SLICE[0].CARRY4", "O0"), &[g.pin(cx, cy, "SLICE[0].AFF", "D")]) == Some(1),
        "CARRY4 O0 -> AFF D in one site pip".into(),
    );
    let mut path = |msg: &str, src: Node, dsts: &[Node]| {
        let h = g.reachable(src, dsts);
        check(h.is_some(), format!("{msg} ({} hops)", h.map_or("-".into(), |h| h.to_string())));
    };

    // DSP cascade: PCOUT reaches the PCIN of the DSP one site up.
    let dsps: Vec<(usize, usize)> = g.bels().filter(|(_, _, b)| b.typ == "DSP").map(|(x, y, _)| (x, y)).collect();
    if let Some(&(dx, dy)) = dsps.iter().find(|(x, y)| dsps.contains(&(*x, y + 4))) {
        path("DSP PCOUT0 -> PCIN0 of the DSP above", g.pin(dx, dy, "DSP", "PCOUT0"), &[g.pin(dx, dy + 4, "DSP", "PCIN0")]);
    }

    let all_pins = |typ: &str, pin: &str| -> Vec<Node> {
        g.bels()
            .filter(|(_, _, b)| b.typ == typ && b.pins.iter().any(|p| p.name == pin))
            .map(|(x, y, b)| g.pin_node(x, y, b, pin))
            .collect()
    };
    if d.device == "xc6slx9" {
        let pkg = d.packages.iter().find(|p| p.name == "tqg144").expect("tqg144");
        let io_pin = |pin: &str, p: &str| {
            let io = pkg.pins.iter().find(|q| q.pin == pin).and_then(|q| q.io.as_ref()).expect("pad");
            g.pin(io.x, io.y, &io.bel, p)
        };
        let mut bufg_in = all_pins("BUFGMUX", "I0");
        bufg_in.extend(all_pins("BUFGMUX", "I1"));
        path("P56 (Mojo clock) GCLK_I -> BUFGMUX", io_pin("P56", "GCLK_I"), &bufg_in);
        path("P56 I (data path) -> LUT A1", io_pin("P56", "I"), &[g.pin(fx, fy, "SLICE[1].B6LUT", "A1")]);
        path("FF Q -> P134 (Mojo LED0) O", g.pin(fx, fy, "SLICE[1].AFF", "Q"), &[io_pin("P134", "O")]);
        path("FF Q -> P134 T", g.pin(fx, fy, "SLICE[1].AFF", "Q"), &[io_pin("P134", "T")]);
    }
    let (bx, by, bufg) = g.bels().find(|(_, _, b)| b.typ == "BUFGMUX").unwrap();
    path("BUFGMUX O -> FF CK", g.pin_node(bx, by, bufg, "O"), &[g.pin(fx, fy, "SLICE[1].AFF", "CK")]);
    drop(path);

    // Constants: LUT inputs must be tieable to VCC.
    let vcc: Vec<Node> = d
        .tiles
        .iter()
        .flat_map(|t| d.tile_types[t.typ].wires.iter().enumerate().filter(|(_, w)| w.const_value == "VCC").map(move |(i, _)| (t.x, t.y, i)))
        .map(|(x, y, i)| g.node(x, y, i))
        .collect();
    let a6 = g.pin(fx, fy, "SLICE[1].A6LUT", "A6");
    check(g.reachable_from(&vcc, &[a6]).is_some(), "a VCC wire reaches LUT A6".into());

    // Graph hygiene: pins that can never carry a signal.
    let mut in_nodes: HashMap<Node, Vec<(String, String)>> = HashMap::new();
    let mut out_nodes: HashMap<Node, Vec<(String, String)>> = HashMap::new();
    for (x, y, b) in g.bels() {
        for p in &b.pins {
            let n = g.pin_node(x, y, b, &p.name);
            let m = if p.dir != "OUT" { &mut in_nodes } else { &mut out_nodes };
            m.entry(n).or_default().push((b.typ.clone(), p.name.clone()));
        }
    }
    // A DSP cascade pin of the bottom / top DSP in the column ("PCIN7" -> "PCIN").
    let cascade = |(t, p): &(String, String), names: &[&str]| {
        t == "DSP" && names.contains(&p.trim_end_matches(|c: char| c.is_ascii_digit()))
    };
    let mut undriven: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (n, v) in &in_nodes {
        if !g.has_uphill.contains(n) && !out_nodes.contains_key(n) {
            for t in v {
                *undriven.entry(t.clone()).or_default() += 1;
            }
        }
    }
    // Only the first CARRY4 of a chain, and the bottom DSP's cascade inputs.
    let ok = undriven.keys().all(|t| *t == ("CARRY4".into(), "CI".into()) || cascade(t, &["PCIN", "BCIN", "CARRYIN"]));
    check(ok, format!("undriven bel inputs are only chain-bottom CARRY4.CI and DSP cascade inputs: {undriven:?}"));
    let mut unloaded: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (n, v) in &out_nodes {
        if !g.has_downhill.contains(n) && !in_nodes.contains_key(n) {
            for t in v {
                *unloaded.entry(t.clone()).or_default() += 1;
            }
        }
    }
    // Clock-pad and feedback outputs only connect on clock-capable pads; PLL
    // TEST_CLKIN only feeds factory-test wiring (dropped); the top DSP's
    // cascade outputs.
    let clock_only = [
        ("IOB", "GCLK_I"), ("ILOGIC", "DFB"), ("ILOGIC", "CFB0"), ("ILOGIC", "CFB1"),
        ("IODELAY", "DQSOUTP"), ("IODELAY", "DQSOUTN"), ("PLL", "TEST_CLKIN"),
    ];
    let ok = unloaded.keys().all(|t| clock_only.iter().any(|&(a, b)| t.0 == a && t.1 == b) || cascade(t, &["PCOUT", "BCOUT", "CARRYOUT"]));
    check(ok, format!("unloaded bel outputs are only clock-pad paths and DSP cascade outputs: {unloaded:?}"));

    println!("\n{failures} failure(s)");
    std::process::exit(if failures > 0 { 1 } else { 0 });
}
