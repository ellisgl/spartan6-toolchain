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
    /// Debug: list every ISE test pip per tile kind (needs --rawdump).
    #[arg(long)]
    test_pips: bool,
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

    /// The n-th site of a bel (BRAM[0] has the RAMB8 and the RAMB16 site).
    fn site_n(&self, bcrd: BelCoord, n: usize) -> (String, String) {
        let tcrd = self.edev.bel_tile(bcrd);
        let ntile = &self.nedev.ngrid.tiles[&tcrd];
        let tn = &self.nedev.ngrid.db.tile_class_namings[ntile.naming];
        let site = ntile.bels[bcrd.slot][n].clone();
        let rt = tn.bels[bcrd.slot].tiles[n];
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
    /// ISE (tile, wire) name of one of our wires (local name in a cell).
    fn ise_wire(&self, cell: CellCoord, edev: &prjcombine_interconnect::grid::ExpandedGrid, name: &str) -> Option<(String, String)> {
        self.ise_wire_raw(parse_wire_name(edev, cell, name)?)
    }

    /// ISE (tile, wire) name of a raw wire.
    fn ise_wire_raw(&self, raw: WireCoord) -> Option<(String, String)> {
        let cell = raw.cell;
        let ngrid = &self.nedev.ngrid;
        for (slot, _) in &self.edev[cell].tiles {
            let tcrd = cell.tile(slot);
            let Some(ntile) = ngrid.tiles.get(&tcrd) else { continue };
            let Some(tcw) = self.tile_tcw(tcrd, raw) else { continue };
            let tn = &ngrid.db.tile_class_namings[ntile.naming];
            if let Some(w) = tn.wires.get(&tcw) {
                return Some((ntile.names[RawTileId::from_idx(0)].clone(), w.name.clone()));
            }
        }
        None
    }

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
    /// An IOI_DDR clock input (prjcombine's DDR clocking): in ISE terms a
    /// clock pin of the OLOGIC2 site, reached through the I/O clock line
    /// `port` uses (CLK0 = IOI_IOCLK[0|3], CLK1 = [1|4]; ISE pairs them
    /// crosswise with the site's CLK1 / CLK0 pins).
    DdrClk { ddr: BelCoord, port: String, inst: String, site: String },
    /// A site pin whose routing wire, naming entry and XDL name differ, e.g.
    /// RAMB16 DIA16: wire of BRAM[1] pin DIA[0], naming BRAM[0] RAMB16_DIA16.
    Pin { wire: (BelCoord, String), naming: (BelCoord, String), xdl_pin: String, inst: String, output: bool },
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
    if args.test_pips {
        RawGraph::load(args.rawdump.as_ref().expect("--rawdump")).debug_test_pips();
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
    // OLOGIC (ODDR2) cells and their pads: (ologic bel, inst, iob bel, iob inst).
    // (OLOGIC, instance, its IOB, pad name, tristate register used)
    let mut ologic_pads: Vec<(BelCoord, String, BelCoord, String, bool)> = vec![];
    // ILOGIC (IDDR2) cells and their pads: (ilogic bel, inst, iob bel, iob inst).
    let mut ilogic_pads: Vec<(BelCoord, String, BelCoord, String)> = vec![];
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
            "SLICE_LUT" | "SLICE_LUT5" | "SLICE_LUTRAM" | "SLICE_FF" | "SLICE_FF5" | "CARRY4" | "SLICE_F7MUX" | "SLICE_F8MUX" => {
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
                if c.typ == "SLICE_LUTRAM" {
                    // SLICEM LUT as RAM or shift register.
                    assert!(routed, "{}: distributed RAM needs routed mode", c.name);
                    let mode = c.params.get("RAM_MODE").map(String::as_str).unwrap_or("RAM64");
                    let init: Vec<bool> = c.params.get("INIT").map(|v| v.chars().rev().map(|ch| ch == '1').collect()).unwrap_or_default();
                    let bit = |i: usize| init.get(i).copied().unwrap_or(false);
                    let hex = |bits: &[bool]| -> String {
                        bits.chunks(4).rev().map(|n| char::from_digit(n.iter().enumerate().fold(0, |a, (k, &b)| a | (b as u32) << k), 16).unwrap().to_ascii_uppercase()).collect()
                    };
                    // XDL takes an SRL's logical INIT; RAMs give the table.
                    let (xmode, table): (&str, Vec<bool>) = match mode {
                        "SRL32" => ("SRL32", (0..64).map(|i| i < 32 && bit(i)).collect()),
                        "SRL16" => ("SRL16", (0..64).map(|i| i < 16 && bit(i)).collect()),
                        "RAM32" => ("SPRAM32", (0..64).map(bit).collect()),
                        _ => ("SPRAM64", (0..64).map(bit).collect()),
                    };
                    // A RAM32 LUT is two 32-bit RAMs: O6 is the upper half of
                    // the table (A6 high), O5 the lower. XDL wants 64 bits for
                    // O6, so its 32 are repeated (A6 doesn't matter to it).
                    let o6: Vec<bool> = if mode == "RAM32" { table[32..].iter().chain(&table[32..]).copied().collect() } else { table.clone() };
                    s.set(&format!("{l}6LUT"), &[&c.name, "#RAM", &format!("O6=0x{}", hex(&o6))]);
                    s.set(&format!("{l}6RAMMODE"), &["", xmode]);
                    if mode == "RAM32" {
                        s.set(&format!("{l}5LUT"), &[&format!("{}$o5", c.name), "#RAM", &format!("O5=0x{}", hex(&table[..32]))]);
                        s.set(&format!("{l}5RAMMODE"), &["", "SPRAM32"]);
                    }
                    s.set("CLKINV", &["", if clk_inv(c) { "CLK_B" } else { "CLK" }]);
                    for wa in ["WA7", "WA8"] {
                        if c.conns.contains_key(wa) {
                            s.set(&format!("{wa}USED"), &["", "0"]);
                        }
                    }
                    for (port, _) in &c.conns {
                        let (pins, output): (Vec<String>, bool) = match port.as_str() {
                            p if p.len() == 2 && p.starts_with('A') => (vec![format!("{l}{}", &p[1..])], false),
                            // WA1..WA6 are the D LUT's inputs, WA7/WA8 the CX/BX pins.
                            "WA7" => (vec!["CX".into()], false),
                            "WA8" => (vec!["BX".into()], false),
                            p if p.starts_with("WA") && p.len() == 3 => (vec![format!("D{}", &p[2..])], false),
                            // Always the letter's own bypass pin (the chip database
                            // offers ALT only for shift cascades).
                            // A, B and C can also take DX through the letter above
                            // (<L>DI1MUX = DX / BDI1; the wide RAMs).
                            "DI1" if l != 'D' => (vec![format!("{l}X"), "DX".into()], false),
                            "DI1" => (vec![format!("{l}X")], false),
                            "DI2" => (vec![format!("{l}I")], false),
                            "WE" => (vec!["WE".into()], false), // only the WE pin (CE would gate the FFs)
                            "CLK" => (vec!["CLK".into()], false),
                            "O" => (vec![l.to_string()], true),
                            "O5" => (vec![format!("{l}MUX")], true),
                            _ => continue,
                        };
                        if port != "O" && port != "O5" && output {
                            continue;
                        }
                        cands.insert((ci, port.clone()), Cand::Site { bcrd, pins, inst: inst.clone(), output });
                    }
                    continue;
                }
                if matches!(c.typ.as_str(), "SLICE_F7MUX" | "SLICE_F8MUX") {
                    // F7AMUX: S on AX, out on AMUX; F7BMUX: CX/CMUX; F8MUX: BX/BMUX.
                    // Data inputs come from the LUTs inside the slice.
                    assert!(routed, "{}: wide muxes need routed mode", c.name);
                    let l = match sub {
                        "F7AMUX" => 'A',
                        "F7BMUX" => 'C',
                        "F8MUX" => 'B',
                        other => panic!("{}: unexpected wide mux bel {other}", c.name),
                    };
                    s.set(sub, &[&c.name, ""]);
                    if c.conns.contains_key("S") {
                        cands.insert((ci, "S".into()), site_cand(format!("{l}X"), false));
                    }
                    if c.conns.contains_key("O") {
                        cands.insert((ci, "O".into()), site_cand(format!("{l}MUX"), true));
                    }
                    continue;
                }
                if c.typ == "CARRY4" {
                    // Everything else is slice muxes (SITE pips) and the COUT ->
                    // CIN link, emitted where the route uses CYINIT = CIN.
                    assert!(routed, "{}: CARRY4 needs routed mode", c.name);
                    s.set("CARRY4", &[&c.name, ""]);
                    for (i, l) in ['A', 'B', 'C', 'D'].into_iter().enumerate() {
                        for port in [format!("O{i}"), format!("CO{i}")] {
                            if c.conns.contains_key(&port) {
                                cands.insert((ci, port), site_cand(format!("{l}MUX"), true));
                            }
                        }
                        // DI through the bypass pin when not from O5.
                        if c.conns.contains_key(&format!("DI{i}")) {
                            cands.insert((ci, format!("DI{i}")), site_cand(format!("{l}X"), false));
                        }
                    }
                    if c.conns.contains_key("CI") {
                        cands.insert((ci, "CI".into()), site_cand("AX".into(), false));
                    }
                    continue;
                }
                if c.typ.starts_with("SLICE_LUT") {
                    // <L>6LUT drives O6, <L>5LUT drives O5. A LUT6 sharing its
                    // site with a LUT5 has a 5-input table and A6 tied high.
                    let half = &sub[1..2];
                    let mut table = lut_table(&c.params["INIT"]);
                    let k = table.len().trailing_zeros() as usize;
                    let mut vars: Vec<usize> = (1..=k).collect();
                    for j in (1..=6).rev() {
                        let port = format!("A{j}");
                        if !c.conns.contains_key(&port) {
                            continue;
                        }
                        if j > k {
                            // A6 = 1 selecting O6's half of the table: just wire it.
                            if routed {
                                cands.insert((ci, port), site_cand(format!("{l}{j}"), false));
                            } else {
                                pins.insert((ci, port), (inst.clone(), format!("{l}{j}")));
                            }
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
                    if half == "6" && k <= 5 && c.conns.contains_key("A6") {
                        // Paired with a LUT5: ISE wants A6 in the O6 equation
                        // (O6 = A6 * f), else the tied-high A6 pin is "partially
                        // connected". The table's lower half is O5's anyway.
                        let n = table.len();
                        table = (0..64).map(|i| i >= 32 && table[i % n]).collect();
                        vars = (1..=6).collect();
                    }
                    let mut expr = lut_expr(&table, &vars);
                    if half == "6" && c.conns.contains_key("A6") && vars.len() == 6 && (expr == "0" || expr == "1") {
                        // A constant O6 next to a LUT5 must still use the
                        // tied-high A6: 1 = A6, 0 = ~A6.
                        expr = if expr == "1" { "A6".into() } else { "~A6".into() };
                    }
                    s.set(&format!("{l}{half}LUT"), &[&c.name, "#LUT", &format!("O{half}={expr}")]);
                    if let Some(&ni) = c.conns.get("O") {
                        if half == "5" {
                            // O5 leaves only on <L>MUX (OUTMUX=O5), or feeds a FF inside.
                            if routed {
                                cands.insert(
                                    (ci, "O".into()),
                                    Cand::Site { bcrd, pins: vec![format!("{l}MUX")], inst: inst.clone(), output: true },
                                );
                            } else {
                                let to_5ff = nets[ni].users.iter().all(|(u, p)| {
                                    p == "D" && cells[*u].bel == (x, y, format!("{slot}.{l}5FF"))
                                });
                                if !to_5ff {
                                    s.set(&format!("{l}OUTMUX"), &["", "O5"]);
                                    pins.insert((ci, "O".into()), (inst.clone(), format!("{l}MUX")));
                                }
                            }
                        } else if routed {
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
                } else if &sub[1..] == "5FF" {
                    // D is hardwired to O5; Q leaves on <L>MUX (OUTMUX=<L>5Q).
                    let ff_type = c.params["FF_TYPE"].as_str();
                    // The 5FF element is FF-only and has no cfg values in ISE.
                    s.set(&format!("{l}5FF"), &[&c.name, ""]);
                    let srinit = if matches!(ff_type, "FDSE" | "FDPE") { "SRINIT1" } else { "SRINIT0" };
                    s.set(&format!("{l}5FFSRINIT"), &["", srinit]);
                    s.set("SYNC_ATTR", &["", if matches!(ff_type, "FDRE" | "FDSE") { "SYNC" } else { "ASYNC" }]);
                    s.set("CLKINV", &["", if clk_inv(c) { "CLK_B" } else { "CLK" }]);
                    if routed {
                        cands.insert((ci, "Q".into()), site_cand(format!("{l}MUX"), true));
                        for (port, pin) in [("CK", "CLK"), ("CE", "CE"), ("SR", "SR")] {
                            if c.conns.contains_key(port) {
                                cands.insert((ci, port.into()), site_cand(pin.into(), false));
                            }
                        }
                        continue;
                    }
                    s.set(&format!("{l}OUTMUX"), &["", &format!("{l}5Q")]);
                    pins.insert((ci, "Q".into()), (inst.clone(), format!("{l}MUX")));
                    pins.insert((ci, "CK".into()), (inst.clone(), "CLK".into()));
                    if c.conns.get("CE").is_some_and(|&ni| net_const(ni) != Some(true)) {
                        s.set("CEUSED", &["", "0"]);
                        pins.insert((ci, "CE".into()), (inst.clone(), "CE".into()));
                    }
                    if c.conns.get("SR").is_some_and(|&ni| net_const(ni) != Some(false)) {
                        s.set("SRUSED", &["", "0"]);
                        pins.insert((ci, "SR".into()), (inst.clone(), "SR".into()));
                    }
                } else {
                    assert_eq!(&sub[1..], "FF", "{}: unexpected FF bel", c.name);
                    let ff_type = c.params["FF_TYPE"].as_str();
                    let latch = matches!(ff_type, "LDCE" | "LDPE");
                    s.set(&format!("{l}FF"), &[&c.name, if latch { "#LATCH" } else { "#FF" }]);
                    let srinit = if matches!(ff_type, "FDSE" | "FDPE" | "LDPE") { "SRINIT1" } else { "SRINIT0" };
                    s.set(&format!("{l}FFSRINIT"), &["", srinit]);
                    s.set("SYNC_ATTR", &["", if matches!(ff_type, "FDRE" | "FDSE") { "SYNC" } else { "ASYNC" }]);
                    s.set("CLKINV", &["", if clk_inv(c) { "CLK_B" } else { "CLK" }]);
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
                    // T from the OLOGIC's DDR tristate register (no T net here).
                    if c.params.contains_key("OLOGIC_DDR_T") {
                        cfg.push(vec!["TUSED".into(), "".into(), "0".into()]);
                    }
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
            "OLOGIC" => {
                // ODDR2: the OLOGIC2 site as a DDR output register (as ISE
                // writes it); it drives its pad (net added below).
                assert!(routed, "{}: OLOGIC needs routed mode", c.name);
                let bcrd = names.bel(x, y, belname);
                let (tile, site) = names.site(bcrd);
                let inst = c.name.clone();
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let kv = |k: &str, v: String| vec![k.to_string(), "".into(), v];
                let init = p("INIT").map(|v| if v.contains('1') { "1" } else { "0" }.to_string()).unwrap_or("0".into());
                let mut cfg = vec![
                    vec!["OUTFF".into(), inst.clone(), "".into()],
                    kv("CLK0INV", "CLK0".into()),
                    kv("CLK1INV", if c.params.contains_key("TWO_CLOCKS") { "CLK1" } else { "CLK1_B" }.into()),
                    kv("DDR_ALIGNMENT", p("DDR_ALIGNMENT").unwrap_or("NONE".into())),
                    kv("OMUX", "OUTFF".into()),
                    kv("OUTFFTYPE", "DDR".into()),
                    kv("SRINIT_OQ", init),
                    kv("SRTYPE_OQ", p("SRTYPE").unwrap_or("SYNC".into())),
                ];
                // A tristate ODDR2 on the same pad: the tristate register
                // (TFF) of the same site.
                let tff = p("TFF");
                if let Some(t) = &tff {
                    let tinit = p("T_INIT").map(|v| if v.contains('1') { "1" } else { "0" }.to_string()).unwrap_or("0".into());
                    cfg.push(vec!["TFF".into(), t.clone(), "".into()]);
                    cfg.push(kv("TDDR_ALIGNMENT", p("T_DDR_ALIGNMENT").unwrap_or("NONE".into())));
                    cfg.push(kv("TFFTYPE", "DDR".into()));
                    cfg.push(kv("TMUX", "TFF".into()));
                    cfg.push(kv("SRINIT_TQ", tinit));
                    cfg.push(kv("SRTYPE_TQ", p("T_SRTYPE").unwrap_or("SYNC".into())));
                    if c.conns.contains_key("SR") {
                        cfg.push(kv("TSRUSED", "0".into()));
                    }
                }
                for (port, used) in [
                    ("D1", "D1USED"), ("D2", "D2USED"), ("OCE", "OCEUSED"), ("SR", "OSRUSED"),
                    ("T1", "T1USED"), ("T2", "T2USED"), ("TCE", "TCEUSED"),
                ] {
                    if c.conns.contains_key(port) {
                        cfg.push(kv(used, "0".into()));
                        pins.insert((ci, port.into()), (inst.clone(), port.into()));
                        cands.insert((ci, port.into()), Cand::Site { bcrd, pins: vec![port.into()], inst: inst.clone(), output: false });
                    }
                }
                let pad = p("PAD").expect("OLOGIC without PAD");
                let k = belname.trim_start_matches("OLOGIC");
                ologic_pads.push((bcrd, inst.clone(), names.bel(x, y, &format!("IOB{k}")), pad, tff.is_some()));
                instances.push(Instance { name: inst, kind: "OLOGIC2".into(), placement: Placement::Placed { tile, site }, cfg });
            }
            "ILOGIC" => {
                // IDDR2: the ILOGIC2 site as a DDR input register (as ISE writes
                // it); it reads its pad (net added below).
                assert!(routed, "{}: ILOGIC needs routed mode", c.name);
                let bcrd = names.bel(x, y, belname);
                let (tile, site) = names.site(bcrd);
                let inst = c.name.clone();
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let kv = |k: &str, v: String| vec![k.to_string(), "".into(), v];
                let cfg = vec![
                    vec!["IFF".into(), inst.clone(), "".into()],
                    kv("CLK0INV", "CLK0".into()),
                    kv("CLK1INV", if c.params.contains_key("TWO_CLOCKS") { "CLK1" } else { "CLK1_B" }.into()),
                    kv("D2OBYP_SEL", "GND".into()),
                    kv("DDR_ALIGNMENT", p("DDR_ALIGNMENT").unwrap_or("NONE".into())),
                    kv("IFFMUX", "1".into()),
                    kv("IFFTYPE", "DDR".into()),
                    kv("SAME_EDGE_PIPELINED", "FALSE".into()),
                    kv("SRINIT_Q", "0".into()),
                    vec!["D2OBYPSEL_GND".into(), format!("{inst}.D2OBYPSEL_GND"), "".into()],
                    vec!["D2OFFBYP_SRC".into(), format!("{inst}.D2OFFBYP_SRC"), "".into()],
                ];
                for (port, output) in [("CE0", false), ("SR", false), ("REV", false), ("Q3", true), ("Q4", true)] {
                    if c.conns.contains_key(port) {
                        pins.insert((ci, port.into()), (inst.clone(), port.into()));
                        cands.insert((ci, port.into()), Cand::Site { bcrd, pins: vec![port.into()], inst: inst.clone(), output });
                    }
                }
                let pad = p("PAD").expect("ILOGIC without PAD");
                let k = belname.trim_start_matches("ILOGIC");
                ilogic_pads.push((bcrd, inst.clone(), names.bel(x, y, &format!("IOB{k}")), pad));
                instances.push(Instance { name: inst, kind: "ILOGIC2".into(), placement: Placement::Placed { tile, site }, cfg });
            }
            "IOI_DDR" => {
                // No ISE site: its clock inputs are the clock pins of the
                // ILOGIC2 / OLOGIC2 it clocks.
                let ddr = names.bel(x, y, belname);
                let k = belname.trim_start_matches("IOI_DDR");
                let logic = c.params.get("LOGIC").map(String::as_str).unwrap_or("OLOGIC");
                let (_, site) = names.site(names.bel(x, y, &format!("{logic}{k}")));
                let inst = c.params.get("OLOGIC").expect("IOI_DDR without OLOGIC").clone();
                for port in ["CLK0", "CLK1"] {
                    if c.conns.contains_key(port) {
                        cands.insert((ci, port.into()), Cand::DdrClk { ddr, port: port.into(), inst: inst.clone(), site: site.clone() });
                    }
                }
            }
            "DCM" | "PLL" | "BUFIO2" | "BUFIO2FB" => {
                assert!(routed, "{}: clock management needs routed mode", c.name);
                let bcrd = names.bel(x, y, belname);
                let (tile, site) = names.site(bcrd);
                let inst = c.name.clone();
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let num = |k: &str, d: u64| -> u64 {
                    p(k).map(|v| if v.len() > 8 && v.chars().all(|ch| ch == '0' || ch == '1') { u64::from_str_radix(&v, 2).unwrap() } else { v.parse().unwrap_or(d) }).unwrap_or(d)
                };
                let real = |k: &str, d: f64| -> f64 { p(k).and_then(|v| v.parse().ok()).unwrap_or(d) };
                let kv = |k: &str, v: String| vec![k.to_string(), "".into(), v];
                let inv = |pin: &str| -> Vec<String> {
                    kv(&format!("{pin}INV"), if c.conns.contains_key(pin) { pin.into() } else { "#OFF".into() })
                };
                let (prim, cfg) = match c.typ.as_str() {
                    "DCM" => {
                        let mut cfg = vec![vec!["DCM".into(), inst.clone(), "".into()]];
                        cfg.push(kv("CLKDV_DIVIDE", format!("{:.1}", real("CLKDV_DIVIDE", 2.0))));
                        cfg.push(kv("CLKIN_DIVIDE_BY_2", p("CLKIN_DIVIDE_BY_2").unwrap_or("FALSE".into())));
                        cfg.push(kv("CLKOUT_PHASE_SHIFT", p("CLKOUT_PHASE_SHIFT").unwrap_or("NONE".into())));
                        cfg.push(kv("CLK_FEEDBACK", p("CLK_FEEDBACK").unwrap_or("1X".into())));
                        for pin in ["CTLGO", "CTLSEL0", "CTLSEL1", "CTLSEL2", "PSCLK", "PSEN", "PSINCDEC", "RST", "SKEWIN", "SKEWRST"] {
                            cfg.push(inv(pin));
                        }
                        let deskew = match p("DESKEW_ADJUST").as_deref() {
                            None | Some("SYSTEM_SYNCHRONOUS") => "5".to_string(),
                            Some("SOURCE_SYNCHRONOUS") => "0".to_string(),
                            Some(v) => v.to_string(),
                        };
                        cfg.push(kv("DESKEW_ADJUST", deskew));
                        cfg.push(kv("DFS_FREQUENCY_MODE", p("DFS_FREQUENCY_MODE").unwrap_or("LOW".into())));
                        cfg.push(kv("DLL_FREQUENCY_MODE", p("DLL_FREQUENCY_MODE").unwrap_or("LOW".into())));
                        cfg.push(kv("DSS_MODE", p("DSS_MODE").unwrap_or("NONE".into())));
                        cfg.push(kv("DUTY_CYCLE_CORRECTION", p("DUTY_CYCLE_CORRECTION").unwrap_or("TRUE".into())));
                        cfg.push(kv("STARTUP_WAIT", p("STARTUP_WAIT").unwrap_or("FALSE".into())));
                        cfg.push(kv("VERY_HIGH_FREQUENCY", "FALSE".into()));
                        cfg.push(kv("CLKFX_DIVIDE", num("CLKFX_DIVIDE", 1).to_string()));
                        cfg.push(kv("CLKFX_MULTIPLY", num("CLKFX_MULTIPLY", 4).to_string()));
                        cfg.push(kv("CLKIN_PERIOD", format!("{:.6}", real("CLKIN_PERIOD", 10.0))));
                        // Signed (yosys passes the 32-bit two's complement).
                        cfg.push(kv("PHASE_SHIFT", (num("PHASE_SHIFT", 0) as u32 as i32).to_string()));
                        ("DCM", cfg)
                    }
                    "PLL" => {
                        let mut cfg = vec![vec!["PLL_ADV".into(), inst.clone(), "".into()]];
                        // ISE's own XDL: user-level settings plus the low-level
                        // fields at ISE's defaults (bitgen derives the real ones).
                        for (k, v) in PLL_XDL_DEFAULTS {
                            cfg.push(kv(k, v.to_string()));
                        }
                        cfg.push(kv("BANDWIDTH", p("BANDWIDTH").unwrap_or("OPTIMIZED".into())));
                        cfg.push(kv("CLK_FEEDBACK", p("CLK_FEEDBACK").unwrap_or("CLKFBOUT".into())));
                        cfg.push(kv("COMPENSATION", p("COMPENSATION").unwrap_or("SYSTEM_SYNCHRONOUS".into())));
                        for pin in ["CLKBRST", "CLKINSEL", "ENOUTSYNC", "MANPDLF", "MANPULF", "REL", "RST", "SKEWCLKIN1", "SKEWCLKIN2", "SKEWRST", "SKEWSTB"] {
                            cfg.push(inv(pin));
                        }
                        let period = real("CLKIN_PERIOD", real("CLKIN1_PERIOD", 10.0));
                        cfg.push(kv("CLKFBOUT_MULT", num("CLKFBOUT_MULT", 1).to_string()));
                        cfg.push(kv("CLKFBOUT_PHASE", format!("{:.1}", real("CLKFBOUT_PHASE", 0.0))));
                        cfg.push(kv("CLKIN1_PERIOD", format!("{}", real("CLKIN1_PERIOD", period))));
                        cfg.push(kv("CLKIN2_PERIOD", format!("{}", real("CLKIN2_PERIOD", period))));
                        for i in 0..6 {
                            cfg.push(kv(&format!("CLKOUT{i}_DIVIDE"), num(&format!("CLKOUT{i}_DIVIDE"), 1).to_string()));
                            cfg.push(kv(&format!("CLKOUT{i}_DUTY_CYCLE"), format!("{:.1}", real(&format!("CLKOUT{i}_DUTY_CYCLE"), 0.5))));
                            cfg.push(kv(&format!("CLKOUT{i}_PHASE"), format!("{:.1}", real(&format!("CLKOUT{i}_PHASE"), 0.0))));
                        }
                        cfg.push(kv("DIVCLK_DIVIDE", num("DIVCLK_DIVIDE", 1).to_string()));
                        cfg.push(kv("REF_JITTER", format!("{}", real("REF_JITTER", 0.1))));
                        ("PLL_ADV", cfg)
                    }
                    "BUFIO2FB" => {
                        // A PLL's BUFG feedback (inserted as ISE's map does).
                        let cfg = vec![
                            vec!["BUFIO2FB".into(), inst.clone(), "".into()],
                            kv("DIVIDE_BYPASS", "TRUE".into()),
                            kv("INVERT_INPUTS", "FALSE".into()),
                        ];
                        ("BUFIO2FB", cfg)
                    }
                    _ => {
                        let cfg = vec![
                            vec!["BUFIO2".into(), inst.clone(), "".into()],
                            kv("DIVIDE", "1".into()),
                            kv("DIVIDE_BYPASS", "TRUE".into()),
                            kv("I_INVERT", "FALSE".into()),
                        ];
                        ("BUFIO2", cfg)
                    }
                };
                for (port, _) in &c.conns {
                    let output = !matches!(
                        port.as_str(),
                        "I" | "CLKIN" | "CLKFB" | "RST" | "PSCLK" | "PSEN" | "PSINCDEC" | "CLKIN1" | "CLKIN2" | "CLKFBIN" | "CLKINSEL"
                    );
                    // BUFIO2's CMT output is the DIVCLK site pin in ISE (the
                    // DIVCLK_CMT naming entry is a wire further down the path,
                    // so name it as DIVCLK and let the repair add the buffers).
                    let xdl_pin = if port == "DIVCLK_CMT" { "DIVCLK".to_string() } else { port.clone() };
                    cands.insert(
                        (ci, port.clone()),
                        Cand::Pin { wire: (bcrd, port.clone()), naming: (bcrd, xdl_pin.clone()), xdl_pin, inst: inst.clone(), output },
                    );
                }
                instances.push(Instance { name: inst, kind: prim.into(), placement: Placement::Placed { tile, site }, cfg });
            }
            "DSP" => {
                assert!(routed, "{}: DSP needs routed mode", c.name);
                let bcrd = names.bel(x, y, belname);
                let (tile, site) = names.site(bcrd);
                let inst = c.name.clone();
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let flag = |k: &str| p(k).is_some_and(|v| v.chars().any(|ch| ch == '1'));
                let kv = |k: &str, v: String| vec![k.to_string(), "".into(), v];
                let mut cfg: Vec<Vec<String>> = vec![vec!["DSP48A1".into(), inst.clone(), "".into()]];
                for r in ["A0REG", "A1REG", "B0REG", "B1REG", "CARRYINREG", "CARRYOUTREG", "CREG", "DREG", "MREG", "OPMODEREG", "PREG"] {
                    cfg.push(kv(r, if flag(r) { "1" } else { "0" }.into()));
                }
                cfg.push(kv("B_INPUT", p("B_INPUT").unwrap_or("DIRECT".into())));
                cfg.push(kv("CARRYINSEL", p("CARRYINSEL").unwrap_or("OPMODE5".into())));
                cfg.push(kv("RSTTYPE", p("RSTTYPE").unwrap_or("SYNC".into())));
                for pin in [
                    "CLK", "CEA", "CEB", "CEC", "CED", "CEM", "CEP", "CEOPMODE", "CECARRYIN", "RSTA", "RSTB", "RSTC", "RSTD",
                    "RSTM", "RSTP", "RSTOPMODE", "RSTCARRYIN",
                ] {
                    cfg.push(kv(&format!("{pin}INV"), if c.conns.contains_key(pin) { pin.into() } else { "#OFF".into() }));
                }
                for (port, _) in &c.conns {
                    let output = port.starts_with('M') || port.starts_with('P') || port == "CARRYOUTF";
                    cands.insert((ci, port.clone()), Cand::Site { bcrd, pins: vec![port.clone()], inst: inst.clone(), output });
                }
                instances.push(Instance { name: inst, kind: "DSP48A1".into(), placement: Placement::Placed { tile, site }, cfg });
            }
            "BRAM" => {
                assert!(routed, "{}: block RAM needs routed mode", c.name);
                let kind = c.params.get("RAMB_KIND").map(String::as_str).unwrap_or("RAMB8");
                let bcrd = names.bel(x, y, belname);
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let num = |k: &str| -> usize {
                    p(k).map(|v| if v.len() > 8 && v.chars().all(|ch| ch == '0' || ch == '1') { usize::from_str_radix(&v, 2).unwrap() } else { v.parse().unwrap_or(0) }).unwrap_or(0)
                };
                // RAMB16's BRAM[1] cell: its pins belong to the RAMB16 instance
                // (named after the BRAM[0] cell), as RAMB16 pin names.
                let (inst, b0) = if kind == "RAMB16_HI" {
                    (c.name.trim_end_matches("$hi").to_string(), names.bel(x, y, "BRAM[0]"))
                } else {
                    (c.name.clone(), bcrd)
                };
                let xdl_pin = |port: &str| -> (String, String) {
                    // (naming key, XDL pin) for a cell port like "DIA[3]".
                    let digits = port.len() - port.trim_end_matches(|ch: char| ch.is_ascii_digit()).len();
                    let (base, idx) = if digits > 0 {
                        let (b, i) = port.split_at(port.len() - digits);
                        (b.to_string(), Some(i.parse::<usize>().unwrap()))
                    } else {
                        (port.to_string(), None)
                    };
                    match kind {
                        "RAMB8" => {
                            let r8 = match base.as_str() {
                                "ADDRA" => "ADDRAWRADDR", "ADDRB" => "ADDRBRDADDR", "DIA" => "DIADI", "DIB" => "DIBDI",
                                "DIPA" => "DIPADIP", "DIPB" => "DIPBDIP", "DOA" => "DOADO", "DOB" => "DOBDO",
                                "DOPA" => "DOPADOP", "DOPB" => "DOPBDOP", "WEA" => "WEAWEL", "WEB" => "WEBWEU",
                                "CLKA" => "CLKAWRCLK", "CLKB" => "CLKBRDCLK", "ENA" => "ENAWREN", "ENB" => "ENBRDEN",
                                "REGCEB" => "REGCEBREGCE", "RSTB" => "RSTBRST", other => other,
                            }
                            .to_string();
                            let n = idx.map(|i| i.to_string()).unwrap_or_default();
                            (format!("{base}{n}"), format!("{r8}{n}"))
                        }
                        _ => {
                            let hi = kind == "RAMB16_HI";
                            let off = match base.as_str() {
                                "ADDRA" | "ADDRB" => 13,
                                "DIA" | "DIB" | "DOA" | "DOB" => 16,
                                _ => 2,
                            };
                            let n = idx.map(|i| (i + if hi { off } else { 0 }).to_string()).unwrap_or_default();
                            let pin = format!("{base}{n}");
                            (format!("RAMB16_{pin}"), pin)
                        }
                    }
                };
                for (port, _) in &c.conns {
                    let (nkey, xpin) = xdl_pin(port);
                    let output = port.starts_with("DO");
                    let naming_bel = if kind == "RAMB8" { bcrd } else { b0 };
                    cands.insert(
                        (ci, port.clone()),
                        Cand::Pin { wire: (bcrd, port.clone()), naming: (naming_bel, nkey), xdl_pin: xpin, inst: inst.clone(), output },
                    );
                }
                if kind == "RAMB16_HI" {
                    continue;
                }
                let (tile, site) = if kind == "RAMB16" { names.site_n(bcrd, 1) } else { names.site(bcrd) };
                let prim = if kind == "RAMB16" { "RAMB16BWER" } else { "RAMB8BWER" };
                let mut cfg: Vec<Vec<String>> = vec![vec![prim.into(), inst.clone(), "".into()]];
                let kv = |k: &str, v: String| vec![k.to_string(), "".into(), v];
                let used = |pin: &str| c.conns.contains_key(pin);
                for ab in ["A", "B"] {
                    let w = num(&format!("DATA_WIDTH_{ab}"));
                    cfg.push(kv(&format!("DATA_WIDTH_{ab}"), w.to_string()));
                    cfg.push(kv(&format!("WRITE_MODE_{ab}"), p(&format!("WRITE_MODE_{ab}")).unwrap_or("WRITE_FIRST".into())));
                    cfg.push(kv(&format!("DO{ab}_REG"), num(&format!("DO{ab}_REG")).to_string()));
                    cfg.push(kv(&format!("EN_RSTRAM_{ab}"), p(&format!("EN_RSTRAM_{ab}")).unwrap_or("TRUE".into())));
                    cfg.push(kv(&format!("RST_PRIORITY_{ab}"), p(&format!("RST_PRIORITY_{ab}")).unwrap_or("SR".into())));
                    for attr in ["INIT", "SRVAL"] {
                        let bits = p(&format!("{attr}_{ab}")).unwrap_or_default();
                        let nbits: usize = if kind == "RAMB16" { 36 } else { 18 };
                        let v = u64::from_str_radix(if bits.is_empty() { "0" } else { &bits }, 2).unwrap_or(0);
                        cfg.push(kv(&format!("{attr}_{ab}"), format!("{v:0w$X}", w = nbits.div_ceil(4))));
                    }
                    // Inverter settings: the pin's own name when used, else off.
                    let pins: Vec<String> = if kind == "RAMB16" {
                        vec![format!("CLK{ab}"), format!("EN{ab}"), format!("REGCE{ab}"), format!("RST{ab}")]
                    } else {
                        vec![format!("CLK{ab}"), format!("EN{ab}"), format!("REGCE{ab}"), format!("RST{ab}")]
                    };
                    for pin in pins {
                        let (_, xpin) = xdl_pin(&pin);
                        cfg.push(kv(&format!("{xpin}INV"), if used(&pin) { xpin.clone() } else { "#OFF".into() }));
                    }
                    let nwe = if kind == "RAMB16" { 4 } else { 2 };
                    for i in 0..nwe {
                        let port = if kind == "RAMB16" && i >= 2 { None } else { Some(format!("WE{ab}{i}")) };
                        let (_, xpin) = if kind == "RAMB16" { (String::new(), format!("WE{ab}{i}")) } else { xdl_pin(&format!("WE{ab}{i}")) };
                        let is_used = match &port {
                            Some(pt) => used(pt),
                            None => hi_we_used(&cells, &nets, ci, &format!("WE{ab}{}", i - 2)),
                        };
                        cfg.push(kv(&format!("{xpin}INV"), if is_used { xpin.clone() } else { "#OFF".into() }));
                    }
                }
                cfg.push(kv("RAM_MODE", p("RAM_MODE").unwrap_or("TDP".into())));
                cfg.push(kv("RSTTYPE", p("RSTTYPE").unwrap_or("SYNC".into())));
                let (ninit, ninitp) = if kind == "RAMB16" { (0x40, 8) } else { (0x20, 4) };
                for (prefix, n) in [("INIT", ninit), ("INITP", ninitp)] {
                    for i in 0..n {
                        let key = format!("{prefix}_{i:02X}");
                        let bits = p(&key).unwrap_or_default();
                        let b: Vec<bool> = bits.chars().rev().map(|ch| ch == '1').collect();
                        let hex: String = (0..64)
                            .rev()
                            .map(|d| {
                                let nib = (0..4).fold(0u8, |a, k| a | (u8::from(*b.get(d * 4 + k).unwrap_or(&false)) << k));
                                char::from_digit(nib as u32, 16).unwrap().to_ascii_uppercase()
                            })
                            .collect();
                        cfg.push(kv(&key, hex));
                    }
                }
                instances.push(Instance { name: inst, kind: prim.into(), placement: Placement::Placed { tile, site }, cfg });
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
            // The DDR clock from an IOI_DDR to its OLOGIC has no ISE net (ISE
            // models the DDR clocking as the OLOGIC2's two clock pins).
            if n.driver.as_ref().is_some_and(|(dc, _)| cells[*dc].typ == "IOI_DDR") {
                continue;
            }
            // DSP cascade (PCOUT/BCOUT/CARRYOUT -> the next DSP's PCIN/BCIN/
            // CARRYIN): hardwired, one ISE pip in the lower DSP's tile.
            if let Some((dc, dp)) = &n.driver
                && cells[*dc].typ == "DSP"
                && (dp.starts_with("PCOUT") || dp.starts_with("BCOUT") || dp == "CARRYOUT")
            {
                let lower = &cells[*dc];
                let (lx, ly, lb) = &lower.bel;
                let (tile, _) = names.site(names.bel(*lx, *ly, lb));
                xnets.push(Net {
                    name: n.name.clone(),
                    typ: NetType::Plain,
                    outpins: vec![NetPin { inst_name: lower.name.clone(), pin: dp.clone() }],
                    inpins: n
                        .users
                        .iter()
                        .map(|(uc, up)| NetPin { inst_name: cells[*uc].name.clone(), pin: up.clone() })
                        .collect(),
                    pips: vec![NetPip {
                        tile,
                        wire_from: format!("{dp}_DSP48A1_SITE"),
                        wire_to: format!("{dp}_DSP48A1_B_SITE"),
                        dir: PipDirection::UniBuf,
                    }],
                    cfg: vec![],
                });
                continue;
            }
            // Each emitted item hangs off a canonical node, so constant nets can
            // be split per TIEOFF source.
            let mut items: Vec<(WireCoord, Item)> = vec![];
            let mut parent: HashMap<WireCoord, WireCoord> = HashMap::new();
            let mut reached: HashSet<WireCoord> = HashSet::new();
            let mut sources: HashSet<WireCoord> = HashSet::new();
            // Site input pins a slice mux reads (where the route enters the site).
            let mut consumed: HashSet<WireCoord> = HashSet::new();
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
                        // MUX_<L>DI1 = ALT is the letter above's data or its
                        // shift cascade; ISE names the two separately.
                        let val = if r[2].ends_with("DI1") && r[3] == "ALT" {
                            let l = r[2].trim_start_matches("MUX_").chars().next().unwrap();
                            let above = (l as u8 + 1) as char;
                            if src.ends_with("MC31") {
                                format!("{above}MC31")
                            } else if l == 'A' {
                                "BDI1".to_string()
                            } else {
                                "DX".to_string()
                            }
                        } else {
                            r[3].clone()
                        };
                        site_muxes.push((*x, *y, r[1].clone(), r[2].clone(), val));
                        if r[2] == "CYINIT" && r[3] == "CIN" {
                            // Dedicated carry: COUT of the slice below -> CIN here.
                            let here = names.bel(*x, *y, &r[1]);
                            let below = edev.bel_carry_prev(here).expect("CIN without a slice below");
                            let (_, here_site) = names.site(here);
                            let (_, below_site) = names.site(below);
                            let (bx, by) = (below.cell.col.to_idx(), below.cell.row.to_idx());
                            site_muxes.push((bx, by, r[1].clone(), "COUTUSED".into(), "0".into()));
                            let (_, ot, ow, _) = names.pin_naming(below, "COUT").expect("COUT naming");
                            let (_, it, iw, _) = names.pin_naming(here, "CIN").expect("CIN naming");
                            let key = canon(names.site_pin_wire(here, "A1").expect("slice wire"));
                            items.push((key, Item::Out(below_site, "COUT".into(), ot, ow)));
                            items.push((key, Item::In(here_site, "CIN".into(), it, iw)));
                        }
                        // A slice mux reading a site input pin (e.g. AX -> AFF_D) consumes it.
                        if let Some(w) = parse_wire_name(&edev, names.cell(*x, *y), src) {
                            reached.insert(canon(w));
                            consumed.insert(canon(w));
                        }
                        if let Some(w) = parse_wire_name(&edev, names.cell(*x, *y), dst) {
                            // A slice mux driving a site output pin (e.g. A5Q -> AMUX).
                            sources.insert(canon(w));
                        }
                    }
                    "BELOUT" => {}
                    "RT_ILOGIC" => {
                        // Through an unused ILOGIC: its CLK0 pin to its CFB0
                        // pin, as ISE routes a PLL's BUFG feedback (the
                        // placeholder site is added with the other route-
                        // throughs below). prjcombine
                        // doesn't name the I/O tile's clock wiring in front of
                        // the ILOGIC (IOI_IOCLK / IOI_ICLK / IMUX_ILOGIC_CLK),
                        // so take ISE's own path from the last named wire (the
                        // interconnect fan-out the route came from) to the
                        // site's CLK0 pin.
                        has_int_pips = true;
                        let bcrd = names.bel(*x, *y, &meta.src_ref[1]);
                        let (tile, site) = names.site(bcrd);
                        let (clk_t, clk_w) = raw.site_pin_wire(&site, "CLK0").expect("ILOGIC CLK0 pin");
                        let (cfb_t, cfb_w) = raw.site_pin_wire(&site, "CFB0").expect("ILOGIC CFB0 pin");
                        let mut up = src.clone();
                        let mut from = None;
                        let mut via = None; // the I/O clock line our route used
                        for _ in 0..8 {
                            let Some((_, _, _, usrc)) = n.route.iter().find(|(rx, ry, d, _)| (*rx, *ry) == (*x, *y) && *d == up) else { break };
                            if !ise_unnamed_ioi_clock(&up) {
                                break;
                            }
                            up = usrc.clone();
                            via = via.or_else(|| ioi_ioclk_ise_name(&up));
                            if !ise_unnamed_ioi_clock(&up) {
                                from = names.ise_wire(names.cell(*x, *y), &edev, &up);
                                break;
                            }
                        }
                        let (from_t, from_w) = from.unwrap_or_else(|| panic!("net {}: no named wire before ILOGIC {site} CLK", n.name));
                        let hop = |a: (&str, &str), b: (&str, &str)| {
                            raw.find_path(a, b, 4).unwrap_or_else(|| panic!("net {}: no ISE path from {}/{} to {}/{}", n.name, a.0, a.1, b.0, b.1))
                        };
                        let path = match &via {
                            Some(v) => {
                                let mut p = hop((&from_t, &from_w), (&clk_t, v));
                                p.extend(hop((&clk_t, v), (&clk_t, &clk_w)));
                                p
                            }
                            None => hop((&from_t, &from_w), (&clk_t, &clk_w)),
                        };
                        let cell = names.cell(*x, *y);
                        let raw_src = parse_wire_name(&edev, cell, src).expect("unknown src wire");
                        let raw_dst = parse_wire_name(&edev, cell, dst).expect("unknown dst wire");
                        let (cs, cd) = (canon(raw_src), canon(raw_dst));
                        parent.insert(cd, cs);
                        reached.insert(cd);
                        sources.insert(cs);
                        for p in path.into_iter().chain(std::iter::once((clk_t, clk_w, cfb_w))) {
                            items.push((cs, Item::Pip(p)));
                        }
                        let _ = cfb_t;
                        items.extend(names.trace(raw_src).into_iter().map(|p| (cs, Item::Pip(p))));
                        let _ = tile;
                    }
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
                            // Covered by the ILOGIC route-through's ISE path.
                            None if ise_unnamed_ioi_clock(dst) => {}
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
                        let wire = |pin: &String| canon(names.site_pin_wire(*bcrd, pin).unwrap_or_else(|| panic!("no wire for pin {pin}")));
                        // An input with alternative pins (DI1: its own X or DX):
                        // the route may pass through one pin's wire on the way to
                        // the other; the pin is the one a slice mux reads.
                        let entered = !*output && pins.len() > 1 && pins.iter().any(|p| consumed.contains(&wire(p)));
                        for pin in pins {
                            let w = names.site_pin_wire(*bcrd, pin).unwrap();
                            let c = canon(w);
                            let hit = if *output { sources.contains(&c) } else { reached.contains(&c) };
                            if !hit || (entered && !consumed.contains(&c)) {
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
                    Cand::Pin { wire, naming, xdl_pin, inst, output } => {
                        let w = names.site_pin_wire(wire.0, &wire.1).unwrap_or_else(|| panic!("no wire for {}", wire.1));
                        let c = canon(w);
                        let hit = if *output { sources.contains(&c) } else { reached.contains(&c) };
                        if hit {
                            let (pp, ptile, pname, _) =
                                names.pin_naming(naming.0, &naming.1).unwrap_or_else(|| panic!("no naming for {}", naming.1));
                            items.extend(pp.into_iter().map(|p| (c, Item::Pip(p))));
                            if !*output {
                                items.extend(names.trace(w).into_iter().map(|p| (c, Item::Pip(p))));
                            }
                            items.push((
                                c,
                                if *output {
                                    Item::Out(inst.clone(), xdl_pin.clone(), ptile, pname)
                                } else {
                                    Item::In(inst.clone(), xdl_pin.clone(), ptile, pname)
                                },
                            ));
                        }
                        hit
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
                    Cand::DdrClk { ddr, port, inst, site } => {
                        let w = names.site_pin_wire(*ddr, port).expect("IOI_DDR clock wire");
                        let c = canon(w);
                        let hit = reached.contains(&c);
                        if hit {
                            let up = *parent.get(&c).unwrap_or_else(|| panic!("{inst}: DDR clock has no upstream pip"));
                            let (from_t, from_w) = names.ise_wire_raw(up).unwrap_or_else(|| panic!("{inst}: DDR clock source has no ISE name"));
                            let local = db.int.wires[w.slot].name.clone();
                            let via = ioi_ioclk_ise_name(&local).unwrap_or_else(|| panic!("{inst}: DDR clock on {local}"));
                            let (site_pin, other) = if port == "CLK0" { ("CLK1", "CLK0") } else { ("CLK0", "CLK1") };
                            let (pt, pw) = raw.site_pin_wire(site, site_pin).expect("OLOGIC2 clock pin");
                            let mut path = raw
                                .find_path((&from_t, &from_w), (&pt, &via), 4)
                                .unwrap_or_else(|| panic!("{inst}: no ISE path from {from_w} to {via}"));
                            let (pin, tail) = match raw.find_path((&pt, &via), (&pt, &pw), 4) {
                                Some(t) => (site_pin, t),
                                None => {
                                    let (_, ow) = raw.site_pin_wire(site, other).expect("OLOGIC2 clock pin");
                                    (other, raw.find_path((&pt, &via), (&pt, &ow), 4).unwrap_or_else(|| panic!("{inst}: no ISE path from {via} to a clock pin")))
                                }
                            };
                            path.extend(tail);
                            let (_, pin_wire) = raw.site_pin_wire(site, pin).unwrap();
                            for p in path {
                                items.push((c, Item::Pip(p)));
                            }
                            items.push((c, Item::In(inst.clone(), pin.to_string(), pt.clone(), pin_wire)));
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
                let site_fed = matches!(port.as_str(), "D" | "CI" | "DI0" | "DI1" | "DI2" | "DI3" | "WE");
                let internal = (has_site_pips && site_fed && !is_driver) || (is_driver && !has_int_pips);
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

        // A pad feeding both the clock network (GCLK_I) and logic (I) is one
        // ISE net, driven by the IOB's I pin: merge the GCLK_I net into it.
        for c in &cells {
            let (Some(&i_net), Some(&g_net)) = (c.conns.get("I"), c.conns.get("GCLK_I")) else { continue };
            let (i_name, g_name) = (&nets[i_net].name, &nets[g_net].name);
            let Some(gi) = xnets.iter().position(|x| &x.name == g_name) else { continue };
            let g = xnets.remove(gi);
            let x = xnets.iter_mut().find(|x| &x.name == i_name).expect("pad's I net");
            for p in g.outpins {
                if !x.outpins.iter().any(|q| q.inst_name == p.inst_name && q.pin == p.pin) {
                    x.outpins.push(p);
                }
            }
            x.inpins.extend(g.inpins);
            for p in g.pips {
                if !x.pips.iter().any(|q| q.tile == p.tile && q.wire_from == p.wire_from && q.wire_to == p.wire_to) {
                    x.pips.push(p);
                }
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
                // A constant carry-in comes from a CYINITGND/CYINITVCC element.
                match val.as_str() {
                    "CONST_0" => s.set("CYINITGND", &[&format!("{site}.CYINITGND"), ""]),
                    "CONST_1" => s.set("CYINITVCC", &[&format!("{site}.CYINITVCC"), ""]),
                    _ => {}
                }
                ("PRECYINIT".into(), match val.as_str() { "CONST_0" => "0".into(), "CONST_1" => "1".into(), v => v.into() })
            } else if attr == "CYINIT" {
                // Implied by whether CIN is connected.
                continue;
            } else if attr == "COUTUSED" {
                ("COUTUSED".into(), val)
            } else if attr.starts_with("MUX_") && attr.ends_with("DI1") {
                // ISE names the ALT source: DX for C and B, BDI1 for A (the
                // only ALT uses the chip database offers outside cascades).
                let v = match (attr.as_str(), val.as_str()) {
                    ("MUX_CDI1" | "MUX_BDI1", "ALT") => "DX".to_string(),
                    ("MUX_ADI1", "ALT") => "BDI1".to_string(),
                    _ => val,
                };
                (format!("{}DI1MUX", l(&attr, "MUX_")), v)
            } else if attr == "MUX_WE" {
                ("WEMUX".into(), val)
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
        // Each IDDR2's ILOGIC reads its pad: IOB I -> ILOGIC D.
        for (ilogic, inst, iob, pad) in &ilogic_pads {
            let (iob_pips, iob_tile, _, iob_far) = names.pin_naming(*iob, "I").expect("IOB I naming");
            let (_, ioi_tile, d_name, _) = names.pin_naming(*ilogic, "D").expect("ILOGIC D naming");
            let hop = raw
                .find_path((&iob_tile, &iob_far), (&ioi_tile, &d_name), 4)
                .unwrap_or_else(|| panic!("{inst}: no ISE path from {iob_far} to {d_name}"));
            xnets.push(Net {
                name: format!("{pad}_D"),
                typ: NetType::Plain,
                outpins: vec![NetPin { inst_name: pad.clone(), pin: "I".into() }],
                inpins: vec![NetPin { inst_name: inst.clone(), pin: "D".into() }],
                pips: iob_pips
                    .into_iter()
                    .chain(hop)
                    .map(|(tile, wire_from, wire_to)| NetPip { tile, wire_from, wire_to, dir: PipDirection::UniBuf })
                    .collect(),
                cfg: vec![],
            });
        }
        // Each ODDR2's OLOGIC drives its pad: OQ -> IOB O, and with a
        // tristate ODDR2, TQ -> IOB T.
        for (ologic, inst, iob, pad, tff) in &ologic_pads {
            let links: &[(&str, &str)] = if *tff { &[("OQ", "O"), ("TQ", "T")] } else { &[("OQ", "O")] };
            for &(out, inp) in links {
                let (_, ioi_tile, q_name, _) = names.pin_naming(*ologic, out).expect("OLOGIC output naming");
                let (iob_pips, iob_tile, _, iob_far) = names.pin_naming(*iob, inp).expect("IOB input naming");
                let hop = raw
                    .find_path((&ioi_tile, &q_name), (&iob_tile, &iob_far), 4)
                    .unwrap_or_else(|| panic!("{inst}: no ISE path from {q_name} to {iob_far}"));
                xnets.push(Net {
                    name: format!("{pad}_{out}"),
                    typ: NetType::Plain,
                    outpins: vec![NetPin { inst_name: inst.clone(), pin: out.into() }],
                    inpins: vec![NetPin { inst_name: pad.clone(), pin: inp.into() }],
                    pips: hop
                        .into_iter()
                        .chain(iob_pips)
                        .map(|(tile, wire_from, wire_to)| NetPip { tile, wire_from, wire_to, dir: PipDirection::UniBuf })
                        .collect(),
                    cfg: vec![],
                });
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

/// PLL_ADV settings ISE's XDL lists at their defaults; bitgen derives the
/// real values from the user-level ones (CLKFBOUT_MULT, CLKOUTn_DIVIDE, ...).
const PLL_XDL_DEFAULTS: &[(&str, &str)] = &[
    ("PLL_ADD_LEAKAGE", "2"),
    ("PLL_AVDD_COMP_SET", "2"),
    ("PLL_CLAMP_BYPASS", "FALSE"),
    ("PLL_CLAMP_REF_SEL", "1"),
    ("PLL_CLK0MX", "0"),
    ("PLL_CLK1MX", "0"),
    ("PLL_CLK2MX", "0"),
    ("PLL_CLK3MX", "0"),
    ("PLL_CLK4MX", "0"),
    ("PLL_CLK5MX", "0"),
    ("PLL_CLKBURST_CNT", "0"),
    ("PLL_CLKBURST_ENABLE", "TRUE"),
    ("PLL_CLKCNTRL", "0"),
    ("PLL_CLKFBMX", "0"),
    ("PLL_CLKFBOUT2_EDGE", "TRUE"),
    ("PLL_CLKFBOUT2_NOCOUNT", "TRUE"),
    ("PLL_CLKFBOUT_EDGE", "TRUE"),
    ("PLL_CLKFBOUT_EN", "FALSE"),
    ("PLL_CLKFBOUT_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT0_EDGE", "TRUE"),
    ("PLL_CLKOUT0_EN", "FALSE"),
    ("PLL_CLKOUT0_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT1_EDGE", "TRUE"),
    ("PLL_CLKOUT1_EN", "FALSE"),
    ("PLL_CLKOUT1_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT2_EDGE", "TRUE"),
    ("PLL_CLKOUT2_EN", "FALSE"),
    ("PLL_CLKOUT2_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT3_EDGE", "TRUE"),
    ("PLL_CLKOUT3_EN", "FALSE"),
    ("PLL_CLKOUT3_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT4_EDGE", "TRUE"),
    ("PLL_CLKOUT4_EN", "FALSE"),
    ("PLL_CLKOUT4_NOCOUNT", "TRUE"),
    ("PLL_CLKOUT5_EDGE", "TRUE"),
    ("PLL_CLKOUT5_EN", "FALSE"),
    ("PLL_CLKOUT5_NOCOUNT", "TRUE"),
    ("PLL_CLK_LOST_DETECT", "FALSE"),
    ("PLL_CP", "1"),
    ("PLL_CP_BIAS_TRIP_SHIFT", "TRUE"),
    ("PLL_CP_REPL", "1"),
    ("PLL_CP_RES", "0"),
    ("PLL_DIRECT_PATH_CNTRL", "TRUE"),
    ("PLL_DIVCLK_EDGE", "TRUE"),
    ("PLL_DIVCLK_NOCOUNT", "TRUE"),
    ("PLL_DVDD_COMP_SET", "2"),
    ("PLL_EN", "FALSE"),
    ("PLL_EN_DLY", "TRUE"),
    ("PLL_EN_LEAKAGE", "2"),
    ("PLL_EN_TCLK0", "TRUE"),
    ("PLL_EN_TCLK1", "TRUE"),
    ("PLL_EN_TCLK2", "TRUE"),
    ("PLL_EN_TCLK3", "TRUE"),
    ("PLL_EN_VCO0", "FALSE"),
    ("PLL_EN_VCO1", "FALSE"),
    ("PLL_EN_VCO2", "FALSE"),
    ("PLL_EN_VCO3", "FALSE"),
    ("PLL_EN_VCO4", "FALSE"),
    ("PLL_EN_VCO5", "FALSE"),
    ("PLL_EN_VCO6", "FALSE"),
    ("PLL_EN_VCO7", "FALSE"),
    ("PLL_EN_VCO_DIV1", "FALSE"),
    ("PLL_EN_VCO_DIV6", "TRUE"),
    ("PLL_INTFB", "0"),
    ("PLL_IO_CLKSRC", "0"),
    ("PLL_LFHF", "3"),
    ("PLL_LOCK_FB_DLY", "3"),
    ("PLL_LOCK_REF_DLY", "5"),
    ("PLL_MAN_LF_EN", "TRUE"),
    ("PLL_NBTI_EN", "TRUE"),
    ("PLL_PFD_CNTRL", "8"),
    ("PLL_PFD_DLY", "1"),
    ("PLL_PWRD_CFG", "FALSE"),
    ("PLL_REG_INPUT", "TRUE"),
    ("PLL_RES", "1"),
    ("PLL_SEL_SLIPD", "FALSE"),
    ("PLL_SKEW_CNTRL", "0"),
    ("PLL_TEST_IN_WINDOW", "FALSE"),
    ("PLL_VDD_SEL", "0"),
    ("PLL_VLFHIGH_DIS", "TRUE"),
    ("PLL_CLKFBOUT2_DT", "000000"),
    ("PLL_CLKFBOUT2_HT", "000001"),
    ("PLL_CLKFBOUT2_LT", "000001"),
    ("PLL_CLKFBOUT_DT", "000000"),
    ("PLL_CLKFBOUT_HT", "000001"),
    ("PLL_CLKFBOUT_LT", "000001"),
    ("PLL_CLKFBOUT_PM", "000"),
    ("PLL_CLKOUT0_DT", "000000"),
    ("PLL_CLKOUT0_HT", "000001"),
    ("PLL_CLKOUT0_LT", "000001"),
    ("PLL_CLKOUT0_PM", "000"),
    ("PLL_CLKOUT1_DT", "000000"),
    ("PLL_CLKOUT1_HT", "000001"),
    ("PLL_CLKOUT1_LT", "000001"),
    ("PLL_CLKOUT1_PM", "000"),
    ("PLL_CLKOUT2_DT", "000000"),
    ("PLL_CLKOUT2_HT", "000001"),
    ("PLL_CLKOUT2_LT", "000001"),
    ("PLL_CLKOUT2_PM", "000"),
    ("PLL_CLKOUT3_DT", "000000"),
    ("PLL_CLKOUT3_HT", "000001"),
    ("PLL_CLKOUT3_LT", "000001"),
    ("PLL_CLKOUT3_PM", "000"),
    ("PLL_CLKOUT4_DT", "000000"),
    ("PLL_CLKOUT4_HT", "000001"),
    ("PLL_CLKOUT4_LT", "000001"),
    ("PLL_CLKOUT4_PM", "000"),
    ("PLL_CLKOUT5_DT", "000000"),
    ("PLL_CLKOUT5_HT", "000001"),
    ("PLL_CLKOUT5_LT", "000001"),
    ("PLL_CLKOUT5_PM", "000"),
    ("PLL_DIVCLK_DT", "000000"),
    ("PLL_DIVCLK_HT", "000001"),
    ("PLL_DIVCLK_LT", "000001"),
    ("PLL_EN_CNTRL", "0000000000100010011110001110011000011110101000101111110010111110100001000010000000000"),
    ("PLL_IN_DLY_MX_SEL", "00000"),
    ("PLL_IN_DLY_SET", "000000000"),
    ("PLL_LOCK_CNT", "224"),
    ("PLL_LOCK_SAT_HIGH", "255"),
    ("PLL_MISC", "0000"),
    ("PLL_OPT_INV", "000000"),
    ("PLL_UNLOCK_CNT", "96"),
];

/// Whether the BRAM[1] partner of RAMB16 cell `ci` uses `port`.
fn hi_we_used(cells: &[s6_export::nextpnr::NCell], _nets: &[s6_export::nextpnr::NNet], ci: usize, port: &str) -> bool {
    let hi = format!("{}$hi", cells[ci].name);
    cells.iter().any(|c| c.name == hi && c.conns.contains_key(port))
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


/// CLK_INV = 1 (nextpnr): the slice's clock is inverted (CLKINV = CLK_B).
fn clk_inv(c: &s6_export::nextpnr::NCell) -> bool {
    c.params.get("CLK_INV").is_some_and(|v| v.chars().any(|ch| ch == '1'))
}

/// I/O tile clock wiring in front of the ILOGIC that prjcombine doesn't name
/// in ISE terms (the ILOGIC route-through supplies ISE's own path).
fn ise_unnamed_ioi_clock(wire: &str) -> bool {
    let w = wire.rsplit('.').next().unwrap_or(wire);
    w.starts_with("IOI_IOCLK[") || w.starts_with("IOI_ICLK[") || w.starts_with("IMUX_ILOGIC_CLK[")
}

/// ISE name of an I/O tile clock line (prjcombine IOI_IOCLK[k]): k = 0, 1 are
/// CLK0/CLK1 of the S side, 3, 4 of the M side (2 and 5 are PLL clocks).
fn ioi_ioclk_ise_name(wire: &str) -> Option<String> {
    let w = wire.rsplit('.').next().unwrap_or(wire);
    let k: usize = w.strip_prefix("IOI_IOCLK[")?.strip_suffix(']')?.parse().ok()?;
    match k {
        0 | 1 => Some(format!("IOI_CLK{k}INTER_S")),
        3 | 4 => Some(format!("IOI_CLK{}INTER_M", k - 3)),
        _ => None,
    }
}
