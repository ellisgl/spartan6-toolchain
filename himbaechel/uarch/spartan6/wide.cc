/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: wide-function muxes (MUXF7/MUXF8).
 *
 *  Permission to use, copy, modify, and/or distribute this software for any
 *  purpose with or without fee is hereby granted, provided that the above
 *  copyright notice and this permission notice appear in all copies.
 *
 *  THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 *  WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 *  MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
 *  ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 *  WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
 *  ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
 *  OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

#include "log.h"
#include "nextpnr.h"
#include "util.h"

#define HIMBAECHEL_CONSTIDS "uarch/spartan6/constids.inc"
#include "himbaechel_constids.h"

#include "spartan6.h"

NEXTPNR_NAMESPACE_BEGIN

// Wide muxes exist only in SLICE[0] of CLEXL/CLEXM (SLICEL/SLICEM):
//   F7AMUX: I1 <- A6LUT O6, I0 <- B6LUT O6, S <- AX, O -> AFF/AMUX, F8 I1
//   F7BMUX: I1 <- C6LUT O6, I0 <- D6LUT O6, S <- CX, O -> CFF/CMUX, F8 I0
//   F8MUX:  I1 <- F7AMUX,   I0 <- F7BMUX,   S <- BX, O -> BFF/BMUX
// A MUXF8 takes the whole slice (cluster at fixed z). A lone MUXF7 can use
// either F7 mux, which moves its LUTs, so it gets its own cluster placement.

namespace {
// A LUT for mux input `port`: the LUT driving it if free, else a new
// pass-through (or constant) LUT.
CellInfo *mux_lut(Context *ctx, CellInfo *mux, IdString port, pool<IdString> &claimed)
{
    NetInfo *n = mux->getPort(port);
    if (n && n->driver.cell && n->driver.cell->type == id_SLICE_LUT && n->driver.port == id_O &&
        n->driver.cell->cluster == ClusterId() && !claimed.count(n->driver.cell->name)) {
        int mux_loads = 0;
        for (auto &usr : n->users)
            if (usr.cell->type.in(id_MUXF7, id_SLICE_F7MUX))
                mux_loads++;
        if (mux_loads == 1) {
            claimed.insert(n->driver.cell->name);
            return n->driver.cell;
        }
    }
    CellInfo *lut = ctx->createCell(ctx->idf("%s$%s_lut", mux->name.c_str(ctx), port.c_str(ctx)), id_SLICE_LUT);
    lut->addOutput(id_O);
    if (!n || n->constant_value == id_GND || n->constant_value == id_VCC) {
        lut->params[id_INIT] = Property(n && n->constant_value == id_VCC ? 1 : 0, 1);
    } else {
        lut->addInput(id_A1);
        lut->connectPort(id_A1, n);
        lut->params[id_INIT] = Property(0b10, 2);
    }
    if (n)
        mux->disconnectPort(port);
    NetInfo *o = ctx->createNet(ctx->idf("%s$%s", mux->name.c_str(ctx), port.c_str(ctx)));
    lut->connectPort(id_O, o);
    mux->connectPort(port, o);
    claimed.insert(lut->name);
    return lut;
}

void add_child(CellInfo *root, CellInfo *child, int z)
{
    child->cluster = root->name;
    root->constr_children.push_back(child);
    child->constr_x = 0;
    child->constr_y = 0;
    child->constr_z = z;
    child->constr_abs_z = true;
}
} // namespace

void Spartan6Impl::pack_wide()
{
    pool<IdString> claimed;
    int f8s = 0, f7s = 0, created = 0;
    size_t cells_before = ctx->cells.size();
    const int base = s6z::BASE; // SLICE[0]

    // MUXF8 with its two MUXF7s and four LUTs: one slice.
    std::vector<CellInfo *> f8_list;
    for (auto &cell : ctx->cells)
        if (cell.second->type == id_MUXF8)
            f8_list.push_back(cell.second.get());
    for (CellInfo *f8 : f8_list) {
        CellInfo *f7[2]; // [0] = I0 -> F7BMUX, [1] = I1 -> F7AMUX
        for (int i = 0; i < 2; i++) {
            NetInfo *n = f8->getPort(i ? id_I1 : id_I0);
            if (!n || !n->driver.cell || n->driver.cell->type != id_MUXF7 || n->driver.cell->cluster != ClusterId())
                log_error("MUXF8 %s: input I%d is not driven by a free MUXF7\n", ctx->nameOf(f8), i);
            f7[i] = n->driver.cell;
        }
        f8->type = id_SLICE_F8MUX;
        f8->cluster = f8->name;
        f8->constr_abs_z = true;
        f8->constr_z = base + s6z::F8MUX;
        // F7AMUX (I1 of the F8) uses LUTs A (its I1) and B (I0); F7BMUX uses C and D.
        struct
        {
            CellInfo *mux;
            int z, lut1, lut0;
        } muxes[2] = {{f7[1], s6z::F7AMUX, 0, 1}, {f7[0], s6z::F7BMUX, 2, 3}};
        for (auto &m : muxes) {
            m.mux->type = id_SLICE_F7MUX;
            add_child(f8, m.mux, base + m.z);
            add_child(f8, mux_lut(ctx, m.mux, id_I1, claimed), base + m.lut1 * 8 + s6z::LUT6);
            add_child(f8, mux_lut(ctx, m.mux, id_I0, claimed), base + m.lut0 * 8 + s6z::LUT6);
        }
        f8s++;
    }

    // Lone MUXF7s: F7AMUX or F7BMUX, decided at placement.
    std::vector<CellInfo *> f7_list;
    for (auto &cell : ctx->cells)
        if (cell.second->type == id_MUXF7)
            f7_list.push_back(cell.second.get());
    for (CellInfo *f7 : f7_list) {
        f7->type = id_SLICE_F7MUX;
        f7->cluster = f7->name;
        CellInfo *l1 = mux_lut(ctx, f7, id_I1, claimed);
        CellInfo *l0 = mux_lut(ctx, f7, id_I0, claimed);
        // Offsets as for F7AMUX, for the placer's estimates; the real
        // placement is in getClusterPlacement.
        add_child(f7, l1, base + 0 * 8 + s6z::LUT6);
        add_child(f7, l0, base + 1 * 8 + s6z::LUT6);
        f7_clusters[f7->name] = {l1, l0};
        f7s++;
    }
    created = int(ctx->cells.size() - cells_before);
    if (f8s || f7s)
        log_info("Packed %d MUXF8 slices and %d lone MUXF7s, %d LUTs added for mux inputs.\n", f8s, f7s, created);
}

bool Spartan6Impl::place_f7_cluster(ClusterId cluster, BelId root_bel,
                                    std::vector<std::pair<CellInfo *, BelId>> &placement) const
{
    const auto &fc = f7_clusters.at(cluster);
    if (ctx->getBelType(root_bel) != id_SLICE_F7MUX)
        return false;
    Loc l = ctx->getBelLocation(root_bel);
    int base = s6z::slice_base(l.z);
    bool is_a = l.z == base + s6z::F7AMUX;
    int z1 = base + (is_a ? 0 : 2) * 8 + s6z::LUT6, z0 = base + (is_a ? 1 : 3) * 8 + s6z::LUT6;
    BelId b1 = ctx->getBelByLocation(Loc(l.x, l.y, z1)), b0 = ctx->getBelByLocation(Loc(l.x, l.y, z0));
    if (b1 == BelId() || b0 == BelId())
        return false;
    placement.clear();
    placement.emplace_back(ctx->cells.at(cluster).get(), root_bel);
    placement.emplace_back(fc.lut1, b1);
    placement.emplace_back(fc.lut0, b0);
    return true;
}

NEXTPNR_NAMESPACE_END
