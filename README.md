# xc6slx9-pnr

Open-source place and route for the Spartan-6 xc6slx9 (Mojo V3, TQG144), as a
nextpnr himbaechel uarch built on prjcombine. The plan is in
[`Open-Source P&R for Spartan-6 (xc6slx9).md`](Open-Source%20P&R%20for%20Spartan-6%20(xc6slx9).md).

## Status

- **Chip database exporter (§1):** working. Slices are split into sub-bels
  (see below).
- **nextpnr uarch (§3–5):** lives in the nextpnr fork, branch
  [`spartan6`](https://github.com/ellisgl/nextpnr/tree/spartan6/himbaechel/uarch/spartan6). first version. Blinky
  (`tests/blinky`, Mojo V3 pinout) packs, places and routes. Clock routing
  uses the dedicated BUFGMUX → GCLK → HCLK network.
- **XDL writer (§6):** working. `s6-xdl` converts our packed, placed and
  routed design into XDL with our routing as ISE pips. ISE only converts it,
  checks it (`drc`: 0 errors on blinky and cylon) and runs `bitgen`; `par` is
  not used. Converting ISE's result back to XDL gives back exactly our pips.
  Cylon and Blinky with our own routing run correctly on the Mojo V3.
  `XDL_MODE=pins` writes pins only and lets ISE route instead (stage 1).
- **Bitstream writer (§7):** working. `s6-bitgen` generates the bitstream
  from nextpnr's routed design using prjcombine's bit database. Its output is
  byte-identical to ISE's bitgen for blinky, cylon, uart_echo and tristate.
  The default flow (`make bit`) uses no ISE at all. `tests/uart_echo` and
  `tests/tristate`, built this way, pass automatic hardware tests over the
  Mojo's USB serial port (`tools/uart-test.py`, `tools/tristate-test.py`).

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
make pnr       TEST=cylon   # yosys + nextpnr        -> build/cylon/routed.json
make bit       TEST=cylon   # + s6-bitgen (no ISE)   -> build/cylon/design.bin
make load      TEST=cylon   # + load into the Mojo's FPGA RAM
make ise-check TEST=cylon   # ISE cross-check: XDL -> drc -> bitgen, must be byte-identical
```

Hardware tests (load the design with `make load TEST=<name>` first; each
talks to the Mojo over its USB serial port):

| Test design | Script | Covers |
|---|---|---|
| `uart_echo` | `tools/uart-test.py` | basic logic, I/O |
| `tristate` | `tools/tristate-test.py` | OBUFT/IOBUF, pull-ups/downs (P50/P51 unconnected) |
| `alu` | `tools/alu-test.py` | carry chains, DSP |
| `rom` | `tools/rom-test.py` | MUXF7/MUXF8 |
| `bram` | `tools/bram-test.py` | RAMB16BWER/RAMB8BWER, initial contents (slow: ~12 min) |
| `dsp` | `tools/dsp-test.py` | DSP48A1 multiply and multiply-accumulate |
| `clocks`, `clocks2` | `tools/clocks-test.py [--config 2]` | DCM_SP, PLL_BASE frequencies |
| `lutram` | `tools/lutram-test.py` | RAM64X1S, RAM32M, RAM64M (separate read address), RAM128X1S, SRL16E, with initial contents; 70-deep SRLC32E chain (Q31) |
| `speed` | `tools/speed-test.py` | on-chip Fmax sweep of a timed path (see Delay model) |
| `falledge` | `tools/falledge-test.py` | falling-edge FF, SRL and RAM (slice clock inversion) |
| `latch` | `tools/latch-test.py` | latches: LDCE, LDPE, inverted gate |
| `dll` | `tools/dll-test.py` | DCM_SP DLL: CLK0 feedback, CLK90, CLK2X, CLKDV (2.5), CLKFX |

`fill` (no script) is a capacity test that only fits with LUT pairing and
5FFs. Shared UART helpers are in `tests/common/uart.v` (they honour the
AVR's `avr_rx_busy` and leave gaps between bytes: the bridge drops bytes
otherwise).

Only `make ise-check` needs ISE (see below). Its first run also dumps ISE's
view of the device and builds prjcombine's naming database
(`build/xc6slx9-geom.zstd`, about 2 minutes). Test designs live in
`tests/<name>/`, with pins from `tests/<name>/pins.ucf` or
`tests/mojo_v3.ucf`.

`make` clones nextpnr from the fork's `spartan6` branch, at the revision
pinned in the Makefile (`NEXTPNR_REV`), into `build/nextpnr`. To work on the
uarch, use your own checkout of that branch instead; it is built in
`build/nextpnr-build-dev`:

```
make NEXTPNR_SRC=../nextpnr-fork pnr TEST=cylon
```

After pushing uarch changes, bump `NEXTPNR_REV`. To run nextpnr by hand:

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

openFPGALoader can do the same with the `mojov3` board from the fork's
[`mojo-v3`](https://github.com/ellisgl/openFPGALoader/tree/mojo-v3) branch
(.bin or .bit; RAM and flash both tested on hardware):

```
openFPGALoader -b mojov3 build/cylon/design.bin       # RAM
openFPGALoader -b mojov3 -f build/cylon/design.bin    # flash, verified
```

## Delay model (`exporter/src/bin/s6-timing.rs`, `gen/s6_timing.py`)

prjcombine has no Spartan-6 timing, so the delays come from ISE: for each
test design placed and routed by nextpnr, ISE's `netgen -sim` writes an SDF
file for the same placement and routing (from the NCD that `make ise-check`
builds). In it every cell input carries the routing delay from its driver
(netgen puts FF and block RAM inputs' routing on `NlwBufferBlock_*`
buffers). `s6-timing` walks our route back from each LUT, FF, block RAM and
DSP input and fits one delay per pip class (the class of the driven wire,
e.g. `SNG_W`, `QUAD_NN`, `IMUX_LOGICIN`, or the slice mux; input muxes in
the block RAM / DSP columns' interface tiles are separate classes,
`IMUX_LOGICIN@INTF`) by non-negative least squares, and collects ISE's cell delays and
setup/hold checks. The result, `data/timing-xc6slx9-2.json`, is committed;
`make timing-data` regenerates it (needs ISE).

`gen/s6_timing.py` puts it in the chip database: pip class delays, and cell
timing for LUTs, FFs, CARRY4, wide muxes, RAM/SRL LUTs, BRAM, BUFGMUX and
DSP48A1 (below). nextpnr then places and routes timing-driven and reports
Fmax.

**DSP48A1.** Its timing depends on which pipeline registers are on and, when
OPMODE is tied to a constant, on the mode (ISE drops the paths the mode
can't use: a plain multiply's clock-to-P is 4.6 ns, multiply-add's 5.9 ns).
`tools/gen_dsp_char.py` generates characterization designs
(`tests/dsp_char*`, 16 DSPs each): all 128 combinations of the A, B, C, D,
M, P and OPMODE registers with OPMODE from logic, and the 64 combinations of
A..P for each of the constant modes 01, 09 (with PREG; ISE requires it), 0d,
11, 1d and 03. That's 480 chip database variants, `DSP_<registers>` and
`DSP_<registers>_<mode>`; the uarch picks one per cell in `prePlace`, the
constant-mode one when it exists. ISE times buses as a whole (`A[17:0] ->
P[47:0]`), so arcs are stored once on bit 0 and the uarch maps every bus pin
onto it. M isn't measured (ISE won't use M and P together) and is timed as
P. The characterization designs give cell timing only (`--cells-only`):
their long shift registers would skew the routing fit.

Fit: 46,080 connections from 10 designs, 164 ps rms error (mean connection
delay 1.42 ns). Against ISE's own analyser (`trce`) on the same placement
and routing (`tools/timing-compare.py`):

| Design | nextpnr | ISE trce | Error |
|---|---|---|---|
| bram | 9.34 ns | 9.29 ns | +0.6% |
| fill | 10.07 ns | 9.98 ns | +1.0% |
| rom | 6.51 ns | 6.67 ns | -2.5% |
| speed | 10.33 ns | 9.82 ns | +5.2% |
| alu (DSPs) | 14.73 ns | 13.95 ns | +5.7% |
| uart_echo | 7.19 ns | 7.63 ns | -5.8% |
| lutram | 7.06 ns | 7.65 ns | -7.6% |
| dsp | 8.55 ns | 7.91 ns | +8.0% |
| clocks | 7.67 ns | 8.38 ns | -8.6% |

What's left is mostly the routing model: per connection it's off by
~160 ps rms, and a long or high-fanout route (dsp's critical path is LUTs
with one 4.4 ns net) carries most of a path's error.

LUT inputs a table doesn't depend on get no timing arc (the uarch overrides
`getCellDelay`/`getPortTimingClass`): LUTs sharing a LUT6 site are wired to
the merged inputs, which otherwise adds false arcs and even false loops.

**On the board (`tests/speed`, `tools/speed-test.py`):** the test path is
three chained 32-bit additions and an XOR, launched every 4 test clocks and
captured both after 1 clock (timed) and after 3 (settled); mismatches are
counted. The sweep rewrites the PLL settings in the routed design and runs
s6-bitgen for each frequency, so placement and routing stay fixed. On the
Mojo V3 at room temperature it passes up to 162.5 MHz and starts failing at
166.7 MHz; worst-case predictions for the same routing are 98.8 MHz (nextpnr)
and 106.8 MHz (ISE), a ~1.5x margin, as expected between the slow corner
(85 C, 1.14 V) and a board on a desk.

prjcombine's SDF parser needed a few additions for ISE's output (operating
conditions, `PATHPULSE`, `RECREM`, single-value delays, escaped names and
bus ranges). The crate is vendored with those fixes in
`exporter/vendor/prjcombine-re-sdf` (see its README); the problems are
written up for upstream in `docs/prjcombine-sdf-issue.md`.

## Bitstream writer (`exporter/src/bin/s6-bitgen/`)

- **`write.rs`:** serializes a prjcombine `Bitstream` into ISE's exact
  packet sequence (registers, one FDRI with all-ones padding frames,
  startup), with the 22-bit CRC. `s6-bitgen roundtrip` parses an ISE `.bin`
  and writes it back byte-identical.
- **`config.rs`:** turns the routed design into frame contents.
  - **Pips:** routed pips set their switchbox bits.
  - **Slices:** LUT INIT, FF settings and slice muxes become bel attributes.
  - **IOBs:** drive and slew come from prjcombine's `IOB_DATA` table.
    `NDRIVE` uses the 2.5 V VCCAUX column, as ISE does.
  - **I/O logic:** ILOGIC and OLOGIC get their pass-through settings.
  - **Fixed values:** bitgen's register defaults, plus 5 always-set analog
    trim bits from ISE's empty-design output.
  - **SLICEX defaults:** a used SLICEX gets ISE's "unused" mux encodings.
- **`explain.rs`:** maps any bit back to its tile and prjcombine setting.

Commands:

- **`s6-bitgen gen --compare ise.bin`:** generates a bitstream and lists
  every differing bit with its meaning.
- **`s6-bitgen bits`:** lists a bitstream's set bits.

Supported: everything the packer supports (see the uarch section). Only LVTTL
has been verified against ISE among the I/O standards. Details worth knowing:

- **BRAM:** ISE's layout of RAMB16 contents (and its swizzle when a port is
  36 bits wide); a tile used as RAMB8s is loaded combined and split by a
  second write of one frame (frame fixups, also in `write.rs`).
- **DCM/PLL:** fixed values as ISE 14.7 writes them, PLL counters computed
  from the dividers, loop filter and lock settings from prjcombine's
  `PLL_MULT` table (BANDWIDTH OPTIMIZED = HIGH column). Checked against ISE
  for two configurations.
- **LUTRAM/SRL:** `<L>RAMMODE`, and GLUTMASK in the HCLK tile of each column
  region holding RAM/SRL LUTs.

Not supported: `.bit` file headers and bitgen options other than the
defaults.

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

## The uarch (nextpnr fork, `himbaechel/uarch/spartan6`)

- **Device and pins:** parses the device (`xc6slx9[-speed]tqg144`), loads the
  chip database and reads a UCF subset (`-o ucf=`): LOC, IOSTANDARD, DRIVE,
  SLEW, PULLUP, PULLDOWN. Bus syntax `led<0>` maps to `led[0]`.
- **Packer:** maps IBUF/IBUFG/OBUF/OBUFT/IOBUF to `IOB`, BUFG to `BUFGMUX`,
  LUT1–6 and INV to `SLICE_LUT`, and FDRE/FDSE/FDCE/FDPE to `SLICE_FF`.
  A pad that only feeds clock loads uses `GCLK_I`. The clock tree is placed
  during packing by searching the routing graph: pad -> BUFG / BUFIO2 ->
  DCM / PLL -> the BUFGs they drive, each on a site the previous stage reaches.
- **Carry chains (`carry.cc`):** a chain is one cluster placed by following
  CO3 -> CIN (it skips rows in places). Each CARRY4's S inputs get the LUT6 of
  their letter (inserting constant/pass-through LUTs), CI/CYINIT merge into
  one CI (behind the CYINIT mux), and DI goes through the letter's LUT5 (O5)
  when it fits, as ISE does. A letter that would export both O and CO gets CO
  rebuilt as O(i+1) ^ S(i+1).
- **Wide muxes (`wide.cc`):** a MUXF8 takes a whole SLICEL/SLICEM with its two
  MUXF7s and four LUTs; a lone MUXF7 sits on F7AMUX (LUTs A/B) or F7BMUX
  (C/D), chosen at placement.
- **Block RAM (`bram.cc`):** RAMB16BWER uses both halves of a BRAM tile (a
  cluster, pins split as in ISE's naming); RAMB8BWER one half. Don't-care pins
  (unused port, address bits below the width, data bits above it) are
  disconnected and used-but-undriven inputs tied low, as ISE does.
- **DSP (`dsp.cc`):** DSP48A1 with every input connected (open ones tied).
- **DCM DLL (CLK_FEEDBACK = 1X):** CLK0/90/180/270, CLK2X/CLK2X180 and
  CLKDV; the DLL bits as ISE sets them, and CLKDV_DIVIDE encoded as ISE
  does for all 22 legal values (checked against ISE for each). REG_BG in the
  CMT regulators: every DCM tile with a PLL in use, else the used DCMs'
  tiles.
- **Global clocks stay on the clock network:** clock input muxes fed from
  general routing (chip database pip flag 2) aren't available to nets
  driven by a BUFG; router2 otherwise sometimes finished a clock through
  the fabric (~1 ns of skew, a hold violation in `dll`).
- **Clock management (`cmt.cc`):** DCM_SP fed from the pad through an inserted
  BUFIO2 (as ISE's map does); PLL_BASE/PLL_ADV fed from the BUFG on CLKIN2.
- **Timing:** the speed grade comes from the device name (default -2);
  DSP cells get a timing variant by pipeline registers and constant OPMODE
  (`prePlace`); bus pins are timed through bit 0 (`dsp_timing_pin`).
- **Distributed RAM / SRL (`lutram.cc`):** SLICEM LUTs as RAM64X1S (up to four
  per slice sharing the address, one always the D LUT, whose inputs are the
  write address), RAM32M and RAM64M (whole slice; what yosys uses for a
  RAM with separate read and write addresses), RAM128X1S (D and C LUTs plus
  F7BMUX; C takes the data from DX through CDI1MUX, A6 on CX as WA7, as
  ISE's map lays it out; ISE allows that DX path only for RAM, so the
  uarch vetoes the pip, flagged in the chip database, for anything else)
  and SRLC32E/SRL16E (any SLICEM LUT). A cascade output Q31 (yosys chains
  SRLC32Es for shift registers longer than 32) is taken from Q at address
  31 when Q is unused, so the cascade is ordinary routing. Address pins tied to 0 (an SRL's fixed tap) and undriven
  data inputs are fed from one shared constant-0 LUT: some LUT address pins
  can't reach ground, and a RAM's address can't be folded into its table.
- **Bus pins** are named without brackets (`ADDRA5`): nextpnr's JSON writer
  regroups `X[i]` ports and loses where a partly used bus starts.
- **LUT pairing (LUT6 fracturing):** two LUTs with at most 5 distinct inputs
  between them, sharing at least one, are packed into one LUT6 site: both get
  the merged inputs on A1..A5 (tables rewritten), the LUT6 half (O6) gets A6
  tied to VCC, and the LUT5 half (O5, cell type `SLICE_LUT5`) is a cluster
  child one z above. Partners are chosen greedily by most shared inputs.
  `-o nolut5` turns this off. On the test designs it saves 25–35% of LUT
  sites. The XDL writer gives ISE `O6=(A6*f)`, since ISE rejects a tied-high
  A6 the equation doesn't use.
- **5FFs:** a 5FF's D is hardwired to its letter's O5 and its Q leaves only
  through `<L>MUX` (ISE's primitive definitions; no AX path on Spartan-6). So
  a FF that is the sole load of a pair's LUT5 half joins the pair's cluster
  on the 5FF bel (cell type `SLICE_FF5`); pairs are oriented so such a LUT
  becomes the LUT5 half. `tests/fill` (5824 FFs, 7451 LUTs) only fits the
  xc6slx9 this way, and matches ISE bitgen byte for byte. XDL quirk: the 5FF
  element has no cfg values, so it is written `A5FF:<name>:`.
- **Placement rules:** the FFs in a slice share CLK, CE and SR, and
  sync/async mode; RAM/SRL LUTs share WE and the clock. Each bypass pin
  (AX..DX) and each `<L>MUX` output carries one net (FF D, carry DI and
  carry-in, wide-mux selects, RAM data in; O5, 5FF Q, carry O/CO, wide-mux
  outputs). I/Os only go on bonded pads.
- **router2 fixes (in the fork's core):** sink-wire reservation no longer
  takes another output pin of the driver's bel as a possible source, and
  treats a wire tied to a constant net's value as drivable.
- **Constants:** GND/VCC are routed by router2 to any `TIE_0`/`TIE_1`/`CONST_*`
  wire.

Packer limits for now:

- **5FFs only behind paired LUTs.** A FF fed by anything else (another FF,
  an I/O, a LUT with other loads) uses a main FF; a pass-through LUT5 could
  free those too.
- **Placement doesn't minimise slices:** on a mostly empty chip the placer
  spreads logic out, so small designs don't use fewer slices; the gain from
  pairing and 5FFs is capacity.
- **Not supported yet (packer errors out):** DSP48A1 cascades (PCIN/BCIN/
  CARRYIN), an SRL using both Q and Q31 (MC31 cascade), RAM32X1S/D,
  RAM64X1D, RAM128X1D, DCM CLK_FEEDBACK = 2X, DLL_FREQUENCY_MODE = HIGH,
  phase shifting (CLKOUT_PHASE_SHIFT, PSDONE) and duty-cycle correction off,
  PLL compensation other than INTERNAL, PLL CLKINSEL = CLKIN2 driven
  dynamically.
- **Untested:** PLL fed through CLKIN1 (BUFIO2) rather than CLKIN2.
- **Timing:** from the delay model below; DSP OPMODE values other than the
  six measured fall back to the OPMODE-from-logic numbers (pessimistic).
- **Latches (LDCE/LDPE):** slice FFs in latch mode (`FF_LATCH`, slice-wide,
  so part of the control set; never 5FFs), gate on the clock pin with the
  slice's clock inversion for an active-high gate (as ISE writes CLKINV =
  CLK_B), gate enable on CE, CLR/PRE asynchronous on SR. Timing treats them
  as edge-triggered (the transparent D -> Q path isn't modelled).
- **Clock inversion:** FDxE_1 and `IS_*_INVERTED` clocks set the slice's
  CLKINV (prjcombine: the SLICE CLK pin's inversion bit), so it's part of
  the slice control set; a LUT1 inverter on a FF/RAM/SRL clock is absorbed
  into it, as ISE's map does (yosys inverts RAM64M clocks that way). Timing
  uses falling-edge variants (`SLICE_FF_N`, `SLICE_LUTRAM_N`, ...).
- **Constant CE/SR:** CE tied high and SR tied low are disconnected in the
  packer (slice defaults), since ISE treats GND -> SR as an excluded arc.
- **No OUTMUX=O6:** the exporter leaves out the OUTMUX=O6 slice option. It
  duplicates the direct `<L>` pin, and using both makes ISE see two drivers.

## Not done yet

- **Uarch-specific data.** `carry_prev` and the pips' `src_ref` are in the JSON
  but not yet in the `.bba`. They need a uarch `extra_data` struct.
- **Pad functions.** GCLK and other pad functions are empty.
- **Timing coverage:** I/O, DCM/PLL and the clock spines use a typical
  routing delay; the delay model was measured for -2 only (the other speed
  grades get the same numbers).
- **Bitstream references.** The JSON keeps a `src_ref` for each pip for the
  future bitstream writer. The `.bba` doesn't carry it.
