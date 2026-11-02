//! Export one Spartan-6 device from the prjcombine database as JSON.
//!
//! The JSON is consumed by `gen/s6_gen.py`, which builds the nextpnr himbaechel
//! chip database (.bba). Splitting the work this way keeps the prjcombine
//! dependency in Rust, and the nextpnr `.bba` layout in nextpnr's own Python
//! generator library, so neither side has to reimplement the other.
//!
//! Mapping from prjcombine to himbaechel:
//! - one himbaechel tile per prjcombine cell (col, row);
//! - a himbaechel tile type is the set of prjcombine tile classes anchored at
//!   that cell (e.g. INT + CLEXL);
//! - tile wires are the tile-class wires referenced by pips or bel pins;
//! - nodes are groups of tile wires that prjcombine resolves to the same
//!   canonical wire (`ExpandedGrid::resolve_wire`);
//! - pips come from switchbox items, one pip per source/destination pair.

mod io;
mod slice;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use clap::Parser;
use prjcombine_entity::EntityId;
use prjcombine_interconnect::db::{
    BelInfo, BelInput, BelKind, BelSlotId, IntDb, SwitchBoxItem, TileClass, TileClassId, TileSlotId,
    TileWireCoord, WireKind,
};
use prjcombine_interconnect::grid::{CellCoord, DieId, TileCoord, WireCoord};
use prjcombine_spartan6::db::Database;
use prjcombine_spartan6::defs::bslots;
use s6_export::{pin_name, wire_name};
use serde::Serialize;

#[derive(Parser)]
#[command(about = "Export a Spartan-6 device from prjcombine as JSON for nextpnr")]
struct Args {
    /// Path to prjcombine's databases/spartan6.zstd
    #[arg(long)]
    db: PathBuf,
    /// Device name, e.g. xc6slx9
    #[arg(long, default_value = "xc6slx9")]
    device: String,
    /// Output JSON path
    #[arg(long)]
    out: PathBuf,
}

#[derive(Serialize)]
struct OutWire {
    name: String,
    /// prjcombine wire kind (e.g. "MUX", "BRANCH", "REGIONAL GCLK").
    kind: String,
    /// "GND" / "VCC" / "PULLUP" for tie wires, else empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    const_value: String,
}

#[derive(Serialize)]
struct OutPip {
    src: usize,
    dst: usize,
    /// Switchbox item kind: MUX, PROGBUF, PERMABUF, PASS, BIPASS, PROGINV,
    /// PROGDELAY, PAIRMUX, or BELOUT for synthesized bel-output fan-out pips.
    kind: &'static str,
    /// The source is used inverted (only possible on some MUX inputs).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    inv: bool,
    /// Where the pip comes from in prjcombine: [tile slot, bel slot, item index].
    /// Empty for synthesized pips. Kept for the bitstream writer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    src_ref: Vec<String>,
}

#[derive(Serialize)]
struct OutBelPin {
    name: String,
    wire: usize,
    dir: &'static str,
    /// Pin sees its wire inverted (fixed inversion in silicon).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    inv: bool,
    /// Pin has a programmable inverter (configured via a bel bit).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    invertible: bool,
}

#[derive(Serialize)]
struct OutBel {
    name: String,
    #[serde(rename = "type")]
    bel_type: String,
    /// prjcombine bel class (differs from `type` only for slices).
    class: String,
    z: usize,
    /// For slice sub-bels: 1 = SLICEX, 2 = SLICEL, 3 = SLICEM. 0 otherwise.
    #[serde(skip_serializing_if = "is_zero")]
    site_kind: u32,
    tile_slot: String,
    pins: Vec<OutBelPin>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Serialize)]
struct OutTileType {
    name: String,
    /// The prjcombine tile classes merged into this type, as [slot, class].
    classes: Vec<[String; 2]>,
    wires: Vec<OutWire>,
    pips: Vec<OutPip>,
    bels: Vec<OutBel>,
}

#[derive(Serialize)]
struct OutTile {
    x: usize,
    y: usize,
    #[serde(rename = "type")]
    tile_type: usize,
    name: String,
    /// For tiles with a carry-capable slice: location of the slice feeding CIN.
    #[serde(skip_serializing_if = "Option::is_none")]
    carry_prev: Option<[usize; 2]>,
}

#[derive(Serialize)]
struct OutPad {
    pin: String,
    /// prjcombine pad names bonded to this pin (several for shared power pins).
    pads: Vec<String>,
    /// Present when the pin is a single user I/O pad.
    #[serde(skip_serializing_if = "Option::is_none")]
    io: Option<OutIo>,
}

#[derive(Serialize)]
struct OutIo {
    x: usize,
    y: usize,
    bel: String,
    bank: u32,
}

#[derive(Serialize)]
struct OutPackage {
    name: String,
    pins: Vec<OutPad>,
}

#[derive(Serialize)]
struct Output {
    prjcombine_rev: &'static str,
    device: String,
    width: usize,
    height: usize,
    speed_grades: Vec<String>,
    tile_types: Vec<OutTileType>,
    tiles: Vec<OutTile>,
    /// Each node is a list of [x, y, wire index]; the first entry is the root.
    nodes: Vec<Vec<[usize; 3]>>,
    packages: Vec<OutPackage>,
}

const PRJCOMBINE_REV: &str = "2b3317db3799b82b3ef5606438145ba76b9b4485";

/// A tile type under construction.
struct TypeBuilder {
    out: OutTileType,
    wire_idx: HashMap<String, usize>,
    /// For each local wire, the prjcombine tile wires it stands for. Empty for
    /// synthesized wires that exist only inside this tile.
    sources: Vec<Vec<(TileSlotId, TileWireCoord)>>,
}

impl TypeBuilder {
    fn new(name: String) -> Self {
        TypeBuilder {
            out: OutTileType {
                name,
                classes: vec![],
                wires: vec![],
                pips: vec![],
                bels: vec![],
            },
            wire_idx: HashMap::new(),
            sources: vec![],
        }
    }

    fn wire(&mut self, db: &IntDb, slot: TileSlotId, tcls: &TileClass, tcw: TileWireCoord) -> usize {
        let name = wire_name(db, tcls, tcw);
        let idx = match self.wire_idx.get(&name) {
            Some(&i) => i,
            None => {
                let kind = db.wires[tcw.wire].kind;
                let const_value = match kind {
                    WireKind::Tie0 => "GND",
                    WireKind::Tie1 => "VCC",
                    WireKind::TiePullup => "PULLUP",
                    _ => "",
                };
                let i = self.out.wires.len();
                self.out.wires.push(OutWire {
                    name: name.clone(),
                    kind: kind.to_string(db),
                    const_value: const_value.into(),
                });
                self.sources.push(vec![]);
                self.wire_idx.insert(name, i);
                i
            }
        };
        if !self.sources[idx].contains(&(slot, tcw)) {
            self.sources[idx].push((slot, tcw));
        }
        idx
    }

    fn pseudo_wire(&mut self, name: String) -> usize {
        assert!(!self.wire_idx.contains_key(&name), "duplicate pseudo wire {name}");
        let i = self.out.wires.len();
        self.out.wires.push(OutWire {
            name: name.clone(),
            kind: "PSEUDO".into(),
            const_value: String::new(),
        });
        self.sources.push(vec![]);
        self.wire_idx.insert(name, i);
        i
    }

    /// Get or create a tile-local constant source wire ("GND" or "VCC").
    /// router2 ends constant nets on any wire with a matching const_value,
    /// so these need no driver.
    fn const_wire(&mut self, value: &str) -> usize {
        let name = format!("CONST_{value}");
        if let Some(&i) = self.wire_idx.get(&name) {
            return i;
        }
        let i = self.out.wires.len();
        self.out.wires.push(OutWire {
            name: name.clone(),
            kind: "CONST".into(),
            const_value: value.into(),
        });
        self.sources.push(vec![]);
        self.wire_idx.insert(name, i);
        i
    }

    fn pip(&mut self, src: usize, dst: usize, kind: &'static str, inv: bool, src_ref: Vec<String>) {
        // Factory-test wiring (OUT_TEST, CMT_TEST_CLK, TEST_PLL_CLKIN) is not
        // meant for designs; left in, router2 used it to detour global clocks
        // through the PLL's test clock path.
        if is_test_wire(&self.out.wires[src].name) || is_test_wire(&self.out.wires[dst].name) {
            return;
        }
        self.out.pips.push(OutPip { src, dst, kind, inv, src_ref });
    }
}

/// nextpnr bel type. prjcombine has one SLICE class, but the packer must tell
/// the three slice kinds apart. Only SLICE[0] is on the carry chain (see
/// prjcombine's `bel_carry_prev`), so SLICE[1] is SLICEX, and SLICE[0] is
/// SLICEM in CLEXM tiles and SLICEL in CLEXL tiles.
fn bel_type(bslot: BelSlotId, class: &str, tcls: &str) -> String {
    if class != "SLICE" {
        return class.to_string();
    }
    if bslot == bslots::SLICE[1] {
        "SLICEX".into()
    } else if bslot == bslots::SLICE[0] && tcls == "CLEXM" {
        "SLICEM".into()
    } else if bslot == bslots::SLICE[0] && tcls == "CLEXL" {
        "SLICEL".into()
    } else {
        panic!("unexpected slice {bslot:?} in tile class {tcls}")
    }
}

fn is_test_wire(local_name: &str) -> bool {
    // Local names may carry a "CLASS.CELL[n]." prefix; test only the wire part.
    local_name.rsplit('.').next().unwrap_or(local_name).contains("TEST")
}

fn build_type(db: &IntDb, key: &[(TileSlotId, TileClassId)]) -> TypeBuilder {
    let name = key
        .iter()
        .map(|&(_, tcid)| db.tile_classes[tcid].name.as_str())
        .collect::<Vec<_>>()
        .join("+");
    let mut tb = TypeBuilder::new(name);
    for &(slot, tcid) in key {
        let tcls = &db.tile_classes[tcid];
        let slot_name = db.tile_slots[slot].clone();
        tb.out.classes.push([slot_name.clone(), tcls.name.clone()]);
        for (bslot, info) in &tcls.bels {
            let bslot_name = db.bel_slots[bslot].name.clone();
            match info {
                BelInfo::SwitchBox(sb) => {
                    for (item_idx, item) in sb.items.iter().enumerate() {
                        let r = || vec![slot_name.clone(), bslot_name.clone(), item_idx.to_string()];
                        match item {
                            SwitchBoxItem::Mux(m) => {
                                let dst = tb.wire(db, slot, tcls, m.dst);
                                for src in m.src.keys() {
                                    let s = tb.wire(db, slot, tcls, src.tw);
                                    tb.pip(s, dst, "MUX", src.inv, r());
                                }
                            }
                            SwitchBoxItem::ProgBuf(b) => {
                                let dst = tb.wire(db, slot, tcls, b.dst);
                                let s = tb.wire(db, slot, tcls, b.src.tw);
                                tb.pip(s, dst, "PROGBUF", b.src.inv, r());
                            }
                            SwitchBoxItem::PermaBuf(b) => {
                                let dst = tb.wire(db, slot, tcls, b.dst);
                                let s = tb.wire(db, slot, tcls, b.src.tw);
                                tb.pip(s, dst, "PERMABUF", b.src.inv, r());
                            }
                            SwitchBoxItem::Pass(p) => {
                                let dst = tb.wire(db, slot, tcls, p.dst);
                                let s = tb.wire(db, slot, tcls, p.src);
                                tb.pip(s, dst, "PASS", false, r());
                            }
                            SwitchBoxItem::BiPass(p) => {
                                let a = tb.wire(db, slot, tcls, p.a);
                                let b = tb.wire(db, slot, tcls, p.b);
                                tb.pip(a, b, "BIPASS", false, r());
                                tb.pip(b, a, "BIPASS", false, r());
                            }
                            SwitchBoxItem::ProgInv(p) => {
                                let dst = tb.wire(db, slot, tcls, p.dst);
                                let s = tb.wire(db, slot, tcls, p.src);
                                tb.pip(s, dst, "PROGINV", false, r());
                            }
                            SwitchBoxItem::ProgDelay(p) => {
                                let dst = tb.wire(db, slot, tcls, p.dst);
                                let s = tb.wire(db, slot, tcls, p.src.tw);
                                tb.pip(s, dst, "PROGDELAY", p.src.inv, r());
                            }
                            SwitchBoxItem::PairMux(p) => {
                                let dsts = [tb.wire(db, slot, tcls, p.dst[0]), tb.wire(db, slot, tcls, p.dst[1])];
                                for srcs in p.src.keys() {
                                    for (i, src) in srcs.iter().enumerate() {
                                        if let Some(src) = src {
                                            let s = tb.wire(db, slot, tcls, src.tw);
                                            tb.pip(s, dsts[i], "PAIRMUX", src.inv, r());
                                        }
                                    }
                                }
                            }
                            // Not routing: configuration side effects, handled by the bitstream writer.
                            SwitchBoxItem::Bidi(_) | SwitchBoxItem::WireSupport(_) => {}
                        }
                    }
                }
                BelInfo::Bel(bel) => {
                    let BelKind::Class(bcid) = db.bel_slots[bslot].kind else {
                        unreachable!()
                    };
                    let bcls = &db.bel_classes[bcid];
                    let mut pins = vec![];
                    for (pid, inp) in &bel.inputs {
                        let (base, idx) = bcls.inputs.key(pid);
                        let ix = bcls.inputs[pid].indexing;
                        let name = pin_name(base, idx, |i| ix.phys_to_virt(i));
                        let (tw, inv, invertible) = match *inp {
                            BelInput::Fixed(pw) => (pw.tw, pw.inv, false),
                            BelInput::Invertible(tw, _) => (tw, false, true),
                        };
                        let wire = tb.wire(db, slot, tcls, tw);
                        pins.push(OutBelPin { name, wire, dir: "IN", inv, invertible });
                    }
                    for (pid, wires) in &bel.outputs {
                        let (base, idx) = bcls.outputs.key(pid);
                        let ix = bcls.outputs[pid].indexing;
                        let name = pin_name(base, idx, |i| ix.phys_to_virt(i));
                        let wire = if wires.len() == 1 {
                            tb.wire(db, slot, tcls, *wires.iter().next().unwrap())
                        } else {
                            // A bel pin drives several wires (e.g. PLL LOCKED). nextpnr
                            // bel pins have one wire, so fan out through always-on pips.
                            let pw = tb.pseudo_wire(format!("{bslot_name}.{name}"));
                            for &w in wires {
                                let dst = tb.wire(db, slot, tcls, w);
                                tb.pip(pw, dst, "BELOUT", false, vec![]);
                            }
                            pw
                        };
                        pins.push(OutBelPin { name, wire, dir: "OUT", inv: false, invertible: false });
                    }
                    for (pid, &w) in &bel.bidirs {
                        let (base, idx) = bcls.bidirs.key(pid);
                        let ix = bcls.bidirs[pid].indexing;
                        let name = pin_name(base, idx, |i| ix.phys_to_virt(i));
                        let wire = tb.wire(db, slot, tcls, w);
                        pins.push(OutBelPin { name, wire, dir: "INOUT", inv: false, invertible: false });
                    }
                    let bel_type = bel_type(bslot, bcls.name.as_str(), tcls.name.as_str());
                    if bcls.name == "SLICE" {
                        // Replace the site-level slice with LUT/FF/mux/carry sub-bels.
                        slice::expand(&mut tb, db, &bslot_name, &bel_type, bcls, bel, &slot_name, &pins);
                        continue;
                    }
                    tb.out.bels.push(OutBel {
                        bel_type,
                        class: bcls.name.clone(),
                        name: bslot_name,
                        z: bslot.to_idx(),
                        site_kind: 0,
                        tile_slot: slot_name.clone(),
                        pins,
                    });
                }
                // Factory test muxes. In normal operation each destination is
                // driven by its primary source; that path is how bel outputs
                // reach the interconnect in INTF tiles, so export it as an
                // always-on pip. The test-mode sources are never used.
                BelInfo::TestMux(tm) => {
                    for (item_idx, (&dst, tw)) in tm.wires.iter().enumerate() {
                        let d = tb.wire(db, slot, tcls, dst);
                        let s = tb.wire(db, slot, tcls, tw.primary_src.tw);
                        let r = vec![slot_name.clone(), bslot_name.clone(), item_idx.to_string()];
                        tb.pip(s, d, "TESTMUX", tw.primary_src.inv, r);
                    }
                }
            }
        }
    }
    io::merge_iob_paths(&mut tb);
    tb
}

fn main() {
    let args = Args::parse();
    let db = Database::from_file(&args.db).expect("failed to read prjcombine database");
    let dev = db
        .devices
        .iter()
        .find(|d| d.name == args.device)
        .unwrap_or_else(|| panic!("device {} not in database", args.device));
    let chip = &db.chips[dev.chip];
    let int = &db.int;
    let edev = chip.expand_grid(int, &dev.disabled);
    let die = DieId::from_idx(0);
    let width = edev.cols(die).into_iter().count();
    let height = edev.rows(die).into_iter().count();

    // Build tile types, deduplicated by the set of tile classes at each cell.
    let mut type_idx: BTreeMap<Vec<(TileSlotId, TileClassId)>, usize> = BTreeMap::new();
    let mut types: Vec<TypeBuilder> = vec![];
    let mut tiles = vec![];
    let mut cell_type: HashMap<CellCoord, usize> = HashMap::new();
    for cell in edev.die_cells(die) {
        let key: Vec<_> = edev[cell].tiles.iter().map(|(slot, t)| (slot, t.class)).collect();
        let ti = *type_idx.entry(key.clone()).or_insert_with(|| {
            types.push(build_type(int, &key));
            types.len() - 1
        });
        cell_type.insert(cell, ti);
        let carry_prev = if edev.has_bel(cell.bel(bslots::SLICE[0])) {
            edev.bel_carry_prev(cell.bel(bslots::SLICE[0]))
                .map(|b| [b.cell.col.to_idx(), b.cell.row.to_idx()])
        } else {
            None
        };
        tiles.push(OutTile {
            x: cell.col.to_idx(),
            y: cell.row.to_idx(),
            tile_type: ti,
            name: format!("X{}Y{}", cell.col.to_idx(), cell.row.to_idx()),
            carry_prev,
        });
    }

    // Group tile wires into nodes by their canonical prjcombine wire.
    let mut nodes: HashMap<WireCoord, Vec<[usize; 3]>> = HashMap::new();
    let mut unresolved = 0usize;
    for cell in edev.die_cells(die) {
        let tb = &types[cell_type[&cell]];
        for (wi, srcs) in tb.sources.iter().enumerate() {
            let mut canon: Option<WireCoord> = None;
            for &(slot, tcw) in srcs {
                let tcrd: TileCoord = cell.tile(slot);
                let Some(w) = edev.resolve_tile_wire(tcrd, tcw) else {
                    continue;
                };
                match canon {
                    None => canon = Some(w),
                    Some(c) => assert_eq!(
                        c, w,
                        "tile wire {} at {cell} maps to two different nodes",
                        tb.out.wires[wi].name
                    ),
                }
            }
            match canon {
                Some(c) => nodes.entry(c).or_default().push([cell.col.to_idx(), cell.row.to_idx(), wi]),
                None if !srcs.is_empty() => unresolved += 1,
                None => {}
            }
        }
    }
    let mut nodes: Vec<Vec<[usize; 3]>> = nodes.into_values().filter(|n| n.len() > 1).collect();

    // Dedicated carry chain: COUT (= CO3, the D carry) of the slice below
    // drives CIN. prjcombine has no wire for it, so join the synthesized wires.
    let (cout, cin) = (slice::carry_out_wire(), slice::carry_in_wire());
    let tile_at: HashMap<(usize, usize), usize> = tiles.iter().map(|t| ((t.x, t.y), t.tile_type)).collect();
    for t in &tiles {
        if let Some([px, py]) = t.carry_prev {
            let prev = &types[tile_at[&(px, py)]];
            let here = &types[t.tile_type];
            nodes.push(vec![[px, py, prev.wire_idx[&cout]], [t.x, t.y, here.wire_idx[&cin]]]);
        }
    }
    nodes.sort();

    // Package pins.
    let mut packages = vec![];
    for (_, pkg, &bond_id) in &dev.bonds {
        let bond = &db.bonds[bond_id];
        let mut pins = vec![];
        for (pin, pads) in &bond.pins {
            let mut io = None;
            if let [pad] = pads.as_slice()
                && db.int.bel_slots[pad.slot].name.starts_with("IOB")
            {
                let tcrd = edev.bel_tile(pad.bel);
                io = Some(OutIo {
                    x: tcrd.cell.col.to_idx(),
                    y: tcrd.cell.row.to_idx(),
                    bel: int.bel_slots[pad.slot].name.clone(),
                    bank: chip.get_io_bank(*pad),
                });
            }
            pins.push(OutPad {
                pin: pin.clone(),
                pads: pads.iter().map(|p| p.to_string(int)).collect(),
                io,
            });
        }
        packages.push(OutPackage { name: pkg.clone(), pins });
    }

    let n_pips: usize = types.iter().map(|t| t.out.pips.len()).sum();
    let n_wires: usize = types.iter().map(|t| t.out.wires.len()).sum();
    eprintln!(
        "{}: {}x{} grid, {} tile types ({} wires, {} pips), {} multi-wire nodes, {} unresolved tile wires",
        args.device,
        width,
        height,
        types.len(),
        n_wires,
        n_pips,
        nodes.len(),
        unresolved
    );

    let out = Output {
        prjcombine_rev: PRJCOMBINE_REV,
        device: args.device.clone(),
        width,
        height,
        speed_grades: dev.speeds.values().cloned().collect(),
        tile_types: types.into_iter().map(|t| t.out).collect(),
        tiles,
        nodes,
        packages,
    };
    let f = std::fs::File::create(&args.out).expect("failed to create output");
    serde_json::to_writer(std::io::BufWriter::new(f), &out).expect("failed to write JSON");
}
