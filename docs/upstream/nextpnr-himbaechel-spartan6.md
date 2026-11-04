# himbaechel: Spartan-6 uarch

**Source:** `nextpnr/himbaechel/uarch/spartan6` here (~4,400 lines) and one
line in `himbaechel/CMakeLists.txt` (`spartan6` in `HIMBAECHEL_UARCHES`).
Depends on the router2 fixes (separate PR).

## Summary

A himbaechel uarch for the Xilinx Spartan-6 (LX/LXT: xc6slx4 to xc6slx45
exported and checked), using prjcombine's Spartan-6 database. It places and
routes yosys `synth_xilinx -family xc6s` netlists. The bitstream is written
by a separate tool, and every feature has been checked against ISE 14.7
bit for bit: the same placement and routing, written as XDL and run
through ISE's xdl2ncd and bitgen, gives a byte-identical bitstream. Most
features are also tested on a board (Mojo V3, xc6slx9).

Supported:
- **Logic:** slices as sub-bels (LUT6/LUT5 pairs, FFs and latches, 5FFs,
  F7/F8 muxes, CARRY4 chains; when the main FFs run out, other FFs go on
  5FFs through pass-through LUT5s); LUT RAM and SRLs (RAM32/64/128/256, RAM32M,
  SRLC32E with cascades).
- **Block RAM:** RAMB8/RAMB16.
- **DSP48A1:** all pipeline options, the pre-adder, and PCOUT/BCOUT/CARRYOUT
  cascades as clusters.
- **Clocking:** BUFG, DCM (DLL, CLKFX, fixed and variable phase shift),
  PLL_BASE/PLL_ADV (constant or dynamic CLKINSEL, DRP from logic),
  BUFIO2/BUFIO2FB, with clock trees placed during packing.
- **I/O:** IOB, IOBUF, ODDR2/IDDR2 (including tristate, two clocks and
  set/reset), LVDS, the VREF standards (SSTL/HSTL) and LVCMOS/LVTTL; UCF
  constraints (pins, I/O standards and clock periods: PERIOD, TNM_NET +
  TIMESPEC, carried through clock buffers and derived for DCM/PLL outputs).
- **Timing:** a delay model fitted to ISE's timing data, for the -2 and -3
  grades. DSP timing varies by configuration (1024 variants), and paths
  through cascades are timed.

## Chip database

As for other himbaechel uarches, the chip database is built outside the
nextpnr build: a Rust exporter reads prjcombine's database and writes the
`.bba` (a port of `himbaechel_dbgen`), with the delay model added. These
tools live in the toolchain repository (github.com/ellisgl/spartan6-toolchain;
`s6-export`, `s6-chipdb`). **Open question for review:** whether upstream
would rather have the generator in-tree (it depends on prjcombine crates,
so it is Rust) or keep it external with a pinned prjcombine revision, as
the toolchain does now.

## Testing

The toolchain's CI builds nextpnr with this uarch, the chip databases of
six devices, and every test design. Its regression (145 designs) is
checked byte-for-byte against ISE locally (ISE can't run in CI).
