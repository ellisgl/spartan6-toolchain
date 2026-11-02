# Open-Source P&R for Spartan-6 (xc6slx9)

Nov 1, 2026 · @Ken

The bitstream and routing-graph data already exist in Project Combine. What's missing is a chip database export, a packer, Spartan-6-specific placement checks, an XDL writer for validation, and a bitstream writer. The most practical route is a new **nextpnr himbaechel uarch**, using mainline nextpnr's `himbaechel/uarch/xilinx` (xc7) as the template.

Spartan-6 logic resources are close to 7-series: 4×LUT6, 8 storage elements, CARRY4, and F7/F8 muxes. The interconnect and clocking are not: Spartan-6 has BUFH, BUFIO2/BUFIO2FB, BUFPLL, and a different HCLK structure. Each Spartan-6 CLB also mixes slice types (one SLICEX plus one SLICEL or SLICEM), where a 7-series CLB doesn't.

## Components

### 1. Chip database (prjcombine → nextpnr .bba)

> **Status (Nov 1):** first version working. See `README.md`. The exporter builds an xc6slx9 `.bba` that nextpnr's `bbasm` assembles. Bel counts match the datasheet, and path searches for Blinky's connections pass. Still open: slice internals (packer decision), uarch `extra_data` (carry chains), pad functions, and ISE names.

- Write a Rust exporter against the `prjcombine-spartan6` crate (`public/spartan6`): tiles, bels, wires, pips, and the package pinout for xc6slx9 (TQG144 on the Mojo V3). The published database (`databases/spartan6.zstd`, with a `.txt` dump next to it) includes the xc6slx9-tqg144 bond. *Verified against prjcombine commit `2b3317d` (Oct 30, 2026).*
- Deduplicate by tile type. Himbaechel expects tile-type and instance tables, which matches how prjcombine already stores the data.
- The pip graph has to be complete, including pseudo-pips for route-throughs and the clock spine/HCLK structure.
- **Routing inside slices:** prjcombine models general interconnect as an interconnect database. The muxes inside a slice (O5/O6 output select, FF D-input mux, CYINIT, etc.) are stored as bel attributes, not pips. The exporter has to turn those into pseudo-pips, or the packer has to own them the way the xc7 uarch handles slice routing.
- **Keep ISE's names:** carry ISE's exact tile, site, bel pin, and pip names into the database. The XDL validation path depends on them. The published database uses prjcombine's own names (e.g. tile class `CLEXL`, wire `IMUX_LOGICIN[0]`), not ISE's. The ISE names are in the reverse-engineering "geom" database (`prjcombine-re-xilinx-naming-spartan6`, `prjcombine-re-xilinx-geom`), which isn't published. You generate it from an ISE dump (`rawdump` → `rd2geom`). The XDL path needs ISE anyway, so this costs one extra step, not a new dependency.
- **Package pins:** the pin table needs bonded pins plus their roles (GCLK, clock-capable, bank), not just pin locations.

### 2. Synthesis glue (Yosys)

- `synth_xilinx -family xc6s` already maps to Spartan-6 primitives. Switch the output to `-json`.
- You may need small techmap tweaks so cell and port names match what the packer expects. Expect only minor work here.

### 3. Packer

> **Status (Nov 1):** first version in `uarch/spartan6`, using sub-bels for slices. Blinky packs, places and routes on the xc6slx9 with the Mojo V3 pinout. Supported: LUTs (LUT6 bels only), FFs, simple I/O and BUFG. **Nov 2:** LUT6/LUT5 pairing added (two LUTs with ≤5 merged inputs share a LUT6 site, A6 tied high), byte-identical to ISE and passing the hardware tests. 5FFs added the same day: a FF that is the sole load of a LUT5 half goes on that letter's 5FF; `tests/fill` (5824 FFs, 7451 LUTs) only fits this way and is byte-identical to ISE. Not supported yet: CARRY4, F7/F8, SRL/LUTRAM, BRAM, DSP and clock management (DCM/PLL).

This is where the Spartan-6-specific logic lives:

- **Slice types:** SLICEX has LUT/FF only, with no carry and no wide muxes. SLICEL adds CARRY4 and F7/F8. SLICEM adds distributed RAM and SRL. Every CLB has one SLICEX plus one SLICEL or SLICEM, so the packer and placer have to respect which slice type each site is.
- **LUT6 fracturing:** a LUT6 can split into two LUT5s (O5/O6).
- **Storage elements:** each slice has 8. Four can be FFs or latches. The other four are FF-only, and their D input comes from O5 or the bypass input (AX–DX). If any element in the slice is used as a latch, the four FF-only elements can't be used.
- **Carry chains:** CARRY4 is only in SLICEL/SLICEM, so a carry chain stacks one slice per CLB in the same column. Chains need relative-placement clusters.
- **Control sets:** the FFs in a slice share CLK, CE, and SR, and SR has sync/async and init constraints.
- **Hard blocks:** RAMB16BWER (or 2×RAMB8BWER), DSP48A1, IOB with IOLOGIC (ILOGIC2, OLOGIC2, IODELAY2, ISERDES2, OSERDES2), BUFG/BUFGMUX, BUFH, BUFIO2/BUFIO2FB, BUFPLL/BUFPLL\_MCB, DCM\_SP, DCM\_CLKGEN, PLL\_ADV.
- **Out of scope for xc6slx9 on TQG144:** GTP and PCIe don't exist on this part, and its MCBs aren't usable in this package.

### 4. Placement

- Himbaechel's HeAP and simulated-annealing placers work once you write the `isBelLocationValid` checks for the slice rules above.
- IOB banking and I/O standard compatibility checks are also needed.

### 5. Routing

- router2 works on the interconnect pip graph as-is. Routing inside slices depends on the pseudo-pips or packer handling from §1.
- Clocks need special handling. Global nets should go BUFG → spine → HCLK → leaf, not through general interconnect.
- **The BUFIO2 path is needed from day one.** On Spartan-6, a GCLK input pin reaches BUFG/BUFGMUX through the BUFIO2 site, and the Mojo's 50 MHz oscillator is on a GCLK pin (P56). *Update (exporter, Nov 1):* prjcombine models the BUFIO2 bypass as an ordinary routing mux in the CLK_S tile (`OUT_CLKPAD_I` → `DIVCLK_CLKC`). The exported graph routes P56 → BUFGMUX in two pips without a BUFIO2 bel, so the router handles it and the packer doesn't need BUFIO2 for Blinky. The bitstream writer still has to set that mux, and the XDL writer has to express it the way ISE expects (probably as a BUFIO2 route-through; still to check).
- BUFPLL and the full BUFIO2/BUFH regional rules can wait until after the minimum viable version.

### 6. XDL writer

> **Status (Nov 1):** working. `s6-xdl` writes our packing, placement and routing as XDL. ISE converts it (`xdl -xdl2ncd`), checks it (`drc`: 0 errors) and runs `bitgen` with no `par`. Converting ISE's result back to XDL gives back exactly our pips. ISE names come from prjcombine's naming database, built from ISE with prjcombine's own dumper. Pips that prjcombine models as plain wires (clock-spine buffers, terminations, BUFH route-throughs) are filled in from ISE's own pip graph. Cylon and Blinky with our own routing run correctly on the Mojo V3.

- Write out the placed and routed design as XDL (instances, site configuration strings, nets with pips) using the ISE names kept in §1.
- Reuse `prjcombine-re-xilinx-xdl`. It already has an XDL `Design` model with `write` and `parse`, plus a `run_bitgen` helper. The new work is mapping nextpnr's placement and routing onto that model.
- The Blinky milestone needs this before the bitstream writer exists. See *Validation shortcut* below.

### 7. Bitstream writer

> **Status (Nov 1):** working (`s6-bitgen`). Its output is byte-identical to ISE's bitgen for blinky, cylon, uart_echo and tristate, using prjcombine's bit database, the bitstream module's frame layout and our own packet writer. ISE is no longer needed to build bitstreams. A UART echo design built entirely with the open flow passes an automatic hardware test on the Mojo V3. **Nov 2:** tristate outputs, bidirectional pins and pull-ups/pull-downs added; `tests/tristate` checks driven, pulled-up and pulled-down pins on hardware.

- Take the routed design, set the config bits from the prjcombine bitstream database, and lay them out into frames.
- Also handle the defaults for unused tiles and IOBs (pulls, tristates), GSR, FF INIT values, and STARTUP\_SPARTAN6 settings, then the header, packets, and CRC.
- Spartan-6 uses 16-bit config words, unlike the 32-bit words of xc7.
- **prjcombine has no Spartan-6 bitstream writer, and there is no `prjcombine-xilinx-bitstream` crate.** The `bitstream` module inside `prjcombine-spartan6` has the `Bitstream` and frame-address types, `empty_bitstream()`, a parser (`parse_bitstream`), and `Bitstream::diff`. Reuse those for the frame layer. Writing packets and CRC is new work, and the parser is the reference for the format. `diff` and the parser together are useful for comparing your output against ISE's.
- **Output formats:** write both `.bit` and raw `.bin`. The Mojo's AVR-based loader takes `.bin`.

### 8. Timing

- prjcombine has no Spartan-6 timing data. The published database lists speed grades only, with no delays. Start non-timing-driven with unit or fanout delays.
- Real timing would mean extracting delays from ISE speed files. prjcombine has done this only for the Xilinx CPLDs (`re/xilinx/cpld` speed DB), not for any FPGA family. `prjcombine-types` already has delay types (`speed.rs`) you could reuse. This is a separate project.

### 9. Constraints

- Parse a minimal UCF subset: `LOC`, `IOSTANDARD`, `PULLUP`, `DRIVE`, `SLEW`, `PERIOD`.
- `PERIOD` does nothing while the flow isn't timing-driven. Parse it now anyway, so that later timing work doesn't change the constraint format.

## Validation shortcut: XDL

ISE 14.7 still supports **XDL** for Spartan-6. You can:

1. Place and route with nextpnr, then emit XDL (§6).
2. Convert with `xdl -xdl2ncd`, then run ISE `bitgen` and `drc`.
3. Compare against your own bitgen output bit for bit.

This lets you debug the P&R separately from the bitgen, and ISE's DRC catches illegal packing before you burn a board. Once your bitgen matches, ISE drops out of the flow.

**Requirements for a useful bit-for-bit comparison:**

- Your writer has to copy the `-g` options passed to ISE bitgen (StartUpClk, UnusedPin, DONE/config pin pulls, etc.).
- Ignore the `.bit` file header, which contains a timestamp. Otherwise the first diffs will be noise.

This path needs ISE installed, which is the same cost as the *Containerize ISE* alternative below. The container is useful as a validation harness even if you build the fully open flow.

## Staging

| Milestone | Scope | Rough effort (one person, familiar with nextpnr) |
| --- | --- | --- |
| Blinky | LUT/FF in SLICEX, plain IOB, IBUFG → BUFIO2 → BUFG, via the XDL path (**done Nov 1**) | 1–2 months |
| Own bitgen | Matches ISE bitgen on test designs (**done Nov 1**) | +1–2 months |
| Useful | Carry, F7/F8, SLICEM RAM/SRL, BRAM, DCM/PLL | months |
| Parity-ish | DSP48A1, SERDES/IODELAY, BUFPLL, timing-driven | much longer |

**What drives these estimates:**

- **Blinky** covers the chip-database exporter, generating ISE's names from an ISE dump, pseudo-pips for routing inside slices, and the BUFIO2 clock path. The existing `prjcombine-re-xilinx-xdl` crate keeps the XDL side small.
- **Own bitgen** is new work, because prjcombine has no Spartan-6 bitstream writer. That means 16-bit packet serialization and CRC, plus defaults for unused tiles and IOBs, STARTUP settings, and matching ISE bitgen's `-g` options. prjcombine's parser and `diff` make each mismatch quick to locate, but expect a long tail of default bits.

For scale, nextpnr-xilinx for xc7 reached usable designs in a few months. It also started from a finished database (Project X-Ray, built by many contributors), so Spartan-6 starts from a similar position, not a better one. There is also less community tooling and testing for Spartan-6 than for xc7.

## Alternatives

- **Contribute to Project Unnamed** instead. It's Rust, built on prjcombine, and aims at end-to-end synthesis through bitstream across several families. It's less mature than nextpnr, but it's the native consumer of this data.
- **Containerize ISE.** If the real problem is ISE's size, put ISE 14.7 in a slim Docker image and drive the hybrid flow from a Makefile. It's still large on disk, but you never touch the IDE. This container also works as the validation harness for the XDL path.

## References

- [prjcombine (Codeberg)](https://codeberg.org/prjunnamed/prjcombine)
- [Project Unnamed (Codeberg)](https://codeberg.org/prjunnamed/prjunnamed)
- [nextpnr himbaechel xilinx uarch (mainline)](https://github.com/YosysHQ/nextpnr/tree/main/himbaechel/uarch/xilinx)
- [openXC7 nextpnr-xilinx (older standalone fork)](https://github.com/openXC7/nextpnr-xilinx)
- [yosys-spartan6-example](https://github.com/Streetwalrus/yosys-spartan6-example)
