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
    Bel, BelAttribute, BelAttributeType, BelClass, BelInfo, BelInput, IntDb, SwitchBoxItem, TableValue,
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
    /// While set, bit writes go to frame fixups (a second write of the frame
    /// after the main frame data) instead of the main frame data.
    fixup: bool,
    /// PairMux items in use: one setting drives two wires, so the sources of
    /// both are collected first, (tile, item index) -> [source per output].
    pair_muxes: HashMap<(TileCoord, prjcombine_interconnect::db::BelSlotId, usize), [Option<prjcombine_interconnect::db::TileWireCoord>; 2]>,
    /// A PLL is used (sets REG_BG in every DCM tile's regulator).
    cmt_used: bool,
    /// Regulators in the tiles of the DCMs used (REG_BG).
    dcm_vregs: Vec<BelCoord>,
}

impl<'a> Gen<'a> {
    pub fn new(edev: &'a ExpandedDevice<'a>, int: &'a IntDb) -> Self {
        let mut bs = edev.empty_bitstream();
        for &(r, v) in DEFAULT_REGS {
            bs.regs.insert(r, v);
        }
        Gen { edev, int, bs, rects: HashMap::new(), set_enums: HashSet::new(), used_slices: HashSet::new(), fixup: false, pair_muxes: HashMap::new(), cmt_used: false, dcm_vregs: vec![] }
    }

    fn set_pos(&mut self, pos: BitPos, v: bool) {
        if self.fixup {
            let BitPos::Main(f, b) = pos else { panic!("fixup of non-main bit {pos:?}") };
            self.bs.frame_fixups.insert((f, b), v);
            return;
        }
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

    /// Set bits `offset..offset + val.len()` of a bit-vector attribute.
    pub fn set_bits_at(&mut self, bcrd: BelCoord, name: &str, offset: usize, val: &[bool]) {
        let (tcrd, attr, _) = self.attr(bcrd, name);
        let BelAttribute::BitVec(bits) = attr else { panic!("{name} is not a bit vector") };
        assert!(offset + val.len() <= bits.len(), "{name}: width");
        for (b, &v) in bits[offset..].iter().zip(val) {
            self.set_tb(tcrd, b.bit, v ^ b.inv);
        }
    }

    fn attr_len(&self, bcrd: BelCoord, name: &str) -> usize {
        match self.attr(bcrd, name).1 {
            BelAttribute::BitVec(bits) => bits.len(),
            BelAttribute::Enum(e) => e.bits.len(),
        }
    }

    /// Set the raw bitstream bits of a bit-vector attribute, ignoring its
    /// stored polarity (for bits only known as ISE writes them).
    fn set_raw_bits(&mut self, bcrd: BelCoord, name: &str, raw: &[bool]) {
        let (tcrd, attr, _) = self.attr(bcrd, name);
        let tbits: Vec<TileBit> = match attr {
            BelAttribute::BitVec(bits) => bits.iter().map(|b| b.bit).collect(),
            BelAttribute::Enum(e) => e.bits.clone(),
        };
        assert_eq!(tbits.len(), raw.len(), "{name}: width");
        for (b, &v) in tbits.into_iter().zip(raw) {
            self.set_tb(tcrd, b, v);
        }
    }

    /// Set the raw inversion bit of every invertible input whose name starts
    /// with `prefix` (e.g. "WEA" for WEA0..WEA1).
    fn set_input_inv_raw(&mut self, bcrd: BelCoord, prefix: &str, raw: bool) {
        let (tcrd, bel, bcls) = self.bel(bcrd);
        let mut bits = vec![];
        for (pid, inp) in &bel.inputs {
            if let BelInput::Invertible(_, b) = inp {
                let (pn, _) = bcls.inputs.key(pid);
                if pn.starts_with(prefix) {
                    bits.push(b.bit);
                }
            }
        }
        for b in bits {
            self.set_tb(tcrd, b, raw);
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
            "RT_ILOGIC" => {
                // An unused ILOGIC passing CLK through to CFB0: ISE sets its
                // IODELAY's IDELAY_FIXED.
                let k = meta.src_ref[1].trim_start_matches("ILOGIC");
                let iodelay = cell.bel(self.slot(&format!("IODELAY{k}")));
                self.set_bits(iodelay, "IDELAY_FIXED", &[true]);
            }
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
                let (tw_in, tw_out) = (tcw(src), tcw(dst));
                match &tcls.bels[bslot] {
                    BelInfo::SwitchBox(sb) => match &sb.items[idx] {
                        SwitchBoxItem::Mux(m) => {
                            let (_, pat) = m.src.iter().find(|(k, _)| k.tw == tw_in).expect("mux source not found");
                            let pat = pat.clone();
                            for (i, &b) in m.bits.iter().enumerate() {
                                self.set_tb(tcrd, b, pat[i]);
                            }
                        }
                        SwitchBoxItem::PairMux(pm) => {
                            let k = pm.dst.iter().position(|&d| d == tw_out).expect("pair mux output");
                            let e = self.pair_muxes.entry((tcrd, bslot, idx)).or_insert([None, None]);
                            assert!(e[k].is_none_or(|s| s == tw_in), "pair mux output used twice");
                            e[k] = Some(tw_in);
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
            "BRAM" => self.bram_config(c, cell)?,
            "DCM" | "PLL" | "BUFIO2" | "BUFIO2FB" => self.cmt_config(c, cell)?,
            "DSP" => {
                let bcrd = cell.bel(self.slot(belname));
                let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
                let flag = |k: &str| p(k).is_some_and(|v| v.chars().any(|ch| ch == '1'));
                for r in [
                    "A0REG", "A1REG", "B0REG", "B1REG", "CREG", "DREG", "MREG", "PREG", "OPMODEREG", "CARRYINREG",
                    "CARRYOUTREG",
                ] {
                    self.set_bits(bcrd, r, &[flag(r)]);
                }
                self.set_enum(bcrd, "B_INPUT", &p("B_INPUT").unwrap_or("DIRECT".into()));
                self.set_enum(bcrd, "CARRYINSEL", &p("CARRYINSEL").unwrap_or("OPMODE5".into()));
                self.set_enum(bcrd, "RSTTYPE", &p("RSTTYPE").unwrap_or("SYNC".into()));
            }
            "CARRY4" | "SLICE_F7MUX" | "SLICE_F8MUX" => {
                // Everything is in the slice muxes (SITE pips: MUX_<L>CY0,
                // PRECYINIT, CYINIT, MUX_FF<L>/MUX_<L>OUT = XOR/CY/F7/F8).
                let (slot, _) = belname.split_once('.').unwrap();
                self.used_slices.insert(cell.bel(self.slot(slot)));
            }
            "SLICE_LUTRAM" => {
                // SLICEM LUT as RAM or shift register: RAMMODE plus the table
                // (RAM32: O5 half low, O6 half high).
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = cell.bel(self.slot(slot));
                self.used_slices.insert(bcrd);
                let l = &sub[..1];
                if c.params.get("CLK_INV").is_some_and(|v| v.chars().any(|ch| ch == '1')) {
                    self.set_input_inv_raw(bcrd, "CLK", true);
                }
                let mode = c.params.get("RAM_MODE").map(String::as_str).unwrap_or("RAM64");
                self.set_enum(bcrd, &format!("{l}RAMMODE"), mode);
                // 7th/8th write address bit from CX/BX (RAM128X1S, RAM256).
                for wa in ["WA7", "WA8"] {
                    if c.conns.contains_key(wa) {
                        self.set_bits(bcrd, &format!("{wa}USED"), &[true]);
                    }
                }
                let init: Vec<bool> = c.params.get("INIT").map(|v| v.chars().rev().map(|ch| ch == '1').collect()).unwrap_or_default();
                let bit = |i: usize| init.get(i).copied().unwrap_or(false);
                // SRLs read with A1 (and for SRL16, A6) high: each bit is
                // doubled across A1, SRL16 in the upper half (as ISE writes).
                let table: Vec<bool> = match mode {
                    "SRL32" => (0..64).map(|i| bit(i / 2)).collect(),
                    "SRL16" => (0..64).map(|i| i >= 32 && bit((i - 32) / 2)).collect(),
                    _ => (0..64).map(bit).collect(),
                };
                self.set_bits(bcrd, &format!("{l}6LUT"), &table);
            }
            "SLICE_LUT" | "SLICE_LUT5" => {
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = cell.bel(self.slot(slot));
                self.used_slices.insert(bcrd);
                // One 64-bit table per letter: O6 reads all of it, O5 the lower
                // half (A6 = 0). A LUT6 sharing its site with a LUT5 has A6
                // tied high and lives in the upper half; otherwise LUTk uses
                // A1..Ak and its table is replicated over the unused inputs.
                let init: Vec<bool> = c.params["INIT"].chars().rev().map(|ch| ch == '1').collect();
                let k = init.len();
                let l = &sub[..1];
                let range = match &sub[1..] {
                    "6LUT" if k <= 32 && c.conns.contains_key("A6") => 32..64,
                    "6LUT" => 0..64,
                    "5LUT" => 0..32,
                    other => return Err(format!("{}: unexpected LUT bel {other}", c.name)),
                };
                let table: Vec<bool> = range.clone().map(|i| init[i % k]).collect();
                self.set_bits_at(bcrd, &format!("{l}6LUT"), range.start, &table);
            }
            "SLICE_FF" | "SLICE_FF5" => {
                let (slot, sub) = belname.split_once('.').unwrap();
                let bcrd = cell.bel(self.slot(slot));
                self.used_slices.insert(bcrd);
                // Inverted slice clock (CLKINV = CLK_B): the CLK pin's inversion bit.
                if c.params.get("CLK_INV").is_some_and(|v| v.chars().any(|ch| ch == '1')) {
                    self.set_input_inv_raw(bcrd, "CLK", true);
                }
                let l = &sub[..1];
                // <L>FF or <L>5FF (D from O5, Q out through MUX_<L>OUT=<L>5Q).
                let five = if &sub[1..] == "5FF" { "5" } else { "" };
                let ff_type = c.params["FF_TYPE"].as_str();
                let srinit = matches!(ff_type, "FDSE" | "FDPE" | "LDPE");
                if matches!(ff_type, "LDCE" | "LDPE") {
                    self.set_bits(bcrd, "FF_LATCH", &[true]);
                }
                self.set_bits(bcrd, &format!("FF{l}{five}_SRINIT"), &[srinit]);
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
                    // ... and T1 straight to the output enable (tristate when high).
                    if nets_uses(c, "T") {
                        self.set_bits(ologic, "ENABLE", &[true]);
                        self.set_bits(ologic, "FFT_RANK1_BYPASS", &[true]);
                        self.set_enum(ologic, "MUX_T", "T1");
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
                    if c.conns.get("I").is_some_and(|&ni| !nets[ni].users.is_empty()) {
                        // Data input through ILOGIC (pass-through to FABRICOUT);
                        // ISE leaves it off for an input nothing reads.
                        let ilogic = cell.bel(self.slot(&format!("ILOGIC[{n}]")));
                        self.set_bits(ilogic, "ENABLE", &[true]);
                        // No input delay (the bit is stored inverted).
                        self.set_bits(ilogic, "I_DELAY_ENABLE", &[false]);
                    }
                }
                if let Some(pull) = c.pull() {
                    self.set_enum(bcrd, "PULL", pull);
                }
            }
            other => return Err(format!("cell {}: type {other} not supported by the bitstream writer yet", c.name)),
        }
        Ok(())
    }

    /// Block RAM: one RAMB8 half, or a RAMB16 over both halves (the cell on
    /// BRAM[0] carries the parameters; its BRAM[1] partner has none).
    fn bram_config(&mut self, c: &NCell, cell: CellCoord) -> Result<(), String> {
        let kind = c.params.get("RAMB_KIND").map(String::as_str).unwrap_or("RAMB8");
        if kind == "RAMB16_HI" {
            return Ok(());
        }
        let belname = &c.bel.2;
        let halves: Vec<BelCoord> = if kind == "RAMB16" {
            vec![cell.bel(self.slot("BRAM[0]")), cell.bel(self.slot("BRAM[1]"))]
        } else {
            vec![cell.bel(self.slot(belname))]
        };
        let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
        let num = |k: &str| -> usize {
            p(k).map(|v| if v.chars().all(|ch| ch == '0' || ch == '1') && v.len() > 8 { usize::from_str_radix(&v, 2).unwrap() } else { v.parse().unwrap_or(0) }).unwrap_or(0)
        };
        let bits_of = |k: &str, n: usize| -> Vec<bool> {
            let v = p(k).unwrap_or_default();
            let b: Vec<bool> = v.chars().rev().map(|ch| ch == '1').collect();
            (0..n).map(|i| b.get(i).copied().unwrap_or(false)).collect()
        };
        let truthy = |k: &str, def: bool| -> bool {
            match p(k).as_deref() {
                None => def,
                Some("TRUE") => true,
                Some("FALSE") => false,
                Some(v) => v.chars().any(|ch| ch == '1'),
            }
        };
        // Pin usage on the cell that has the control pins (BRAM[0] for RAMB16).
        let used = |pin: &str| c.conns.contains_key(pin);
        for &h in &halves {
            if kind == "RAMB16" {
                self.set_bits(h, "COMBINE", &[true]);
            }
            // As ISE writes them for non-inverted pins (raw bits): CLK_INV set
            // when the clock is used, RST_INV set when the reset is not (an
            // unconnected input reads 1), EN/REGCE_INV always set; write-enable
            // inversion bits, DDEL = 011 and BW_EN always set.
            for ab in ["A", "B"] {
                self.set_raw_bits(h, &format!("CLK{ab}_INV"), &[used(&format!("CLK{ab}"))]);
                self.set_raw_bits(h, &format!("RST{ab}_INV"), &[!used(&format!("RST{ab}"))]);
                self.set_raw_bits(h, &format!("EN{ab}_INV"), &[true]);
                self.set_raw_bits(h, &format!("REGCE{ab}_INV"), &[true]);
                self.set_raw_bits(h, &format!("DDEL_{ab}"), &[true, true, false]);
                self.set_raw_bits(h, &format!("BW_EN_{ab}"), &[true]);
                self.set_input_inv_raw(h, &format!("WE{ab}"), true);
            }
            let mode = if kind == "RAMB16" { "TDP".to_string() } else { p("RAM_MODE").unwrap_or("TDP".into()) };
            self.set_enum(h, "RAM_MODE", &mode);
            for ab in ["A", "B"] {
                self.set_enum(h, &format!("DATA_WIDTH_{ab}"), &format!("_{}", num(&format!("DATA_WIDTH_{ab}"))));
                self.set_enum(h, &format!("WRITE_MODE_{ab}"), &p(&format!("WRITE_MODE_{ab}")).unwrap_or("WRITE_FIRST".into()));
                self.set_bits(h, &format!("DO{ab}_REG"), &[num(&format!("DO{ab}_REG")) != 0]);
                self.set_bits(h, &format!("EN_RSTRAM_{ab}"), &[truthy(&format!("EN_RSTRAM_{ab}"), true)]);
                self.set_enum(h, &format!("RSTTYPE_{ab}"), &p("RSTTYPE").unwrap_or("SYNC".into()));
                self.set_enum(h, &format!("RST_PRIORITY_{ab}"), &p(&format!("RST_PRIORITY_{ab}")).unwrap_or("SR".into()));
            }
        }
        // INIT_A/B and SRVAL_A/B: RAMB16's 36 bits are low 16 + 2 parity per half.
        for attr in ["INIT_A", "INIT_B", "SRVAL_A", "SRVAL_B"] {
            if kind == "RAMB16" {
                let v = bits_of(attr, 36);
                for (hi, &h) in halves.iter().enumerate() {
                    let mut hv = v[hi * 16..hi * 16 + 16].to_vec();
                    hv.extend_from_slice(&v[32 + hi * 2..32 + hi * 2 + 2]);
                    self.set_bits(h, attr, &hv);
                }
            } else {
                self.set_bits(halves[0], attr, &bits_of(attr, 18));
            }
        }
        // Contents. RAMB16 narrow: INIT bit n -> BRAM[n >> 13].DATA[n & 0x1fff];
        // with a 36-bit port ISE swizzles bit w to narrow position
        // (w & 0xf) | (w & 0x10) << 9 | (w & 0x3fe0) >> 1 (parity likewise).
        let wide = kind == "RAMB16" && (num("DATA_WIDTH_A") == 36 || num("DATA_WIDTH_B") == 36);
        let (ninit, ninitp) = if kind == "RAMB16" { (0x40, 8) } else { (0x20, 4) };
        let mut data = vec![false; ninit * 256];
        for i in 0..ninit {
            let v = bits_of(&format!("INIT_{i:02X}"), 256);
            data[i * 256..i * 256 + 256].copy_from_slice(&v);
        }
        let mut datap = vec![false; ninitp * 256];
        for i in 0..ninitp {
            let v = bits_of(&format!("INITP_{i:02X}"), 256);
            datap[i * 256..i * 256 + 256].copy_from_slice(&v);
        }
        if wide {
            let mut d = vec![false; data.len()];
            for (w, &b) in data.iter().enumerate() {
                d[(w & 0xf) | (w & 0x10) << 9 | (w & 0x3fe0) >> 1] = b;
            }
            data = d;
            let mut d = vec![false; datap.len()];
            for (w, &b) in datap.iter().enumerate() {
                d[(w & 1) | (w & 2) << 9 | (w & 0x7fc) >> 1] = b;
            }
            datap = d;
        }
        for (hi, &h) in halves.iter().enumerate() {
            self.set_bits(h, "DATA", &data[hi * 0x2000..hi * 0x2000 + 0x2000]);
            self.set_bits(h, "DATAP", &datap[hi * 0x400..hi * 0x400 + 0x400]);
        }
        // A tile used as RAMB8s is configured combined while its contents load,
        // then split by a second write of that frame (as ISE does).
        if kind == "RAMB8" {
            for half in ["BRAM[0]", "BRAM[1]"] {
                let h = cell.bel(self.slot(half));
                self.set_bits(h, "COMBINE", &[true]);
                self.fixup = true;
                self.set_bits(h, "COMBINE", &[false]);
                self.fixup = false;
            }
        }
        Ok(())
    }

    /// A raw number into a bit-vector attribute (LSB first, polarity ignored).
    fn set_raw_num(&mut self, bcrd: BelCoord, name: &str, v: u64) {
        let n = self.attr_len(bcrd, name);
        let bits: Vec<bool> = (0..n).map(|i| v >> i & 1 != 0).collect();
        self.set_raw_bits(bcrd, name, &bits);
    }

    /// Set raw bits `idx` of the switchbox mux driving the wire named `dst`
    /// in tile `tcrd` (defaults ISE writes for unused CMT inputs).
    fn set_mux_bits_raw(&mut self, tcrd: TileCoord, dst: &str, idx: &[usize]) {
        let tcls = &self.int.tile_classes[self.edev[tcrd].class];
        let mut bits = vec![];
        for (_, info) in &tcls.bels {
            if let BelInfo::SwitchBox(sb) = info {
                for item in &sb.items {
                    if let SwitchBoxItem::Mux(m) = item
                        && m.dst.to_string(self.int, tcls).ends_with(dst)
                    {
                        bits.extend(idx.iter().map(|&i| m.bits[i]));
                    }
                }
            }
        }
        assert!(!bits.is_empty(), "no mux for {dst}");
        for b in bits {
            self.set_tb(tcrd, b, true);
        }
    }

    /// Clock management: DCM, PLL, BUFIO2, BUFIO2FB.
    /// Values are as ISE 14.7 bitgen writes them; the PLL loop settings come
    /// from the PLL_MULT table (BANDWIDTH OPTIMIZED = the HIGH column).
    fn cmt_config(&mut self, c: &NCell, cell: CellCoord) -> Result<(), String> {
        let belname = c.bel.2.clone();
        let bcrd = cell.bel(self.slot(&belname));
        let p = |k: &str| c.params.get(k).map(|v| v.trim_matches('"').to_string());
        let num = |k: &str, d: u64| -> u64 {
            p(k).map(|v| if v.len() > 8 && v.chars().all(|ch| ch == '0' || ch == '1') { u64::from_str_radix(&v, 2).unwrap() } else { v.parse().unwrap_or(d) }).unwrap_or(d)
        };
        match c.typ.as_str() {
            "BUFIO2" => {
                // Divide 1, bypassed, feeding the CMT (DIVCLK_CMT).
                self.set_bits(bcrd, "ENABLE", &[true]);
                self.set_bits(bcrd, "CMT_ENABLE", &[true]);
                // DIVIDE stays 000 when bypassed (as ISE writes it).
                self.set_bits(bcrd, "DIVIDE_BYPASS", &[true]);
            }
            "BUFIO2FB" => {
                // A PLL's BUFG feedback, bypassed; ISE also sets CMT_ENABLE on
                // the BUFIO2 of the same index.
                self.set_bits(bcrd, "ENABLE", &[true]);
                let n = belname.trim_start_matches("BUFIO2FB");
                let pair = cell.bel(self.slot(&format!("BUFIO2{n}")));
                self.set_bits(pair, "CMT_ENABLE", &[true]);
            }
            "DCM" => {
                let used: Vec<&str> = c.conns.keys().map(String::as_str).collect();
                if used.contains(&"PSDONE") {
                    return Err(format!("{}: DCM dynamic phase shift (PSDONE) is not supported yet", c.name));
                }
                let feedback = p("CLK_FEEDBACK").unwrap_or("1X".into());
                if !matches!(feedback.as_str(), "1X" | "NONE") {
                    return Err(format!("{}: DCM CLK_FEEDBACK = {feedback} is not supported yet (1X or NONE)", c.name));
                }
                for (attr, ok) in [
                    ("DLL_FREQUENCY_MODE", "LOW"),
                    ("DUTY_CYCLE_CORRECTION", "TRUE"),
                ] {
                    if let Some(v) = p(attr)
                        && v != ok
                    {
                        return Err(format!("{}: DCM {attr} = {v} is not supported yet ({ok})", c.name));
                    }
                }
                let dll = feedback == "1X";
                let phase_mode = p("CLKOUT_PHASE_SHIFT").unwrap_or("NONE".into());
                if !matches!(phase_mode.as_str(), "NONE" | "FIXED") {
                    return Err(format!("{}: DCM CLKOUT_PHASE_SHIFT = {phase_mode} is not supported yet (NONE or FIXED)", c.name));
                }
                // ISE's register blocks first: some of their bits are the same
                // bits as the fields below (CLKFX_*, DESKEW_ADJUST, ...), which
                // must win.
                for (attr, bits) in [
                    ("REG_DFS_S", &[2usize, 9, 10, 17, 21, 24, 26, 32, 33, 37, 41, 43, 45, 52, 64, 68, 76, 77][..]),
                    ("REG_DLL_C", &[0, 1, 17][..]),
                    ("REG_DLL_S", &[0, 6, 8][..]),
                    ("REG_INTERFACE", &[3, 5, 9, 10, 12, 13, 18, 37][..]),
                ] {
                    let n = self.attr_len(bcrd, attr);
                    let raw: Vec<bool> = (0..n).map(|i| bits.contains(&i)).collect();
                    self.set_raw_bits(bcrd, attr, &raw);
                }
                let (m, d) = (num("CLKFX_MULTIPLY", 4), num("CLKFX_DIVIDE", 1));
                self.set_raw_num(bcrd, "CLKFX_MULTIPLY", m - 1);
                self.set_raw_num(bcrd, "CLKFX_DIVIDE", d - 1);
                let deskew = match p("DESKEW_ADJUST").as_deref() {
                    None | Some("SYSTEM_SYNCHRONOUS") => 5,
                    Some("SOURCE_SYNCHRONOUS") => 0,
                    Some(v) => v.parse().unwrap_or(5),
                };
                self.set_raw_num(bcrd, "DESKEW_ADJUST", deskew);
                for (attr, v) in [
                    ("CLKFXDV_DIVIDE", 3u64), ("CLKIN_CLKFB_ENABLE", 1),
                    ("DUTY_CYCLE_CORRECTION", 1), ("REG_DFS_C", 5), ("SPREAD_SPECTRUM", 0x7f),
                ] {
                    self.set_raw_num(bcrd, attr, v);
                }
                // CLKDV_DIVIDE (1.5 .. 7.5 in halves, 2 .. 16): as ISE encodes
                // every legal value.
                let dv = p("CLKDV_DIVIDE").and_then(|v| v.parse::<f64>().ok()).unwrap_or(2.0);
                let half = (dv * 2.0).round() as u64;
                if !(3..=32).contains(&half) || (half % 2 == 1 && half > 15) {
                    return Err(format!("{}: DCM CLKDV_DIVIDE = {dv} is not a legal value", c.name));
                }
                let (mode, max, fall, fall2, phase) = if half % 2 == 0 {
                    let d = half / 2;
                    (1, d - 1, (d - 1) / 2, 0, if d % 2 == 1 { 2 } else { 0 })
                } else {
                    let k = (half - 1) / 2;
                    (0, half - 1, k / 2, 3 * k / 2 + 1, if k % 2 == 1 { 3 } else { 1 })
                };
                for (attr, v) in [
                    ("CLKDV_MODE", mode), ("CLKDV_COUNT_MAX", max), ("CLKDV_COUNT_FALL", fall),
                    ("CLKDV_COUNT_FALL_2", fall2), ("CLKDV_PHASE_FALL", phase),
                ] {
                    self.set_raw_num(bcrd, attr, v);
                }
                // Fixed phase shift (1/256 of the period, -255 .. 255), as ISE
                // encodes it: twice the magnitude plus a sign bit; from 128 on
                // it wraps to the equivalent shift the other way.
                self.set_raw_num(bcrd, "CLKOUT_PHASE_SHIFT", if phase_mode == "FIXED" { 3 } else { 1 });
                if phase_mode == "FIXED" {
                    let v = num("PHASE_SHIFT", 0) as u32 as i32 as i64;
                    if !(-255..=255).contains(&v) {
                        return Err(format!("{}: DCM PHASE_SHIFT = {v} is out of range (-255 .. 255)", c.name));
                    }
                    let a = v.unsigned_abs();
                    let (field, neg) = if a < 128 { (2 * a, v < 0) } else { (2 * (256 - a) - 1, v > 0) };
                    self.set_raw_num(bcrd, "PHASE_SHIFT", field);
                    self.set_raw_num(bcrd, "PHASE_SHIFT_NEGATIVE", neg as u64);
                }
                if dll {
                    // DLL in use (CLK0 fed back): as ISE sets it.
                    for attr in ["DLL_ENABLE", "DFS_FEEDBACK", "CLKIN_IOB"] {
                        self.set_raw_num(bcrd, attr, 1);
                    }
                    // Shared with REG_DLL_S's default bits (written above).
                    self.set_raw_num(bcrd, "CLK_FEEDBACK_DISABLE", 0);
                } else {
                    self.set_raw_num(bcrd, "CLK_FEEDBACK_DISABLE", 1);
                }
                // The frequency synthesizer only when CLKFX/CLKFX180 is used (ISE
                // clears these otherwise; some share bits with REG_DFS_S's
                // defaults, written above).
                let dfs = used.contains(&"CLKFX") || used.contains(&"CLKFX180");
                self.set_raw_num(bcrd, "DFS_ENABLE", dfs as u64);
                self.set_raw_num(bcrd, "OUT_CLKFX_ENABLE", dfs as u64);
                if !dfs {
                    self.set_raw_num(bcrd, "DFS_FEEDBACK", 0);
                }
                for out in ["CLK0", "CLK90", "CLK180", "CLK270", "CLK2X", "CLK2X180", "CLKDV"] {
                    if used.contains(&out) {
                        if !dll {
                            return Err(format!("{}: DCM output {out} needs CLK_FEEDBACK = \"1X\"", c.name));
                        }
                        self.set_raw_num(bcrd, &format!("OUT_{out}_ENABLE"), 1);
                    }
                }
                // Unconnected test/control inputs: inversion bits set.
                for pin in ["CTLMODE", "CTLOSC1", "CTLOSC2", "FREEZEDFS", "FREEZEDLL", "STSADRS"] {
                    self.set_input_inv_raw(bcrd, pin, true);
                }
                // The CMT's voltage regulator, in this DCM's tile.
                let vreg = cell.bel(self.slot("CMT_VREG"));
                self.set_raw_num(vreg, "REG_REG", 0b1_0100_0011);
                self.dcm_vregs.push(vreg);
            }
            "PLL" => {
                // Counter for divide d: high/low times, odd-divide edge, bypass at 1.
                let counter = |g: &mut Self, name: &str, d: u64, en: bool| {
                    let (ht, lt) = if d == 1 { (1, 1) } else { (d / 2, d - d / 2) };
                    g.set_raw_num(bcrd, &format!("PLL_{name}_HT"), ht);
                    g.set_raw_num(bcrd, &format!("PLL_{name}_LT"), lt);
                    g.set_raw_num(bcrd, &format!("PLL_{name}_EDGE"), (d > 1 && d % 2 == 1) as u64);
                    g.set_raw_num(bcrd, &format!("PLL_{name}_NOCOUNT"), (d == 1) as u64);
                    if en {
                        g.set_raw_num(bcrd, &format!("PLL_{name}_EN"), 1);
                    }
                };
                let mult = num("CLKFBOUT_MULT", 1);
                counter(self, "CLKFBOUT", mult, true);
                counter(self, "CLKFBOUT2", 1, false);
                counter(self, "DIVCLK", num("DIVCLK_DIVIDE", 1), true);
                for i in 0..6 {
                    let used = c.conns.contains_key(&format!("CLKOUT{i}"));
                    counter(self, &format!("CLKOUT{i}"), num(&format!("CLKOUT{i}_DIVIDE"), 1), used);
                }
                let bw = match p("BANDWIDTH").as_deref().unwrap_or("OPTIMIZED") {
                    "LOW" => "LOW",
                    _ => "HIGH",
                };
                let row = format!("_{mult}");
                for (attr, field) in [("PLL_CP", format!("PLL_CP_{bw}")), ("PLL_RES", format!("PLL_RES_{bw}")), ("PLL_LFHF", format!("PLL_LFHF_{bw}"))] {
                    let v = self.table("PLL_MULT", &row, &field);
                    let raw: Vec<bool> = (0..v.len()).map(|i| v[i]).collect();
                    self.set_raw_bits(bcrd, attr, &raw);
                }
                for attr in ["PLL_LOCK_CNT", "PLL_LOCK_FB_DLY", "PLL_LOCK_REF_DLY", "PLL_LOCK_SAT_HIGH", "PLL_UNLOCK_CNT"] {
                    let v = self.table("PLL_MULT", &row, attr);
                    let raw: Vec<bool> = (0..v.len()).map(|i| v[i]).collect();
                    self.set_raw_bits(bcrd, attr, &raw);
                }
                let intfb = match p("COMPENSATION").as_deref().unwrap_or("SYSTEM_SYNCHRONOUS") {
                    "INTERNAL" => 2,
                    // Feedback through a BUFG and a BUFIO2FB. (The uarch turns
                    // direct feedback into INTERNAL, as ISE's map does.)
                    "SYSTEM_SYNCHRONOUS" | "SOURCE_SYNCHRONOUS" => 0,
                    v => return Err(format!("{}: PLL COMPENSATION = {v} is not supported yet", c.name)),
                };
                self.set_raw_num(bcrd, "PLL_INTFB", intfb);
                if intfb == 0 {
                    // Compensated (feedback through a BUFIO2FB): as ISE sets it.
                    self.set_raw_num(bcrd, "PLL_EN_DLY", 1);
                    self.set_raw_num(bcrd, "PLL_IN_DLY_MX_SEL", 0b1010);
                }
                let clkin2 = p("CLKIN_SOURCE").as_deref() == Some("CLKIN2");
                self.set_raw_num(bcrd, "CLKINSEL_STATIC_VAL", clkin2 as u64);
                for (attr, v) in [
                    ("ENABLE", 1u64), ("PLL_ADD_LEAKAGE", 2), ("PLL_AVDD_COMP_SET", 2), ("PLL_CLAMP_BYPASS", 1),
                    ("PLL_CLAMP_REF_SEL", 1), ("PLL_CLKCNTRL", 1), ("PLL_CLK_LOST_DETECT", 1), ("PLL_CP_REPL", 1),
                    ("PLL_DVDD_COMP_SET", 2), ("PLL_EN", 1), ("PLL_EN_LEAKAGE", 2), ("PLL_EN_VCO0", 1),
                    ("PLL_EN_VCO1", 1), ("PLL_EN_VCO2", 1), ("PLL_EN_VCO3", 1), ("PLL_EN_VCO4", 1), ("PLL_EN_VCO5", 1),
                    ("PLL_EN_VCO6", 1), ("PLL_EN_VCO7", 1), ("PLL_EN_VCO_DIV1", 1), ("PLL_EN_VCO_DIV6", 1),
                    ("PLL_PFD_CNTRL", 8), ("PLL_PFD_DLY", 1), ("PLL_PWRD_CFG", 1), ("PLL_SEL_SLIPD", 1),
                    ("PLL_TEST_IN_WINDOW", 1),
                ] {
                    self.set_raw_num(bcrd, attr, v);
                }
                let en_cntrl: &[usize] = &[
                    10, 15, 20, 22, 23, 24, 25, 26, 28, 31, 32, 33, 34, 35, 36, 38, 42, 44, 46, 47, 48, 49, 54, 55, 58, 59,
                    60, 64, 65, 66, 67, 70, 74,
                ];
                let raw: Vec<bool> = (0..85).map(|i| en_cntrl.contains(&i)).collect();
                self.set_raw_bits(bcrd, "PLL_EN_CNTRL", &raw);
                // Unused SKEWCLKIN inputs: mux defaults.
                let tcrd = self.edev.bel_tile(bcrd);
                self.cmt_used = true;
                self.set_mux_bits_raw(tcrd, "OMUX_PLL_SKEWCLKIN1", &[1, 2]);
                self.set_mux_bits_raw(tcrd, "OMUX_PLL_SKEWCLKIN2", &[1, 2]);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Settings that depend on the design as a whole.
    pub fn global(&mut self, cells: &[NCell]) {
        // RAM/SRL LUTs change at run time: ISE masks the LUT frames of their
        // column region (GLUTMASK in the region's HCLK tile).
        let mut masked: HashSet<(usize, RowId)> = HashSet::new();
        for c in cells.iter().filter(|c| c.typ == "SLICE_LUTRAM") {
            let (x, y, _) = c.bel;
            masked.insert((x, self.edev.chip.row_hclk(RowId::from_idx(y))));
        }
        for (x, hrow) in masked {
            let cell = CellCoord { die: DieId::from_idx(0), col: ColId::from_idx(x), row: hrow };
            let b = cell.bel(self.slot("GLUTMASK_HCLK"));
            for f in ["FRAME21", "FRAME22", "FRAME24", "FRAME25"] {
                self.set_bits(b, f, &[true]);
            }
        }
        // REG_BG in the CMT regulators, as ISE writes it: with a DCM or PLL
        // in use every DCM tile's regulator loses the default trim bit; with a
        // PLL, all of them get REG_BG; otherwise the tiles of the DCMs used.
        if self.cmt_used || !self.dcm_vregs.is_empty() {
            for b in self.all_bels("CMT_VREG") {
                let tcrd = self.edev.bel_tile(b);
                if self.int.tile_classes[self.edev[tcrd].class].name == "CMT_DCM" {
                    self.set_raw_num(b, "REG_BG", 0);
                }
            }
        }
        for b in std::mem::take(&mut self.dcm_vregs) {
            self.set_raw_num(b, "REG_BG", 0b101_0010_0000);
        }
        if self.cmt_used {
            for b in self.all_bels("CMT_VREG") {
                let tcrd = self.edev.bel_tile(b);
                if self.int.tile_classes[self.edev[tcrd].class].name == "CMT_DCM" {
                    self.set_raw_num(b, "REG_BG", 0b101_0010_0000);
                }
            }
        }
        // PairMux settings: the pattern whose sources match every used output.
        let pms: Vec<_> = self.pair_muxes.drain().collect();
        for ((tcrd, bslot, idx), used) in pms {
            let tcls = &self.int.tile_classes[self.edev[tcrd].class];
            let BelInfo::SwitchBox(sb) = &tcls.bels[bslot] else { unreachable!() };
            let SwitchBoxItem::PairMux(pm) = &sb.items[idx] else { unreachable!() };
            let (_, pat) = pm
                .src
                .iter()
                .find(|(key, _)| (0..2).all(|k| used[k].is_none_or(|u| key[k].is_some_and(|s| s.tw == u))))
                .expect("no pair mux setting for this combination");
            let (pat, bits) = (pat.clone(), pm.bits.clone());
            for (i, b) in bits.into_iter().enumerate() {
                self.set_tb(tcrd, b, pat[i]);
            }
        }
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
