//! Map bitstream bits back to what prjcombine says they control: for every
//! tile, every switchbox item and bel attribute bit is translated to its
//! absolute bitstream position. Used to explain diffs against ISE.

use std::collections::HashMap;

use prjcombine_interconnect::db::{BelAttribute, BelInfo, BelInput, IntDb, SwitchBoxItem};
use prjcombine_spartan6::bitstream::BitPos;
use prjcombine_spartan6::expanded::ExpandedDevice;
use prjcombine_types::bitrect::{BitRect as _, TileBit};

/// What a bit is: tile name, and the item using it.
#[derive(Clone, Debug)]
pub struct BitUse {
    pub tile: String,
    pub what: String,
}

pub struct Explainer {
    pub uses: HashMap<BitPos, Vec<BitUse>>,
}

impl Explainer {
    pub fn new(edev: &ExpandedDevice, int: &IntDb) -> Self {
        let mut uses: HashMap<BitPos, Vec<BitUse>> = HashMap::new();
        for (tcrd, tile) in edev.tiles() {
            let tcls = &int.tile_classes[tile.class];
            let rects = edev.tile_bits(tcrd);
            let tname = format!("{}@{}", tcls.name, tcrd.to_string(int));
            let mut add = |tb: TileBit, what: String| {
                let Some(rect) = rects.get(tb.rect) else { return };
                let pos = rect.xlat_pos_fwd((tb.frame, tb.bit));
                uses.entry(pos).or_default().push(BitUse { tile: tname.clone(), what });
            };
            let wn = |w: prjcombine_interconnect::db::TileWireCoord| w.to_string(int, tcls);
            for (bslot, info) in &tcls.bels {
                let bname = &int.bel_slots[bslot].name;
                match info {
                    BelInfo::SwitchBox(sb) => {
                        for item in &sb.items {
                            match item {
                                SwitchBoxItem::Mux(m) => {
                                    for (i, &b) in m.bits.iter().enumerate() {
                                        add(b, format!("mux {}[{i}]", wn(m.dst)));
                                    }
                                }
                                SwitchBoxItem::ProgBuf(p) => add(p.bit.bit, format!("buf {} <- {}", wn(p.dst), wn(p.src.tw))),
                                SwitchBoxItem::Pass(p) => add(p.bit.bit, format!("pass {} <- {}", wn(p.dst), wn(p.src))),
                                SwitchBoxItem::BiPass(p) => add(p.bit.bit, format!("bipass {} = {}", wn(p.a), wn(p.b))),
                                SwitchBoxItem::ProgInv(p) => add(p.bit.bit, format!("inv {} <- {}", wn(p.dst), wn(p.src))),
                                SwitchBoxItem::ProgDelay(p) => {
                                    for (i, &b) in p.bits.iter().enumerate() {
                                        add(b, format!("delay {}[{i}]", wn(p.dst)));
                                    }
                                }
                                SwitchBoxItem::Bidi(b) => add(b.bit_upstream.bit, format!("bidi {}", wn(b.wire))),
                                SwitchBoxItem::PairMux(p) => {
                                    for (i, &b) in p.bits.iter().enumerate() {
                                        add(b, format!("pairmux {}/{}[{i}]", wn(p.dst[0]), wn(p.dst[1])));
                                    }
                                }
                                SwitchBoxItem::WireSupport(ws) => {
                                    for (i, b) in ws.bits.iter().enumerate() {
                                        add(b.bit, format!("wiresupport {:?}[{i}]", ws.wires.iter().map(|&w| wn(w)).collect::<Vec<_>>()));
                                    }
                                }
                                SwitchBoxItem::PermaBuf(_) => {}
                            }
                        }
                    }
                    BelInfo::Bel(bel) => {
                        let bcls = &int.bel_classes[int.bel_class(bslot)];
                        for (aid, attr) in &bel.attributes {
                            let aname = &bcls.attributes[aid].name;
                            match attr {
                                BelAttribute::BitVec(bits) => {
                                    for (i, b) in bits.iter().enumerate() {
                                        add(b.bit, format!("{bname}.{aname}[{i}]"));
                                    }
                                }
                                BelAttribute::Enum(e) => {
                                    for (i, &b) in e.bits.iter().enumerate() {
                                        add(b, format!("{bname}.{aname}<{i}>"));
                                    }
                                }
                            }
                        }
                        for (pid, inp) in &bel.inputs {
                            if let BelInput::Invertible(_, b) = inp {
                                let (pn, _) = bcls.inputs.key(pid);
                                add(b.bit, format!("{bname}.{pn} inversion"));
                            }
                        }
                    }
                    BelInfo::TestMux(tm) => {
                        for (i, &b) in tm.bits.iter().enumerate() {
                            add(b, format!("testmux {bname}[{i}]"));
                        }
                    }
                }
            }
        }
        Explainer { uses }
    }

    pub fn describe(&self, pos: BitPos) -> String {
        match self.uses.get(&pos) {
            Some(u) => u.iter().map(|u| format!("{} {}", u.tile, u.what)).collect::<Vec<_>>().join(" | "),
            None => "(unknown)".into(),
        }
    }
}
