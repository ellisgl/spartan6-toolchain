//! Naming conventions shared by the chipdb exporter (s6-export) and the XDL
//! writer (s6-xdl). The XDL writer has to map nextpnr wire names back to
//! prjcombine wires, so both must use exactly these rules.

pub mod chipdb;
pub mod nextpnr;

use prjcombine_entity::bundle_map::EntityBundleItemIndex;
use prjcombine_interconnect::db::{IntDb, TileClass, TileWireCoord};
use prjcombine_interconnect::grid::{CellCoord, ExpandedGrid, WireCoord};

/// Local (per tile type) name of a tile-class wire. Single-cell classes always
/// refer to the anchor cell, so they share plain names; multi-cell classes get
/// the class and cell prefix, because CELL[n] means a different cell in each
/// class.
pub fn wire_name(db: &IntDb, tcls: &TileClass, tcw: TileWireCoord) -> String {
    let w = &db.wires[tcw.wire].name;
    if tcls.cells.len() == 1 {
        w.clone()
    } else {
        format!("{}.{}.{}", tcls.name, tcls.cells[tcw.cell], w)
    }
}

/// Inverse of [`wire_name`]: the raw prjcombine wire behind a local wire name
/// in the tile at `cell`. Returns None for names the exporter synthesized
/// (slice internals, constants, pseudo wires).
pub fn parse_wire_name(egrid: &ExpandedGrid, cell: CellCoord, name: &str) -> Option<WireCoord> {
    let db = egrid.db;
    // "CLASS.CELL[n].WIRE" for multi-cell classes.
    let mut parts = name.splitn(3, '.');
    if let (Some(cls), Some(cslot), Some(wire)) = (parts.next(), parts.next(), parts.next())
        && let Some((tcid, tcls)) = db.tile_classes.iter().find(|(_, t)| t.name == cls)
    {
        let cid = tcls.cells.iter().find(|(_, n)| n.as_str() == cslot).map(|(id, _)| id)?;
        let wslot = db.wires.iter().find(|(_, w)| w.name == wire).map(|(id, _)| id)?;
        let (_, tile) = egrid[cell].tiles.iter().find(|(_, t)| t.class == tcid)?;
        return Some(tile.cells[cid].wire(wslot));
    }
    let wslot = db.wires.iter().find(|(_, w)| w.name == name).map(|(id, _)| id)?;
    Some(cell.wire(wslot))
}

/// Bel pin name: bus bits are written without brackets ("ADDRA5"), as ISE
/// names them. nextpnr's JSON writer regroups "X[i]" ports into buses and
/// loses where a partly used bus starts, so brackets can't survive a
/// place-and-route round trip.
pub fn pin_name(base: &str, idx: EntityBundleItemIndex, virt: impl Fn(usize) -> usize) -> String {
    match idx {
        EntityBundleItemIndex::Single => base.to_string(),
        EntityBundleItemIndex::Array { index, .. } => format!("{base}{}", virt(index)),
    }
}
