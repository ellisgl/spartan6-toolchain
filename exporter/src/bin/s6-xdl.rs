//! Convert a design placed (and routed) by nextpnr-himbaechel-spartan6 into
//! Xilinx XDL, so ISE can check it (`xdl -xdl2ncd`, `drc`) and build a
//! bitstream (`bitgen`).
//!
//! Stage 1 (this version): instances and site configurations, and nets as
//! pins only. ISE's `par -p` then routes the design, keeping our packing and
//! placement. That validates the packer, placer and site configs.
//! Stage 2 will add our own routing as pips.
//!
//! ISE names come from prjcombine's naming database for the device (built by
//! `s6-rawdump` + `xrd2geom` from ISE itself).
//!
//!     s6-xdl --db spartan6.zstd --geom xc6slx9-geom.zstd \
//!            --routed routed.json --package tqg144 --out design.xdl

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use clap::Parser;
use prjcombine_entity::EntityId;
use prjcombine_interconnect::db::BelSlotId;
use prjcombine_interconnect::grid::{BelCoord, CellCoord, ColId, DieId, RowId};
use prjcombine_re_xilinx_geom::GeomDb;
use prjcombine_re_xilinx_naming_spartan6::{ExpandedNamedDevice, name_device};
use prjcombine_re_xilinx_xdl::{Design, Instance, Net, NetPin, NetType, Placement};
use prjcombine_spartan6::db::Database;
use prjcombine_spartan6::expanded::ExpandedDevice;
use serde_json::Value;

#[derive(Parser)]
#[command(about = "Convert a nextpnr-himbaechel-spartan6 result into XDL")]
struct Args {
    /// prjcombine databases/spartan6.zstd (the database the chipdb was built from)
    #[arg(long)]
    db: PathBuf,
    /// Geom database with ISE names (s6-rawdump + xrd2geom)
    #[arg(long)]
    geom: PathBuf,
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
    out: PathBuf,
}

// ---------------------------------------------------------------------------
// nextpnr JSON

struct NCell {
    name: String,
    typ: String,
    /// Parameters and attributes (UCF constraints land in attributes).
    params: BTreeMap<String, String>,
    /// port -> net index
    conns: BTreeMap<String, usize>,
    /// (tile x, tile y, bel name)
    bel: (usize, usize, String),
}

struct NNet {
    name: String,
    driver: Option<(usize, String)>,
    users: Vec<(usize, String)>,
    constant: Option<bool>,
}

fn load_routed(path: &PathBuf) -> (Vec<NCell>, Vec<NNet>) {
    let v: Value = serde_json::from_reader(std::fs::File::open(path).expect("open routed json")).expect("parse json");
    let m = v["modules"].as_object().unwrap().values().next().expect("no module");
    // Net names: one name per bit, preferring public names.
    let mut bit_name: HashMap<i64, String> = HashMap::new();
    for (name, n) in m["netnames"].as_object().unwrap() {
        let hidden = n["hide_name"].as_i64().unwrap_or(0) != 0;
        for b in n["bits"].as_array().unwrap() {
            if let Some(b) = b.as_i64() {
                let e = bit_name.entry(b).or_insert_with(|| name.clone());
                if !hidden && e.starts_with('$') {
                    *e = name.clone();
                }
            }
        }
    }
    let mut nets: Vec<NNet> = vec![];
    let mut net_of_bit: HashMap<i64, usize> = HashMap::new();
    let mut cells = vec![];
    for (name, c) in m["cells"].as_object().unwrap() {
        let bel = c["attributes"]["NEXTPNR_BEL"].as_str().unwrap_or_else(|| panic!("cell {name} is not placed"));
        let (tile, belname) = bel.split_once('/').unwrap();
        let (xs, ys) = tile.trim_start_matches('X').split_once('Y').unwrap();
        let mut conns = BTreeMap::new();
        let ci = cells.len();
        for (port, bits) in c["connections"].as_object().unwrap() {
            let Some(b) = bits.as_array().unwrap().first().and_then(|b| b.as_i64()) else { continue };
            let ni = *net_of_bit.entry(b).or_insert_with(|| {
                nets.push(NNet { name: bit_name[&b].clone(), driver: None, users: vec![], constant: None });
                nets.len() - 1
            });
            conns.insert(port.clone(), ni);
            if c["port_directions"][port].as_str() == Some("output") {
                nets[ni].driver = Some((ci, port.clone()));
            } else {
                nets[ni].users.push((ci, port.clone()));
            }
        }
        let params = c["attributes"]
            .as_object()
            .unwrap()
            .iter()
            .chain(c["parameters"].as_object().unwrap())
            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
            .collect();
        cells.push(NCell {
            name: name.clone(),
            typ: c["type"].as_str().unwrap().to_string(),
            params,
            conns,
            bel: (xs.parse().unwrap(), ys.parse().unwrap(), belname.to_string()),
        });
    }
    for n in &mut nets {
        if let Some((ci, _)) = &n.driver {
            match cells[*ci].typ.as_str() {
                "PSEUDO_GND" => n.constant = Some(false),
                "PSEUDO_VCC" => n.constant = Some(true),
                _ => {}
            }
        }
    }
    (cells, nets)
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
    let mut slices: BTreeMap<(String, String), (String, SliceInst)> = BTreeMap::new(); // (tile, site) -> (kind, cfg)
    // (cell index, port) -> (instance name, site pin); missing = internal to a site.
    let mut pins: HashMap<(usize, String), (String, String)> = HashMap::new();
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
                let entry = slices
                    .entry((tile.clone(), site.clone()))
                    .or_insert_with(|| (kind.to_string(), SliceInst::default()));
                let s = &mut entry.1;
                let l = sub.chars().next().unwrap();
                let inst = site.clone();
                if c.typ == "SLICE_LUT" {
                    assert_eq!(&sub[1..], "6LUT", "{}: only LUT6 bels are supported", c.name);
                    // Fold constant inputs into the truth table.
                    let mut table = lut_table(&c.params["INIT"]);
                    let k = table.len().trailing_zeros() as usize;
                    let mut vars: Vec<usize> = (1..=k).collect();
                    for j in (1..=k).rev() {
                        let port = format!("A{j}");
                        match c.conns.get(&port).and_then(|&ni| net_const(ni)) {
                            Some(val) => {
                                table = lut_fix(&table, j - 1, val);
                                vars.remove(j - 1);
                            }
                            None if c.conns.contains_key(&port) => {
                                pins.insert((ci, port), (inst.clone(), format!("{l}{j}")));
                            }
                            None => {}
                        }
                    }
                    let expr = lut_expr(&table, &vars);
                    s.set(&format!("{l}6LUT"), &[&c.name, "#LUT", &format!("O6={expr}")]);
                    // O6 leaves the slice through pin <L> unless its only use is this letter's FF.
                    if let Some(&ni) = c.conns.get("O") {
                        let internal_only = nets[ni].users.iter().all(|(u, p)| {
                            cells[*u].typ == "SLICE_FF" && p == "D" && cells[*u].bel.0 == x && cells[*u].bel.1 == y
                                && cells[*u].bel.2 == format!("{slot}.{l}FF")
                        });
                        if !internal_only {
                            s.set(&format!("{l}USED"), &["", "0"]);
                            pins.insert((ci, "O".into()), (inst.clone(), l.to_string()));
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
                    // D: from this letter's LUT6 (FFMUX O6) or through the bypass pin <L>X.
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
                // nextpnr keeps the top-level port name on the cell's PAD net; use the cell name.
                let inst = c.name.clone();
                let mut cfg: Vec<Vec<String>> = vec![vec!["PAD".into(), inst.clone(), "".into()]];
                let input = matches!(io_type, "IBUF" | "IBUFG" | "IOBUF");
                let output = matches!(io_type, "OBUF" | "OBUFT" | "IOBUF");
                if input {
                    cfg.push(vec!["BYPASS_MUX".into(), "".into(), "I".into()]);
                    cfg.push(vec!["IMUX".into(), format!("{inst}.IMUX"), "I".into()]);
                    cfg.push(vec!["INBUF".into(), format!("{inst}_IBUF"), "".into()]);
                    cfg.push(vec!["ISTANDARD".into(), "".into(), std.clone()]);
                    for port in ["I", "GCLK_I"] {
                        if c.conns.contains_key(port) {
                            pins.insert((ci, port.into()), (inst.clone(), "I".into()));
                        }
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
                    if c.conns.contains_key("T") {
                        cfg.push(vec!["TUSED".into(), "".into(), "0".into()]);
                        pins.insert((ci, "T".into()), (inst.clone(), "T".into()));
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
                        pins.insert((ci, port.into()), (inst.clone(), "I0".into()));
                    }
                }
                pins.insert((ci, "O".into()), (inst.clone(), "O".into()));
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

    for ((tile, site), (kind, s)) in slices {
        instances.push(Instance {
            name: site.clone(),
            kind,
            placement: Placement::Placed { tile, site },
            cfg: s.cfg.into_values().collect(),
        });
    }

    // Nets: pins only (stage 1).
    let mut xnets = vec![];
    for n in &nets {
        let mut outpins = BTreeSet::new();
        let mut inpins = BTreeSet::new();
        if let Some((dc, dp)) = &n.driver
            && let Some(p) = pins.get(&(*dc, dp.clone()))
        {
            outpins.insert(p.clone());
        }
        for (uc, up) in &n.users {
            if let Some(p) = pins.get(&(*uc, up.clone())) {
                inpins.insert(p.clone());
            }
        }
        if inpins.is_empty() {
            continue;
        }
        let typ = match n.constant {
            Some(true) => NetType::Vcc,
            Some(false) => NetType::Gnd,
            None => NetType::Plain,
        };
        if typ == NetType::Plain && outpins.is_empty() {
            panic!("net {} has sinks but no driver pin", n.name);
        }
        let mk = |(inst_name, pin): (String, String)| NetPin { inst_name, pin };
        xnets.push(Net {
            name: n.name.clone(),
            typ,
            outpins: outpins.into_iter().map(mk).collect(),
            inpins: inpins.into_iter().map(mk).collect(),
            pips: vec![],
            cfg: vec![],
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
    let mut f = std::io::BufWriter::new(std::fs::File::create(&args.out).expect("create output"));
    design.write(&mut f).expect("write XDL");
    eprintln!(
        "wrote {} ({} instances, {} nets)",
        args.out.display(),
        design.instances.len(),
        design.nets.len()
    );
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
