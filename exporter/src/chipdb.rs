//! Index of the exporter's JSON (s6-export output): what each nextpnr pip is
//! in prjcombine terms (switchbox item or slice mux option).

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

pub struct PipMeta {
    pub kind: String,
    pub src_ref: Vec<String>,
}

/// nextpnr pips by (tile x, tile y, src wire name, dst wire name).
pub struct ChipdbIndex {
    tile_type: HashMap<(usize, usize), usize>,
    pips: Vec<HashMap<(String, String), PipMeta>>,
}

impl ChipdbIndex {
    pub fn load(path: &Path) -> Self {
        let v: Value = serde_json::from_reader(std::io::BufReader::new(
            std::fs::File::open(path).expect("open chipdb JSON (s6-export output)"),
        ))
        .expect("parse chipdb JSON");
        let mut pips = vec![];
        for tt in v["tile_types"].as_array().unwrap() {
            let names: Vec<&str> = tt["wires"].as_array().unwrap().iter().map(|w| w["name"].as_str().unwrap()).collect();
            let mut m = HashMap::new();
            for p in tt["pips"].as_array().unwrap() {
                let src = names[p["src"].as_u64().unwrap() as usize].to_string();
                let dst = names[p["dst"].as_u64().unwrap() as usize].to_string();
                let src_ref = p
                    .get("src_ref")
                    .and_then(|r| r.as_array())
                    .map(|r| r.iter().map(|s| s.as_str().unwrap().to_string()).collect())
                    .unwrap_or_default();
                m.insert((src, dst), PipMeta { kind: p["kind"].as_str().unwrap().to_string(), src_ref });
            }
            pips.push(m);
        }
        let tile_type = v["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    (t["x"].as_u64().unwrap() as usize, t["y"].as_u64().unwrap() as usize),
                    t["type"].as_u64().unwrap() as usize,
                )
            })
            .collect();
        ChipdbIndex { tile_type, pips }
    }

    pub fn get(&self, x: usize, y: usize, src: &str, dst: &str) -> Option<&PipMeta> {
        self.pips[self.tile_type[&(x, y)]].get(&(src.to_string(), dst.to_string()))
    }
}

