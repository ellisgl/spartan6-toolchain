# router2: two fixes in `is_wire_undriveable`

**Source:** `nextpnr/common/route/router2.cc` (9 lines)

## Summary

Two cases where router2 judged a wire's drivability wrongly, found while
bringing up a new himbaechel uarch (Spartan-6); both are general:

1. **Constant nets.** A net with a `constant_value` can start from any wire
   tied to that value (`getWireConstantValue`), not only from its driver
   bel's pins. Without this, wires fed only by tie-offs were treated as
   undriveable for GND/VCC nets.
2. **Other outputs of the driver's bel.** The check accepted any output pin
   of the driver's bel as a possible start, so a wire on a *different*
   output of the same bel (e.g. a CARRY4's CO3 next to the O2 that actually
   drives the net) counted as drivable. Now only the net's own source wire
   does.

```diff
+        // A constant net can start from any wire tied to its value.
+        if (net->constant_value != IdString() && ctx->getWireConstantValue(wire) == net->constant_value)
+            return false;
+        // Only the driver's own output pin counts: another output of the same
+        // bel (e.g. CARRY4 CO3 next to the O2 actually driving the net) can't
+        // drive it.
         for (auto bp : ctx->getWireBelPins(wire))
-            if ((net->driver.cell == nullptr || bp.bel == net->driver.cell->bel) &&
+            if ((net->driver.cell == nullptr ||
+                 (bp.bel == net->driver.cell->bel && wire == nets.at(net->udata).src_wire)) &&
                 ctx->getBelPinType(bp.bel, bp.pin) != PORT_IN)
                 return false;
```

## Testing

Used for every design of the Spartan-6 uarch (~110 test designs, each
routed and checked bit-for-bit against the vendor tools). Worth running the
existing uarches' CI on it too.
