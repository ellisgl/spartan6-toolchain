# xilinx_dsp: use the DSP48A1 pre-adder when the sum is the first operand

**Branch:** `xc6s-preadd-swap` on ellisgl/yosys (one commit, 469b408f2)

## Summary

The DSP48A/DSP48A1 (Spartan-3A DSP, Spartan-6) pre-adder only feeds the
multiplier's B input, and `xilinx_dsp48a.pmg` only looks for it there. So

```verilog
p <= a * (d + b);   // packs the pre-adder
p <= (d + b) * a;   // didn't: the adder became LUTs and carry chains
```

synthesised differently. The multiplier is symmetric (both inputs 18-bit
signed), so this adds a small pre-pass in `xilinx_dsp.cc` for the xc6s /
xc3sda families: on a freshly mapped DSP48A/DSP48A1 (no A/B pipeline
registers absorbed yet, D unused) whose A input is the whole output of an
`$add`/`$sub` (up to 18 bits, sign- or zero-extended) and whose B input is
not, swap A and B. The existing matcher then finds the pre-adder.

## Testing

- New case in `tests/arch/xilinx/dsp_preadder_sub.ys`: `(d + b) * a` must
  give one DSP48A1 and no CARRY4/LUTs. It fails before the change and
  passes after.
- The other `tests/arch/xilinx` DSP tests pass (`dsp_abc9.ys` fails the
  same way with and without the change, on an unrelated tri-state warning
  check when run standalone).
- On hardware (Spartan-6 xc6slx9, an open flow with nextpnr): `a * (d + b)`
  and `a * (d - b)` designs pack into the pre-adder (OPMODE 0x11 / 0x51)
  and compute correctly over 500 random and corner cases.

## Notes

The swap is only done before any register absorption, so the pipeline
register matching that follows sees the swapped ports from the start.
