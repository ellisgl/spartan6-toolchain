//! Convert a design placed and routed by nextpnr-himbaechel-spartan6 into
//! Xilinx XDL, so ISE can check it (`xdl -xdl2ncd`, `drc`) and build a
//! bitstream (`bitgen`).
//!
//! Default (stage 2): our routing is written as ISE pips, so ISE only checks
//! it. With `--pins-only` (stage 1), nets are written as pins and ISE's `par`
//! routes them, keeping our placement.
//!
//! ISE names come from prjcombine's naming database for the device (built by
//! `s6-rawdump` + `xrd2geom` from ISE itself). Each nextpnr pip is identified
//! through the exporter's JSON; interconnect pips are named with prjcombine's
//! `get_tile_pip_naming`-style rules, plus the buffer/termination hops from
//! `resolve_wire_trace`. Slice pseudo-pips become site configuration. The
//! IOB <-> ILOGIC/OLOGIC hops, which prjcombine does not model as routing,
//! are found in ISE's pip graph (the rawdump).
//!
//!     s6-xdl --db spartan6.zstd --geom xc6slx9-geom.zstd --rawdump xc6slx9-rawdump.zstd \
//!            --chipdb-json xc6slx9.json --routed routed.json --out design.xdl

mod route;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use clap::Parser;
use prjcombine_entity::EntityId;
use prjcombine_interconnect::db::{BelInfo, BelInput, BelSlotId, TileWireCoord, WireKind};
use prjcombine_interconnect::grid::{BelCoord, CellCoord, ColId, DieId, RowId, TileCoord, WireCoord};
use prjcombine_re_xilinx_geom::GeomDb;
use prjcombine_re_xilinx_naming::db::RawTileId;
use prjcombine_re_xilinx_naming_spartan6::{ExpandedNamedDevice, name_device};
use prjcombine_re_xilinx_xdl::{Design, Instance, Net, NetPin, NetPip, NetType, PipDirection, Placement};
use prjcombine_spartan6::db::Database;
use prjcombine_spartan6::expanded::ExpandedDevice;
use route::{IsePip, RawGraph};
use s6_export::chipdb::ChipdbIndex;
use s6_export::nextpnr::load_routed;
use s6_export::{parse_wire_name, pin_name};

#[derive(Parser)]
#[command(about = "Convert a nextpnr-himbaechel-spartan6 result into XDL")]
struct Args {
    /// prjcombine databases/spartan6.zstd (the database the chipdb was built from)
    #[arg(long)]
    db: PathBuf,
    /// Geom database with ISE names (s6-rawdump + xrd2geom)
    #[arg(long)]
    geom: PathBuf,
    /// ISE rawdump of the device (s6-rawdump); needed for routed output
    #[arg(long)]
    rawdump: Option<PathBuf>,
    /// The exporter's JSON (s6-export); needed for routed output
    #[arg(long)]
    chipdb_json: Option<PathBuf>,
    /// nextpnr --write output
    #[arg(long)]
    routed: PathBuf,
    #[arg(long, default_value = "xc6slx9")]
    device: String,
    #[arg(long, default_value = "tqg144")]
    package: String,
    #[arg(long, default_value = "2")]
    speed: String,
    #[arg(long)]
    out: Option<PathBuf>,
    /// Stage 1: write nets as pins only and let ISE route.
    #[arg(long)]
    pins_only: bool,
    /// Debug: print ISE naming for the bels in tile X,Y (e.g. 8,63) and exit.
    #[arg(long)]
    dump_naming: Option<String>,
    /// Debug: print the naming-database entries that mention this ISE wire name.
    #[arg(long)]
    find_ise_wire: Option<String>,
    /// Debug: print ISE pips leaving TILE/WIRE (needs --rawdump).
    #[arg(long)]
    raw_pips_from: Option<String>,
}

// ---------------------------------------------------------------------------
// LUT equations

/// LUT truth table bits from a nextpnr INIT property (MSB-first binary string).
fn lut_table(init: &str) -> Vec<bool> {
    init.chars().rev().map(|c| c == '1').collect()
}

/// Boolean expression in XDL syntax (A1..A6, *, +, ~) for a truth table over
/// the given inputs (vars[i] is the pin number of table bit i).
fn lut_expr(table: &[bool], vars: &[usize]) -> String {
    if table.iter().all(|&b| !b) {
        return "0".into();
    }
    if table.iter().all(|&b| b) {
        return "1".into();
    }
    let k = vars.len();
    let half = 1 << (k - 1);
    let (lo, hi): (Vec<bool>, Vec<bool>) = (0..half).map(|i| (table[i], table[i + half])).unzip();
    let v = format!("A{}", vars[k - 1]);
    let rest = &vars[..k - 1];
    if lo == hi {
        return lut_expr(&lo, rest);
    }
    let (e0, e1) = (lut_expr(&lo, rest), lut_expr(&hi, rest));
    match (e0.as_str(), e1.as_str()) {
        ("0", "1") => v,
        ("1", "0") => format!("~{v}"),
        ("0", _) => format!("({v}*{e1})"),
        (_, "0") => format!("(~{v}*{e0})"),
        ("1", _) => format!("(~{v}+{e1})"),
        (_, "1") => format!("({v}+{e0})"),
        _ => format!("(({v}*{e1})+(~{v}*{e0}))"),
    }
}

/// Restrict a truth table: input `idx` (0-based) fixed to `val`.
fn lut_fix(table: &[bool], idx: usize, val: bool) -> Vec<bool> {
    (0..table.len() / 2)
        .map(|i| {
            let lo = i & ((1 << idx) - 1);
            let hi = (i >> idx) << (idx + 1);
            table[hi | lo | ((val as usize) << idx)]
        })
        .collect()
}

// ---------------------------------------------------------------------------

/// Compare the parts of two interconnect databases that naming depends on.
fn same_structure(a: &prjcombine_interconnect::db::IntDb, b: &prjcombine_interconnect::db::IntDb) -> Result<(), String> {
    fn cmp<I: PartialEq + std::fmt::Debug, T>(
        what: &str,
        a: impl Iterator<Item = (I, T)>,
        b: impl Iterator<Item = (I, T)>,
        eq: impl Fn(&T, &T) -> bool,
    ) -> Result<(), String> {
        let (a, b): (Vec<_>, Vec<_>) = (a.collect(), b.collect());
        if a.len() != b.len() {
            return Err(format!("{what}: {} vs {} entries", a.len(), b.len()));
        }
        for ((ia, ta), (ib, tb)) in a.iter().zip(&b) {
            if ia != ib || !eq(ta, tb) {
                return Err(format!("{what}: entry {ia:?} differs"));
            }
        }
        Ok(())
    }
    cmp("wires", a.wires.iter(), b.wires.iter(), |x, y| x == y)?;
    cmp("tile slots", a.tile_slots.iter(), b.tile_slots.iter(), |x, y| x == y)?;
    cmp("bel slots", a.bel_slots.iter(), b.bel_slots.iter(), |x, y| x == y)?;
    cmp("region slots", a.region_slots.iter(), b.region_slots.iter(), |x, y| x == y)?;
    cmp("connector slots", a.conn_slots.iter(), b.conn_slots.iter(), |x, y| x == y)?;
    cmp("connector classes", a.conn_classes.iter(), b.conn_classes.iter(), |x, y| x == y)?;
    cmp("tile classes", a.tile_classes.iter(), b.tile_classes.iter(), |x, y| {
        x.name == y.name && x.slot == y.slot && x.cells == y.cells
    })?;
    Ok(())
}

struct Names<'a> {
    edev: &'a ExpandedDevice<'a>,
    nedev: ExpandedNamedDevice<'a>,
    db: &'a Database,
}

impl Names<'_> {
    fn slot(&self, name: &str) -> BelSlotId {
        self.db
            .int
            .bel_slots
            .iter()
            .find(|(_, s)| s.name == name)
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("unknown bel slot {name}"))
    }

    fn bel(&self, x: usize, y: usize, slot: &str) -> BelCoord {
        CellCoord { die: DieId::from_idx(0), col: ColId::from_idx(x), row: RowId::from_idx(y) }.bel(self.slot(slot))
    }

    /// (ISE tile, ISE site) of a bel.
    fn site(&self, bcrd: BelCoord) -> (String, String) {
        let tcrd = self.edev.bel_tile(bcrd);
        let ntile = &self.nedev.ngrid.tiles[&tcrd];
        let tn = &self.nedev.ngrid.db.tile_class_namings[ntile.naming];
        let site = ntile.bels[bcrd.slot][0].clone();
        let rt = tn.bels[bcrd.slot].tiles[0];
        (ntile.names[rt].clone(), site)
    }

    fn tile_class(&self, bcrd: BelCoord) -> &str {
        let tcrd = self.edev.bel_tile(bcrd);
        &self.db.int.tile_classes[self.edev[tcrd].class].name
    }
}

#[derive(Default)]
struct SliceInst {
    cfg: BTreeMap<String, Vec<String>>,
}

impl SliceInst {
    fn set(&mut self, key: &str, parts: &[&str]) {
        let v: Vec<String> = std::iter::once(key.to_string()).chain(parts.iter().map(|s| s.to_string())).collect();
        if let Some(old) = self.cfg.get(key)
            && old != &v
        {
            panic!("conflicting slice setting {key}: {old:?} vs {v:?}");
        }
        self.cfg.insert(key.to_string(), v);
    }
}

impl Names<'_> {
    fn cell(&self, x: usize, y: usize) -> CellCoord {
        CellCoord { die: DieId::from_idx(0), col: ColId::from_idx(x), row: RowId::from_idx(y) }
    }

    /// Raw wire of a bel's site pin (prjcombine pin name).
    fn site_pin_wire(&self, bcrd: BelCoord, pin: &str) -> Option<WireCoord> {
        let int = &self.db.int;
        let tcrd = self.edev.bel_tile(bcrd);
        let tcls = &int.tile_classes[self.edev[tcrd].class];
        let BelInfo::Bel(bel) = tcls.bels.get(bcrd.slot)? else { return None };
        let bcls = &int.bel_classes[int.bel_class(bcrd.slot)];
        for (pid, inp) in &bel.inputs {
            let (b, idx) = bcls.inputs.key(pid);
            let ix = bcls.inputs[pid].indexing;
            if pin_name(b, idx, |i| ix.phys_to_virt(i)) == pin {
                let tw = match *inp {
                    BelInput::Fixed(p) => p.tw,
                    BelInput::Invertible(tw, _) => tw,
                };
                return Some(self.edev.tile_wire(tcrd, tw));
            }
        }
        for (pid, ws) in &bel.outputs {
            let (b, idx) = bcls.outputs.key(pid);
            let ix = bcls.outputs[pid].indexing;
            if pin_name(b, idx, |i| ix.phys_to_virt(i)) == pin {
                return Some(self.edev.tile_wire(tcrd, *ws.iter().next()?));
            }
        }
        None
    }

    /// A bel pin's ISE naming: its pips, ISE tile, near and far wire names.
    fn pin_naming(&self, bcrd: BelCoord, pin: &str) -> Option<(Vec<IsePip>, String, String, String)> {
        let ngrid = &self.nedev.ngrid;
        let tcrd = self.edev.bel_tile(bcrd);
        let ntile = ngrid.tiles.get(&tcrd)?;
        let tn = &ngrid.db.tile_class_namings[ntile.naming];
        let pn = tn.bels.get(bcrd.slot)?.pins.get(pin)?;
        let pips = pn
            .pips
            .iter()
            .map(|p| (ntile.names[p.tile].clone(), p.wire_from.clone(), p.wire_to.clone()))
            .collect();
        Some((pips, ntile.names[pn.tile].clone(), pn.name.clone(), pn.name_far.clone()))
    }

    fn tile_tcw(&self, tcrd: TileCoord, raw: WireCoord) -> Option<TileWireCoord> {
        let tile = &self.edev[tcrd];
        tile.cells
            .iter()
            .find(|&(_, &c)| c == raw.cell)
            .map(|(cell, _)| TileWireCoord { cell, wire: raw.slot })
    }

    /// ISE pips for one prjcombine interconnect pip (rdverify's rules: explicit
    /// ext_pips, alternate wire names, and buffered source wires).
    fn name_pip(&self, tcrd: TileCoord, out: TileWireCoord, inp: TileWireCoord) -> Option<Vec<IsePip>> {
        let ngrid = &self.nedev.ngrid;
        let ntile = ngrid.tiles.get(&tcrd)?;
        let tn = &ngrid.db.tile_class_namings[ntile.naming];
        if let Some(pn) = tn.ext_pips.get(&(out, inp)) {
            return Some(vec![(ntile.names[pn.tile].clone(), pn.wire_from.clone(), pn.wire_to.clone())]);
        }
        let t = ntile.names[RawTileId::from_idx(0)].clone();
        let wo = tn.wires.get(&out)?;
        let wi = tn.wires.get(&inp)?;
        if wo.alt_pips_to.contains(&inp) {
            let alt = wo.alt_name.clone()?;
            return Some(vec![(t.clone(), wi.name.clone(), alt.clone()), (t, alt, wo.name.clone())]);
        }
        if wi.alt_pips_from.contains(&out) {
            let alt = wi.alt_name.clone()?;
            return Some(vec![(t.clone(), wi.name.clone(), alt.clone()), (t, alt, wo.name.clone())]);
        }
        let mut v = vec![(t, wi.name.clone(), wo.name.clone())];
        if let Some(buf) = tn.wire_bufs.get(&inp) {
            v.push((ntile.names[buf.tile].clone(), buf.wire_from.clone(), buf.wire_to.clone()));
        }
        Some(v)
    }

    /// Buffer/termination hops from a node's root to this raw wire.
    fn trace(&self, raw: WireCoord) -> Vec<IsePip> {
        self.nedev
            .ngrid
            .resolve_wire_trace(raw)
            .map(|(_, t)| t.iter().map(|p| (p.tile.to_string(), p.wire_from.to_string(), p.wire_to.to_string())).collect())
            .unwrap_or_default()
    }

    /// The TIEOFF site (ISE tile, site) serving a cell's tie wires.
    fn tie(&self, cell: CellCoord) -> Option<(String, String)> {
        let ngrid = &self.nedev.ngrid;
        for (slot, _) in &self.edev[cell].tiles {
            if let Some(nt) = ngrid.tiles.get(&cell.tile(slot))
                && let Some(site) = &nt.tie_name
            {
                return Some((nt.names[nt.tie_rt].clone(), site.clone()));
            }
        }
        None
    }
}

/// Where a cell port can meet the routing, for routed output.
#[derive(Clone)]
enum Cand {
    /// Ordinary site pins of `bcrd` (prjcombine pin names). Several pins when
    /// the signal can use more than one, e.g. a LUT output on <L> or <L>MUX.
    Site { bcrd: BelCoord, pins: Vec<String>, inst: String, output: bool },
    /// IOB data input: pad -> ILOGIC (route-through D -> FABRICOUT) -> fabric.
    IobIn { iob: BelCoord, ilogic: BelCoord, inst: String },
    /// IOB dedicated clock path (GCLK_I).
    IobClk { iob: BelCoord, inst: String },
    /// IOB output or tristate: fabric -> OLOGIC (route-through D1 -> OQ or T1 -> TQ) -> pad.
    IobOut { iob: BelCoord, ologic: BelCoord, inst: String, d: &'static str, q: &'static str, iob_pin: &'static str },
}

#[derive(Default)]
struct XNet {
    outpins: BTreeSet<(String, String)>,
    inpins: BTreeSet<(String, String)>,
    pips: BTreeSet<IsePip>,
    /// ISE wires (tile, wire) of the source and sink pins, for repair.
    src_wires: Vec<(String, String)>,
    sink_wires: Vec<(String, String)>,
}

fn main() {
    let args = Args::parse();
    let db = Database::from_file(&args.db).expect("read prjcombine database");
    let geom = GeomDb::from_file(&args.geom).expect("read geom database");
    let dev = db.devices.iter().find(|d| d.name == args.device).expect("device not in database");
    // The naming database must describe the same interconnect *structure* as
    // the database the chipdb came from, or wire/tile/bel identities won't
    // match. (The published database is the "finished" one with bitstream data
    // the geom database lacks, so whole-database equality is too strict.)
    let gint = geom.ints.get("spartan6").expect("geom database has no spartan6 interconnect");
    if let Err(what) = same_structure(gint, &db.int) {
        panic!("geom interconnect structure differs from the prjcombine database ({what}); rebuild both at the same revision");
    }
    let ndb = &geom.namings["spartan6"];
    let chip = &db.chips[dev.chip];
    let edev = chip.expand_grid(&db.int, &dev.disabled);
    let names = Names { nedev: name_device(&edev, ndb), edev: &edev, db: &db };

    if let Some(xy) = &args.dump_naming {
        let (x, y) = xy.split_once(',').expect("--dump-naming X,Y");
        dump_naming(&names, x.parse().unwrap(), y.parse().unwrap());
        return;
    }
    if let Some(tw) = &args.raw_pips_from {
        let (t, w) = tw.split_once('/').expect("--raw-pips-from TILE/WIRE");
        RawGraph::load(args.rawdump.as_ref().expect("--rawdump")).debug_pips_from(t, w);
        return;
    }
    if let Some(w) = &args.find_ise_wire {
        find_ise_wire(&names, w);
        return;
    }
    let routed = !args.pins_only;
    let (chipdb, raw) = if routed {
        (
            Some(ChipdbIndex::load(args.chipdb_json.as_ref().expect("--chipdb-json is required for routed output"))),
            Some(RawGraph::load(args.rawdump.as_ref().expect("--rawdump is required for routed output"))),
        )
    } else {
        (None, None)
    };
    let (cells, nets) = load_routed(&args.routed);

    // Package pin for each IOB bel.
    let bond = &db.bonds[*dev.bonds.get(&args.package).expect("package not in database").1];
    let mut pad_pin: HashMap<BelCoord, String> = HashMap::new();
    for (pin, pads) in &bond.pins {
        if let [pad] = pads.as_slice() {
            pad_pin.insert(pad.bel, pin.clone());
        }
    }

    let mut instances: Vec<Instance> = vec![];
    // (tile, site) -> (kind, cfg); slice instances are named after their site.
    let mut slices: BTreeMap<(String, String), (String, SliceInst)> = BTreeMap::new();
    let mut slice_key: HashMap<String, (String, String)> = HashMap::new();
    // Stage 1: (cell, port) -> (instance, site pin). Routed: (cell, port) -> candidate.
    let mut pins: HashMap<(usize, String), (String, String)> = HashMap::new();
    let mut cands: HashMap<(usize, String), Cand> = HashMap::new();
    let net_const = |ni: usize| nets[ni].constant;

    for (ci, c) in cells.iter().enumerate() {
        let (x, y, ref belname) = c.bel;
        match c.typ.as_str() {
            "PSEUDO_GND" | "PSEUDO_VCC" => {}
            "SLICE_LUT" | "SLICE_FF" => {
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = names.bel(x, y, slot);
                let (tile, site) = names.site(bcrd);
                let kind = if slot == "SLICE[1]" {
                    "SLICEX"
                } else if names.tile_class(bcrd) == "CLEXM" {
                    "SLICEM"
                } else {
                    "SLICEL"
                };
                slice_key.insert(site.clone(), (tile.clone(), site.clone()));
                let entry = slices
                    .entry((tile.clone(), site.clone()))
                    .or_insert_with(|| (kind.to_string(), SliceInst::default()));
                let s = &mut entry.1;
                let l = sub.chars().next().unwrap();
                let inst = site.clone();
                let site_cand = |pin: String, output: bool| Cand::Site { bcrd, pins: vec![pin], inst: inst.clone(), output };
                if c.typ == "SLICE_LUT" {
                    assert_eq!(&sub[1..], "6LUT", "{}: only LUT6 bels are supported", c.name);
                    let mut table = lut_table(&c.params["INIT"]);
                    let k = table.len().trailing_zeros() as usize;
                    let mut vars: Vec<usize> = (1..=k).collect();
                    for j in (1..=k).rev() {
                        let port = format!("A{j}");
                        if !c.conns.contains_key(&port) {
                            continue;
                        }
                        if routed {
                            // Routed: constants are physically routed to the pin, keep the input.
                            cands.insert((ci, port), site_cand(format!("{l}{j}"), false));
                            continue;
                        }
                        match net_const(c.conns[&port]) {
                            Some(val) => {
                                table = lut_fix(&table, j - 1, val);
                                vars.remove(j - 1);
                            }
                            None => {
                                pins.insert((ci, port), (inst.clone(), format!("{l}{j}")));
                            }
                        }
                    }
                    let expr = lut_expr(&table, &vars);
                    s.set(&format!("{l}6LUT"), &[&c.name, "#LUT", &format!("O6={expr}")]);
                    if let Some(&ni) = c.conns.get("O") {
                        if routed {
                            // O6 can leave on <L>, or on <L>MUX through OUTMUX=O6.
                            cands.insert(
                                (ci, "O".into()),
                                Cand::Site { bcrd, pins: vec![l.to_string(), format!("{l}MUX")], inst: inst.clone(), output: true },
                            );
                        } else {
                            // O6 leaves through pin <L> unless its only use is this letter's FF.
                            let internal_only = nets[ni].users.iter().all(|(u, p)| {
                                cells[*u].typ == "SLICE_FF" && p == "D" && cells[*u].bel == (x, y, format!("{slot}.{l}FF"))
                            });
                            if !internal_only {
                                s.set(&format!("{l}USED"), &["", "0"]);
                                pins.insert((ci, "O".into()), (inst.clone(), l.to_string()));
                            }
                        }
                    }
                } else {
                    assert_eq!(&sub[1..], "FF", "{}: only main FF bels are supported", c.name);
                    let ff_type = c.params["FF_TYPE"].as_str();
                    s.set(&format!("{l}FF"), &[&c.name, "#FF"]);
                    let srinit = if matches!(ff_type, "FDSE" | "FDPE") { "SRINIT1" } else { "SRINIT0" };
                    s.set(&format!("{l}FFSRINIT"), &["", srinit]);
                    s.set("SYNC_ATTR", &["", if matches!(ff_type, "FDRE" | "FDSE") { "SYNC" } else { "ASYNC" }]);
                    s.set("CLKINV", &["", "CLK"]);
                    if routed {
                        // D comes through a slice mux (a SITE pip); the bypass pin <L>X if routed there.
                        cands.insert((ci, "D".into()), site_cand(format!("{l}X"), false));
                        cands.insert((ci, "Q".into()), site_cand(format!("{l}Q"), true));
                        for (port, pin) in [("CK", "CLK"), ("CE", "CE"), ("SR", "SR")] {
                            if c.conns.contains_key(port) {
                                cands.insert((ci, port.into()), site_cand(pin.into(), false));
                            }
                        }
                        continue;
                    }
                    let d = c.conns["D"];
                    let from_own_lut = nets[d].driver.as_ref().is_some_and(|(dc, dp)| {
                        dp == "O" && cells[*dc].typ == "SLICE_LUT" && cells[*dc].bel == (x, y, format!("{slot}.{l}6LUT"))
                    });
                    if from_own_lut {
                        s.set(&format!("{l}FFMUX"), &["", "O6"]);
                    } else {
                        s.set(&format!("{l}FFMUX"), &["", &format!("{l}X")]);
                        pins.insert((ci, "D".into()), (inst.clone(), format!("{l}X")));
                    }
                    pins.insert((ci, "Q".into()), (inst.clone(), format!("{l}Q")));
                    pins.insert((ci, "CK".into()), (inst.clone(), "CLK".into()));
                    if c.conns.get("CE").is_some_and(|&ni| net_const(ni) != Some(true)) {
                        s.set("CEUSED", &["", "0"]);
                        pins.insert((ci, "CE".into()), (inst.clone(), "CE".into()));
                    }
                    if c.conns.get("SR").is_some_and(|&ni| net_const(ni) != Some(false)) {
                        s.set("SRUSED", &["", "0"]);
                        pins.insert((ci, "SR".into()), (inst.clone(), "SR".into()));
                    }
                }
            }
            "IOB" => {
                let bcrd = names.bel(x, y, belname);
                let (tile, _) = names.site(bcrd);
                let site = pad_pin.get(&bcrd).unwrap_or_else(|| panic!("{}: IOB not bonded in {}", c.name, args.package));
                let io_type = c.params.get("IO_TYPE").map(String::as_str).unwrap_or("IBUF");
                let std = c.params.get("IOSTANDARD").map(String::as_str).unwrap_or("LVCMOS25").to_string();
                let inst = c.name.clone();
                let n = belname.trim_start_matches("IOB[").trim_end_matches(']');
                let ilogic = names.bel(x, y, &format!("ILOGIC[{n}]"));
                let ologic = names.bel(x, y, &format!("OLOGIC[{n}]"));
                let mut cfg: Vec<Vec<String>> = vec![vec!["PAD".into(), inst.clone(), "".into()]];
                let input = matches!(io_type, "IBUF" | "IBUFG" | "IOBUF");
                let output = matches!(io_type, "OBUF" | "OBUFT" | "IOBUF");
                if let Some(pull) = c.pull() {
                    cfg.push(vec!["PULLTYPE".into(), "".into(), pull.into()]);
                }
                if input {
                    cfg.push(vec!["BYPASS_MUX".into(), "".into(), "I".into()]);
                    cfg.push(vec!["IMUX".into(), format!("{inst}.IMUX"), "I".into()]);
                    cfg.push(vec!["INBUF".into(), format!("{inst}_IBUF"), "".into()]);
                    cfg.push(vec!["ISTANDARD".into(), "".into(), std.clone()]);
                    if c.conns.contains_key("I") {
                        pins.insert((ci, "I".into()), (inst.clone(), "I".into()));
                        cands.insert((ci, "I".into()), Cand::IobIn { iob: bcrd, ilogic, inst: inst.clone() });
                    }
                    if c.conns.contains_key("GCLK_I") {
                        pins.insert((ci, "GCLK_I".into()), (inst.clone(), "I".into()));
                        cands.insert((ci, "GCLK_I".into()), Cand::IobClk { iob: bcrd, inst: inst.clone() });
                    }
                }
                if output {
                    cfg.push(vec!["OUSED".into(), "".into(), "0".into()]);
                    cfg.push(vec!["OUTBUF".into(), format!("{inst}_OBUF"), "".into()]);
                    cfg.push(vec!["OSTANDARD".into(), "".into(), std.clone()]);
                    let drive = c.params.get("DRIVE").cloned().unwrap_or_else(|| "12".into());
                    let slew = c.params.get("SLEW").cloned().unwrap_or_else(|| "SLOW".into());
                    cfg.push(vec!["DRIVEATTRBOX".into(), "".into(), drive]);
                    cfg.push(vec!["SLEW".into(), "".into(), slew]);
                    cfg.push(vec!["SUSPEND".into(), "".into(), "3STATE".into()]);
                    pins.insert((ci, "O".into()), (inst.clone(), "O".into()));
                    cands.insert(
                        (ci, "O".into()),
                        Cand::IobOut { iob: bcrd, ologic, inst: inst.clone(), d: "D1", q: "OQ", iob_pin: "O" },
                    );
                    if c.conns.contains_key("T") {
                        cfg.push(vec!["TUSED".into(), "".into(), "0".into()]);
                        pins.insert((ci, "T".into()), (inst.clone(), "T".into()));
                        cands.insert(
                            (ci, "T".into()),
                            Cand::IobOut { iob: bcrd, ologic, inst: inst.clone(), d: "T1", q: "TQ", iob_pin: "T" },
                        );
                    }
                }
                instances.push(Instance {
                    name: inst,
                    kind: "IOB".into(),
                    placement: Placement::Placed { tile, site: site.clone() },
                    cfg,
                });
            }
            "BUFGMUX" => {
                let bcrd = names.bel(x, y, belname);
                let (tile, site) = names.site(bcrd);
                let inst = c.name.clone();
                for port in ["I0", "I1"] {
                    if c.conns.contains_key(port) {
                        assert_eq!(port, "I0", "{}: BUFGMUX I1 input not supported yet", c.name);
                        pins.insert((ci, port.into()), (inst.clone(), "I0".into()));
                        cands.insert((ci, port.into()), Cand::Site { bcrd, pins: vec!["I0".into()], inst: inst.clone(), output: false });
                    }
                }
                pins.insert((ci, "O".into()), (inst.clone(), "O".into()));
                cands.insert((ci, "O".into()), Cand::Site { bcrd, pins: vec!["O".into()], inst: inst.clone(), output: true });
                instances.push(Instance {
                    name: inst.clone(),
                    kind: "BUFG".into(),
                    placement: Placement::Placed { tile, site },
                    cfg: vec![vec!["BUFG".into(), inst, "".into()]],
                });
            }
            other => panic!("cell {}: type {other} not supported by the XDL writer yet", c.name),
        }
    }

    let mut xnets: Vec<Net> = vec![];
    let mk_net = |name: String, typ: NetType, x: XNet| Net {
        name,
        typ,
        outpins: x.outpins.into_iter().map(|(inst_name, pin)| NetPin { inst_name, pin }).collect(),
        inpins: x.inpins.into_iter().map(|(inst_name, pin)| NetPin { inst_name, pin }).collect(),
        pips: x
            .pips
            .into_iter()
            .map(|(tile, wire_from, wire_to)| NetPip { tile, wire_from, wire_to, dir: PipDirection::UniBuf })
            .collect(),
        cfg: vec![],
    };

    if !routed {
        // Stage 1: nets as pins only.
        for n in &nets {
            let mut x = XNet::default();
            if let Some((dc, dp)) = &n.driver
                && let Some(p) = pins.get(&(*dc, dp.clone()))
            {
                x.outpins.insert(p.clone());
            }
            for (uc, up) in &n.users {
                if let Some(p) = pins.get(&(*uc, up.clone())) {
                    x.inpins.insert(p.clone());
                }
            }
            if x.inpins.is_empty() {
                continue;
            }
            let typ = match n.constant {
                Some(true) => NetType::Vcc,
                Some(false) => NetType::Gnd,
                None => NetType::Plain,
            };
            assert!(typ != NetType::Plain || !x.outpins.is_empty(), "net {} has sinks but no driver pin", n.name);
            xnets.push(mk_net(n.name.clone(), typ, x));
        }
    } else {
        let chipdb = chipdb.as_ref().unwrap();
        let raw = raw.as_ref().unwrap();
        let mut unnamed: BTreeMap<String, usize> = BTreeMap::new();
        let mut repaired = 0usize;
        let mut unmatched: Vec<String> = vec![];
        let mut site_muxes: Vec<(usize, usize, String, String, String)> = vec![];
        let mut used_site_pins: Vec<(String, String)> = vec![];
        let canon = |w: WireCoord| edev.resolve_wire(w).unwrap_or(w);

        for n in &nets {
            // Each emitted item hangs off a canonical node, so constant nets can
            // be split per TIEOFF source.
            let mut items: Vec<(WireCoord, Item)> = vec![];
            let mut parent: HashMap<WireCoord, WireCoord> = HashMap::new();
            let mut reached: HashSet<WireCoord> = HashSet::new();
            let mut sources: HashSet<WireCoord> = HashSet::new();
            let mut has_site_pips = false;
            let mut has_int_pips = false;

            for (x, y, dst, src) in &n.route {
                let meta = chipdb
                    .get(*x, *y, src, dst)
                    .unwrap_or_else(|| panic!("net {}: pip X{x}Y{y}/{dst}/{src} not in chipdb JSON", n.name));
                match meta.kind.as_str() {
                    "SITE" => {
                        has_site_pips = true;
                        let r = &meta.src_ref;
                        site_muxes.push((*x, *y, r[1].clone(), r[2].clone(), r[3].clone()));
                        // A slice mux reading a site input pin (e.g. AX -> AFF_D) consumes it.
                        if let Some(w) = parse_wire_name(&edev, names.cell(*x, *y), src) {
                            reached.insert(canon(w));
                        }
                        if let Some(w) = parse_wire_name(&edev, names.cell(*x, *y), dst) {
                            // A slice mux driving a site output pin (e.g. A5Q -> AMUX).
                            sources.insert(canon(w));
                        }
                    }
                    "BELOUT" => {}
                    _ => {
                        has_int_pips = true;
                        let cell = names.cell(*x, *y);
                        let raw_src = parse_wire_name(&edev, cell, src).expect("unknown src wire");
                        let raw_dst = parse_wire_name(&edev, cell, dst).expect("unknown dst wire");
                        let tslot = db
                            .int
                            .tile_slots
                            .iter()
                            .find(|(_, s)| **s == meta.src_ref[0])
                            .map(|(id, _)| id)
                            .expect("unknown tile slot");
                        let tcrd = cell.tile(tslot);
                        let (cs, cd) = (canon(raw_src), canon(raw_dst));
                        parent.insert(cd, cs);
                        reached.insert(cd);
                        sources.insert(cs);
                        let named = names
                            .tile_tcw(tcrd, raw_src)
                            .zip(names.tile_tcw(tcrd, raw_dst))
                            .and_then(|(ti, to)| names.name_pip(tcrd, to, ti));
                        match named {
                            Some(pips) => items.extend(pips.into_iter().map(|p| (cs, Item::Pip(p)))),
                            None => *unnamed.entry(format!("{} {dst} <- {src}", meta.kind)).or_default() += 1,
                        }
                        items.extend(names.trace(raw_src).into_iter().map(|p| (cs, Item::Pip(p))));
                    }
                }
            }

            // Site pins the route actually reaches (inputs) or leaves from (outputs).
            let ports = n.driver.iter().map(|d| (d, true)).chain(n.users.iter().map(|u| (u, false)));
            for ((ci, port), is_driver) in ports {
                let Some(cand) = cands.get(&(*ci, port.clone())) else { continue };
                let matched = match cand {
                    Cand::Site { bcrd, pins, inst, output } => {
                        let mut any = false;
                        for pin in pins {
                            let Some(w) = names.site_pin_wire(*bcrd, pin) else { panic!("no wire for pin {pin}") };
                            let c = canon(w);
                            let hit = if *output { sources.contains(&c) } else { reached.contains(&c) };
                            if !hit {
                                continue;
                            }
                            any = true;
                            let (pp, ptile, pname, _) = names.pin_naming(*bcrd, pin).expect("site pin naming");
                            items.extend(pp.into_iter().map(|p| (c, Item::Pip(p))));
                            if !*output {
                                items.extend(names.trace(w).into_iter().map(|p| (c, Item::Pip(p))));
                            }
                            used_site_pins.push((inst.clone(), pin.clone()));
                            items.push((
                                c,
                                if *output {
                                    Item::Out(inst.clone(), pin.clone(), ptile, pname)
                                } else {
                                    Item::In(inst.clone(), pin.clone(), ptile, pname)
                                },
                            ));
                        }
                        any
                    }
                    Cand::IobClk { iob, inst } => {
                        let w = names.site_pin_wire(*iob, "I").expect("IOB I wire");
                        let c = canon(w);
                        let hit = sources.contains(&c);
                        if hit {
                            let (pp, ptile, pname, _) = names.pin_naming(*iob, "I").expect("IOB I naming");
                            items.extend(pp.into_iter().map(|p| (c, Item::Pip(p))));
                            items.push((c, Item::Out(inst.clone(), "I".into(), ptile, pname)));
                        }
                        hit
                    }
                    Cand::IobIn { iob, ilogic, inst } => {
                        let w = names.site_pin_wire(*ilogic, "FABRICOUT").expect("ILOGIC FABRICOUT wire");
                        let c = canon(w);
                        let hit = sources.contains(&c);
                        if hit {
                            let (iob_pips, iob_tile, iob_name, iob_far) = names.pin_naming(*iob, "I").expect("IOB I naming");
                            let (_, ioi_tile, d_name, _) = names.pin_naming(*ilogic, "D").expect("ILOGIC D naming");
                            let (fo_pips, _, fo_name, _) = names.pin_naming(*ilogic, "FABRICOUT").expect("FABRICOUT naming");
                            let hop = raw
                                .find_path((&iob_tile, &iob_far), (&ioi_tile, &d_name), 4)
                                .unwrap_or_else(|| panic!("{inst}: no ISE path from {iob_far} to {d_name}"));
                            for p in iob_pips.into_iter().chain(hop).chain(fo_pips) {
                                items.push((c, Item::Pip(p)));
                            }
                            items.push((c, Item::Pip((ioi_tile.clone(), d_name, fo_name))));
                            items.push((c, Item::Out(inst.clone(), "I".into(), iob_tile.clone(), iob_name)));
                        }
                        hit
                    }
                    Cand::IobOut { iob, ologic, inst, d, q, iob_pin } => {
                        let w = names.site_pin_wire(*ologic, d).expect("OLOGIC input wire");
                        let c = canon(w);
                        let hit = reached.contains(&c);
                        if hit {
                            let (d_pips, ioi_tile, d_name, _) = names.pin_naming(*ologic, d).expect("OLOGIC D naming");
                            let (_, _, q_name, _) = names.pin_naming(*ologic, q).expect("OLOGIC Q naming");
                            let (iob_pips, iob_tile, iob_name, iob_far) = names.pin_naming(*iob, iob_pin).expect("IOB pin naming");
                            let hop = raw
                                .find_path((&ioi_tile, &q_name), (&iob_tile, &iob_far), 4)
                                .unwrap_or_else(|| panic!("{inst}: no ISE path from {q_name} to {iob_far}"));
                            for p in d_pips.into_iter().chain(names.trace(w)).chain(hop).chain(iob_pips) {
                                items.push((c, Item::Pip(p)));
                            }
                            items.push((c, Item::Pip((ioi_tile.clone(), d_name, q_name))));
                            items.push((c, Item::In(inst.clone(), iob_pin.to_string(), iob_tile.clone(), iob_name)));
                        }
                        hit
                    }
                };
                // A FF fed straight from its own LUT uses only the slice mux, and
                // a LUT output that only feeds its own FF never leaves the slice.
                let internal = (has_site_pips && port == "D" && !is_driver) || (is_driver && !has_int_pips);
                if !matched && !internal && !(is_driver && n.users.is_empty()) {
                    unmatched.push(format!("{} {}.{}", n.name, cells[*ci].name, port));
                }
            }

            // Group into XDL nets: one per constant source, else one.
            let root_of = |mut w: WireCoord| {
                let mut guard = 0;
                while let Some(&p) = parent.get(&w) {
                    w = p;
                    guard += 1;
                    assert!(guard < 10000, "routing loop in net {}", n.name);
                }
                w
            };
            let mut groups: BTreeMap<String, XNet> = BTreeMap::new();
            for (node, item) in items {
                let key = if n.constant.is_some() { format!("{:?}", root_of(node)) } else { String::new() };
                let g = groups.entry(key).or_default();
                match item {
                    Item::Pip(p) => {
                        g.pips.insert(p);
                    }
                    Item::Out(i, p, t, w) => {
                        g.outpins.insert((i, p));
                        g.src_wires.push((t, w));
                    }
                    Item::In(i, p, t, w) => {
                        g.inpins.insert((i, p));
                        g.sink_wires.push((t, w));
                    }
                }
            }
            let mut idx = 0;
            for (key, mut g) in groups {
                if g.inpins.is_empty() {
                    continue;
                }
                let typ = match n.constant {
                    Some(v) => {
                        // Root must be a tie wire: add its TIEOFF as the source.
                        let root = n
                            .route
                            .iter()
                            .filter_map(|(x, y, _, src)| parse_wire_name(&edev, names.cell(*x, *y), src))
                            .map(canon)
                            .find(|w| format!("{:?}", root_of(*w)) == key)
                            .map(root_of)
                            .unwrap_or_else(|| panic!("constant net {} has no routed source", n.name));
                        let kind = db.int.wires[root.slot].kind;
                        assert!(matches!(kind, WireKind::Tie0 | WireKind::Tie1), "constant net {} starts at a non-tie wire", n.name);
                        let (tile, site) = names.tie(root.cell).expect("no TIEOFF for tie wire");
                        let site_name = site.clone();
                        let inst = format!("XDL_DUMMY_{tile}_{site}");
                        let pin = if v { "HARD1" } else { "HARD0" };
                        if !instances.iter().any(|i| i.name == inst) {
                            instances.push(Instance {
                                name: inst.clone(),
                                kind: "TIEOFF".into(),
                                placement: Placement::Placed { tile, site },
                                cfg: vec![
                                    vec!["_NO_USER_LOGIC".into(), "".into(), "".into()],
                                    vec![if v { "_VCC_SOURCE" } else { "_GND_SOURCE" }.into(), "".into(), pin.into()],
                                ],
                            });
                        }
                        let (t, w) = raw.site_pin_wire(&site_name, pin).expect("TIEOFF pin wire");
                        g.src_wires.push((t, w));
                        g.outpins.insert((inst, pin.into()));
                        if v { NetType::Vcc } else { NetType::Gnd }
                    }
                    None => NetType::Plain,
                };
                match raw.repair(&g.src_wires, &g.sink_wires, &mut g.pips) {
                    Ok(added) => repaired += added.len(),
                    Err(e) => eprintln!("warning: net {}: {e}", n.name),
                }
                let name = if n.constant.is_some() {
                    idx += 1;
                    format!("{}_{}", n.name, idx - 1)
                } else {
                    n.name.clone()
                };
                xnets.push(mk_net(name, typ, g));
            }
        }

        // Slice muxes chosen by the router.
        for (x, y, prefix, attr, val) in site_muxes {
            let (_, site) = names.site(names.bel(x, y, &prefix));
            let key = slice_key.get(&site).unwrap_or_else(|| panic!("slice mux in unused slice {site}")).clone();
            let s = &mut slices.get_mut(&key).unwrap().1;
            let l = |a: &str, pre: &str| a.trim_start_matches(pre).chars().next().unwrap();
            let (k, v) = if let Some(rest) = attr.strip_prefix("MUX_FF") {
                (format!("{rest}FFMUX"), val)
            } else if attr.starts_with("MUX_") && attr.ends_with("OUT") {
                (format!("{}OUTMUX", l(&attr, "MUX_")), val)
            } else if attr.starts_with("MUX_") && attr.ends_with("CY0") {
                (format!("{}CY0", l(&attr, "MUX_")), val)
            } else if attr == "PRECYINIT" {
                ("PRECYINIT".into(), match val.as_str() { "CONST_0" => "0".into(), "CONST_1" => "1".into(), v => v.into() })
            } else {
                panic!("unknown slice mux {attr}")
            };
            s.set(&k, &["", &v]);
        }
        // Site pins used: LUT outputs (<L>USED), CE and SR.
        for (inst, pin) in used_site_pins {
            let Some(key) = slice_key.get(&inst) else { continue };
            let s = &mut slices.get_mut(key).unwrap().1;
            match pin.as_str() {
                "A" | "B" | "C" | "D" => s.set(&format!("{pin}USED"), &["", "0"]),
                "CE" => s.set("CEUSED", &["", "0"]),
                "SR" => s.set("SRUSED", &["", "0"]),
                _ => {}
            }
        }
        // Route-throughs (ILOGIC/OLOGIC, BUFH, ...) need dummy instances.
        let mut rt_sites: BTreeSet<String> = instances.iter().map(|i| i.name.clone()).collect();
        for net in &xnets {
            for p in &net.pips {
                let pip = (p.tile.clone(), p.wire_from.clone(), p.wire_to.clone());
                if let Some((site, out_pin)) = raw.route_through(&pip) {
                    let inst = format!("XDL_DUMMY_{}_{}", p.tile, site.site);
                    if rt_sites.insert(inst.clone()) {
                        instances.push(Instance {
                            name: inst,
                            kind: site.kind,
                            placement: Placement::Placed { tile: p.tile.clone(), site: site.site },
                            cfg: vec![
                                vec!["_NO_USER_LOGIC".into(), "".into(), "".into()],
                                vec!["_ROUTETHROUGH".into(), site.pin, out_pin],
                            ],
                        });
                    }
                }
            }
        }
        eprintln!("added {repaired} ISE pip(s) prjcombine models as plain wires (buffers, terminations)");
        for (what, count) in &unnamed {
            eprintln!("warning: {count} pip(s) with no ISE name: {what}");
        }
        for u in &unmatched {
            eprintln!("warning: routing does not reach {u}");
        }
    }

    for ((tile, site), (kind, s)) in slices {
        instances.push(Instance {
            name: site.clone(),
            kind,
            placement: Placement::Placed { tile, site },
            cfg: s.cfg.into_values().collect(),
        });
    }

    let design = Design {
        name: "top".into(),
        part: format!("{}{}-{}", args.device, args.package, args.speed),
        version: "v3.2".into(),
        cfg: vec![],
        instances,
        nets: xnets,
    };
    let out = args.out.as_ref().expect("--out is required");
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).expect("create output"));
    design.write(&mut f).expect("write XDL");
    let n_pips: usize = design.nets.iter().map(|n| n.pips.len()).sum();
    eprintln!(
        "wrote {} ({} instances, {} nets, {} pips)",
        out.display(),
        design.instances.len(),
        design.nets.len(),
        n_pips
    );
}

enum Item {
    Pip(IsePip),
    /// (instance, pin, ISE tile, ISE wire of the pin)
    Out(String, String, String, String),
    In(String, String, String, String),
}

fn find_ise_wire(names: &Names, w: &str) {
    let ndb = names.nedev.ngrid.db;
    let int = &names.db.int;
    for (_, key, tn) in &ndb.tile_class_namings {
        for (tcw, wn) in &tn.wires {
            if wn.name == w || wn.alt_name.as_deref() == Some(w) {
                println!("tile naming {key}: wire {:?} {} (alt {:?})", tcw, int.wires[tcw.wire].name, wn.alt_name);
            }
        }
        for (tcw, pn) in &tn.wire_bufs {
            if pn.wire_to == w || pn.wire_from == w {
                println!("tile naming {key}: wire_buf for {} : {} -> {}", int.wires[tcw.wire].name, pn.wire_from, pn.wire_to);
            }
        }
        for ((o, i), pn) in &tn.ext_pips {
            if pn.wire_to == w || pn.wire_from == w {
                println!("tile naming {key}: ext_pip {} <- {} : {} -> {}", int.wires[o.wire].name, int.wires[i.wire].name, pn.wire_from, pn.wire_to);
            }
        }
        for (tcw, dn) in &tn.delay_wires {
            if dn == w {
                println!("tile naming {key}: delay wire {}", int.wires[tcw.wire].name);
            }
        }
        for (tcw, iw) in &tn.intf_wires_in {
            if format!("{iw:?}").contains(w) {
                println!("tile naming {key}: intf_wire_in {} : {iw:?}", int.wires[tcw.wire].name);
            }
        }
        for (_, bn) in &tn.bels {
            for (pin, pn) in &bn.pins {
                if pn.name == w || pn.name_far == w || pn.pips.iter().any(|p| p.wire_to == w || p.wire_from == w)
                    || pn.int_pips.values().any(|p| p.wire_to == w || p.wire_from == w)
                {
                    println!("tile naming {key}: bel pin {pin}: {} / {} pips {:?} int_pips {:?}", pn.name, pn.name_far,
                        pn.pips.iter().map(|p| format!("{}->{}", p.wire_from, p.wire_to)).collect::<Vec<_>>(),
                        pn.int_pips.iter().map(|(t, p)| format!("{}={}->{}", int.wires[t.wire].name, p.wire_from, p.wire_to)).collect::<Vec<_>>());
                }
            }
        }
    }
    for (_, key, cn) in &ndb.conn_class_namings {
        if format!("{:?}", cn).contains(w) {
            println!("conn naming {key}: mentions {w}");
        }
    }
}

fn dump_naming(names: &Names, x: usize, y: usize) {
    let cell = CellCoord { die: DieId::from_idx(0), col: ColId::from_idx(x), row: RowId::from_idx(y) };
    let ngrid = &names.nedev.ngrid;
    for (slot, tile) in &names.edev[cell].tiles {
        let tcrd = cell.tile(slot);
        let tcls = &names.db.int.tile_classes[tile.class];
        let Some(ntile) = ngrid.tiles.get(&tcrd) else {
            println!("tile {} ({}): no naming", tcls.name, names.db.int.tile_slots[slot]);
            continue;
        };
        let tn = &ngrid.db.tile_class_namings[ntile.naming];
        println!(
            "tile {} naming {} ISE tiles {:?} tie {:?}",
            tcls.name,
            ngrid.db.tile_class_namings.key(ntile.naming),
            ntile.names.values().collect::<Vec<_>>(),
            ntile.tie_name
        );
        for (bslot, bn) in &tn.bels {
            println!(
                "  bel {} sites {:?} tiles {:?}",
                names.db.int.bel_slots[bslot].name,
                ntile.bels.get(bslot),
                bn.tiles.iter().map(|rt| &ntile.names[*rt]).collect::<Vec<_>>()
            );
            for (pin, pn) in &bn.pins {
                let pips: Vec<_> = pn.pips.iter().map(|p| format!("{}:{}<-{}", ntile.names[p.tile], p.wire_to, p.wire_from)).collect();
                let ipips: Vec<_> = pn.int_pips.iter().map(|(w, p)| format!("{}={}:{}<-{}", w.to_string(&names.db.int, tcls), ntile.names[p.tile], p.wire_to, p.wire_from)).collect();
                println!(
                    "    pin {pin}: name {} far {} orig {:?} intf {} pips {:?} int_pips {:?}",
                    pn.name, pn.name_far, pn.orig_pin, pn.is_intf, pips, ipips
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_restricts_table() {
        // f = A1 xor A2, fix A2 = 1 -> ~A1
        let t = vec![false, true, true, false];
        assert_eq!(lut_fix(&t, 1, true), vec![true, false]);
        assert_eq!(lut_fix(&t, 0, false), vec![false, true]);
    }

    #[test]
    fn expr_simple() {
        assert_eq!(lut_expr(&[false, true], &[1]), "A1");
        assert_eq!(lut_expr(&[true, false], &[1]), "~A1");
        assert_eq!(lut_expr(&[false, false, false, true], &[1, 2]), "(A2*A1)");
        assert_eq!(lut_expr(&lut_table("0110"), &[1, 2]), "((A2*~A1)+(~A2*A1))");
    }
}
