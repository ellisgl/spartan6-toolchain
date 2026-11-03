//! nextpnr himbaechel chip database (.bba) writer: the parts of nextpnr's
//! himbaechel_dbgen (`nextpnr/himbaechel/himbaechel_dbgen/chip.py`) this
//! toolchain uses, writing the same text (checked byte for byte against it;
//! only the generator name differs), so `bbasm` gives the same binary.
//! Keep it in step with `ChipInfoPOD` (`nextpnr/himbaechel/chipdb.h`,
//! version 6).

use std::collections::HashMap;
use std::fmt::Write as _;

pub const BEL_FLAG_GLOBAL: u32 = 0x01;

const MODE_TILE_WIRE: i64 = 0x7000;
const MODE_IS_ROOT: i64 = 0x7001;

#[derive(Default)]
pub struct StringPool {
    idx: HashMap<String, u32>,
    strs: Vec<String>,
    known: u32,
}

impl StringPool {
    fn new() -> Self {
        let mut p = StringPool::default();
        p.id("");
        p.known = 1;
        p
    }

    pub fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.idx.get(s) {
            return i;
        }
        let i = self.strs.len() as u32;
        self.idx.insert(s.to_string(), i);
        self.strs.push(s.to_string());
        i
    }

    /// Preload a uarch's constids.inc ("X(NAME)" lines, in order).
    pub fn read_constids(&mut self, text: &str) {
        let mut n = 1;
        for line in text.lines() {
            let l = line.trim();
            let Some(rest) = l.strip_prefix("X(") else { continue };
            let name = rest.strip_suffix(')').expect("constids line").trim();
            assert_eq!(self.id(name), n, "constid {name} out of order");
            n += 1;
        }
        self.known = n;
    }
}

pub struct BelPin {
    name: u32,
    wire: u32,
    dir: u32,
}

pub struct Bel {
    name: u32,
    bel_type: u32,
    z: u32,
    pub flags: u32,
    pub site: u32,
    pins: Vec<BelPin>,
}

struct TileWire {
    name: u32,
    wire_type: u32,
    const_value: u32,
    uphill: Vec<u32>,
    downhill: Vec<u32>,
    bel_pins: Vec<(u32, u32)>,
}

pub struct Pip {
    src: u32,
    dst: u32,
    pub flags: u32,
    timing_idx: i64,
}

pub struct TileType {
    type_name: u32,
    pub bels: Vec<Bel>,
    wires: Vec<TileWire>,
    pub pips: Vec<Pip>,
    wire_idx: HashMap<u32, u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PinDir {
    Input = 0,
    Output = 1,
    Inout = 2,
}

#[derive(Clone, Copy)]
pub enum ClockEdge {
    Rising = 0,
    Falling = 1,
}

struct RegArc {
    clock: u32,
    edge: u32,
    setup: i64,
    hold: i64,
    clk_q: i64,
}

struct CellPinTiming {
    pin: u32,
    flags: u32,
    comb: Vec<(u32, i64)>,
    reg: Vec<RegArc>,
}

pub struct CellTiming {
    type_variant: u32,
    pin_data: Vec<(String, CellPinTiming)>,
    pin_idx: HashMap<String, usize>,
}

struct SpeedGrade {
    name: u32,
    pip_classes: Vec<Option<i64>>,
    cell_types: Vec<CellTiming>,
}

#[derive(Default)]
struct TimingPool {
    grades: Vec<SpeedGrade>,
    grade_idx: HashMap<String, usize>,
    pip_classes: HashMap<String, i64>,
}

impl TimingPool {
    fn pip_class_idx(&mut self, name: &str) -> i64 {
        if name.is_empty() {
            return -1;
        }
        let n = self.pip_classes.len() as i64;
        *self.pip_classes.entry(name.to_string()).or_insert(n)
    }
}

struct TileInst {
    type_idx: Option<u32>,
    shape: Vec<i64>,
    shape_idx: usize,
}

struct Pad {
    package_pin: u32,
    tile: u32,
    bel: u32,
    function: u32,
    bank: u32,
}

struct Package {
    name: u32,
    pads: Vec<Pad>,
}

pub struct Chip {
    pub strs: StringPool,
    uarch: String,
    name: String,
    width: usize,
    height: usize,
    pub tile_types: Vec<TileType>,
    tile_type_idx: HashMap<String, u32>,
    tiles: Vec<Vec<TileInst>>,
    node_shapes: Vec<(Vec<i64>, i64)>,
    node_shape_idx: HashMap<Vec<i64>, usize>,
    tile_shapes: Vec<Vec<i64>>,
    packages: Vec<Package>,
    timing: TimingPool,
}

impl Chip {
    pub fn new(uarch: &str, name: &str, width: usize, height: usize) -> Self {
        let tiles = (0..height)
            .map(|_| (0..width).map(|_| TileInst { type_idx: None, shape: vec![], shape_idx: 0 }).collect())
            .collect();
        Chip {
            strs: StringPool::new(),
            uarch: uarch.into(),
            name: name.into(),
            width,
            height,
            tile_types: vec![],
            tile_type_idx: HashMap::new(),
            tiles,
            node_shapes: vec![],
            node_shape_idx: HashMap::new(),
            tile_shapes: vec![],
            packages: vec![],
            timing: TimingPool::default(),
        }
    }

    pub fn set_speed_grades(&mut self, grades: &[String]) {
        for (i, g) in grades.iter().enumerate() {
            let name = self.strs.id(g);
            self.timing.grades.push(SpeedGrade { name, pip_classes: vec![], cell_types: vec![] });
            self.timing.grade_idx.insert(g.clone(), i);
        }
    }

    pub fn create_tile_type(&mut self, name: &str) -> usize {
        let type_name = self.strs.id(name);
        self.tile_type_idx.insert(name.to_string(), self.tile_types.len() as u32);
        self.tile_types.push(TileType { type_name, bels: vec![], wires: vec![], pips: vec![], wire_idx: HashMap::new() });
        self.tile_types.len() - 1
    }

    pub fn create_wire(&mut self, tt: usize, name: &str, kind: &str, const_value: &str) {
        let name = self.strs.id(name);
        let wire_type = self.strs.id(kind);
        let const_value = self.strs.id(const_value);
        let t = &mut self.tile_types[tt];
        t.wire_idx.insert(name, t.wires.len() as u32);
        t.wires.push(TileWire { name, wire_type, const_value, uphill: vec![], downhill: vec![], bel_pins: vec![] });
    }

    pub fn create_pip(&mut self, tt: usize, src: &str, dst: &str, timing_class: &str) -> usize {
        let (s, d) = (self.strs.id(src), self.strs.id(dst));
        let timing_idx = self.timing.pip_class_idx(timing_class);
        let t = &mut self.tile_types[tt];
        let (src, dst) = (t.wire_idx[&s], t.wire_idx[&d]);
        let i = t.pips.len() as u32;
        t.wires[src as usize].downhill.push(i);
        t.wires[dst as usize].uphill.push(i);
        t.pips.push(Pip { src, dst, flags: 0, timing_idx });
        i as usize
    }

    pub fn create_bel(&mut self, tt: usize, name: &str, kind: &str, z: u32) -> usize {
        let name = self.strs.id(name);
        let bel_type = self.strs.id(kind);
        let t = &mut self.tile_types[tt];
        t.bels.push(Bel { name, bel_type, z, flags: 0, site: 0, pins: vec![] });
        t.bels.len() - 1
    }

    pub fn add_bel_pin(&mut self, tt: usize, bel: usize, pin: &str, wire: &str, dir: PinDir) {
        let pin = self.strs.id(pin);
        let w = self.strs.id(wire);
        let t = &mut self.tile_types[tt];
        let wire = t.wire_idx[&w];
        t.bels[bel].pins.push(BelPin { name: pin, wire, dir: dir as u32 });
        t.wires[wire as usize].bel_pins.push((bel as u32, pin));
    }

    pub fn set_tile_type(&mut self, x: usize, y: usize, name: &str) {
        self.tiles[y][x].type_idx = Some(self.tile_type_idx[name]);
    }

    /// Join tile wires (x, y, wire index) into one node.
    pub fn add_node(&mut self, wires: &[[usize; 3]]) {
        let twos = |x: i64| if x & 0x8000 != 0 { x - 0x10000 } else { x };
        let (x0, y0) = (wires[0][0] as i64, wires[0][1] as i64);
        let mut shape = vec![];
        for w in wires {
            shape.extend([w[0] as i64 - x0, w[1] as i64 - y0, w[2] as i64]);
        }
        let mut key = shape.clone();
        key.push(-1); // timing index
        let shape_idx = match self.node_shape_idx.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.node_shapes.len();
                self.node_shape_idx.insert(key, i);
                self.node_shapes.push((shape.clone(), -1));
                i
            }
        };
        for (i, w) in wires.iter().enumerate() {
            let inst = &mut self.tiles[w[1]][w[0]];
            let wi = shape[i * 3 + 2] as usize;
            while 3 * wi >= inst.shape.len() {
                inst.shape.extend([MODE_TILE_WIRE, 0, 0]);
            }
            assert_eq!(inst.shape[3 * wi], MODE_TILE_WIRE, "wire in two nodes");
            if i == 0 {
                inst.shape[3 * wi] = MODE_IS_ROOT;
                inst.shape[3 * wi + 1] = twos(shape_idx as i64 & 0xFFFF);
                inst.shape[3 * wi + 2] = (shape_idx as i64 >> 16) & 0xFFFF;
            } else {
                inst.shape[3 * wi] = x0 - w[0] as i64;
                inst.shape[3 * wi + 1] = y0 - w[1] as i64;
                inst.shape[3 * wi + 2] = shape[2];
            }
        }
    }

    pub fn create_package(&mut self, name: &str) -> usize {
        let name = self.strs.id(name);
        self.packages.push(Package { name, pads: vec![] });
        self.packages.len() - 1
    }

    pub fn create_pad(&mut self, pkg: usize, pin: &str, tile: &str, bel: &str, function: &str, bank: u32) {
        let package_pin = self.strs.id(pin);
        let tile = self.strs.id(tile);
        let bel = self.strs.id(bel);
        let function = self.strs.id(function);
        self.packages[pkg].pads.push(Pad { package_pin, tile, bel, function, bank });
    }

    // --- timing ---

    pub fn set_pip_class(&mut self, grade: &str, name: &str, delay: i64) {
        let idx = self.timing.pip_class_idx(name) as usize;
        let sg = &mut self.timing.grades[self.timing.grade_idx[grade]];
        if idx >= sg.pip_classes.len() {
            sg.pip_classes.resize(idx + 1, None);
        }
        assert!(sg.pip_classes[idx].is_none(), "pip class {name} set twice");
        sg.pip_classes[idx] = Some(delay);
    }

    /// A cell timing variant; returns (grade, index) for the arc methods.
    pub fn add_cell_variant(&mut self, grade: &str, name: &str) -> (usize, usize) {
        let type_variant = self.strs.id(name);
        let g = self.timing.grade_idx[grade];
        let sg = &mut self.timing.grades[g];
        sg.cell_types.push(CellTiming { type_variant, pin_data: vec![], pin_idx: HashMap::new() });
        (g, sg.cell_types.len() - 1)
    }

    fn pin_timing(&mut self, cell: (usize, usize), pin: &str) -> usize {
        let ct = &self.timing.grades[cell.0].cell_types[cell.1];
        if let Some(&i) = ct.pin_idx.get(pin) {
            return i;
        }
        let id = self.strs.id(pin);
        let ct = &mut self.timing.grades[cell.0].cell_types[cell.1];
        ct.pin_idx.insert(pin.to_string(), ct.pin_data.len());
        ct.pin_data.push((pin.to_string(), CellPinTiming { pin: id, flags: 0, comb: vec![], reg: vec![] }));
        ct.pin_data.len() - 1
    }

    pub fn add_comb_arc(&mut self, cell: (usize, usize), from: &str, to: &str, delay: i64) {
        let p = self.pin_timing(cell, to);
        let from = self.strs.id(from);
        self.timing.grades[cell.0].cell_types[cell.1].pin_data[p].1.comb.push((from, delay));
    }

    pub fn add_setup_hold(&mut self, cell: (usize, usize), clock: &str, pin: &str, edge: ClockEdge, setup: i64, hold: i64) {
        let p = self.pin_timing(cell, pin);
        let c = self.pin_timing(cell, clock);
        let clock = self.strs.id(clock);
        let ct = &mut self.timing.grades[cell.0].cell_types[cell.1];
        ct.pin_data[p].1.reg.push(RegArc { clock, edge: edge as u32, setup, hold, clk_q: 0 });
        ct.pin_data[c].1.flags |= 1;
    }

    pub fn add_clock_out(&mut self, cell: (usize, usize), clock: &str, pin: &str, edge: ClockEdge, delay: i64) {
        let p = self.pin_timing(cell, pin);
        let c = self.pin_timing(cell, clock);
        let clock = self.strs.id(clock);
        let ct = &mut self.timing.grades[cell.0].cell_types[cell.1];
        ct.pin_data[p].1.reg.push(RegArc { clock, edge: edge as u32, setup: 0, hold: 0, clk_q: delay });
        ct.pin_data[c].1.flags |= 1;
    }

    // --- writing ---

    pub fn write_bba(mut self) -> String {
        // Timing: cell types by variant, pins by name, arcs by pin (stable).
        for sg in &mut self.timing.grades {
            sg.cell_types.sort_by_key(|c| c.type_variant);
            for ct in &mut sg.cell_types {
                ct.pin_data.sort_by_key(|p| p.1.pin);
                for (_, p) in &mut ct.pin_data {
                    p.comb.sort_by_key(|a| a.0);
                    p.reg.sort_by_key(|a| a.clock);
                }
            }
        }
        let mut b = Bba(String::with_capacity(256 << 20));
        b.raw("pre #include \"nextpnr.h\"");
        b.raw("pre NEXTPNR_NAMESPACE_BEGIN");
        b.raw("post NEXTPNR_NAMESPACE_END");
        b.raw("push chipdb_blob");
        b.r#ref("chip_info");
        self.serialise(&mut b);
        b.raw("pop");
        b.0
    }

    fn serialise(&mut self, b: &mut Bba) {
        // Deduplicate tile routing shapes (row by row).
        let mut shape_idx: HashMap<Vec<i64>, usize> = HashMap::new();
        for row in &mut self.tiles {
            for t in row {
                t.shape_idx = *shape_idx.entry(t.shape.clone()).or_insert_with(|| {
                    self.tile_shapes.push(t.shape.clone());
                    self.tile_shapes.len() - 1
                });
            }
        }
        // Lists.
        for (i, tt) in self.tile_types.iter_mut().enumerate() {
            let c = format!("tt{i}");
            for (j, bel) in tt.bels.iter_mut().enumerate() {
                bel.pins.sort_by_key(|p| p.name);
                b.label(&format!("{c}_bel{j}_pins"));
                for p in &bel.pins {
                    b.u32(p.name as i64);
                    b.u32(p.wire as i64);
                    b.u32(p.dir as i64);
                }
            }
            for (j, w) in tt.wires.iter().enumerate() {
                let wc = format!("{c}_wire{j}");
                b.label(&format!("{wc}_pips_uh"));
                for &p in &w.uphill {
                    b.u32(p as i64);
                }
                b.label(&format!("{wc}_pips_dh"));
                for &p in &w.downhill {
                    b.u32(p as i64);
                }
                b.label(&format!("{wc}_bel_pins"));
                for &(bel, pin) in &w.bel_pins {
                    b.u32(bel as i64);
                    b.u32(pin as i64);
                }
            }
            b.label(&format!("{c}_bels"));
            for (j, bel) in tt.bels.iter().enumerate() {
                b.u32(bel.name as i64);
                b.u32(bel.bel_type as i64);
                b.u16(bel.z as i64);
                b.u16(0);
                b.u32(bel.flags as i64);
                b.u32(bel.site as i64);
                b.u32(0);
                b.slice(&format!("{c}_bel{j}_pins"), bel.pins.len());
                b.u32(0);
            }
            b.label(&format!("{c}_wires"));
            for (j, w) in tt.wires.iter().enumerate() {
                let wc = format!("{c}_wire{j}");
                b.u32(w.name as i64);
                b.u32(w.wire_type as i64);
                b.u32(0);
                b.u32(w.const_value as i64);
                b.u32(0);
                b.u32(-1);
                b.slice(&format!("{wc}_pips_uh"), w.uphill.len());
                b.slice(&format!("{wc}_pips_dh"), w.downhill.len());
                b.slice(&format!("{wc}_bel_pins"), w.bel_pins.len());
            }
            b.label(&format!("{c}_pips"));
            for p in &tt.pips {
                b.u32(p.src as i64);
                b.u32(p.dst as i64);
                b.u32(0);
                b.u32(p.flags as i64);
                b.u32(p.timing_idx);
                b.u32(0);
            }
            b.label(&format!("{c}_groups"));
        }
        for (i, (wires, _)) in self.node_shapes.iter().enumerate() {
            b.label(&format!("nshp{i}_wires"));
            for &w in wires {
                b.u16(w);
            }
            if wires.len() % 2 != 0 {
                b.u16(0);
            }
        }
        for (i, s) in self.tile_shapes.iter().enumerate() {
            b.label(&format!("tshp{i}_w2n"));
            for &x in s {
                b.u16(x);
            }
            if s.len() % 2 != 0 {
                b.u16(0);
            }
        }
        for (i, pkg) in self.packages.iter().enumerate() {
            b.label(&format!("pkg{i}_pads"));
            for p in &pkg.pads {
                b.u32(p.package_pin as i64);
                b.u32(p.tile as i64);
                b.u32(p.bel as i64);
                b.u32(p.function as i64);
                b.u32(p.bank as i64);
                b.u32(0);
                b.u32(0);
            }
        }
        for (i, sg) in self.timing.grades.iter().enumerate() {
            let c = format!("sg{i}");
            for (j, ct) in sg.cell_types.iter().enumerate() {
                let cc = format!("{c}_cellty{j}");
                for (k, (_, p)) in ct.pin_data.iter().enumerate() {
                    let pc = format!("{cc}_pin{k}");
                    b.label(&format!("{pc}_comb"));
                    for &(from, d) in &p.comb {
                        b.u32(from as i64);
                        b.tv(d);
                    }
                    b.label(&format!("{pc}_reg"));
                    for a in &p.reg {
                        b.u32(a.clock as i64);
                        b.u32(a.edge as i64);
                        b.tv(a.setup);
                        b.tv(a.hold);
                        b.tv(a.clk_q);
                    }
                }
                b.label(&format!("{cc}_pins"));
                for (k, (_, p)) in ct.pin_data.iter().enumerate() {
                    let pc = format!("{cc}_pin{k}");
                    b.u32(p.pin as i64);
                    b.u32(p.flags as i64);
                    b.slice(&format!("{pc}_comb"), p.comb.len());
                    b.slice(&format!("{pc}_reg"), p.reg.len());
                }
            }
            b.label(&format!("{c}_pip_classes"));
            for p in &sg.pip_classes {
                let d = p.expect("pip class without a delay");
                b.tv(d); // internal delay
                b.tv(0); // input capacitance
                b.tv(0); // output resistance
                b.u32(1); // buffered
            }
            b.label(&format!("{c}_node_classes"));
            b.label(&format!("{c}_cell_types"));
            for (j, ct) in sg.cell_types.iter().enumerate() {
                b.u32(ct.type_variant as i64);
                b.slice(&format!("{c}_cellty{j}_pins"), ct.pin_data.len());
            }
        }
        b.label("constids_strs");
        for s in &self.strs.strs[self.strs.known as usize..] {
            b.str(s);
        }
        // Top-level arrays.
        b.label("tile_types");
        for (i, tt) in self.tile_types.iter().enumerate() {
            let c = format!("tt{i}");
            b.u32(tt.type_name as i64);
            b.slice(&format!("{c}_bels"), tt.bels.len());
            b.slice(&format!("{c}_wires"), tt.wires.len());
            b.slice(&format!("{c}_pips"), tt.pips.len());
            b.slice(&format!("{c}_groups"), 0);
            b.u32(0);
        }
        b.label("node_shapes");
        for (i, (wires, timing)) in self.node_shapes.iter().enumerate() {
            b.slice(&format!("nshp{i}_wires"), wires.len() / 3);
            b.u32(*timing);
        }
        b.label("tile_shapes");
        for (i, s) in self.tile_shapes.iter().enumerate() {
            b.slice(&format!("tshp{i}_w2n"), s.len() / 3);
            b.u32(-1);
        }
        b.label("packages");
        for (i, pkg) in self.packages.iter().enumerate() {
            b.u32(pkg.name as i64);
            b.slice(&format!("pkg{i}_pads"), pkg.pads.len());
            b.u32(0);
        }
        b.label("tile_insts");
        for row in &self.tiles {
            for t in row {
                b.u32(0);
                b.u32(t.type_idx.expect("tile without a type") as i64);
                b.u32(t.shape_idx as i64);
                b.u32(0);
            }
        }
        b.label("speed_grades");
        for (i, sg) in self.timing.grades.iter().enumerate() {
            let c = format!("sg{i}");
            b.u32(sg.name as i64);
            b.slice(&format!("{c}_pip_classes"), sg.pip_classes.len());
            b.slice(&format!("{c}_node_classes"), 0);
            b.slice(&format!("{c}_cell_types"), sg.cell_types.len());
            b.u32(0);
        }
        b.label("constids");
        b.u32(self.strs.known as i64);
        b.slice("constids_strs", self.strs.strs.len() - self.strs.known as usize);

        b.label("chip_info");
        b.u32(0x00ca7ca7);
        b.u32(6);
        b.u32(self.width as i64);
        b.u32(self.height as i64);
        b.str(&self.uarch);
        b.str(&self.name);
        b.str("s6-chipdb"); // generator
        b.slice("tile_types", self.tile_types.len());
        b.slice("tile_insts", self.width * self.height);
        b.slice("node_shapes", self.node_shapes.len());
        b.slice("tile_shapes", self.tile_shapes.len());
        b.slice("packages", self.packages.len());
        b.slice("speed_grades", self.timing.grades.len());
        b.r#ref("constids");
        b.u32(0);
    }
}

/// bbasm text, as himbaechel_dbgen's BBAWriter prints it (including the
/// space before the empty comment).
struct Bba(String);

impl Bba {
    fn raw(&mut self, s: &str) {
        self.0.push_str(s);
        self.0.push('\n');
    }
    fn label(&mut self, s: &str) {
        let _ = writeln!(self.0, "label {s}");
    }
    fn r#ref(&mut self, s: &str) {
        let _ = writeln!(self.0, "ref {s} ");
    }
    fn slice(&mut self, s: &str, n: usize) {
        let _ = writeln!(self.0, "ref {s} \nu32 {n}");
    }
    fn str(&mut self, s: &str) {
        let _ = writeln!(self.0, "str |{s}| ");
    }
    fn u16(&mut self, n: i64) {
        let _ = writeln!(self.0, "u16 {n} ");
    }
    fn u32(&mut self, n: i64) {
        let _ = writeln!(self.0, "u32 {n} ");
    }
    /// A TimingValue with all four corners the same.
    fn tv(&mut self, d: i64) {
        for _ in 0..4 {
            self.u32(d);
        }
    }
}
