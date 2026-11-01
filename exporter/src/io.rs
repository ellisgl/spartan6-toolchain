//! Give each IOB bel the fabric-facing pins nextpnr needs.
//!
//! In prjcombine the IOB bel only exposes `I`, the dedicated clock-pad path
//! (`OUT_CLKPAD_I`). Ordinary data reaches the pad through the IOI tile:
//! fabric -> OLOGIC D1/T1 -> pad, and pad -> ILOGIC FABRICOUT -> fabric. The
//! hop between ILOGIC/OLOGIC and the pad is internal and has no wire.
//!
//! For simple I/O (IBUF/OBUF/OBUFT), the IOB bel gets:
//!   I      <- ILOGIC[n].FABRICOUT (input data to fabric)
//!   O      -> OLOGIC[n].D1        (output data from fabric)
//!   T      -> OLOGIC[n].T1        (tristate from fabric)
//!   GCLK_I <- the original clock-pad output
//! The bitstream writer must then configure ILOGIC/OLOGIC as pass-throughs.
//!
//! Pairing ILOGIC[n]/OLOGIC[n] with IOB[n] follows prjcombine's IOI tile,
//! where ILOGIC[n].DFB drives OUT_CLKPAD_DFB[n] alongside IOB[n]'s
//! OUT_CLKPAD_I[n]. Still to confirm against ISE.

use crate::{OutBelPin, TypeBuilder};

pub fn merge_iob_paths(tb: &mut TypeBuilder) {
    let find_pin = |tb: &TypeBuilder, bel: &str, pin: &str| -> Option<usize> {
        let b = tb.out.bels.iter().find(|b| b.name == bel)?;
        Some(b.pins.iter().find(|p| p.name == pin)?.wire)
    };
    for n in 0..2 {
        let iob = format!("IOB[{n}]");
        let Some(bi) = tb.out.bels.iter().position(|b| b.name == iob) else {
            continue;
        };
        let (Some(i), Some(o), Some(t)) = (
            find_pin(tb, &format!("ILOGIC[{n}]"), "FABRICOUT"),
            find_pin(tb, &format!("OLOGIC[{n}]"), "D1"),
            find_pin(tb, &format!("OLOGIC[{n}]"), "T1"),
        ) else {
            panic!("{} has {iob} without a matching ILOGIC/OLOGIC", tb.out.name);
        };
        let bel = &mut tb.out.bels[bi];
        for p in &mut bel.pins {
            if p.name == "I" {
                p.name = "GCLK_I".into();
            }
        }
        let mk = |name: &str, wire, dir| OutBelPin { name: name.into(), wire, dir, inv: false, invertible: false };
        bel.pins.push(mk("I", i, "OUT"));
        bel.pins.push(mk("O", o, "IN"));
        bel.pins.push(mk("T", t, "IN"));
    }
}
