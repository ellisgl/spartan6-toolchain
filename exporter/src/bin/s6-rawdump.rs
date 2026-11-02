//! Dump ISE's view of one device (tiles, sites, wires, pips, with ISE's
//! names) into a prjcombine rawdump, for prjcombine's rd2geom.
//!
//! prjcombine's own `dump_ise_parts` dumps whole families; this dumps a single
//! device. ISE is found through the toolchain file's PATH (tools/ise-bin).
//!
//!     s6-rawdump <toolchain.kdl> xc6slx9 xc6slx9.zstd

use std::path::PathBuf;

use clap::Parser;
use prjcombine_re_toolchain::Toolchain;
use prjcombine_re_xilinx_ise_dump::partgen::get_pkgs;
use prjcombine_re_xilinx_ise_dump::rawdump::get_rawdump;

#[derive(Parser)]
#[command(about = "Dump one device's ISE geometry into a prjcombine rawdump")]
struct Args {
    toolchain: PathBuf,
    device: String,
    out: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let tc = Toolchain::from_file(&args.toolchain)?;
    let pkgs: Vec<_> = get_pkgs(&tc, &args.device)?
        .into_iter()
        .filter(|p| p.device == args.device)
        .collect();
    if pkgs.is_empty() {
        return Err(format!("partgen knows no packages for {}", args.device).into());
    }
    eprintln!(
        "dumping {} ({})",
        args.device,
        pkgs.iter().map(|p| p.package.as_str()).collect::<Vec<_>>().join(", ")
    );
    let rd = get_rawdump(&tc, &pkgs)?;
    rd.to_file(&args.out)?;
    eprintln!("wrote {}", args.out.display());
    Ok(())
}
