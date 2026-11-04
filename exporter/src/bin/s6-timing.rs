//! Spartan-6 delay model from ISE's timing output.
//!
//! ISE's `netgen -sim` writes an SDF file for a placed and routed design:
//! every cell input gets a PORT delay (the routing delay from its net's
//! driver) and every cell its IOPATH / SETUPHOLD timing. For designs placed
//! and routed by nextpnr (converted to NCD through s6-xdl), each PORT delay is
//! the sum of the pips our route uses to reach that input, so fitting one
//! delay per pip class over many inputs gives a routing delay model. Cell
//! delays are taken as ISE reports them (they are the same per cell type).
//!
//!   s6-timing fit --chipdb-json build/xc6slx9.json \
//!       --design build/alu/routed.json:build/alu/timing_sim.sdf ... --out timing.json

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use clap::Parser;
use prjcombine_re_sdf::{Edge, Sdf};
use s6_export::nextpnr::{NCell, NNet, load_routed};
use serde_json::{Value, json};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    chipdb_json: PathBuf,
    /// routed.json:timing_sim.sdf pairs (several designs give more data).
    #[arg(long, required = true)]
    design: Vec<String>,
    /// Like --design, but only for cell timing (not the routing fit):
    /// characterization designs that would skew the routing model.
    #[arg(long)]
    cells_only: Vec<String>,
    #[arg(long)]
    out: PathBuf,
    /// Print the worst-fitting connections.
    #[arg(long, default_value_t = 10)]
    show_worst: usize,
    /// The speed grade the designs were timed at (recorded in the output).
    #[arg(long, default_value = "2")]
    speed: String,
}

/// Wire -> node lookup and per-tile wire/bel-pin names from the chipdb JSON.
struct Graph {
    /// (x, y, local wire name) -> node id
    node_of: HashMap<(usize, usize, String), usize>,
    /// (x, y, bel name, pin) -> (x, y, wire name)
    bel_pin: HashMap<(usize, usize, String, String), String>,
    /// (x, y, src, dst) -> pip class name
    pip_class: HashMap<(usize, usize, String, String), String>,
}

impl Graph {
    fn load(path: &PathBuf) -> Self {
        let d: Value = serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(path).expect("open chipdb json")))
            .expect("parse chipdb json");
        let types = d["tile_types"].as_array().unwrap();
        let wname = |t: usize, i: usize| -> String {
            let w = &types[t]["wires"][i];
            w["name"].as_str().or(w.as_str()).unwrap().to_string()
        };
        let mut tile_type: HashMap<(usize, usize), usize> = HashMap::new();
        for t in d["tiles"].as_array().unwrap() {
            tile_type.insert((t["x"].as_u64().unwrap() as usize, t["y"].as_u64().unwrap() as usize), t["type"].as_u64().unwrap() as usize);
        }
        let mut node_of = HashMap::new();
        let mut next = 0usize;
        for n in d["nodes"].as_array().unwrap() {
            for w in n.as_array().unwrap() {
                let (x, y, wi) = (w[0].as_u64().unwrap() as usize, w[1].as_u64().unwrap() as usize, w[2].as_u64().unwrap() as usize);
                node_of.insert((x, y, wname(tile_type[&(x, y)], wi)), next);
            }
            next += 1;
        }
        let mut bel_pin = HashMap::new();
        let mut pip_class = HashMap::new();
        for (&(x, y), &t) in &tile_type {
            let tt = &types[t];
            for b in tt["bels"].as_array().unwrap() {
                let bn = b["name"].as_str().unwrap();
                for p in b["pins"].as_array().unwrap() {
                    let w = wname(t, p["wire"].as_u64().unwrap() as usize);
                    bel_pin.insert((x, y, bn.to_string(), p["name"].as_str().unwrap().to_string()), w);
                }
            }
            for p in tt["pips"].as_array().unwrap() {
                let (s, dd) = (wname(t, p["src"].as_u64().unwrap() as usize), wname(t, p["dst"].as_u64().unwrap() as usize));
                let class = p["timing_class"].as_str().expect("chipdb JSON without timing classes").to_string();
                pip_class.insert((x, y, s, dd), class);
            }
        }
        // Wires in no multi-wire node are their own node.
        Graph { node_of, bel_pin, pip_class }
    }

    fn node(&mut self, x: usize, y: usize, w: &str) -> usize {
        let key = (x, y, w.to_string());
        if let Some(&n) = self.node_of.get(&key) {
            return n;
        }
        let n = 1_000_000_000 + self.node_of.len();
        self.node_of.insert(key, n);
        n
    }
}

/// SDF port of a netgen simulation primitive -> our cell port.
fn our_port(celltype: &str, port: &str) -> Option<String> {
    match celltype {
        "X_LUT6" | "X_LUT5" => port.strip_prefix("ADR").and_then(|i| i.parse::<usize>().ok()).map(|i| format!("A{}", i + 1)),
        "X_FF" | "X_SFF" => match port {
            "I" => Some("D".into()),
            "CLK" => Some("CK".into()),
            "CE" => Some("CE".into()),
            "SRST" | "RST" | "SET" | "SSET" => Some("SR".into()),
            _ => None,
        },
        // Bus pins: ISE "C[23]", ours "C23" (the clock isn't routed by us
        // through general routing in a way worth fitting).
        "X_DSP48A1" | "X_RAMB16BWER" | "X_RAMB8BWER" if port.contains('[') => {
            Some(port.replace('[', "").replace(']', ""))
        }
        "X_DSP48A1" | "X_RAMB16BWER" | "X_RAMB8BWER" if !port.starts_with("CLK") => Some(port.to_string()),
        _ => None,
    }
}

fn unescape(s: &str) -> String {
    s.replace('\\', "")
}

struct Sample {
    classes: BTreeMap<String, f64>,
    delay: f64,
    what: String,
}

fn main() {
    let args = Args::parse();
    let mut g = Graph::load(&args.chipdb_json);
    let mut samples: Vec<Sample> = vec![];
    // (celltype, from, to) -> delays; (celltype, data, clock) -> setup, hold
    let mut iopaths: BTreeMap<(String, String, String), Vec<f64>> = BTreeMap::new();
    let mut setups: BTreeMap<(String, String, String), (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    let mut unmatched = 0usize;

    let designs = args.design.iter().map(|d| (d, true)).chain(args.cells_only.iter().map(|d| (d, false)));
    for (d, fit_routing) in designs {
        let (routed, sdf_path) = d.split_once(':').expect("--design routed.json:file.sdf");
        let (cells, nets): (Vec<NCell>, Vec<NNet>) = load_routed(std::path::Path::new(routed));
        let sdf = Sdf::parse(&std::fs::read_to_string(sdf_path).expect("read sdf"));
        let by_name: HashMap<String, usize> = cells.iter().enumerate().map(|(i, c)| (c.name.clone(), i)).collect();
        // Per net: node -> (driving pip's src node, class).
        let mut drivers: Vec<HashMap<usize, (usize, String)>> = vec![];
        for n in &nets {
            let mut m = HashMap::new();
            for (x, y, dst, src) in &n.route {
                let class = g.pip_class.get(&(*x, *y, src.clone(), dst.clone())).cloned().unwrap_or_else(|| "?".into());
                let (sn, dn) = (g.node(*x, *y, src), g.node(*x, *y, dst));
                m.insert(dn, (sn, class));
            }
            drivers.push(m);
        }
        // netgen puts a FF input's routing delay on a buffer in front of it,
        // "NlwBufferBlock_<cell>/<pin>" (IN = D, CLK = clock); the FF's own
        // PORT delay is the rest. (cell, SDF port) -> buffer delay.
        let mut nlw: HashMap<(String, String), f64> = HashMap::new();
        for (inst, cell) in &sdf.cells_by_name {
            let name = unescape(inst);
            if let Some(rest) = name.strip_prefix("NlwBufferBlock_")
                && let Some((cname, pin)) = rest.rsplit_once('/')
                && let Some(p) = cell.iopath.first()
            {
                // Block RAM bus pins: "ADDRA<10>" (the cell's port is "ADDRA[10]").
                let bus = pin.replace('<', "[").replace('>', "]");
                let port = match pin {
                    "IN" => "I",
                    _ => bus.as_str(),
                };
                nlw.insert((cname.to_string(), port.to_string()), p.del_rise.max.0.0.max(p.del_fall.max.0.0));
            }
        }
        // DSP timing depends on the pipeline registers: record its arcs per
        // configuration, keyed by the registered groups in the order
        // A B C D M P O(PMODE), e.g. "ACP"; "" when none. When all of OPMODE is
        // tied to constants, "_<hex>" follows (ISE drops the paths that mode
        // can't use).
        let dsp_cfg = |name: &str| -> Option<String> {
            let c = &cells[*by_name.get(name)?];
            let on = |k: &str| c.params.get(k).is_some_and(|v| v.chars().any(|ch| ch == '1'));
            let groups = [
                ('A', on("A0REG") || on("A1REG")),
                ('B', on("B0REG") || on("B1REG")),
                ('C', on("CREG")),
                ('D', on("DREG")),
                ('M', on("MREG")),
                ('P', on("PREG")),
                ('O', on("OPMODEREG")),
            ];
            let mut key: String = groups.iter().filter(|g| g.1).map(|g| g.0).collect();
            let mut opmode = 0u32;
            let mut constant = true;
            for i in 0..8 {
                match c.conns.get(&format!("OPMODE{i}")).map(|&n| nets[n].constant) {
                    None | Some(Some(false)) => {}
                    Some(Some(true)) => opmode |= 1 << i,
                    Some(None) => constant = false,
                }
            }
            if constant {
                key += &format!("_{opmode:02x}");
            }
            Some(key)
        };
        for (inst, cell) in &sdf.cells_by_name {
            let name = unescape(inst);
            let ty_owned = match (cell.typ.as_str(), dsp_cfg(&name)) {
                ("X_DSP48A1", Some(cfg)) => format!("X_DSP48A1:{cfg}"),
                (t, _) => t.to_string(),
            };
            let ty = ty_owned.as_str();
            for p in &cell.iopath {
                let (Edge::Plain(f) | Edge::Posedge(f) | Edge::Negedge(f)) = &p.port_from;
                let (Edge::Plain(t) | Edge::Posedge(t) | Edge::Negedge(t)) = &p.port_to;
                iopaths.entry((ty.into(), f.clone(), t.clone())).or_default().push(p.del_rise.max.0.0.max(p.del_fall.max.0.0));
            }
            for sh in &cell.setuphold {
                let (Edge::Plain(dd) | Edge::Posedge(dd) | Edge::Negedge(dd)) = &sh.edge_d;
                let (Edge::Plain(cc) | Edge::Posedge(cc) | Edge::Negedge(cc)) = &sh.edge_c;
                let e = setups.entry((ty.into(), dd.clone(), cc.clone())).or_default();
                if let Some(s) = sh.setup {
                    e.0.push(s.max.0.0);
                }
                if let Some(h) = sh.hold {
                    e.1.push(h.max.0.0);
                }
            }
            let Some(&ci) = by_name.get(&name) else { continue };
            let c = &cells[ci];
            for port in &cell.ports {
                let Some(ours) = our_port(&cell.typ, &port.port) else { continue };
                let Some(&ni) = c.conns.get(&ours) else { continue };
                let net = &nets[ni];
                if net.constant.is_some() || net.route.is_empty() {
                    continue;
                }
                let (x, y, ref bel) = c.bel;
                let Some(w) = g.bel_pin.get(&(x, y, bel.clone(), ours.clone())).cloned() else { continue };
                let mut node = g.node(x, y, &w);
                let mut classes: BTreeMap<String, f64> = BTreeMap::new();
                let mut ok = false;
                for _ in 0..200 {
                    match drivers[ni].get(&node) {
                        Some((src, class)) => {
                            *classes.entry(class.clone()).or_default() += 1.0;
                            node = *src;
                        }
                        None => {
                            ok = true;
                            break;
                        }
                    }
                }
                if !ok || classes.is_empty() {
                    unmatched += 1;
                    continue;
                }
                let delay = port.del_rise.max.0.0.max(port.del_fall.max.0.0)
                    + nlw.get(&(name.clone(), port.port.clone())).copied().unwrap_or(0.0);
                // Loading: extra delay per additional load on the net.
                if net.users.len() > 1 {
                    classes.insert("~FANOUT".into(), (net.users.len() - 1) as f64);
                }
                if fit_routing {
                    samples.push(Sample { classes, delay, what: format!("{} {}.{}", net.name, c.name, ours) });
                }
            }
        }
    }

    // Non-negative least squares over the class counts.
    let class_names: Vec<String> = {
        let mut s: Vec<String> = samples.iter().flat_map(|s| s.classes.keys().cloned()).collect();
        s.sort();
        s.dedup();
        s
    };
    let idx: HashMap<&str, usize> = class_names.iter().enumerate().map(|(i, c)| (c.as_str(), i)).collect();
    let k = class_names.len();
    let rows: Vec<(Vec<(usize, f64)>, f64)> = samples
        .iter()
        .map(|s| (s.classes.iter().map(|(c, &n)| (idx[c.as_str()], n)).collect(), s.delay))
        .collect();
    let x = nnls(&rows, k);
    let mut counts = vec![0usize; k];
    for (r, _) in &rows {
        for &(i, _) in r {
            counts[i] += 1;
        }
    }
    // Fit quality.
    let mut errs: Vec<(f64, usize)> = rows
        .iter()
        .enumerate()
        .map(|(si, (r, d))| (r.iter().map(|&(i, n)| n * x[i]).sum::<f64>() - d, si))
        .collect();
    let rms = (errs.iter().map(|e| e.0 * e.0).sum::<f64>() / errs.len().max(1) as f64).sqrt();
    let mean_d = rows.iter().map(|r| r.1).sum::<f64>() / rows.len().max(1) as f64;
    eprintln!(
        "{} connections ({} not traced), {} pip classes; mean delay {:.0} ps, rms error {:.0} ps",
        samples.len(),
        unmatched,
        k,
        mean_d,
        rms
    );
    errs.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap());
    for &(e, si) in errs.iter().take(args.show_worst) {
        let s = &samples[si];
        eprintln!("  {:+6.0} ps (ISE {:.0}) {} via {:?}", e, s.delay, s.what, s.classes);
    }

    let pips: BTreeMap<String, Value> = class_names
        .iter()
        .enumerate()
        .map(|(i, c)| (c.clone(), json!({"delay_ps": (x[i] * 10.0).round() / 10.0, "samples": counts[i]})))
        .collect();
    let max = |v: &Vec<f64>| v.iter().cloned().fold(0.0, f64::max);
    let cells_out: Vec<Value> = iopaths
        .iter()
        .map(|((t, f, to), v)| json!({"celltype": t, "from": f, "to": to, "delay_ps": max(v), "min_ps": v.iter().cloned().fold(f64::MAX, f64::min), "samples": v.len()}))
        .collect();
    let checks: Vec<Value> = setups
        .iter()
        .map(|((t, d, c), (s, h))| json!({"celltype": t, "data": d, "clock": c, "setup_ps": max(s), "hold_ps": max(h)}))
        .collect();
    let out = json!({
        "source": format!("ISE 14.7 netgen SDF, xc6slx9-{}, 85C 1.14V (max delays)", args.speed),
        "fit": {"connections": samples.len(), "rms_error_ps": rms, "mean_delay_ps": mean_d},
        "pip_classes": pips,
        "cell_delays": cells_out,
        "cell_checks": checks,
    });
    std::fs::write(&args.out, serde_json::to_string_pretty(&out).unwrap()).expect("write output");
    eprintln!("wrote {}", args.out.display());
}

/// Non-negative least squares (Lawson-Hanson active set) on sparse rows.
fn nnls(rows: &[(Vec<(usize, f64)>, f64)], k: usize) -> Vec<f64> {
    // Normal equations: A^T A and A^T b.
    let mut ata = vec![vec![0.0; k]; k];
    let mut atb = vec![0.0; k];
    for (r, b) in rows {
        for &(i, a) in r {
            atb[i] += a * b;
            for &(j, c) in r {
                ata[i][j] += a * c;
            }
        }
    }
    let mut passive = vec![false; k];
    let mut x = vec![0.0; k];
    for _ in 0..(3 * k + 10) {
        // Gradient w = A^T b - A^T A x.
        let w: Vec<f64> = (0..k).map(|i| atb[i] - (0..k).map(|j| ata[i][j] * x[j]).sum::<f64>()).collect();
        let cand = (0..k).filter(|&i| !passive[i] && w[i] > 1e-6).max_by(|&a, &b| w[a].partial_cmp(&w[b]).unwrap());
        let Some(t) = cand else { break };
        passive[t] = true;
        loop {
            let z = solve_subset(&ata, &atb, &passive);
            if (0..k).all(|i| !passive[i] || z[i] > 0.0) {
                x = z;
                break;
            }
            // Step back toward x until a passive variable hits zero.
            let mut alpha = 1.0f64;
            for i in 0..k {
                if passive[i] && z[i] <= 0.0 {
                    alpha = alpha.min(x[i] / (x[i] - z[i]).max(1e-12));
                }
            }
            for i in 0..k {
                x[i] += alpha * (z[i] - x[i]);
                if passive[i] && x[i] <= 1e-9 {
                    passive[i] = false;
                    x[i] = 0.0;
                }
            }
        }
    }
    x
}

/// Least squares restricted to the passive variables (others 0).
fn solve_subset(ata: &[Vec<f64>], atb: &[f64], passive: &[bool]) -> Vec<f64> {
    let p: Vec<usize> = (0..atb.len()).filter(|&i| passive[i]).collect();
    let n = p.len();
    let mut m: Vec<Vec<f64>> = p.iter().map(|&i| p.iter().map(|&j| ata[i][j]).chain([atb[i]]).collect()).collect();
    for i in 0..n {
        m[i][i] += 1e-9; // keep rank-deficient systems solvable
    }
    for c in 0..n {
        let piv = (c..n).max_by(|&a, &b| m[a][c].abs().partial_cmp(&m[b][c].abs()).unwrap()).unwrap();
        m.swap(c, piv);
        let d = m[c][c];
        if d.abs() < 1e-12 {
            continue;
        }
        for r in 0..n {
            if r != c {
                let f = m[r][c] / d;
                for cc in c..=n {
                    m[r][cc] -= f * m[c][cc];
                }
            }
        }
    }
    let mut x = vec![0.0; atb.len()];
    for (r, &i) in p.iter().enumerate() {
        x[i] = if m[r][r].abs() < 1e-12 { 0.0 } else { m[r][n] / m[r][r] };
    }
    x
}
