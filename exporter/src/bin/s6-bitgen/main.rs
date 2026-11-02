//! Spartan-6 bitstream writer (work in progress).
//!
//!     s6-bitgen roundtrip --db spartan6.zstd --input ise.bin --out out.bin
//!         Parse an ISE bitstream with prjcombine and write it back; the
//!         output must be byte-identical. Tests the packet writer.
//!     s6-bitgen diff --db spartan6.zstd A.bin B.bin
//!         Compare two bitstreams frame by frame and name the tiles involved.

mod explain;
mod config;
mod write;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use prjcombine_spartan6::bitstream::{BitPos, Bitstream};
use prjcombine_spartan6::db::Database;

#[derive(Parser)]
#[command(about = "Spartan-6 bitstream writer")]
struct Args {
    /// prjcombine databases/spartan6.zstd
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[arg(long, global = true, default_value = "xc6slx9")]
    device: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a bitstream and write it back (packet writer test).
    Roundtrip {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Compare two bitstreams.
    Diff { a: PathBuf, b: PathBuf },
    /// Debug: print a parsed frame's nonzero bits and its frame address.
    Frame { input: PathBuf, frame: usize },
    /// List a bitstream's set bits with what prjcombine says they control.
    Bits { input: PathBuf },
    /// Generate a bitstream from a nextpnr-routed design.
    Gen {
        /// nextpnr --write output
        #[arg(long)]
        routed: PathBuf,
        /// The exporter's JSON (s6-export output)
        #[arg(long)]
        chipdb_json: PathBuf,
        /// Output .bin (raw packets, as the Mojo loader expects)
        #[arg(long)]
        out: PathBuf,
        /// Compare with this bitstream (e.g. ISE bitgen of the same design)
        #[arg(long)]
        compare: Option<PathBuf>,
    },
}

fn main() {
    let args = Args::parse();
    let db = Database::from_file(args.db.as_ref().expect("--db is required")).expect("read prjcombine database");
    let dev = db.devices.iter().find(|d| d.name == args.device).expect("device not in database");
    let edev = db.chips[dev.chip].expand_grid(&db.int, &dev.disabled);
    let parse = |p: &PathBuf| -> Bitstream {
        let data = std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        edev.parse_bitstream(&data, None)
    };

    match args.command {
        Command::Roundtrip { input, out } => {
            let orig = std::fs::read(&input).expect("read input");
            let bs = parse(&input);
            let ours = write::write_bitstream(&bs);
            std::fs::write(&out, &ours).expect("write output");
            if ours == orig {
                println!("identical ({} bytes)", ours.len());
            } else {
                let first = ours.iter().zip(&orig).position(|(a, b)| a != b).unwrap_or(ours.len().min(orig.len()));
                println!(
                    "DIFFERENT: ours {} bytes, input {} bytes, first difference at byte {first} (word {})",
                    ours.len(),
                    orig.len(),
                    first / 2
                );
                std::process::exit(1);
            }
        }
        Command::Frame { input, frame } => {
            let bs = parse(&input);
            println!("frame {frame}: {:?}", bs.frame_info[frame]);
            let len = prjcombine_spartan6::bitstream::FRAME_LEN;
            let bits: Vec<usize> = (0..len).filter(|&b| bs.frame_data[frame * len + b]).collect();
            println!("set bits: {bits:?}");
            println!("{} main frames, {} bram frames, iob {} bits", bs.frame_info.len(), bs.bram_frame_info.len(), bs.iob.len());
        }
        Command::Gen { routed, chipdb_json, out, compare } => {
            let chipdb = s6_export::chipdb::ChipdbIndex::load(&chipdb_json);
            let (cells, nets) = s6_export::nextpnr::load_routed(&routed);
            let mut g = config::Gen::new(&edev, &db.int);
            g.defaults();
            let mut errors = vec![];
            for c in &cells {
                if let Err(e) = g.cell_config(c, &nets) {
                    errors.push(e);
                }
            }
            for n in &nets {
                for (x, y, dst, src) in &n.route {
                    g.pip(&chipdb, *x, *y, dst, src);
                }
            }
            g.global(&cells);
            if !errors.is_empty() {
                for e in &errors {
                    eprintln!("error: {e}");
                }
                std::process::exit(1);
            }
            std::fs::write(&out, write::write_bitstream(&g.bs)).expect("write output");
            println!("wrote {}", out.display());
            if let Some(cmp) = compare {
                let ex = explain::Explainer::new(&edev, &db.int);
                let theirs = parse(&cmp);
                let mut d: Vec<(BitPos, bool)> = Bitstream::diff(&theirs, &g.bs).into_iter().collect();
                d.sort();
                for (pos, val) in &d {
                    println!("{} {pos:?}  {}", if *val { "extra  " } else { "missing" }, ex.describe(*pos));
                }
                println!("{} bits differ from {}", d.len(), cmp.display());
                let identical = std::fs::read(&cmp).ok() == std::fs::read(&out).ok();
                println!("files {}", if identical { "IDENTICAL" } else { "differ" });
            }
        }
        Command::Diff { a, b } => {
            let ex = explain::Explainer::new(&edev, &db.int);
            let (ba, bb) = (parse(&a), parse(&b));
            let mut d: Vec<(BitPos, bool)> = Bitstream::diff(&ba, &bb).into_iter().collect();
            d.sort();
            for (pos, val) in &d {
                println!("{pos:?} -> {}  {}", *val as u8, ex.describe(*pos));
            }
            println!("{} differing bits", d.len());
        }
        Command::Bits { input } => {
            let ex = explain::Explainer::new(&edev, &db.int);
            let bs = parse(&input);
            let empty = edev.empty_bitstream();
            let mut d: Vec<(BitPos, bool)> = Bitstream::diff(&empty, &bs)
                .into_iter()
                .filter(|(p, v)| *v && !matches!(p, BitPos::Reg(..) | BitPos::RegPresent(..)))
                .collect();
            d.sort();
            for (pos, _) in &d {
                println!("{pos:?}  {}", ex.describe(*pos));
            }
            for (reg, val) in &bs.regs {
                println!("reg {reg:?} = {val:#06x}");
            }
            let unknown = d.iter().filter(|(p, _)| !ex.uses.contains_key(p)).count();
            println!("{} set bits ({unknown} unknown to prjcombine)", d.len());
        }
    }
}
