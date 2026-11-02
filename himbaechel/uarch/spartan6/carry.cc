/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: carry chains (CARRY4).
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

// CARRY4 lives in SLICE[0] of CLEXL/CLEXM tiles. Its S inputs are hardwired
// to the four LUT6 outputs (O6) of that slice, its carry input CI comes from
// CYINIT {CIN, PRECYINIT {0, 1, AX}}, and CO3 drives CIN of the next slice up
// the column. The chain mostly steps one row, but skips rows in places, so
// placement follows the CO3 -> CIN wiring (carry_next) instead of fixed
// offsets.

// For each CARRY4 bel, the CARRY4 bel its CO3 feeds (through CIN).
void Spartan6Impl::init_carry_next()
{
    for (BelId bel : ctx->getBels()) {
        if (ctx->getBelType(bel) != id_CARRY4)
            continue;
        WireId co3 = ctx->getBelPinWire(bel, id_CO3);
        for (PipId pip : ctx->getPipsDownhill(co3)) {
            WireId dst = ctx->getPipDstWire(pip);
            for (auto bp : ctx->getWireBelPins(dst)) {
                if (bp.pin == id_CI && bp.bel != bel && ctx->getBelType(bp.bel) == id_CARRY4)
                    carry_next[bel] = bp.bel;
            }
        }
    }
}

namespace {
IdString bus_port(Context *ctx, const char *base, int i) { return ctx->idf("%s[%d]", base, i); }

// A LUT for S input `i` of `carry`: the LUT driving it if that LUT can go in
// this letter's LUT6 bel, else a new constant or pass-through LUT.
CellInfo *s_lut(Context *ctx, CellInfo *carry, int i, pool<IdString> &claimed)
{
    IdString s_port = ctx->idf("S%d", i);
    NetInfo *s = carry->getPort(s_port);
    if (s && s->driver.cell && s->driver.cell->type == id_SLICE_LUT && s->driver.port == id_O &&
        s->driver.cell->cluster == ClusterId() && !claimed.count(s->driver.cell->name)) {
        // The LUT may have other loads (O6 also leaves on <L>), but only one S.
        int s_loads = 0;
        for (auto &usr : s->users)
            if (usr.cell->type == id_CARRY4)
                s_loads++;
        if (s_loads == 1) {
            claimed.insert(s->driver.cell->name);
            return s->driver.cell;
        }
    }
    CellInfo *lut = ctx->createCell(ctx->idf("%s$S%d_lut", carry->name.c_str(ctx), i), id_SLICE_LUT);
    lut->addOutput(id_O);
    if (!s || s->constant_value == id_GND || s->constant_value == id_VCC) {
        // Constant S (or unconnected: treat as 0).
        bool one = s && s->constant_value == id_VCC;
        lut->params[id_INIT] = Property(one ? 1 : 0, 1);
    } else {
        // Pass-through: O = A1.
        lut->addInput(id_A1);
        lut->connectPort(id_A1, s);
        lut->params[id_INIT] = Property(0b10, 2);
    }
    if (s)
        carry->disconnectPort(s_port);
    NetInfo *o = ctx->createNet(ctx->idf("%s$S%d", carry->name.c_str(ctx), i));
    lut->connectPort(id_O, o);
    carry->connectPort(s_port, o);
    claimed.insert(lut->name);
    return lut;
}
// DI input `i` through the letter's LUT5 (O5 -> MUX_<L>CY0), as ISE does,
// instead of the <L>X bypass pin, which a FF in the same letter may need.
// Possible when DI is a constant or the S LUT's inputs plus DI fit in 5: the
// S LUT becomes the LUT6 half of a pair (A6 tied high) and a new LUT5 passes
// DI through on the shared inputs.
CellInfo *di_lut5(Context *ctx, CellInfo *carry, int i, CellInfo *s_lut, NetInfo *vcc)
{
    IdString di_port = ctx->idf("DI%d", i);
    NetInfo *di = carry->getPort(di_port);
    if (!di)
        return nullptr;
    std::vector<NetInfo *> merged;
    if (!s6lut::inputs(s_lut, merged))
        return nullptr;
    bool is_const = di->constant_value == id_GND || di->constant_value == id_VCC;
    if (!is_const && std::find(merged.begin(), merged.end(), di) == merged.end())
        merged.push_back(di);
    if (merged.size() > 5)
        return nullptr;

    CellInfo *l5 = ctx->createCell(ctx->idf("%s$DI%d_lut", carry->name.c_str(ctx), i), id_SLICE_LUT5);
    l5->addOutput(id_O);
    if (is_const) {
        l5->params[id_INIT] = Property(di->constant_value == id_VCC ? 1 : 0, 1);
    } else {
        l5->addInput(id_A1);
        l5->connectPort(id_A1, di);
        l5->params[id_INIT] = Property(0b10, 2);
    }
    carry->disconnectPort(di_port);
    NetInfo *o = ctx->createNet(ctx->idf("%s$DI%d", carry->name.c_str(ctx), i));
    l5->connectPort(id_O, o);
    carry->connectPort(di_port, o);

    s6lut::remap(ctx, s_lut, merged);
    s6lut::remap(ctx, l5, merged);
    s_lut->ports[id_A6].name = id_A6;
    s_lut->ports[id_A6].type = PORT_IN;
    s_lut->connectPort(id_A6, vcc);
    return l5;
}
// O_i and CO_i both leave the slice only through <L>MUX, so a letter can't
// export both. Where it would, rebuild CO_i from the next stage instead:
// O_{i+1} = S_{i+1} ^ CO_i, so CO_i = O_{i+1} ^ S_{i+1}, from outputs that
// can leave (O_{i+1} on the next <L>MUX, S_{i+1} on the <L> pin). Repeats up
// the chain if the next letter then has the same problem.
int rebuild_carry_outs(Context *ctx, const std::vector<CellInfo *> &chain)
{
    int count = 0;
    auto used = [](NetInfo *n, bool skip_ci) {
        if (!n)
            return false;
        for (auto &usr : n->users)
            if (!(skip_ci && usr.cell->type == id_CARRY4 && usr.port == id_CI))
                return true;
        return false;
    };
    for (size_t k = 0; k < chain.size(); k++) {
        for (int i = 0; i < 4; i++) {
            CellInfo *c = chain[k];
            NetInfo *o = c->getPort(ctx->idf("O%d", i));
            NetInfo *co = c->getPort(ctx->idf("CO%d", i));
            if (!used(o, false) || !used(co, i == 3))
                continue;
            // The next stage: letter i+1 here, or letter A of the next CARRY4.
            CellInfo *nc = i < 3 ? c : (k + 1 < chain.size() ? chain[k + 1] : nullptr);
            int ni = i < 3 ? i + 1 : 0;
            if (!nc)
                log_error("CARRY4 %s: both O3 and CO3 are used at the end of the chain\n", ctx->nameOf(c));
            IdString o_next = ctx->idf("O%d", ni);
            NetInfo *on = nc->getPort(o_next);
            if (!on) {
                on = ctx->createNet(ctx->idf("%s$O%d", nc->name.c_str(ctx), ni));
                nc->connectPort(o_next, on);
            }
            NetInfo *sn = nc->getPort(ctx->idf("S%d", ni));
            CellInfo *x = ctx->createCell(ctx->idf("%s$CO%d_xor", c->name.c_str(ctx), i), id_SLICE_LUT);
            x->addInput(id_A1);
            x->addInput(id_A2);
            x->addOutput(id_O);
            x->params[id_INIT] = Property(0b0110, 4);
            x->connectPort(id_A1, on);
            x->connectPort(id_A2, sn);
            // Move CO_i's loads (except the chain's own CI) to the rebuilt net.
            NetInfo *rebuilt = ctx->createNet(ctx->idf("%s$CO%d_rebuilt", c->name.c_str(ctx), i));
            x->connectPort(id_O, rebuilt);
            std::vector<PortRef> moved;
            for (auto &usr : co->users)
                if (!(i == 3 && usr.cell->type == id_CARRY4 && usr.port == id_CI))
                    moved.push_back(usr);
            for (auto &usr : moved) {
                usr.cell->disconnectPort(usr.port);
                usr.cell->connectPort(usr.port, rebuilt);
            }
            count++;
        }
    }
    return count;
}
} // namespace

void Spartan6Impl::pack_carries()
{
    // Per-bit port names (yosys buses DI[3:0] etc. arrive as "DI[0]"...).
    std::vector<CellInfo *> carries;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_CARRY4)
            continue;
        for (int i = 0; i < 4; i++) {
            ci->renamePort(bus_port(ctx, "DI", i), ctx->idf("DI%d", i));
            ci->renamePort(bus_port(ctx, "S", i), ctx->idf("S%d", i));
            ci->renamePort(bus_port(ctx, "O", i), ctx->idf("O%d", i));
            ci->renamePort(bus_port(ctx, "CO", i), ctx->idf("CO%d", i));
        }
        carries.push_back(ci);
    }
    if (carries.empty())
        return;

    // Chain links: CO3 -> CI of the next CARRY4.
    dict<IdString, CellInfo *> next, prev;
    for (CellInfo *ci : carries) {
        NetInfo *co3 = ci->getPort(id_CO3);
        if (!co3)
            continue;
        for (auto &usr : co3->users) {
            if (usr.cell->type == id_CARRY4 && usr.port == id_CI) {
                if (next.count(ci->name))
                    log_error("CARRY4 %s drives more than one carry chain\n", ctx->nameOf(ci));
                next[ci->name] = usr.cell;
                prev[usr.cell->name] = ci;
            }
        }
    }

    pool<IdString> claimed;
    NetInfo *vcc = ctx->nets.at(ctx->id("$PACKER_VCC")).get();
    int chains = 0, created_luts = 0, di_o5 = 0, split_carry_outs = 0;
    size_t longest = 0;
    for (CellInfo *root : carries) {
        if (prev.count(root->name))
            continue;
        std::vector<CellInfo *> chain;
        for (CellInfo *c = root; c; c = next.count(c->name) ? next.at(c->name) : nullptr)
            chain.push_back(c);
        longest = std::max(longest, chain.size());

        CarryChain &cc = carry_chains[root->name];
        root->cluster = root->name;
        for (size_t k = 0; k < chain.size(); k++) {
            CellInfo *c = chain[k];
            // One carry input: the previous CO3, else the chain's initial value.
            NetInfo *ci = c->getPort(id_CI), *cyinit = c->getPort(id_CYINIT);
            if (k > 0) {
                if (cyinit && cyinit->constant_value != id_GND)
                    log_error("CARRY4 %s: chained, but CYINIT is not 0\n", ctx->nameOf(c));
                c->disconnectPort(id_CYINIT);
            } else if (cyinit && cyinit->constant_value != id_GND) {
                if (ci && ci->constant_value != id_GND)
                    log_error("CARRY4 %s: both CI and CYINIT are used\n", ctx->nameOf(c));
                c->disconnectPort(id_CI);
                c->disconnectPort(id_CYINIT);
                c->connectPort(id_CI, cyinit);
            } else {
                c->disconnectPort(id_CYINIT);
            }
            if (c->ports.count(id_CYINIT))
                c->ports.erase(id_CYINIT);

            if (k > 0) {
                c->cluster = root->name;
                root->constr_children.push_back(c);
                c->constr_x = 0;
                c->constr_y = k;
                c->constr_z = s6z::BASE + s6z::CARRY4;
                c->constr_abs_z = true;
            }
            cc.members.push_back({c, int(k), s6z::BASE + s6z::CARRY4});
            for (int i = 0; i < 4; i++) {
                size_t before = ctx->cells.size();
                CellInfo *lut = s_lut(ctx, c, i, claimed);
                if (ctx->cells.size() != before)
                    created_luts++;
                lut->cluster = root->name;
                root->constr_children.push_back(lut);
                lut->constr_x = 0;
                lut->constr_y = k;
                lut->constr_z = s6z::BASE + i * 8 + s6z::LUT6;
                lut->constr_abs_z = true;
                cc.members.push_back({lut, int(k), lut->constr_z});
                if (CellInfo *l5 = di_lut5(ctx, c, i, lut, vcc)) {
                    l5->cluster = root->name;
                    root->constr_children.push_back(l5);
                    l5->constr_x = 0;
                    l5->constr_y = k;
                    l5->constr_z = s6z::BASE + i * 8 + s6z::LUT5;
                    l5->constr_abs_z = true;
                    cc.members.push_back({l5, int(k), l5->constr_z});
                    di_o5++;
                }
            }
        }
        root->constr_abs_z = true;
        root->constr_z = s6z::BASE + s6z::CARRY4;
        split_carry_outs += rebuild_carry_outs(ctx, chain);
        chains++;
    }
    log_info("Packed %d carry chains (longest %d CARRY4s), %d LUTs added for S inputs, %d DI inputs through O5.\n",
             chains, int(longest), created_luts, di_o5);
    if (split_carry_outs)
        log_info("Rebuilt %d carry outputs (CO used next to O) as O(i+1) ^ S(i+1).\n", split_carry_outs);
}

// Place a carry chain by following CO3 -> CIN from the root's bel.
bool Spartan6Impl::getClusterPlacement(ClusterId cluster, BelId root_bel,
                                       std::vector<std::pair<CellInfo *, BelId>> &placement) const
{
    if (f7_clusters.count(cluster))
        return place_f7_cluster(cluster, root_bel, placement);
    auto it = carry_chains.find(cluster);
    if (it == carry_chains.end())
        return HimbaechelAPI::getClusterPlacement(cluster, root_bel, placement);
    if (ctx->getBelType(root_bel) != id_CARRY4)
        return false;
    // Tiles of the chain, root first.
    std::vector<Loc> tiles;
    int len = 0;
    for (auto &m : it->second.members)
        len = std::max(len, m.step + 1);
    BelId b = root_bel;
    for (int k = 0; k < len; k++) {
        if (b == BelId())
            return false;
        tiles.push_back(ctx->getBelLocation(b));
        auto nx = carry_next.find(b);
        b = nx == carry_next.end() ? BelId() : nx->second;
    }
    placement.clear();
    for (auto &m : it->second.members) {
        Loc l = tiles.at(m.step);
        BelId bel = ctx->getBelByLocation(Loc(l.x, l.y, m.z));
        if (bel == BelId() || !isValidBelForCellType(m.cell->type, bel))
            return false;
        placement.emplace_back(m.cell, bel);
    }
    return true;
}

NEXTPNR_NAMESPACE_END
