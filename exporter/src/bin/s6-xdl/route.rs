//! Lookups used to write our routing as XDL pips.
//!
//! - [`RawGraph`]: ISE's own pip graph from the rawdump, for the few hops
//!   prjcombine does not model as routing (IOB <-> IOI site wiring) and for
//!   recognizing route-through pips.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use prjcombine_re_xilinx_rawdump::{Coord, NodeOrWire, Part, TkSitePinDir, WireId};

/// An ISE pip: (tile, from wire, to wire).
pub type IsePip = (String, String, String);

/// A site pin seen on a pip: the site's kind and name, and the pin's name.
pub struct SitePinRef {
    pub kind: String,
    pub site: String,
    pub pin: String,
}

pub struct RawGraph {
    part: Part,
    tile_by_name: HashMap<String, Coord>,
    /// Site name -> (tile, site index within its tile kind).
    site_by_name: HashMap<String, (Coord, usize)>,
    /// Per tile kind: from wire -> [(to wire, is_test)].
    fwd: Vec<HashMap<WireId, Vec<(WireId, bool)>>>,
    /// Per tile kind: wire -> (site index, pin name, is_input).
    site_pins: Vec<HashMap<WireId, (usize, String, bool)>>,
    /// Per tile kind: route-through edges the repair may use (input pin wire
    /// -> output pin wires of BUFH sites). ISE lists these as pips through the
    /// site, but they are not in the tile's pip list.
    rt_fwd: Vec<HashMap<WireId, Vec<WireId>>>,
}

/// Site kinds the repair may route through: pure buffers prjcombine folds into
/// its routing model (the BUFH horizontal clock buffers in HCLK_ROW muxes).
const REPAIR_RT_SITES: &[&str] = &["BUFH"];

impl RawGraph {
    pub fn load(path: &Path) -> Self {
        let part = Part::from_file(path).expect("read rawdump");
        let tile_by_name = part.tiles.iter().map(|(&c, t)| (t.name.clone(), c)).collect();
        let mut site_by_name = HashMap::new();
        for (&c, t) in &part.tiles {
            let tk = &part.tile_kinds[t.kind];
            for (si, (sid, _, _)) in tk.sites.iter().enumerate() {
                if let Some(n) = t.sites.get(sid) {
                    site_by_name.insert(n.clone(), (c, si));
                }
            }
        }
        let mut fwd = vec![];
        let mut site_pins = vec![];
        let mut rt_fwd = vec![];
        for (_, _, tk) in &part.tile_kinds {
            let mut m: HashMap<WireId, Vec<(WireId, bool)>> = HashMap::new();
            for (_, &(wf, wt), pip) in &tk.pips {
                m.entry(wf).or_default().push((wt, pip.is_test));
            }
            fwd.push(m);
            let mut sp = HashMap::new();
            for (si, (_, _, site)) in tk.sites.iter().enumerate() {
                for (pname, pin) in &site.pins {
                    if let Some(w) = pin.wire {
                        sp.insert(w, (si, pname.clone(), pin.dir == TkSitePinDir::Input));
                    }
                }
            }
            site_pins.push(sp);
            let mut rt: HashMap<WireId, Vec<WireId>> = HashMap::new();
            for (_, _, site) in &tk.sites {
                if !REPAIR_RT_SITES.contains(&site.kind.as_str()) {
                    continue;
                }
                let ins = site.pins.values().filter(|p| p.dir == TkSitePinDir::Input).filter_map(|p| p.wire);
                let outs: Vec<WireId> =
                    site.pins.values().filter(|p| p.dir == TkSitePinDir::Output).filter_map(|p| p.wire).collect();
                for i in ins {
                    rt.entry(i).or_default().extend(outs.iter().copied());
                }
            }
            rt_fwd.push(rt);
        }
        RawGraph { part, tile_by_name, site_by_name, fwd, site_pins, rt_fwd }
    }

    fn members(&self, nw: NodeOrWire) -> Vec<(Coord, WireId)> {
        match nw {
            NodeOrWire::Node(ni) => self.part.iter_node(ni).collect(),
            NodeOrWire::Wire(crd, twi) => {
                let tk = &self.part.tile_kinds[self.part.tiles[&crd].kind];
                vec![(crd, *tk.wires.key(twi))]
            }
        }
    }

    fn lookup(&self, tile: &str, wire: &str) -> Option<NodeOrWire> {
        let crd = *self.tile_by_name.get(tile)?;
        self.part.lookup_wire(crd, wire)
    }

    /// Shortest chain of (non-test) ISE pips from `from` to `to`, at most
    /// `max_hops` long.
    pub fn find_path(&self, from: (&str, &str), to: (&str, &str), max_hops: usize) -> Option<Vec<IsePip>> {
        let start = self.lookup(from.0, from.1)?;
        let goal = self.lookup(to.0, to.1)?;
        let mut prev: HashMap<NodeOrWire, (NodeOrWire, IsePip)> = HashMap::new();
        let mut seen: HashSet<NodeOrWire> = HashSet::from([start]);
        let mut queue = VecDeque::from([(start, 0)]);
        while let Some((nw, depth)) = queue.pop_front() {
            if nw == goal {
                let mut path = vec![];
                let mut cur = nw;
                while let Some((p, pip)) = prev.get(&cur) {
                    path.push(pip.clone());
                    cur = *p;
                }
                path.reverse();
                return Some(path);
            }
            if depth == max_hops {
                continue;
            }
            for (crd, w) in self.members(nw) {
                let tile = &self.part.tiles[&crd];
                let Some(outs) = self.fwd[tile.kind.to_idx_usize()].get(&w) else { continue };
                for &(wt, is_test) in outs {
                    if is_test {
                        continue;
                    }
                    let Some(next) = self.part.lookup_wire_raw(crd, wt) else { continue };
                    if seen.insert(next) {
                        let pip = (tile.name.clone(), self.part.wires[w].clone(), self.part.wires[wt].clone());
                        prev.insert(next, (nw, pip));
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        None
    }

    /// Debug: ISE pips leaving `wire` in `tile` (following the wire's node),
    /// with their flags.
    pub fn debug_pips_from(&self, tile: &str, wire: &str) {
        let Some(nw) = self.lookup(tile, wire) else {
            println!("{tile}/{wire}: unknown");
            return;
        };
        for (crd, w) in self.members(nw) {
            let t = &self.part.tiles[&crd];
            let tk = &self.part.tile_kinds[t.kind];
            for (_, &(wf, wt), pip) in &tk.pips {
                if wf == w {
                    let p = (t.name.clone(), self.part.wires[wf].clone(), self.part.wires[wt].clone());
                    println!(
                        "  {} {} -> {} test={} excl={} rt={:?}",
                        p.0, p.1, p.2, pip.is_test, pip.is_excluded,
                        self.route_through(&p).map(|(s, o)| format!("{} {} {}->{}", s.kind, s.site, s.pin, o))
                    );
                }
            }
        }
    }

    /// The ISE wire (tile, wire name) of a site pin, by site name.
    pub fn site_pin_wire(&self, site: &str, pin: &str) -> Option<(String, String)> {
        let &(crd, si) = self.site_by_name.get(site)?;
        let tile = &self.part.tiles[&crd];
        let tk = &self.part.tile_kinds[tile.kind];
        let (_, _, s) = tk.sites.iter().nth(si)?;
        let w = s.pins.get(pin)?.wire?;
        Some((tile.name.clone(), self.part.wires[w].clone()))
    }

    /// Make a net's pips connected in ISE's graph: walk from the source pins
    /// along the net's pips; where a pip or sink pin is not reached, add the
    /// shortest chain of ISE pips from the reached set (buffers and
    /// terminations prjcombine models as plain wires). Returns the pips added.
    pub fn repair(
        &self,
        sources: &[(String, String)],
        sinks: &[(String, String)],
        pips: &mut std::collections::BTreeSet<IsePip>,
    ) -> Result<Vec<IsePip>, String> {
        let node = |t: &str, w: &str| self.lookup(t, w).ok_or_else(|| format!("unknown ISE wire {t}/{w}"));
        let mut added = vec![];
        for _ in 0..64 {
            // Reachable nodes from the sources through the net's own pips.
            let mut reached: HashSet<NodeOrWire> = HashSet::new();
            for (t, w) in sources {
                reached.insert(node(t, w)?);
            }
            let edges: Vec<(NodeOrWire, NodeOrWire)> =
                pips.iter().map(|p| Ok((node(&p.0, &p.1)?, node(&p.0, &p.2)?))).collect::<Result<_, String>>()?;
            loop {
                let before = reached.len();
                for (f, t) in &edges {
                    if reached.contains(f) {
                        reached.insert(*t);
                    }
                }
                if reached.len() == before {
                    break;
                }
            }
            // Repair where a disconnected piece starts: an unreached wire that
            // no pip of the net drives (the gap is just upstream of it).
            let driven: HashSet<NodeOrWire> = edges.iter().map(|e| e.1).collect();
            let sink_nodes = sinks.iter().map(|(t, w)| node(t, w)).collect::<Result<Vec<_>, _>>()?;
            let unreached: Vec<NodeOrWire> =
                edges.iter().map(|e| e.0).chain(sink_nodes).filter(|n| !reached.contains(n)).collect();
            let missing = unreached.iter().find(|n| !driven.contains(n)).or(unreached.first()).copied();
            let Some(goal) = missing else { return Ok(added) };
            let path = self.bfs_from_set(&reached, goal, 6).ok_or_else(|| {
                let names: Vec<String> = self
                    .members(goal)
                    .into_iter()
                    .take(6)
                    .map(|(c, w)| format!("{}/{}", self.part.tiles[&c].name, self.part.wires[w]))
                    .collect();
                format!("cannot reach ISE wire {}", names.join(" = "))
            })?;
            for p in path {
                if pips.insert(p.clone()) {
                    added.push(p);
                }
            }
        }
        Err("too many repairs".into())
    }

    fn bfs_from_set(&self, start: &HashSet<NodeOrWire>, goal: NodeOrWire, max_hops: usize) -> Option<Vec<IsePip>> {
        let mut prev: HashMap<NodeOrWire, (NodeOrWire, IsePip)> = HashMap::new();
        let mut seen: HashSet<NodeOrWire> = start.clone();
        let mut queue: VecDeque<(NodeOrWire, usize)> = start.iter().map(|&n| (n, 0)).collect();
        while let Some((nw, depth)) = queue.pop_front() {
            if nw == goal {
                let mut path = vec![];
                let mut cur = nw;
                while let Some((p, pip)) = prev.get(&cur) {
                    path.push(pip.clone());
                    cur = *p;
                }
                path.reverse();
                return Some(path);
            }
            if depth == max_hops {
                continue;
            }
            for (crd, w) in self.members(nw) {
                let tile = &self.part.tiles[&crd];
                let ki = tile.kind.to_idx_usize();
                let rt_edges = self.rt_fwd[ki].get(&w).into_iter().flatten().map(|&wt| (wt, false));
                let outs = self.fwd[ki].get(&w).into_iter().flatten().copied().chain(rt_edges);
                for (wt, is_test) in outs {
                    if is_test {
                        continue;
                    }
                    let pip = (tile.name.clone(), self.part.wires[w].clone(), self.part.wires[wt].clone());
                    // Never borrow another site's logic as a route-through, except
                    // BUFH: prjcombine folds the horizontal clock buffers into its
                    // HCLK_ROW muxes, but ISE routes through the BUFH site.
                    if let Some((site, _)) = self.route_through(&pip)
                        && site.kind != "BUFH"
                    {
                        continue;
                    }
                    let Some(next) = self.part.lookup_wire_raw(crd, wt) else { continue };
                    if seen.insert(next) {
                        prev.insert(next, (nw, pip));
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        None
    }

    /// If the pip goes from an input pin to an output pin of one site, it is
    /// a route-through; return the site and the two pins.
    pub fn route_through(&self, pip: &IsePip) -> Option<(SitePinRef, String)> {
        let crd = *self.tile_by_name.get(&pip.0)?;
        let tile = &self.part.tiles[&crd];
        let ki = tile.kind.to_idx_usize();
        let (wf, wt) = (self.part.wires.get(pip.1.as_str())?, self.part.wires.get(pip.2.as_str())?);
        let (sf, pf, f_in) = self.site_pins[ki].get(&wf)?;
        let (st, pt, t_in) = self.site_pins[ki].get(&wt)?;
        if sf != st || !*f_in || *t_in {
            return None;
        }
        let tk = &self.part.tile_kinds[tile.kind];
        let (sid, _, site) = tk.sites.iter().nth(*sf)?;
        let name = tile.sites.get(sid)?.clone();
        Some((SitePinRef { kind: site.kind.clone(), site: name, pin: pf.clone() }, pt.clone()))
    }
}

trait ToIdxUsize {
    fn to_idx_usize(self) -> usize;
}

impl<T: prjcombine_entity::EntityId> ToIdxUsize for T {
    fn to_idx_usize(self) -> usize {
        self.to_idx()
    }
}
