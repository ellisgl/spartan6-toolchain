# xc6slx9-pnr

Open-source place and route for the Spartan-6 xc6slx9 (Mojo V3, TQG144), as a
nextpnr himbaechel uarch built on prjcombine. The plan is in
[`Open-Source P&R for Spartan-6 (xc6slx9).md`](Open-Source%20P&R%20for%20Spartan-6%20(xc6slx9).md).

## Status

- **Chip database exporter (§1):** working. Slices are split into sub-bels
  (see below).
- **nextpnr uarch (§3–5, `uarch/spartan6`):** first version. Blinky
  (`tests/blinky`, Mojo V3 pinout) packs, places and routes. Clock routing
  uses the dedicated BUFGMUX → GCLK → HCLK network.
- **XDL writer (§6):** working. `s6-xdl` converts our packed, placed and
  routed design into XDL with our routing as ISE pips. ISE only converts it,
  checks it (`drc`: 0 errors on blinky and cylon) and runs `bitgen`; `par` is
  not used. Converting ISE's result back to XDL gives back exactly our pips.
  Cylon and Blinky with our own routing run correctly on the Mojo V3.
  `XDL_MODE=pins` writes pins only and lets ISE route instead (stage 1).
- **Bitstream writer (§7):** not started; ISE's `bitgen` is used for now.

## Pipeline

```
prjcombine spartan6.zstd ──s6-export (Rust)──▶ xc6slx9.json ──s6_gen.py (Python)──▶ xc6slx9.bba
```

- `exporter/` (Rust): loads prjcombine's database, expands the device grid,
  and writes tiles, wires, pips, bels, nodes and package pins as JSON.
  prjcombine is pinned to `2b3317d`.
- `gen/s6_gen.py`: builds the `.bba` with nextpnr's own `himbaechel_dbgen`,
  so the layout always matches the nextpnr checkout.
- `gen/check_db.py`: checks bel counts against the datasheet and searches the
  routing graph for the connections Blinky needs.

```
make                    # build/chipdb-xc6slx9.bin and build/nextpnr-build/nextpnr-himbaechel
make check              # chip database sanity checks
make pnr  TEST=cylon    # yosys + nextpnr                 -> build/cylon/routed.json
make bit  TEST=cylon    # + s6-xdl + ISE xdl2ncd/drc/bitgen -> build/cylon/design.bin
make load TEST=cylon    # + load into the Mojo's FPGA RAM
```

`make bit` needs ISE (see below). The first run also dumps ISE's view of the
device and builds prjcombine's naming database (`build/xc6slx9-geom.zstd`,
about 2 minutes).

`make` clones nextpnr at a pinned revision into `build/nextpnr`, links
`uarch/spartan6` into its source tree, and adds `spartan6` to himbaechel's
uarch list. To run nextpnr by hand:

```
build/nextpnr-build/nextpnr-himbaechel --device xc6slx9-2tqg144 \
    --chipdb build/chipdb-xc6slx9.bin --json synth.json -o ucf=pins.ucf --write routed.json
```

Requires Rust 1.85+ (edition 2024), Python 3.10+, curl, CMake, a C++17
compiler, Boost and Eigen (for nextpnr), and Yosys.

## ISE (for validation)

ISE 14.7 is only used to check our output (XDL → NCD → `drc`/`bitgen`). It
installs into a host directory, and a small container (`docker/ise`, Ubuntu
14.04 plus ISE's libraries) supplies the old userland it needs.

1. Download `Xilinx_ISE_DS_Lin_14.7_1015_1.tar` from AMD and extract it.
2. Put a free WebPACK licence in `~/.Xilinx/Xilinx.lic`.
3. Run the one-time install, about 10 minutes and 18 GB:
   ```
   tools/ise-install.sh ~/path/to/Xilinx_ISE_DS_Lin_14.7_1015_1   # -> ~/Xilinx
   ```
4. Run any ISE command-line tool. The current directory is mounted, so
   relative paths work:
   ```
   tools/ise.sh xdl -report xc6slx9tqg144 xc6slx9.xdlrc
   tools/ise.sh bitgen -w design.ncd
   ```

`tools/ise.sh` hands the container a trimmed `/proc/cpuinfo`. ISE's FlexNet
licensing code (used by map, par and bitgen) overflows on the long CPU flags
line of modern CPUs and crashes.

Reference build and loading onto the Mojo V3, verified on hardware with
`tests/cylon`:

```
tools/ise-build.sh cylon tests/mojo_v3.ucf build/cylon-ise tests/cylon/cylon.v
tools/mojo-load.py build/cylon-ise/cylon.bin            # RAM (lost on power cycle)
tools/mojo-load.py --flash build/cylon-ise/cylon.bin    # flash, verified
```

`mojo-load.py` speaks the official loader's serial protocol to the Mojo's
AVR (`/dev/ttyACM0`; needs membership of `dialout`).

## XDL output (`exporter/src/bin/s6-xdl/`)

ISE names (tiles, sites, wires) come from prjcombine's own naming database,
built from ISE: `s6-rawdump` runs prjcombine's ISE dumper for the xc6slx9
only, through the `tools/ise-bin` wrappers, and prjcombine's `xrd2geom`
converts the dump. `s6-xdl` checks that this database's interconnect
structure matches the published database the chipdb came from.

**Routing as pips:**

- **Pip origin:** each nextpnr pip is identified through the exporter's JSON
  (`src_ref`).
- **Interconnect pips:** named with prjcombine's naming rules (`ext_pips`,
  alternate wire names, buffered source wires), plus the termination and
  buffer hops from `resolve_wire_trace`.
- **Site pins:** a pin counts as used when the route really reaches its wire
  (compared as prjcombine canonical wires). Its pin pips (e.g.
  `CLEXM_LOGICIN_B1 -> X_A2`) are added.
- **Slice pseudo-pips:** written as site configuration (`<L>FFMUX`,
  `<L>OUTMUX`, ...).
- **I/O:** routes through ILOGIC (D -> FABRICOUT) or OLOGIC (D1 -> OQ, T1 -> TQ)
  as ISE does, with dummy route-through instances. The IOB <-> IOI hops are
  found in ISE's pip graph (the rawdump).
- **Constant nets:** split per TIEOFF source, each with a dummy TIEOFF
  instance (`HARD0`/`HARD1`).
- **Repair pass:** walks each net in ISE's graph and adds the pips that
  prjcombine models as plain wires: clock-spine MIDBUFs, terminations, and
  BUFH route-throughs in the horizontal clock rows.

**Site configuration** follows what ISE writes (compare `xdl -ncd2xdl` on an
ISE build) and prjcombine's `v2xdl-verify`:

- **LUTs:** `<L>6LUT` with an O6 equation.
- **FFs:** `<L>FF` with `#FF`, plus `<L>FFSRINIT`.
- **Slice-wide settings:** `CLKINV`, `SYNC_ATTR`, and `CEUSED`/`SRUSED` when
  routed.
- **IOBs:** standard, drive and slew from the UCF.
- **Clock buffers:** BUFGMUX sites are written as `BUFG` instances.

**Debug options:**

- `--dump-naming X,Y`: a tile's bel and pin naming.
- `--find-ise-wire NAME`: the naming entries that mention an ISE wire.
- `--raw-pips-from TILE/WIRE`: ISE pips leaving a wire.

## How prjcombine maps onto himbaechel

| prjcombine | himbaechel |
| --- | --- |
| cell (col, row) | tile (x = col, y = row; y = 0 is the bottom of the die) |
| set of tile classes at a cell (e.g. `INT+CLEXL`) | tile type |
| tile-class wire used by a pip or bel pin | tile wire |
| tile wires resolving to one canonical wire | node |
| switchbox item (MUX, PROGBUF, PERMABUF, PROGINV, PAIRMUX, ...) | pip, one per source/destination pair; the item kind is the pip's timing class |
| bel (non-slice site) | bel, with prjcombine's site-level pins |

Wires of single-cell tile classes keep prjcombine's names (`IMUX_LOGICIN[0]`).
Wires of multi-cell classes are prefixed with the class and cell
(`BRAM.CELL[2].IMUX_...`), and nodes join them to the real cell's wire.

Slices: prjcombine has one `SLICE` site and stores its internal muxes as
config attributes. The exporter (`exporter/src/slice.rs`) replaces each slice
with sub-bels:

- **Per letter A–D:** `SLICE_LUT` ×2 (LUT6, LUT5) and `SLICE_FF` ×2 (FF, 5FF).
- **Wide-function slices:** F7AMUX, F7BMUX and F8MUX.
- **Carry slices:** CARRY4.

Each internal mux option becomes a `SITE` pseudo-pip, generated from the
options that slice instance really has. A bel's `site` field marks the slice
kind: 1 = SLICEX (`SLICE[1]`), 2 = SLICEL, 3 = SLICEM (`SLICE[0]` in CLEXL /
CLEXM). These counts match the datasheet (715/355/360). The CARRY4 `CI` is
joined to the slice below's `CO3` by a synthesized node.

I/O: prjcombine's IOB only exposes the clock-pad path. `exporter/src/io.rs`
gives each IOB bel `I` (ILOGIC FABRICOUT), `O`/`T` (OLOGIC D1/T1) and
`GCLK_I`. ILOGIC/OLOGIC must then be configured as pass-throughs.

Dropped from the routing graph: prjcombine "test mux" alternatives (only the
primary source is kept, as an always-on pip) and factory-test wires
(`OUT_TEST`, `CMT_TEST_CLK`, `TEST_PLL_CLKIN`).

## Verified on xc6slx9 (`make check`)

- Bel counts match DS160: 1430 slices, 16 DSP48A1, 32 RAMB16 (64 half-BRAM
  bels), 4 DCM, 2 PLL, 16 BUFGMUX. TQG144 has 102 user I/Os in banks 0–3.
- Routing paths exist for:
  - LUT to FF inside a slice, and FF to LUT across the die.
  - Carry from CO3 to the CI of the slice above.
  - Pads to logic (I), logic to pads (O/T), and P56 (Mojo clock) to a BUFGMUX.
  - BUFGMUX to a FF clock input.
- Every bel input has a driver, except the CI at the bottom of each carry
  chain. Every bel output has a load, except clock-pad outputs on pads that
  aren't clock-capable.
- The xc6slx9 has no inverting pips.

## The uarch (`uarch/spartan6`)

- **Device and pins:** parses the device (`xc6slx9[-speed]tqg144`), loads the
  chip database and reads a UCF subset (`-o ucf=`): LOC, IOSTANDARD, DRIVE,
  SLEW, PULLUP, PULLDOWN. Bus syntax `led<0>` maps to `led[0]`.
- **Packer:** maps IBUF/IBUFG/OBUF/OBUFT/IOBUF to `IOB`, BUFG to `BUFGMUX`,
  LUT1–6 and INV to `SLICE_LUT`, and FDRE/FDSE/FDCE/FDPE to `SLICE_FF`.
  A pad that only feeds BUFGs uses `GCLK_I`. Its BUFG is placed during packing
  on a BUFGMUX reachable from that pad, found by searching the routing graph.
- **Placement rules:** the FFs in a slice share CLK, CE and SR, and
  sync/async mode. I/Os only go on bonded pads.
- **Constants:** GND/VCC are routed by router2 to any `TIE_0`/`TIE_1`/`CONST_*`
  wire.

Packer limits for now:

- **LUT5 and 5FF bels are disabled.** LUT6 fracturing needs the A6-high and
  shared-input rules.
- **Not supported yet:** CARRY4, MUXF7/F8, SRL, LUTRAM, BRAM and DSP. Synthesize
  with `-nocarry -nowidelut -nosrl -nolutram -nobram -nodsp`, as
  `make blinky` does.
- **No timing model:** unit delays only.
- **Constant CE/SR:** CE tied high and SR tied low are disconnected in the
  packer (slice defaults), since ISE treats GND -> SR as an excluded arc.
- **No OUTMUX=O6:** the exporter leaves out the OUTMUX=O6 slice option. It
  duplicates the direct `<L>` pin, and using both makes ISE see two drivers.

## Not done yet

- **Uarch-specific data.** `carry_prev` and the pips' `src_ref` are in the JSON
  but not yet in the `.bba`. They need a uarch `extra_data` struct.
- **Pad functions.** GCLK and other pad functions are empty.
- **Own bitstream writer (§7):** replace ISE's `bitgen`, then compare its
  output with ISE's bit for bit on the same NCD.
- **Excluded ISE arcs:** the chipdb still contains arcs ISE marks as excluded
  (e.g. GND -> SR). The rawdump has the flags; the exporter could drop them.
- **Bitstream references.** The JSON keeps a `src_ref` for each pip for the
  future bitstream writer. The `.bba` doesn't carry it.
