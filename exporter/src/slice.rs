//! Split a Spartan-6 slice site into nextpnr sub-bels.
//!
//! prjcombine models a slice as one bel with ISE-style site pins and stores
//! its internal muxes as configuration attributes. nextpnr places LUTs and
//! FFs individually, so this module adds per-slice sub-bels and turns each
//! internal mux option into a pseudo-pip. The available options are read from
//! the attribute definitions of each slice instance, so SLICEX, SLICEL and
//! SLICEM each get exactly the muxes they have in silicon.
//!
//! Per letter X in A..D (UG384 Figures 2–4):
//!   XLUT6 (O6 -> site pin X), XLUT5 (O5 -> internal XO5)
//!   XFF   D <- MUX_FFX {XX, O6, O5, XOR, CY, F7/F8}, Q -> site pin XQ
//!   X5FF  D <- O5 (fixed, no mux in prjcombine), Q -> internal X5Q
//!   site pin XMUX <- MUX_XOUT {X5Q, O6, O5, XOR, CY, F7/F8}
//! Carry slices (SLICEL/SLICEM, SLICE[0]) add CARRY4, with DI from MUX_XCY0
//! and CYINIT from PRECYINIT {CONST_0, CONST_1, AX}. CIN is joined to the
//! slice below's COUT by a node built in main.
//! Wide-function slices add F7AMUX (A, B -> A outputs), F7BMUX (C, D -> C
//! outputs) and F8MUX (both F7s -> B outputs).
//!
//! Not modelled yet: SLICEM distributed RAM / SRL (DI pins, WE, MC31 cascade).

use prjcombine_interconnect::db::{Bel, BelAttribute, BelAttributeType, BelClass, IntDb};

use crate::{OutBel, OutBelPin, TypeBuilder};

const LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

/// Sub-bel z layout: 0x100 + 0x40 * slice index + offset.
pub const Z_BASE: usize = 0x100;
pub const Z_SLICE_STRIDE: usize = 0x40;
/// Per-letter offsets: letter * 8 + kind.
pub const Z_LUT6: usize = 0;
pub const Z_LUT5: usize = 1;
pub const Z_FF: usize = 2;
pub const Z_FF5: usize = 3;
pub const Z_F7AMUX: usize = 0x20;
pub const Z_F7BMUX: usize = 0x21;
pub const Z_F8MUX: usize = 0x22;
pub const Z_CARRY4: usize = 0x23;

pub fn carry_out_wire() -> String {
    "SLICE[0].DCY".into()
}

pub fn carry_in_wire() -> String {
    "SLICE[0].CIN".into()
}

/// Options available for an enum attribute on this slice instance.
fn enum_opts(db: &IntDb, bcls: &BelClass, bel: &Bel, name: &str) -> Vec<String> {
    let Some((aid, cattr)) = bcls.attributes.iter().find(|(_, a)| a.name == name) else {
        return vec![];
    };
    let BelAttributeType::Enum(ecid) = cattr.typ else {
        panic!("slice attribute {name} is not an enum");
    };
    match bel.attributes.get(aid) {
        Some(BelAttribute::Enum(e)) => e
            .values
            .iter()
            .map(|(vid, _)| db.enum_classes[ecid].values[vid].clone())
            .collect(),
        _ => vec![],
    }
}

#[allow(clippy::too_many_arguments)]
pub fn expand(
    tb: &mut TypeBuilder,
    db: &IntDb,
    prefix: &str,
    slice_type: &str,
    bcls: &BelClass,
    bel: &Bel,
    tile_slot: &str,
    site_pins: &[OutBelPin],
) {
    let site_kind = match slice_type {
        "SLICEX" => 1,
        "SLICEL" => 2,
        "SLICEM" => 3,
        _ => panic!("not a slice type: {slice_type}"),
    };
    let slice_idx = if prefix.ends_with("[0]") { 0 } else { 1 };
    let zb = Z_BASE + Z_SLICE_STRIDE * slice_idx;

    let pin = |name: &str| -> usize {
        site_pins
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{prefix} has no site pin {name}"))
            .wire
    };
    let has_carry = !enum_opts(db, bcls, bel, "MUX_ACY0").is_empty();
    let has_wide = enum_opts(db, bcls, bel, "MUX_FFA").iter().any(|v| v == "F7");

    let mut bels: Vec<OutBel> = vec![];
    let mut add_bel = |name: String, bel_type: &str, z: usize, pins: Vec<(&str, usize, &'static str)>| {
        bels.push(OutBel {
            name,
            bel_type: bel_type.into(),
            class: "SLICE".into(),
            z,
            site_kind,
            tile_slot: tile_slot.into(),
            pins: pins
                .into_iter()
                .map(|(n, wire, dir)| OutBelPin { name: n.into(), wire, dir, inv: false, invertible: false })
                .collect(),
        });
    };

    // Internal wires shared across letters.
    let (f7a, f7b, f8) = if has_wide {
        (
            Some(tb.pseudo_wire(format!("{prefix}.F7A"))),
            Some(tb.pseudo_wire(format!("{prefix}.F7B"))),
            Some(tb.pseudo_wire(format!("{prefix}.F8"))),
        )
    } else {
        (None, None, None)
    };

    let (clk, ce, sr) = (pin("CLK"), pin("CE"), pin("SR"));
    let mut carry_pins: Vec<(String, usize, &'static str)> = vec![];

    for (li, x) in LETTERS.into_iter().enumerate() {
        let zl = zb + li * 8;
        let ins: Vec<usize> = (1..=6).map(|k| pin(&format!("{x}{k}"))).collect();
        let bypass = pin(&format!("{x}X"));
        let o6 = pin(&format!("{x}"));
        let q = pin(&format!("{x}Q"));
        let outmux = pin(&format!("{x}MUX"));
        let o5 = tb.pseudo_wire(format!("{prefix}.{x}O5"));
        let ffd = tb.pseudo_wire(format!("{prefix}.{x}FF_D"));
        let q5 = tb.pseudo_wire(format!("{prefix}.{x}5Q"));
        let (xor, cy) = if has_carry {
            let cy = if x == 'D' {
                // CO3 is also the slice's COUT; main joins it to CIN above.
                tb.pseudo_wire(carry_out_wire())
            } else {
                tb.pseudo_wire(format!("{prefix}.{x}CY"))
            };
            (Some(tb.pseudo_wire(format!("{prefix}.{x}XOR"))), Some(cy))
        } else {
            (None, None)
        };
        let wide = match x {
            'A' => f7a,
            'B' => f8,
            'C' => f7b,
            _ => None,
        };

        let lut6_pins: Vec<(&str, usize, &str)> = ["A1", "A2", "A3", "A4", "A5", "A6"]
            .into_iter()
            .zip(ins.iter().copied())
            .map(|(n, w)| (n, w, "IN"))
            .chain([("O", o6, "OUT")])
            .collect();
        add_bel(format!("{prefix}.{x}6LUT"), "SLICE_LUT", zl + Z_LUT6, lut6_pins);
        let lut5_pins: Vec<(&str, usize, &str)> = ["A1", "A2", "A3", "A4", "A5"]
            .into_iter()
            .zip(ins.iter().copied())
            .map(|(n, w)| (n, w, "IN"))
            .chain([("O", o5, "OUT")])
            .collect();
        add_bel(format!("{prefix}.{x}5LUT"), "SLICE_LUT", zl + Z_LUT5, lut5_pins);
        let ff_pins = |d, q| vec![("D", d, "IN"), ("CK", clk, "IN"), ("CE", ce, "IN"), ("SR", sr, "IN"), ("Q", q, "OUT")];
        add_bel(format!("{prefix}.{x}FF"), "SLICE_FF", zl + Z_FF, ff_pins(ffd, q));
        add_bel(format!("{prefix}.{x}5FF"), "SLICE_FF", zl + Z_FF5, ff_pins(o5, q5));

        // Internal mux options -> pseudo-pips.
        let source = |val: &str| -> Option<usize> {
            match val {
                "O6" => Some(o6),
                "O5" => Some(o5),
                "XOR" => xor,
                "CY" => cy,
                "F7" | "F8" => wide,
                v if v == format!("{x}X") => Some(bypass),
                v if v == format!("{x}5Q") => Some(q5),
                // NONE: output unused. MC31: SRL cascade, not modelled yet.
                _ => None,
            }
        };
        for (attr, dst) in [(format!("MUX_FF{x}"), ffd), (format!("MUX_{x}OUT"), outmux)] {
            for val in enum_opts(db, bcls, bel, &attr) {
                // OUTMUX=O6 duplicates the direct <L> pin; using both makes ISE
                // see a net with two driver pins on one slice.
                if attr.starts_with("MUX_") && attr.ends_with("OUT") && val == "O6" {
                    continue;
                }
                if let Some(src) = source(&val) {
                    tb.pip(src, dst, "SITE", false, vec!["SITE".into(), prefix.into(), attr.clone(), val]);
                }
            }
        }
        if has_carry {
            let cy0 = tb.pseudo_wire(format!("{prefix}.{x}CY0"));
            let attr = format!("MUX_{x}CY0");
            for val in enum_opts(db, bcls, bel, &attr) {
                if let Some(src) = source(&val) {
                    tb.pip(src, cy0, "SITE", false, vec!["SITE".into(), prefix.into(), attr.clone(), val]);
                }
            }
            carry_pins.push((format!("DI{li}"), cy0, "IN"));
            carry_pins.push((format!("S{li}"), o6, "IN"));
            carry_pins.push((format!("O{li}"), xor.unwrap(), "OUT"));
            carry_pins.push((format!("CO{li}"), cy.unwrap(), "OUT"));
        }
    }

    if has_wide {
        let (f7a, f7b, f8) = (f7a.unwrap(), f7b.unwrap(), f8.unwrap());
        add_bel(
            format!("{prefix}.F7AMUX"),
            "SLICE_F7MUX",
            zb + Z_F7AMUX,
            vec![("I0", pin("B"), "IN"), ("I1", pin("A"), "IN"), ("S", pin("AX"), "IN"), ("O", f7a, "OUT")],
        );
        add_bel(
            format!("{prefix}.F7BMUX"),
            "SLICE_F7MUX",
            zb + Z_F7BMUX,
            vec![("I0", pin("D"), "IN"), ("I1", pin("C"), "IN"), ("S", pin("CX"), "IN"), ("O", f7b, "OUT")],
        );
        add_bel(
            format!("{prefix}.F8MUX"),
            "SLICE_F8MUX",
            zb + Z_F8MUX,
            vec![("I0", f7b, "IN"), ("I1", f7a, "IN"), ("S", pin("BX"), "IN"), ("O", f8, "OUT")],
        );
    }

    if has_carry {
        assert_eq!(slice_idx, 0, "carry chain on {prefix}; expected SLICE[0] only");
        let cin = tb.pseudo_wire(carry_in_wire());
        let cyinit = tb.pseudo_wire(format!("{prefix}.CYINIT"));
        for val in enum_opts(db, bcls, bel, "PRECYINIT") {
            let src = match val.as_str() {
                "CONST_0" => tb.const_wire("GND"),
                "CONST_1" => tb.const_wire("VCC"),
                "AX" => pin("AX"),
                other => panic!("unexpected PRECYINIT option {other}"),
            };
            tb.pip(src, cyinit, "SITE", false, vec!["SITE".into(), prefix.into(), "PRECYINIT".into(), val]);
        }
        let mut pins: Vec<(&str, usize, &'static str)> = vec![("CI", cin, "IN"), ("CYINIT", cyinit, "IN")];
        pins.extend(carry_pins.iter().map(|(n, w, d)| (n.as_str(), *w, *d)));
        add_bel(format!("{prefix}.CARRY4"), "CARRY4", zb + Z_CARRY4, pins);
    }

    tb.out.bels.extend(bels);
}
