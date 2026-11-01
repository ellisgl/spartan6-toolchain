# xc6slx9-pnr

Open-source place and route for the Spartan-6 xc6slx9 (Mojo V3, TQG144), as a
nextpnr himbaechel uarch built on prjcombine. The plan is in
[`Open-Source P&R for Spartan-6 (xc6slx9).md`](Open-Source%20P&R%20for%20Spartan-6%20(xc6slx9).md).

## Status

**Chip database exporter (§1): first version working.** The exporter
produces a himbaechel chip database for the xc6slx9 that nextpnr's `bbasm`
assembles. No nextpnr uarch loads it yet.

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
make NEXTPNR=/path/to/nextpnr     # build/xc6slx9.bba
make check                        # sanity checks
```

Requires Rust 1.85+ (edition 2024), Python 3.10+, curl, and a nextpnr source
checkout.

## How prjcombine maps onto himbaechel

| prjcombine | himbaechel |
| --- | --- |
| cell (col, row) | tile (x = col, y = row; y = 0 is the bottom of the die) |
| set of tile classes at a cell (e.g. `INT+CLEXL`) | tile type |
| tile-class wire used by a pip or bel pin | tile wire |
| tile wires resolving to one canonical wire | node |
| switchbox item (MUX, PROGBUF, PERMABUF, PROGINV, PAIRMUX, ...) | pip, one per source/destination pair; the item kind is the pip's timing class |
| bel (site) | bel, with prjcombine's site-level pins |

Wires of single-cell tile classes keep prjcombine's names (`IMUX_LOGICIN[0]`).
Wires of multi-cell classes are prefixed with the class and cell
(`BRAM.CELL[2].IMUX_...`), and nodes join them to the real cell's wire.

Slices: prjcombine has one `SLICE` class. The exporter names the nextpnr bel
types `SLICEX` (`SLICE[1]`), `SLICEM` (`SLICE[0]` in CLEXM) and `SLICEL`
(`SLICE[0]` in CLEXL). These counts match the datasheet (715/360/355).

## Verified on xc6slx9 (`make check`)

- Bel counts match DS160: 1430 slices, 16 DSP48A1, 32 RAMB16 (64 half-BRAM
  bels), 4 DCM, 2 PLL, 16 BUFGMUX. TQG144 has 102 user I/Os in banks 0–3.
- Routing paths exist for slice to slice across the die, P56 (Mojo clock)
  to BUFIO2 and to BUFGMUX, and BUFGMUX to slice CLK. No bel input pin is
  left without a driver.
- The xc6slx9 has no inverting pips.

## Not done yet

- **Slice internals.** Slices are exported as whole sites with ISE-style pins
  (A1–A6, AX, AQ, AMUX, ...). How the packer maps LUT/FF/CARRY4 cells onto them
  is still open: either pack whole-slice macro cells, or add sub-bels plus
  routing inside the slice the way the xc7 uarch does.
- **Uarch-specific data.** Carry-chain neighbours (`carry_prev`) are in the JSON
  but not yet in the `.bba`. They need a uarch `extra_data` struct.
- **Pad functions.** GCLK and other pad functions are empty.
- **ISE names** for XDL output. These need prjcombine's unpublished geom
  database (see the plan, §1).
- **Bitstream references.** The JSON keeps a `src_ref` for each pip for the
  future bitstream writer. The `.bba` doesn't carry it.
