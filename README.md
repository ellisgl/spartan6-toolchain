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
- **No bitstream yet.** The routed result is nextpnr's JSON
  (`build/blinky/routed.json`). The XDL writer (§6) and bitstream writer (§7)
  are next.

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
make          # build/chipdb-xc6slx9.bin and build/nextpnr-build/nextpnr-himbaechel
make check    # chip database sanity checks
make blinky   # yosys + nextpnr on tests/blinky -> build/blinky/routed.json
```

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
- **Wasted routing on CE/SR:** unused CE/SR inputs are routed to VCC/GND. The
  bitstream writer could disable them instead.

## Not done yet

- **Uarch-specific data.** `carry_prev` and the pips' `src_ref` are in the JSON
  but not yet in the `.bba`. They need a uarch `extra_data` struct.
- **Pad functions.** GCLK and other pad functions are empty.
- **ISE names** for XDL output. These need prjcombine's unpublished geom
  database (see the plan, §1).
- **Bitstream references.** The JSON keeps a `src_ref` for each pip for the
  future bitstream writer. The `.bba` doesn't carry it.
