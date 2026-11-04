# spartan6-toolchain

[![CI](https://github.com/ellisgl/spartan6-toolchain/actions/workflows/ci.yml/badge.svg)](https://github.com/ellisgl/spartan6-toolchain/actions/workflows/ci.yml)

An open-source FPGA toolchain for the Xilinx Spartan-6 xc6slx9 (developed on
the Mojo V3, TQG144): Verilog to a bitstream with no vendor tools.

```
Verilog ──yosys (synth_xilinx -family xc6s)──▶ netlist
        ──nextpnr-himbaechel (spartan6 uarch)──▶ placed & routed design
        ──s6-bitgen──▶ bitstream ──▶ FPGA (s6-load, or openFPGALoader)
```

Every feature is cross-checked against ISE 14.7 (the same placement and
routing written as XDL, converted and bitgen'd by ISE, must give a
byte-identical bitstream) and tested on the board where it can be.

## Quick start

On Ubuntu 24.04 (CI does exactly this):

```
sudo apt install build-essential cmake curl libboost-filesystem-dev \
    libboost-program-options-dev libboost-iostreams-dev libboost-system-dev \
    libboost-thread-dev libeigen3-dev
# Rust 1.85+ (https://rustup.rs) and Yosys (e.g. the OSS CAD Suite)
git clone https://github.com/ellisgl/spartan6-toolchain && cd spartan6-toolchain
make                          # nextpnr, prjcombine's database, the chip database (~15 min)
make bit TEST=blinky          # -> build/blinky/design.bin
make load TEST=blinky         # onto a Mojo V3 over USB (FPGA RAM)
```

Your own design: `tests/<name>/<name>.v` (top module `<name>`) with a
`pins.ucf` (LOC, IOSTANDARD, DRIVE, SLEW, PULLUP/PULLDOWN, and the clock
period, e.g. `NET "clk" PERIOD = 50 MHz;`; copy one from `tests/`), then
`make bit TEST=<name>`. Another Spartan-6 part: add
`DEVICE=xc6slx16 PACKAGE=ftg256` (see Building and using).

Prebuilt binaries: each [release](https://github.com/ellisgl/spartan6-toolchain/releases)
has nextpnr, the chip databases and the Rust tools for Linux x86-64; the
bitstream writer also needs prjcombine's database:

```
curl -fLO https://codeberg.org/prjunnamed/prjcombine/raw/commit/2b3317db3799b82b3ef5606438145ba76b9b4485/databases/spartan6.zstd
```

## Layout

| Directory | What |
|---|---|
| `nextpnr/` | nextpnr (git subtree of YosysHQ/nextpnr) with the Spartan-6 himbaechel uarch in `nextpnr/himbaechel/uarch/spartan6` (packing, placement rules, timing variants) |
| `exporter/` | Rust tools over prjcombine's Spartan-6 database: `s6-export` (chip database JSON), `s6-chipdb` (nextpnr chip database `.bba` from the JSON, with the delay model), `s6-check` (sanity checks on the JSON), `s6-bitgen` (bitstream writer), `s6-xdl` (XDL writer for the ISE cross-check), `s6-timing` (delay model fit), `s6-load` (Mojo V3 loader) |
| `data/` | the fitted delay models (`timing-xc6slx9-2.json`, `timing-xc6slx9-3.json`) |
| `tests/` | test designs (one directory each, with `pins.ucf`), `tests/common` (UART helpers) |
| `tools/` | ISE wrappers, hardware test scripts (Python), timing comparison, test design generators |
| `docs/` | the original plan and upstream issue drafts |

prjcombine (the Spartan-6 bit and routing database) is used as a pinned
library, not vendored, except for a patched SDF parser in
`exporter/vendor/prjcombine-re-sdf`.

## Building and using

```
make                        # nextpnr and the chip database
make check                  # chip database sanity checks
make pnr       TEST=cylon   # yosys + nextpnr        -> build/cylon/routed.json
make bit       TEST=cylon   # + s6-bitgen (no ISE)   -> build/cylon/design.bin
make load      TEST=cylon   # + load into the Mojo's FPGA RAM
make ise-check TEST=cylon   # ISE cross-check: XDL -> drc -> bitgen, must be byte-identical
make bit TEST=cylon NPNR_OPTS="-o ff5pass"   # extra nextpnr options (remove routed.json first)
```

Your own design: put it in `tests/<name>/<name>.v` with a `pins.ucf`, or run
the tools directly:

```
yosys -p "synth_xilinx -family xc6s -top top -json synth.json" top.v
build/nextpnr-build/nextpnr-himbaechel --device xc6slx9-2tqg144 \
    --chipdb build/chipdb-xc6slx9.bin --json synth.json -o ucf=pins.ucf --write routed.json
build/target/release/s6-bitgen --db build/spartan6-*.zstd gen --routed routed.json \
    --chipdb-json build/xc6slx9.json --out design.bin
```

Requires Rust 1.85+ (edition 2024), curl, CMake, a C++17 compiler, Boost and
Eigen (for nextpnr), and Yosys. The hardware test scripts in `tools/` need
Python 3.10+ with pyserial. Only `make ise-check` and `make timing-data` need
ISE (see below).

**Speed grade.** `SPEED=3` (default 2, the Mojo's) times and checks a design
at -3: outputs go to `build/<device>-<package>-3/<test>`, and the chip
database has the measured -2 and -3 delay models (`data/timing-xc6slx9-2.json`,
`-3.json`; `make timing-data SPEED=3` regenerates the -3 one).

**Other devices.** `DEVICE` and `PACKAGE` select another Spartan-6 LX/LXT
part; outputs go to `build/<device>-<package>/<test>`:

```
make DEVICE=xc6slx16 PACKAGE=ftg256                           # its chip database
make bit DEVICE=xc6slx16 PACKAGE=ftg256 TEST=blinky
make ise-check DEVICE=xc6slx16 PACKAGE=ftg256 TEST=blinky     # needs ISE
```

Pins come from `tests/<name>/pins-<package>.ucf` if there is one; otherwise
nextpnr picks them (a clock input goes on a clock-capable pin). The xc6slx4,
xc6slx16, xc6slx25, xc6slx25t and xc6slx45 export and pass `make check`, and
19 of the test designs (logic, carry, block RAM, LUT RAM and SRLs, DSPs with
cascades and the pre-adder, DCM, PLL, tristate) are byte-identical to ISE on
each. The LX parts share their die with the LXT, GTPs disabled; ISE still
sets the GLUTMASK bits of the GTP columns' HCLK tiles, which prjcombine
leaves out, so s6-bitgen sets them from the LXT layout. Nothing but the
xc6slx9 has been on hardware, and every device is timed with the xc6slx9's
delay model for its speed grade (same fabric).
GTP transceivers, PCIe and the memory controller (MCB) aren't supported.

**DSP inference.** yosys packs multipliers, multiply-accumulate, P cascades
and the pre-adder into DSP48A1s. Stock yosys only finds the pre-adder when
the sum is the second operand (`a * (d + b)`, not `(d + b) * a`); the
`xc6s-preadd-swap` branch of [ellisgl/yosys](https://github.com/ellisgl/yosys/tree/xc6s-preadd-swap)
handles both.

## Tests

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
| `wideram` | `tools/wideram-test.py` | RAM64X1D, RAM128X1D, RAM256X1S with initial contents |
| `phase` | `tools/phase-test.py` | DCM fixed phase shift +-90 and +-135 degrees (sampled on the board clock) |
| `dll2x` | `tools/dll-test.py --design dll2x` | as `dll`, with CLK2X fed back (CLK_FEEDBACK = 2X) |
| `oddr` | `tools/oddr-test.py` | ODDR2: the pin read back on both clock edges shows D0 then D1 (`oddr_x4`: ISE check only) |
| `iddr` | `tools/iddr-test.py` | IDDR2: a pin driven from the fabric and read back on both clock edges |
| `srlq31` | `tools/srlq31-test.py` | SRLC32E using both a Q tap and Q31 (cascade), 8 lanes |
| `ram32` | `tools/ram32-test.py` | RAM32X1S and RAM32X1D (8 lanes each) with initial contents |
| `dspcasc` | `tools/dspcasc-test.py` | DSP48A1 cascades: PCOUT -> PCIN, BCOUT -> BCIN, CARRYOUT -> CARRYIN over three DSPs |
| `preadd` | `tools/preadd-test.py` | DSP48A1 pre-adder and pre-subtracter (inferred by yosys) |
| `oddrt` | `tools/oddrt-test.py` | tristated DDR outputs: ODDR2 data with an ODDR2 on T, 64 driven/released checks |
| `oddr2clk` | `tools/oddr-test.py` | as `oddr`, with C0/C1 from a DCM's CLK0 and CLK180 |
| `iddr2clk` | `tools/iddr-test.py` | as `iddr`, with C0/C1 from a DCM's CLK0 and CLK180 |
| `lvds` | `tools/lvds-test.py` | LVDS_33 OBUFDS -> IBUFDS with DIFF_TERM, looped back by jumpers P41 -> P6, P40 -> P5 |
| `iddrsr` | `tools/iddrsr-test.py` | IDDR2 with its reset from logic |
| `oddrtsr` | `tools/oddrtsr-test.py` | ODDR2 set/reset shared by a pad's data and tristate registers, 12 cases |
| `psvar` | `tools/psvar-test.py` | DCM variable phase shift (PSEN / PSINCDEC / PSDONE), 1600 steps |
| `pll3` | `tools/pll3-test.py` | PLL_BASE with BUFG feedback (BUFIO2FB, ILOGIC route-through), LOCKED |
| `pll2` | `tools/pll2-test.py` | PLL_BASE with default compensation, direct feedback, LOCKED |
| `pllsel2` | `tools/pll2-test.py` | as `pll2` on a PLL_ADV, clock on CLKIN2 with CLKINSEL = 0 (DRP tied off) |
| `pllseldyn` | (ISE check only) | PLL_ADV CLKINSEL from logic: P56 on CLKIN1, P55 on CLKIN2 (on the Mojo P55 is the AVR's serial line, so not on the board) |
| `plldrp` | `tools/plldrp-test.py` | PLL_ADV DRP from logic: CLKOUT0's divider rewritten over the DRP (125 -> 100 -> 125 MHz), read back, LOCKED |
| `ffcap` | `tools/ffcap-test.py` | 7680 FFs (960-stage, 8-lane shift register with reset): more than the main FFs, so 2629 sit on 5FFs through pass-through LUT5s |
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
build/target/release/s6-load build/cylon-ise/cylon.bin            # RAM (lost on power cycle)
build/target/release/s6-load --flash build/cylon-ise/cylon.bin    # flash, verified
```

`s6-load` (also `make load TEST=...`) speaks the official loader's serial protocol to the Mojo's
AVR (`/dev/ttyACM0`, or `--port`; needs membership of `dialout`). With
`--flash` it writes the flash, reads it back to verify (about as long as the
upload: ~18 s for an LX9 bitstream) and then configures the FPGA from it.

openFPGALoader can do the same with the `mojov3` board from the
[`mojo-v3`](https://github.com/ellisgl/openFPGALoader/tree/mojo-v3) branch of
ellisgl/openFPGALoader
(.bin or .bit; RAM and flash both tested on hardware):

```
openFPGALoader -b mojov3 build/cylon/design.bin       # RAM
openFPGALoader -b mojov3 -f build/cylon/design.bin    # flash, verified
```

## Delay model (`exporter/src/bin/s6-timing.rs`, `exporter/src/bin/s6-chipdb/timing.rs`)

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

`s6-chipdb` puts it in the chip database: pip class delays, and cell
timing for LUTs, FFs, CARRY4, wide muxes, RAM/SRL LUTs, BRAM, BUFGMUX and
DSP48A1 (below). nextpnr then places and routes timing-driven and reports
Fmax against the clock periods from the UCF (`PERIOD`, or `TNM_NET` +
`TIMESPEC`), carried through the clock buffers and derived for DCM/PLL
outputs. A clock without a period is timed at nextpnr's default 12 MHz.
The Mojo test designs all give the board's 50 MHz (`tests/mojo_v3.ucf` and
each `pins.ucf`), except the DSP characterization designs (`dsp_char*`,
`dsp_casc_char*`, deliberately slow paths) and `speed` (its PLL clock is
swept past the failing point), which stay at the default.

**DSP48A1.** Its timing depends on which pipeline registers are on and, when
OPMODE is tied to a constant, on the mode (ISE drops the paths the mode
can't use: a plain multiply's clock-to-P is 4.6 ns, multiply-add's 5.9 ns).
`tools/gen_dsp_char.py` generates characterization designs
(`tests/dsp_char*`, 16 DSPs each): all 128 combinations of the A, B, C, D,
M, P and OPMODE registers with OPMODE from logic, and the 64 combinations of
A..P for each of the constant modes yosys produces, 01, 09 (with PREG; ISE
requires it), 0d, 11, 1d, 03 and 51 (pre-subtract), and for the common
hand-instantiated ones, 0b (P + D:A:B), 0c (C), 0f (C + D:A:B, a 48-bit
add), 19 (pre-adder multiply-accumulate), 89 (P - M) and 8d (C - M); and
05 (multiply plus PCIN, what yosys uses for cascades), 07 (D:A:B + PCIN)
and 15 (pre-adder multiply plus PCIN) in cascade chains. That's 1024 chip
database variants, `DSP_<registers>` and
`DSP_<registers>_<mode>`; the uarch picks one per cell in `prePlace`, the
constant-mode one when it exists. ISE times buses as a whole (`A[17:0] ->
P[47:0]`), so arcs are stored once on bit 0 and the uarch maps every bus pin
onto it. M isn't measured (ISE won't use M and P together) and is timed as
P. Cascades are measured by `tests/dsp_casc_char*`: one chain of all 16
DSPs per design (PCOUT -> PCIN, BCOUT -> B, CARRYOUT -> CARRYIN), the 128
OPMODE-from-logic configurations each twice, rotated by half a chain, so every
configuration is seen driving and receiving a cascade. Constant-OPMODE
variants take their cascade-pin arcs from the OPMODE-from-logic variant with
the same registers. ISE's 59 ps cascade wire delay isn't modelled.
The characterization designs give cell timing only (`--cells-only`):
their long shift registers would skew the routing fit.

Fit: 52,639 connections from 13 designs, 151 ps rms error (mean connection
delay 1.40 ns; at -3, 133 ps rms, mean 1.24 ns). Routing in the I/O tiles' interconnect (INT_IOI: the die's
outer rows and columns) is fitted separately (`<class>@IOI`): it is slower
than the fabric's, and the pad-to-fabric input (`OUT@IOI`) alone is about
1.5 ns. Against ISE's own analyser (`trce`) on the same placement and
routing (`tools/timing-compare.py`; half-cycle paths count double, as both
tools do):

| Design | nextpnr | ISE trce | Error |
|---|---|---|---|
| alu (DSPs) | 14.31 ns | 14.30 ns | +0.1% |
| bram | 9.07 ns | 8.97 ns | +1.1% |
| dsp | 8.17 ns | 8.29 ns | -1.4% |
| dspcasc | 7.42 ns | 7.57 ns | -2.0% |
| lutram | 8.74 ns | 8.92 ns | -2.0% |
| pll2 | 8.28 ns | 8.03 ns | +3.1% |
| fill | 8.84 ns | 9.13 ns | -3.2% |
| dll | 7.68 ns | 7.98 ns | -3.8% |
| clocks | 8.29 ns | 8.65 ns | -4.2% |
| srlq31 | 7.86 ns | 7.48 ns | +5.1% |
| iddr2clk | 8.48 ns | 8.02 ns | +5.7% |
| rom | 7.51 ns | 7.10 ns | +5.8% |
| tristate | 8.41 ns | 7.81 ns | +7.7% |
| speed | 9.67 ns | 8.83 ns | +9.5% |
| uart_echo | 6.88 ns | 7.61 ns | -9.6% |
| oddr (half-cycle path) | 9.47 ns | 10.71 ns | -11.6% |

Mean error 4.7%. What's left is mostly the routing model: per connection
it's off by ~150 ps rms (one delay per wire class can't follow ISE's
per-wire, per-load detail), and a long or high-fanout route carries most of
a path's error.

LUT inputs a table doesn't depend on get no timing arc (the uarch overrides
`getCellDelay`/`getPortTimingClass`): LUTs sharing a LUT6 site are wired to
the merged inputs, which otherwise adds false arcs and even false loops.

**On the board (`tests/speed`, `tools/speed-test.py`):** the test path is
three chained 32-bit additions and an XOR, launched every 4 test clocks and
captured both after 1 clock (timed) and after 3 (settled); mismatches are
counted. The sweep rewrites the PLL settings in the routed design and runs
s6-bitgen for each frequency, so placement and routing stay fixed. On the
Mojo V3 at room temperature it passes up to 180 MHz and starts failing at
183.3 MHz; worst-case predictions for the same routing are 105.2 MHz
(nextpnr) and 116.4 MHz (ISE), a ~1.55x margin, as expected between the slow
corner (85 C, 1.14 V) and a board on a desk. The PLL locks at every point.

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

Supported: everything the packer supports (see the uarch section). I/O
standards: LVTTL, LVCMOS33/25 and LVCMOS18/15/12, all verified against ISE
(the low-voltage ones set OUTPUT_LOW_VOLTAGE and use VCCINT input
thresholds; `tests/iostd*`, bits only, since the Mojo's banks are 3.3 V).
ISE's DRC limits their drive strength in bank 0 (LVCMOS18 16 mA, LVCMOS15
8 mA, LVCMOS12 6 mA). Differential: LVDS_33 (IBUFDS, OBUFDS) and LVDS_25
inputs, also verified against ISE, and the pad and bank bits against
ISE's own build of the same design (`tests/lvds`, also on the board).
VREF standards: HSTL_I/II/III, HSTL_I/II/III_18, SSTL15_II, SSTL18_I/II,
SSTL2_I/II and SSTL3_I/II (`tests/iostdsstl`, `tests/iostdvref*`, bits
only). Details worth knowing:

- **VREF standards:** each standard's own IOB_DATA row (drive and slew;
  no DRIVE/SLEW choice, and no DRIVEATTRBOX/SLEW in XDL), inputs in
  IBUF_MODE = VREF, outputs under 2.5 V in low-voltage mode. Inputs on a high
  reference (SSTL2, SSTL3) set VREF_HV; any input on a low one sets the
  chip-wide MISC_NW.VREF_LV. Every VREF pin of a bank with a VREF input
  (from prjcombine's bond, so s6-bitgen takes `--package`) gets VREF with
  its single-ended driver off; leave those pins free (ISE requires it).
  Class II outputs only in banks 1 and 3 (ISE's DRC).
- **Differential pairs:** the P pin is the pair's master (IOB[1]); it is an
  ordinary input or output to the fabric. The N pin is a slave linked
  inside the I/O tile (IBUFDS: slave PADOUT -> master DIFFI_IN; OBUFDS:
  master DIFFO_OUT -> slave DIFFO_IN; one ISE pip each, from ISE's own site
  pins). Both pins switch the single-ended driver off (IOB_DATA OFF; some of
  those bits are stored inverted, so it must be written) and inputs use
  IBUF_MODE = DIFF; the output master sets DIFF_MODE = LVDS and
  DIFF_OUTPUT_ENABLE, and its bank the LVDSBIAS table value (group 0).
  DIFF_TERM (internal 100 ohm, needed unless the board terminates the
  pair): the slave's DIFF_TERM bit, the master in LVDS mode, and both pins'
  slew written off.
  LVDS outputs only exist in banks 0 and 2 (ISE's rule). Both pins need a
  LOC.

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
  - VCC to a LUT input.
  - DSP48A1 PCOUT to the PCIN of the DSP above (cascade).
- Every bel input has a driver, except the CI at the bottom of each carry
  chain and the cascade inputs of the bottom DSP in each column. Every bel
  output has a load, except clock-pad outputs on pads that aren't
  clock-capable and the cascade outputs of the top DSP.
- The xc6slx9 has no inverting pips.

## The uarch (`nextpnr/himbaechel/uarch/spartan6`)

- **Device and pins:** parses the device (`xc6slx9[-speed]tqg144`), loads the
  chip database and reads a UCF subset (`-o ucf=`): LOC, IOSTANDARD, DRIVE,
  SLEW, PULLUP, PULLDOWN, and clock periods (`NET ... PERIOD = ...`, or
  `TNM_NET` with `TIMESPEC ... = PERIOD "<group>" ...`; times or frequencies,
  optional `HIGH`/`LOW` duty). Bus syntax `led<0>` maps to `led[0]`. Periods
  are carried through IOBs, BUFGs and BUFIO2s, and derived for DCM and PLL
  outputs from their multiply/divide parameters. Auto-named nets on the clock
  path are renamed after their source, so timing reports name clocks
  `clk$bufg`, `clk$bufio2`, `clk$CLKFX` (named nets are kept). Derived
  TIMESPECs (`TS_clk * 2`), OFFSET and FROM/TO are ignored with a warning.
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
  Cascades: PCOUT -> PCIN, CARRYOUT -> CARRYIN, and a B bus fed by the
  previous DSP's BCOUT (the dedicated BCIN, B_INPUT = CASCADE, as XST makes
  it; B itself is still tied to ground, which ISE's DRC wants). They run on
  hardwired wires to the DSP one site (4 rows) up, which prjcombine doesn't
  model: the exporter adds the cascade pins and joins each DSP's outputs to
  the next one's inputs as nodes, a chain is a cluster with each link 4 rows
  above the last, and the XDL writer gives each cascade net ISE's one pip in
  the lower DSP's tile. Cascade outputs reach only the next DSP (use
  CARRYOUTF / P for the fabric). Cascade paths are timed (see the delay
  model).
- **DCM DLL (CLK_FEEDBACK = 1X or 2X):** CLK0/90/180/270, CLK2X/CLK2X180 and
  CLKDV; the DLL bits as ISE sets them, and CLKDV_DIVIDE encoded as ISE
  does for all 22 legal values (checked against ISE for each). Fixed phase
  shift (CLKOUT_PHASE_SHIFT = FIXED, PHASE_SHIFT -255 .. 255): twice the
  magnitude plus a sign bit, wrapping to the equivalent shift the other way
  from 128 on (checked against ISE at 15 values). Variable phase shift
  (CLKOUT_PHASE_SHIFT = VARIABLE, PSEN / PSINCDEC / PSCLK / PSDONE from
  logic). A PSCLK on a pad's BUFG is taken from the pad's input buffer
  instead, through general routing, as ISE does: the pad then drives both
  its clock path (GCLK_I) and logic (I), one ISE net. The frequency synthesizer
  is enabled only when CLKFX/CLKFX180 is used. DLL_FREQUENCY_MODE = HIGH
  (without CLK90/270/2X/2X180, as ISE's DRC requires) and
  DUTY_CYCLE_CORRECTION = FALSE are one bit each (checked against ISE).
  Feedback through a BUFG goes through a BUFIO2FB, as ISE's map does (and
  as for a PLL): over an unused ILOGIC's CLK0 -> CFB0 route-through, whose
  clock comes from its own side's I/O clock line CLK1INTER (the exporter
  drops the ILOGIC clock mux inputs ISE can't route that way). REG_BG in the
  CMT regulators: every DCM tile with a PLL in use, else the used DCMs'
  tiles.
- **Global clocks as data:** a global clock reaches logic inputs only
  through the interconnect's GFAN muxes, which take GCLK 0..7, so a BUFG whose
  output also goes to logic (or a DCM's PSCLK) is placed on BUFGMUX 0..7.
- **Global clocks stay on the clock network:** pips from general routing
  into the clock network (clock input muxes, the CMT's fabric clock outputs
  onto the row clocks; chip database pip flag 2) aren't available to nets
  driven by a BUFG. router2 otherwise sometimes left the clock network and
  came back in (~1 ns of skew and a hold violation in `dll`; a route ISE
  rejects in `pll2`).
- **Clock management (`cmt.cc`):** DCM_SP fed from the pad through an inserted
  BUFIO2 (as ISE's map does); PLL_BASE/PLL_ADV likewise from a clock pad,
  on CLKIN1 or CLKIN2 (whichever the BUFIO2 reaches), else from a BUFG on
  CLKIN2. A constant CLKINSEL only picks which PLL_ADV input carries the
  clock (0: CLKIN2); that one is kept and placed as above, the other
  dropped. A tied-off DRP (constant DCLK/DEN/DWE/DADDR/DI, DO/DRDY unused)
  and the reserved REL pin are dropped too; a DRP used from logic keeps its
  pins (DCLK reaches the PLL through the interconnect's GFAN input, so its
  BUFG needs BUFGMUX 0..7, below). CLKINSEL from logic keeps both inputs,
  each from a clock pin through its own BUFIO2, on their own pins
  (CLKINSEL_MODE_DYNAMIC in the bitstream). The PLL's input pair mux takes
  two BUFIO2s together only as DIVCLK_CMT_V[n] on CLKIN1 with V[n + 4] on
  CLKIN2, which the clock-tree placement enforces: in practice the two pins
  of one clock pair on the PLL's edge (P56 / P55 on the TQG144's bottom). (Fed from the fabric BUFG, the PLL missed LOCKED at some
  multipliers, e.g. x9 and x10, though its outputs were right.) COMPENSATION
  = SYSTEM/SOURCE_SYNCHRONOUS (PLL_BASE's default) with CLKFBOUT wired
  straight to CLKFBIN becomes INTERNAL, as ISE's map does; with the feedback
  through a BUFG (as the Clocking Wizard writes it) a BUFIO2FB is inserted in
  front of CLKFBIN. The BUFG reaches it the way ISE routes it: through the
  clock network into an I/O tile and through an unused ILOGIC (clock to CFB0,
  an `RT_ILOGIC` pip in the chip database, flag 4: not on an ILOGIC whose pad
  is an input). The XDL writer takes ISE's own path for the I/O tile's clock
  wiring, which prjcombine doesn't name, and ISE adds the placeholder site.
- **Timing:** the speed grade comes from the device name (default -2);
  DSP cells get a timing variant by pipeline registers and constant OPMODE
  (`prePlace`); bus pins are timed through bit 0 (`dsp_timing_pin`).
- **Distributed RAM / SRL (`lutram.cc`):** SLICEM LUTs as RAM64X1S (up to four
  per slice sharing the address, one always the D LUT, whose inputs are the
  write address), RAM32M and RAM64M (whole slice; what yosys uses for a
  RAM with separate read and write addresses), the wide RAMs RAM64X1D, RAM128X1S,
  RAM128X1D and RAM256X1S (RAM64 LUTs plus F7/F8 muxes in one slice, laid
  out as ISE's map does: WA7 on CX, WA8 on BX, data on DX, C and B taking it
  through their DI1 mux and A through B's; ISE allows those DI1 paths only
  for RAM, so the uarch vetoes the pips, flagged in the chip database, for
  anything else)
  and SRLC32E/SRL16E (any SLICEM LUT). A cascade output Q31 (yosys chains
  SRLC32Es for shift registers longer than 32) is taken from Q at address
  31 when Q is unused, so the cascade is ordinary routing; with Q used too,
  a copy of the SRL (address 31) supplies Q31. RAM32X1S/D (hand-
  instantiated; yosys uses RAM32M) are RAM64X1S/D with the top address bit
  tied high and the contents in the upper half. Address pins tied to 0 (an SRL's fixed tap) and undriven
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
- **5FFs through pass-through LUT5s** (`pack_ff5_passthrough`): when more
  FFs are left than 90% of the main FF sites, FFs not behind a paired LUT
  get a LUT5 with O5 = A1 = their D in front of them, so they can take a 5FF.
  The pass-through shares a LUT6 site with an unpaired LUT of at most 4
  inputs; with none left, two FFs with the same control set share a site,
  both through pass-throughs (O6 -> main FF, O5 -> 5FF). Register-to-register
  FFs go first (the most slack for the extra LUT delay). `-o ff5pass` does
  it for every eligible FF (used to check it against ISE on small designs),
  `-o noff5pass` never. `tests/ffcap` (7680 FFs) moves 2629 and runs on the
  board; ten test designs with `-o ff5pass` are byte-identical to ISE.
- **Placement rules:** the FFs in a slice share CLK, CE and SR, and
  sync/async mode; RAM/SRL LUTs share WE and the clock. Each bypass pin
  (AX..DX) and each `<L>MUX` output carries one net (FF D, carry DI and
  carry-in, wide-mux selects, RAM data in; O5, 5FF Q, carry O/CO, wide-mux
  outputs). I/Os only go on bonded pads.
- **router2 fixes (in nextpnr's core, `nextpnr/common/route/router2.cc`):** sink-wire reservation no longer
  takes another output pin of the driver's bel as a possible source, and
  treats a wire tied to a constant net's value as drivable.
- **Constants:** GND/VCC are routed by router2 to any `TIE_0`/`TIE_1`/`CONST_*`
  wire.

Packer limits for now:

- **5FFs for other FFs only when the main FFs run out:** a pass-through
  LUT5 costs a LUT delay, so below 90% of the main FF sites a FF not behind
  a paired LUT stays on a main FF.
- **Placement doesn't minimise slices:** on a mostly empty chip the placer
  spreads logic out, so small designs don't use fewer slices; the gain from
  pairing and 5FFs is capacity.
- **Not supported yet (packer errors out):** a PLL's REL driven from logic
  (the site has no REL pin), and CLKINSEL from logic with inputs that aren't
  one clock pair (E/W pairs from the left and right edges aren't tried).
- **Timing:** from the delay model below; DSP OPMODE values other than the
  sixteen measured (01, 03, 05, 07, 09, 0b, 0c, 0d, 0f, 11, 15, 19, 1d, 51,
  89, 8d) fall back to the
  OPMODE-from-logic numbers (pessimistic). Other devices use the xc6slx9's
  delay model for their speed grade.
- **ODDR2 (DDR output, clock forwarding):** packed onto the pad's OLOGIC as
  a DDR register, clocked through the I/O tile's IOI_DDR (prjcombine's DDR
  clocking). With C1 = ~C0 (the LUT inverter yosys makes) it takes C0 on
  both inputs and inverts the CLK0 line feeding the site's CLK1
  (IOI_IOCLK_OPTINV); with two clocks (e.g. a DCM's CLK0 and CLK180) each
  goes on its own line, crosswise (C0 on the IOI_DDR's CLK1 input, which
  feeds the site's CLK0). The pad needs a LOC. In XDL the IOI_DDR's inputs become the OLOGIC2's two clock
  pins (ISE's own path through the clock line used, crosswise as ISE pairs
  them) and the OLOGIC drives the pad on its own net. A tristated DDR
  output: ISE requires T from an ODDR2 too (same clocks); it becomes the
  same OLOGIC's tristate register (TFF, T1/T2/TCE, TQ to the pad's T). ISE also masks frame
  28 of the column region (GLUTMASK_HCLK.FRAME28; in the clock column, which
  has no GLUTMASK bel in prjcombine, frame 18 of its HCLK bits).
- **IDDR2 (DDR input):** the same on the pad's ILOGIC (Q0 on the site's Q4,
  Q1 on Q3), clocked through the IOI_DDR the same way; the pad's input goes
  to the ILOGIC's D on its own XDL net. R or S comes over the SR line the
  ILOGIC shares with its OLOGIC (MUX_SR = OLOGIC_SR, as ISE routes it; the
  exporter gives the ILOGIC's SR pin that wire, since ISE's ILOGIC2 SR pin
  is only reachable that way).
- **ODDR2 set/reset:** R or S on the OLOGIC's SR; a tristate ODDR2 must
  share the data ODDR2's signal or neither use one (one SR line; ISE's
  rule), the kinds may differ. With R/S used, a register's init and
  set/reset value (SRINIT_OQ / SRINIT_TQ) follow its kind, as ISE's map
  folds them.
- **BUFG inputs:** a pad only feeds BUFGs over the dedicated clock path when
  that path reaches one (clock-capable pins); otherwise through general
  routing from the data input, on any free BUFGMUX. BUFGs use I0 only. A
  BUFG whose clock also reaches GFAN inputs (logic data, a DCM's PSCLK, a
  PLL's DCLK) needs BUFGMUX 0..7; when its pad's dedicated path reaches none
  of those (the Mojo's P56 only reaches 8..15), it goes on a free one through
  general routing, with a warning, as ISE does with CLOCK_DEDICATED_ROUTE =
  FALSE (ISE stops with Place:1108 otherwise). Feeding DCLK from the pad
  instead skews it against the logic's global clock: DRP reads and writes
  came back wrong on the board.
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

(Not needed in the chip database: `carry_prev` and the pips' `src_ref` stay
in the JSON only. The uarch follows carry chains through the routing graph
(CO3 -> CIN), and the bitstream and XDL writers read `src_ref` from the
JSON. Pads carry their bank and a function, `IO[_GCLK][_VREF]_<bank>`: GCLK
for the clock-capable ones, as found in the routing graph (all 28 of the
TQG144's, as ISE lists them), VREF for the package's VREF pins.)

- **Timing coverage:** the clock network's own wiring (BUFIO2/BUFPLL, the
  DCM/PLL clock inputs and outputs, the I/O clock lines: 38 classes, which
  `s6-chipdb` lists) uses a typical 250 ps per hop; it only matters across
  clock domains. The delay model is measured for -2 and -3 (-3N uses -3's);
  devices other than the xc6slx9 use the xc6slx9's (same fabric).
- **GTP, PCIe and memory controller** blocks.

## Licence

ISC (see `LICENSE`). Third-party code keeps its own licence: nextpnr in
`nextpnr/` (ISC, `nextpnr/COPYING`), and the patched prjcombine SDF parser in
`exporter/vendor/prjcombine-re-sdf` (0BSD or Apache-2.0). prjcombine itself
is used as a dependency (0BSD or Apache-2.0).
