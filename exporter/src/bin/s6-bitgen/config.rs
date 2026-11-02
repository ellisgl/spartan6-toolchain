//! Generate a bitstream's contents from a design placed and routed by
//! nextpnr-himbaechel-spartan6, using prjcombine's bit-level database.
//!
//! Everything is expressed as prjcombine settings (switchbox items, bel
//! attributes, table rows); bits are only placed through the tile bit maps.
//! The only constants are bitgen's defaults (registers and a few always-set
//! bits), taken from ISE's output for an empty design with
//! `-g UnusedPin:Pullnone`.

use std::collections::{HashMap, HashSet};

use prjcombine_entity::{EntityId, EntityVec};
use prjcombine_interconnect::db::{
    Bel, BelAttribute, BelAttributeType, BelClass, BelInfo, IntDb, SwitchBoxItem, TableValue,
};
use prjcombine_interconnect::grid::{BelCoord, CellCoord, ColId, DieId, RowId, TileCoord};
use prjcombine_spartan6::bitstream::{BRAM_FRAME_LEN, BitPos, BitRect, Bitstream, FRAME_LEN, Reg};
use prjcombine_spartan6::expanded::ExpandedDevice;
use prjcombine_types::bitrect::{BitRect as _, BitRectId, TileBit};
use prjcombine_types::bitvec::BitVec;
use s6_export::chipdb::ChipdbIndex;
use s6_export::nextpnr::{NCell, NNet};
use s6_export::parse_wire_name;

/// bitgen's register values for our options (Binary:Yes, UnusedPin:Pullnone,
/// everything else default), from ISE's output.
const DEFAULT_REGS: &[(Reg, u32)] = &[
    (Reg::Idcode, 0x0400_1093),
    (Reg::Ctl0, 0x0081),
    (Reg::Cor1, 0x3d00),
    (Reg::Cor2, 0x09ee),
    (Reg::Timer, 0xffff),
    (Reg::RbCrcSw, 0x0000),
    (Reg::CclkFrequency, 0x3cc8),
    (Reg::Powerdown, 0x0881),
    (Reg::EyeMask, 0x0000),
    (Reg::HcOpt, 0x001f),
    (Reg::PuGwe, 0x0005),
    (Reg::PuGts, 0x0004),
    (Reg::SeuOpt, 0x1be2),
    (Reg::Mode, 0x0100),
    (Reg::General1, 0x0000),
    (Reg::General2, 0x0000),
    (Reg::General3, 0x0000),
    (Reg::General4, 0x0000),
    (Reg::General5, 0x0000),
];

/// Bits bitgen always sets (analog trims), as (bel slot, attribute, bit index):
/// set in every instance of that bel.
const DEFAULT_ATTR_BITS: &[(&str, &str, usize)] = &[
    ("CMT_VREG", "REG_BG", 0),
    ("MISC_SW", "VGG_SLOPE_OPTIONS", 3),
    ("MISC_SW", "VBG_SLOPE_OPTIONS", 2),
    ("MISC_SW", "VBG_SLOPE_OPTIONS", 3),
];

pub struct Gen<'a> {
    edev: &'a ExpandedDevice<'a>,
    int: &'a IntDb,
    pub bs: Bitstream,
    rects: HashMap<TileCoord, EntityVec<BitRectId, BitRect>>,
    /// Enum attributes set so far, per bel (to apply "unused" defaults later).
    set_enums: HashSet<(BelCoord, String)>,
    /// Slices with at least one cell placed in them.
    used_slices: HashSet<BelCoord>,
}

impl<'a> Gen<'a> {
    pub fn new(edev: &'a ExpandedDevice<'a>, int: &'a IntDb) -> Self {
        let mut bs = edev.empty_bitstream();
        for &(r, v) in DEFAULT_REGS {
            bs.regs.insert(r, v);
        }
        Gen { edev, int, bs, rects: HashMap::new(), set_enums: HashSet::new(), used_slices: HashSet::new() }
    }

    fn set_pos(&mut self, pos: BitPos, v: bool) {
        match pos {
            BitPos::Main(f, b) => self.bs.frame_data.set(f * FRAME_LEN + b, v),
            BitPos::Bram(f, b) => self.bs.bram_data.set(f * BRAM_FRAME_LEN + b, v),
            BitPos::Iob(b) => self.bs.iob.set(b, v),
            BitPos::Reg(r, b) => {
                let e = self.bs.regs.entry(r).or_insert(0);
                *e = (*e & !(1 << b)) | (v as u32) << b;
            }
            p => panic!("cannot set {p:?}"),
        }
    }

    fn set_tb(&mut self, tcrd: TileCoord, tb: TileBit, v: bool) {
        let edev = self.edev;
        let rects = self.rects.entry(tcrd).or_insert_with(|| edev.tile_bits(tcrd));
        let pos = rects[tb.rect].xlat_pos_fwd((tb.frame, tb.bit));
        self.set_pos(pos, v);
    }

    fn bel(&self, bcrd: BelCoord) -> (TileCoord, &'a Bel, &'a BelClass) {
        let tcrd = self.edev.bel_tile(bcrd);
        let tcls = &self.int.tile_classes[self.edev[tcrd].class];
        let BelInfo::Bel(bel) = &tcls.bels[bcrd.slot] else { panic!("not a bel") };
        (tcrd, bel, &self.int.bel_classes[self.int.bel_class(bcrd.slot)])
    }

    fn attr(&self, bcrd: BelCoord, name: &str) -> (TileCoord, &'a BelAttribute, BelAttributeType) {
        let (tcrd, bel, bcls) = self.bel(bcrd);
        let (aid, cattr) = bcls
            .attributes
            .iter()
            .find(|(_, a)| a.name == name)
            .unwrap_or_else(|| panic!("{} has no attribute {name}", bcls.name));
        let attr = bel.attributes.get(aid).unwrap_or_else(|| panic!("{name} not present on this {}", bcls.name));
        (tcrd, attr, cattr.typ)
    }

    /// Set a bit-vector (or bool) attribute to a value.
    pub fn set_bits(&mut self, bcrd: BelCoord, name: &str, val: &[bool]) {
        let (tcrd, attr, _) = self.attr(bcrd, name);
        let BelAttribute::BitVec(bits) = attr else { panic!("{name} is not a bit vector") };
        assert_eq!(bits.len(), val.len(), "{name}: width");
        for (b, &v) in bits.iter().zip(val) {
            self.set_tb(tcrd, b.bit, v ^ b.inv);
        }
    }

    pub fn set_bitvec(&mut self, bcrd: BelCoord, name: &str, val: &BitVec) {
        let v: Vec<bool> = (0..val.len()).map(|i| val[i]).collect();
        self.set_bits(bcrd, name, &v);
    }

    /// Set the raw bitstream bit behind attribute bit `idx` (ignores polarity).
    fn set_raw(&mut self, bcrd: BelCoord, name: &str, idx: usize) {
        let (tcrd, attr, _) = self.attr(bcrd, name);
        let tb = match attr {
            BelAttribute::BitVec(bits) => bits[idx].bit,
            BelAttribute::Enum(e) => e.bits[idx],
        };
        self.set_tb(tcrd, tb, true);
    }

    pub fn set_enum(&mut self, bcrd: BelCoord, name: &str, value: &str) {
        let (tcrd, attr, typ) = self.attr(bcrd, name);
        let BelAttribute::Enum(e) = attr else { panic!("{name} is not an enum") };
        let BelAttributeType::Enum(ecid) = typ else { unreachable!() };
        let vid = self.int.enum_classes[ecid]
            .values
            .iter()
            .find(|(_, v)| v.as_str() == value)
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("{name} has no value {value}"));
        let pat = e.values.get(vid).unwrap_or_else(|| panic!("{name}={value} has no encoding here")).clone();
        for (i, &b) in e.bits.iter().enumerate() {
            self.set_tb(tcrd, b, pat[i]);
        }
        self.set_enums.insert((bcrd, name.to_string()));
    }

    pub fn table(&self, table: &str, row: &str, field: &str) -> BitVec {
        let t = self
            .int
            .tables
            .iter()
            .find(|(_, t)| t.name == table)
            .map(|(_, t)| t)
            .unwrap_or_else(|| panic!("no table {table}"));
        let (fid, _) = t.fields.iter().find(|(_, f)| f.name == field).unwrap_or_else(|| panic!("no field {field}"));
        let (_, r) = t.rows.iter().find(|(_, r)| r.name == row).unwrap_or_else(|| panic!("{table} has no row {row}"));
        match r.fields.get(fid) {
            Some(TableValue::BitVec(bv)) => bv.clone(),
            Some(v) => panic!("{table}.{row}.{field} is not a bit vector: {v:?}"),
            None => {
                let TableValue::BitVec(bv) = t.rows.iter().find(|(_, r)| r.name == "OFF").and_then(|(_, r)| r.fields.get(fid)).cloned().unwrap() else {
                    unreachable!()
                };
                bv
            }
        }
    }

    fn slot(&self, name: &str) -> prjcombine_interconnect::db::BelSlotId {
        self.int.bel_slots.iter().find(|(_, s)| s.name == name).map(|(id, _)| id).unwrap_or_else(|| panic!("no bel slot {name}"))
    }

    fn cell(&self, x: usize, y: usize) -> CellCoord {
        CellCoord { die: DieId::from_idx(0), col: ColId::from_idx(x), row: RowId::from_idx(y) }
    }

    /// All instances of a bel slot in the device.
    fn all_bels(&self, slot_name: &str) -> Vec<BelCoord> {
        let slot = self.slot(slot_name);
        self.edev
            .tiles()
            .filter(|(_, t)| self.int.tile_classes[t.class].bels.contains_id(slot))
            .map(|(tcrd, _)| tcrd.cell.bel(slot))
            .collect()
    }

    pub fn defaults(&mut self) {
        for &(slot, attr, idx) in DEFAULT_ATTR_BITS {
            for b in self.all_bels(slot) {
                self.set_raw(b, attr, idx);
            }
        }
        // UnusedPin:Pullnone: every IOB, bonded or not, gets PULL=NONE.
        for n in 0..2 {
            for b in self.all_bels(&format!("IOB[{n}]")) {
                self.set_enum(b, "PULL", "NONE");
            }
        }
    }

    /// Program one routed nextpnr pip.
    pub fn pip(&mut self, chipdb: &ChipdbIndex, x: usize, y: usize, dst: &str, src: &str) {
        let meta = chipdb.get(x, y, src, dst).unwrap_or_else(|| panic!("pip X{x}Y{y}/{dst}/{src} not in chipdb JSON"));
        let cell = self.cell(x, y);
        match meta.kind.as_str() {
            "SITE" => {
                let r = &meta.src_ref;
                let bcrd = cell.bel(self.slot(&r[1]));
                self.set_enum(bcrd, &r[2], &r[3]);
            }
            "BELOUT" | "PERMABUF" | "PROGINV" | "PROGDELAY" => {}
            _ => {
                let r = &meta.src_ref;
                let tslot = self.int.tile_slots.iter().find(|(_, s)| **s == r[0]).map(|(id, _)| id).unwrap();
                let tcrd = cell.tile(tslot);
                let tcls = &self.int.tile_classes[self.edev[tcrd].class];
                let bslot = self.slot(&r[1]);
                let idx: usize = r[2].parse().unwrap();
                let tile = &self.edev[tcrd];
                let tcw = |name: &str| {
                    let raw = parse_wire_name(self.edev, cell, name).expect("unknown wire");
                    let (cid, _) = tile.cells.iter().find(|&(_, &c)| c == raw.cell).expect("wire not in tile");
                    prjcombine_interconnect::db::TileWireCoord { cell: cid, wire: raw.slot }
                };
                let (tw_in, _tw_out) = (tcw(src), tcw(dst));
                match &tcls.bels[bslot] {
                    BelInfo::SwitchBox(sb) => match &sb.items[idx] {
                        SwitchBoxItem::Mux(m) => {
                            let (_, pat) = m.src.iter().find(|(k, _)| k.tw == tw_in).expect("mux source not found");
                            let pat = pat.clone();
                            for (i, &b) in m.bits.iter().enumerate() {
                                self.set_tb(tcrd, b, pat[i]);
                            }
                        }
                        SwitchBoxItem::ProgBuf(p) => self.set_tb(tcrd, p.bit.bit, !p.bit.inv),
                        SwitchBoxItem::Pass(p) => self.set_tb(tcrd, p.bit.bit, !p.bit.inv),
                        SwitchBoxItem::BiPass(p) => self.set_tb(tcrd, p.bit.bit, !p.bit.inv),
                        other => panic!("switchbox item {other:?} not supported"),
                    },
                    BelInfo::TestMux(tm) => {
                        let pat = tm.bits_primary.clone();
                        for (i, &b) in tm.bits.iter().enumerate() {
                            self.set_tb(tcrd, b, pat[i]);
                        }
                    }
                    BelInfo::Bel(_) => panic!("pip refers to a bel"),
                }
            }
        }
    }

    /// Program a placed cell's own settings (not its routing).
    pub fn cell_config(&mut self, c: &NCell, nets: &[NNet]) -> Result<(), String> {
        let (x, y, ref belname) = c.bel;
        let cell = self.cell(x, y);
        match c.typ.as_str() {
            "PSEUDO_GND" | "PSEUDO_VCC" | "BUFGMUX" => {}
            "SLICE_LUT" => {
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = cell.bel(self.slot(slot));
                self.used_slices.insert(bcrd);
                // LUTk uses A1..Ak; replicate its table over the unused inputs.
                let init: Vec<bool> = c.params["INIT"].chars().rev().map(|ch| ch == '1').collect();
                let k = init.len();
                let table: Vec<bool> = (0..64).map(|i| init[i % k]).collect();
                self.set_bits(bcrd, sub, &table);
            }
            "SLICE_FF" => {
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = cell.bel(self.slot(slot));
                self.used_slices.insert(bcrd);
                let l = &sub[..1];
                let ff_type = c.params["FF_TYPE"].as_str();
                let srinit = matches!(ff_type, "FDSE" | "FDPE");
                self.set_bits(bcrd, &format!("FF{l}_SRINIT"), &[srinit]);
                self.set_bits(bcrd, "FF_SR_SYNC", &[matches!(ff_type, "FDRE" | "FDSE")]);
                if c.conns.contains_key("SR") {
                    self.set_bits(bcrd, "FF_SR_ENABLE", &[true]);
                }
                if c.conns.contains_key("CE") {
                    self.set_bits(bcrd, "FF_CE_ENABLE", &[true]);
                }
            }
            "IOB" => {
                let bcrd = cell.bel(self.slot(belname));
                let n = belname.trim_start_matches("IOB[").trim_end_matches(']');
                let io_type = c.params.get("IO_TYPE").map(String::as_str).unwrap_or("IBUF");
                let std = c.params.get("IOSTANDARD").map(String::as_str).unwrap_or("LVCMOS25");
                let input = matches!(io_type, "IBUF" | "IBUFG" | "IOBUF");
                let output = matches!(io_type, "OBUF" | "OBUFT" | "IOBUF");
                if !matches!(std, "LVTTL" | "LVCMOS33" | "LVCMOS25") {
                    return Err(format!("{}: IOSTANDARD {std} not supported yet", c.name));
                }
                if output {
                    let drive = c.params.get("DRIVE").map(String::as_str).unwrap_or("12");
                    let row = format!("{std}_{drive}");
                    let pd = self.table("IOB_DATA", &row, "PDRIVE");
                    // The NDRIVE_2V5/NDRIVE_3V3 columns follow VCCAUX (bitgen
                    // default 2.5 V), not the I/O standard (matches ISE for LVTTL).
                    let nd = self.table("IOB_DATA", &row, "NDRIVE_2V5");
                    self.set_bitvec(bcrd, "PDRIVE", &pd);
                    self.set_bitvec(bcrd, "NDRIVE", &nd);
                    let slew = match c.params.get("SLEW").map(String::as_str).unwrap_or("SLOW") {
                        "FAST" => "SLEW_FAST",
                        _ => "SLEW_SLOW",
                    };
                    let ps = self.table("IOB_DATA", slew, "PSLEW");
                    let ns = self.table("IOB_DATA", slew, "NSLEW");
                    self.set_bitvec(bcrd, "PSLEW", &ps);
                    self.set_bitvec(bcrd, "NSLEW", &ns);
                    self.set_bits(bcrd, "OUTPUT_ENABLE", &[true]);
                    self.set_enum(bcrd, "SUSPEND", "_3STATE");
                    // OLOGIC passes D1 straight to the pad.
                    let ologic = cell.bel(self.slot(&format!("OLOGIC[{n}]")));
                    if nets_uses(c, "O") {
                        self.set_bits(ologic, "ENABLE", &[true]);
                        self.set_bits(ologic, "FFO_RANK1_BYPASS", &[true]);
                        self.set_enum(ologic, "MUX_O", "D1");
                    }
                    if c.conns.contains_key("T") {
                        return Err(format!("{}: tristate outputs not supported yet", c.name));
                    }
                } else {
                    // Input only: output drivers off.
                    let pd = self.table("IOB_DATA", "OFF", "PDRIVE");
                    let nd = self.table("IOB_DATA", "OFF", "NDRIVE_3V3");
                    self.set_bitvec(bcrd, "PDRIVE", &pd);
                    self.set_bitvec(bcrd, "NDRIVE", &nd);
                }
                if input {
                    self.set_enum(bcrd, "IBUF_MODE", "CMOS_VCCAUX");
                    if c.conns.contains_key("I") {
                        // Data input through ILOGIC (pass-through to FABRICOUT).
                        let ilogic = cell.bel(self.slot(&format!("ILOGIC[{n}]")));
                        self.set_bits(ilogic, "ENABLE", &[true]);
                        // No input delay (the bit is stored inverted).
                        self.set_bits(ilogic, "I_DELAY_ENABLE", &[false]);
                    }
                }
                let _ = nets;
            }
            other => return Err(format!("cell {}: type {other} not supported by the bitstream writer yet", c.name)),
        }
        Ok(())
    }

    /// Settings that depend on the design as a whole.
    pub fn global(&mut self, cells: &[NCell]) {
        // In a used SLICEX, ISE writes the "unused" encodings for muxes the
        // design doesn't set: OUTMUX=O5 and FFMUX=<L>X (raw bit 1). In
        // SLICEL/SLICEM those encodings are all zeros.
        let slicex = self.slot("SLICE[1]");
        let used: Vec<BelCoord> = self.used_slices.iter().copied().filter(|b| b.slot == slicex).collect();
        for b in used {
            for l in ['A', 'B', 'C', 'D'] {
                if !self.set_enums.contains(&(b, format!("MUX_{l}OUT"))) {
                    self.set_enum(b, &format!("MUX_{l}OUT"), "O5");
                }
                if !self.set_enums.contains(&(b, format!("MUX_FF{l}"))) {
                    self.set_enum(b, &format!("MUX_FF{l}"), &format!("{l}X"));
                }
            }
        }
        if cells.iter().any(|c| c.typ == "IOB") {
            for b in self.all_bels("MISC_SE") {
                self.set_raw(b, "GLUTMASK_IOB", 0);
            }
        }
    }
}

fn nets_uses(c: &NCell, port: &str) -> bool {
    c.conns.contains_key(port)
}
