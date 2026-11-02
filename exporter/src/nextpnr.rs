//! Loading nextpnr's `--write` JSON for a placed and routed design.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::Value;

pub struct NCell {
    pub name: String,
    pub typ: String,
    /// Parameters and attributes (UCF constraints land in attributes).
    pub params: BTreeMap<String, String>,
    /// port -> net index
    pub conns: BTreeMap<String, usize>,
    /// (tile x, tile y, bel name)
    pub bel: (usize, usize, String),
}

pub struct NNet {
    pub name: String,
    pub driver: Option<(usize, String)>,
    pub users: Vec<(usize, String)>,
    pub constant: Option<bool>,
    /// Routing: (tile x, tile y, dst wire, src wire) per pip.
    pub route: Vec<(usize, usize, String, String)>,
}

pub fn parse_tile(t: &str) -> (usize, usize) {
    let (xs, ys) = t.trim_start_matches('X').split_once('Y').unwrap();
    (xs.parse().unwrap(), ys.parse().unwrap())
}

pub fn load_routed(path: &Path) -> (Vec<NCell>, Vec<NNet>) {
    let v: Value = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(path).expect("open routed json"),
    ))
    .expect("parse json");
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
        let mut conns = BTreeMap::new();
        let ci = cells.len();
        for (port, bits) in c["connections"].as_object().unwrap() {
            let Some(b) = bits.as_array().unwrap().first().and_then(|b| b.as_i64()) else { continue };
            let ni = *net_of_bit.entry(b).or_insert_with(|| {
                nets.push(NNet { name: bit_name[&b].clone(), driver: None, users: vec![], constant: None, route: vec![] });
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
        let (x, y) = parse_tile(tile);
        cells.push(NCell {
            name: name.clone(),
            typ: c["type"].as_str().unwrap().to_string(),
            params,
            conns,
            bel: (x, y, belname.to_string()),
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
    // Routing: "wire;pip;strength;" triples; pip names are "tile/dst/src".
    for n in m["netnames"].as_object().unwrap().values() {
        let Some(r) = n["attributes"]["ROUTING"].as_str() else { continue };
        let Some(b) = n["bits"].as_array().unwrap().first().and_then(|b| b.as_i64()) else { continue };
        let Some(&ni) = net_of_bit.get(&b) else { continue };
        if !nets[ni].route.is_empty() {
            continue;
        }
        let parts: Vec<&str> = r.split(';').collect();
        for chunk in parts.chunks(3) {
            let Some(&pip) = chunk.get(1) else { continue };
            if pip.is_empty() {
                continue;
            }
            let (tile, rest) = pip.split_once('/').unwrap();
            let (dst, src) = rest.split_once('/').unwrap();
            let (x, y) = parse_tile(tile);
            nets[ni].route.push((x, y, dst.to_string(), src.to_string()));
        }
    }
    (cells, nets)
}

