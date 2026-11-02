## re/sdf: parser rejects SDF files written by ISE 14.7 `netgen`

I'm using `prjcombine-re-sdf` (rev `2b3317d`) to read the timing-simulation
SDF that ISE 14.7 writes for Spartan-6 designs:

```
netgen -sim -ofmt verilog -w design_routed.ncd timing_sim.v
```

(`(PROGRAM "Xilinx SDF Writer") (VERSION "P.20131013")`, SDF 3.0, xc6slx9-2.)
The parser panics on several constructs in these files. All of them are valid
SDF 3.0 as far as I can tell. In the order the parser hits them:

1. **Operating-condition headers.** The file header has

   ```
   (VOLTAGE 1.14)
   (TEMPERATURE 85)
   ```

   → `weird top-level item VOLTAGE`. (`PROCESS` is the third header of this
   kind in the spec.) Skipping them would be enough for my use; storing them
   might be nicer.

2. **Escaped characters at the start of an identifier.** Instance names from
   synthesized netlists start with an escape:

   ```
   (INSTANCE \$auto\$clkbufmap\.cc\:261\:execute\$92236)
   ```

   → `weird char '\'`. Escapes *inside* identifiers are already handled; only
   a leading `\` isn't accepted as the start of an identifier.

3. **`PATHPULSE` inside `DELAY`.** Almost every cell has

   ```
   (DELAY
     (PATHPULSE (240))
     (ABSOLUTE ...))
   ```

   → `weird delay item PATHPULSE`. Skipping it (and `PATHPULSEPERCENT`) works
   for delay extraction.

4. **A single delay value for both edges.** DSP48A1 ports have one value instead
   of a rise/fall pair:

   ```
   (PORT C[47] ( 1004 ))
   ```

   → assertion failure in `eat_lp` (expects a second `(`). Per the spec a
   single value applies to both transitions; this affects `PORT` and `IOPATH`.

5. **Bus ranges in port names.** DSP48A1 timing uses ranges:

   ```
   (IOPATH C[47:0] P[47:0] ( 3141 )( 3141 ))
   (SETUPHOLD(posedge A[17:0]) (posedge CLK) (161)(83))
   ```

   → `weird edge`: the `:` inside the brackets is read as a separate token.

6. **`RECREM` timing checks.** Block RAM (and FFs with asynchronous resets) have

   ```
   (RECREM(negedge RSTA) (posedge CLKA) (280)(300))
   ```

   → `weird timingcheck item RECREM`. It maps onto the existing `RecRem`
   struct, with both `recovery` and `removal` set.

With all six handled, the parser reads every SDF I have from these designs
(LUTs, FFs, CARRY4, MUXF7/F8, distributed RAM and SRLs, RAMB16BWER/RAMB8BWER,
DSP48A1, BUFGMUX, BUFIO2, DCM_SP, PLL_ADV), about 1,500 cells each.

**Context:** I'm building a Spartan-6 delay model for a nextpnr target from
these files: every cell input's PORT delay is the routing delay from its
driver, so fitting per-wire-class delays over many designs gives a routing
model (and the IOPATH/SETUPHOLD entries give cell timing). If prjcombine would
want Spartan-6 speed data along those lines, I'm happy to share the approach
and the SDF files.

I can attach example SDF files if that helps.
