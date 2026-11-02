/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: packing of Yosys `synth_xilinx -family xc6s`
 *  primitives onto the exporter's sub-bels.
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

namespace {
// Rename a port if present; no-op otherwise.
void rename_if(CellInfo *ci, IdString from, IdString to)
{
    if (ci->ports.count(from))
        ci->renamePort(from, to);
}

void drop_port(CellInfo *ci, IdString port)
{
    if (ci->ports.count(port)) {
        ci->disconnectPort(port);
        ci->ports.erase(port);
    }
}
} // namespace

// I/O buffers -> IOB bels.
//
// The IOB bel's pins are named from the fabric's point of view (exporter/src/io.rs):
// I = pad to fabric, O = fabric to pad, T = tristate, GCLK_I = dedicated clock path.
void Spartan6Impl::pack_io()
{
    // Port attributes (e.g. (* LOC = "P56" *) in Verilog) end up on nextpnr's
    // top-level buffer cells; collect them before those cells are removed.
    dict<IdString, dict<IdString, Property>> port_attrs;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type.in(ctx->id("$nextpnr_ibuf"), ctx->id("$nextpnr_obuf"), ctx->id("$nextpnr_iobuf")))
            port_attrs[ci->name] = ci->attrs;
    }
    for (auto &[port, attrs] : ucf_attrs)
        for (auto &[k, v] : attrs)
            port_attrs[port][k] = v;

    const pool<CellTypePort> top_ports{
            CellTypePort(id_IBUF, id_I),  CellTypePort(id_IBUFG, id_I), CellTypePort(id_OBUF, id_O),
            CellTypePort(id_OBUFT, id_O), CellTypePort(id_IOBUF, id_IO),
    };
    h.remove_nextpnr_iobs(top_ports);

    int count = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (!ci->type.in(id_IBUF, id_IBUFG, id_OBUF, id_OBUFT, id_IOBUF))
            continue;
        // The pad-side net is named after the top-level port.
        IdString pad_port = ci->type.in(id_IBUF, id_IBUFG) ? id_I : (ci->type == id_IOBUF ? id_IO : id_O);
        NetInfo *pad_net = ci->getPort(pad_port);
        if (!pad_net)
            log_error("I/O buffer %s is not connected to a top-level port\n", ctx->nameOf(ci));
        IdString port_name = pad_net->name;
        if (port_attrs.count(port_name))
            for (auto &[k, v] : port_attrs.at(port_name))
                ci->attrs[k] = v;
        IdString orig_type = ci->type;
        ci->params[ctx->id("IO_TYPE")] = orig_type.str(ctx);

        if (orig_type.in(id_IBUF, id_IBUFG)) {
            drop_port(ci, id_I);
            rename_if(ci, id_O, id_I);
        } else if (orig_type.in(id_OBUF, id_OBUFT)) {
            drop_port(ci, id_O);
            rename_if(ci, id_I, id_O);
        } else { // IOBUF: I = fabric to pad, O = pad to fabric
            drop_port(ci, id_IO);
            rename_if(ci, id_I, ctx->id("$tmp_I"));
            rename_if(ci, id_O, id_I);
            rename_if(ci, ctx->id("$tmp_I"), id_O);
        }
        ci->type = id_IOB;

        if (ci->attrs.count(id_LOC)) {
            std::string pin = ci->attrs.at(id_LOC).as_string();
            BelId bel;
            for (const auto &pad : ctx->package_info->pads) {
                if (IdString(pad.package_pin) == ctx->id(pin)) {
                    bel = ctx->getBelByName(IdStringList::concat(IdString(pad.tile), IdString(pad.bel)));
                    break;
                }
            }
            if (bel == BelId())
                log_error("Port '%s': LOC '%s' is not a user I/O pin of this package\n", port_name.c_str(ctx),
                          pin.c_str());
            ci->attrs[id_BEL] = ctx->getBelName(bel).str(ctx);
            log_info("    %s -> %s (%s)\n", port_name.c_str(ctx), pin.c_str(), ctx->nameOfBel(bel));
        } else {
            log_warning("Port '%s' has no LOC constraint; the placer will pick a pin\n", port_name.c_str(ctx));
        }
        count++;
    }
    log_info("Packed %d I/O buffers.\n", count);
}

// BUFG -> BUFGMUX (I0 selected; S left unconnected so the bitstream writer
// ties it). An input buffer that only feeds BUFGs uses the dedicated
// clock-pad path (GCLK_I) instead of the ILOGIC data path.
void Spartan6Impl::pack_clocking()
{
    int count = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_BUFG)
            continue;
        ci->type = id_BUFGMUX;
        ci->renamePort(id_I, id_I0);
        count++;
    }
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_IOB)
            continue;
        NetInfo *o = ci->getPort(id_I);
        if (!o || o->users.empty())
            continue;
        // Only clock loads (BUFGs, DCM/PLL clock inputs): use the dedicated
        // clock-pad path.
        bool only_clock = true;
        for (auto &usr : o->users)
            if (!(usr.cell->type == id_BUFGMUX || (usr.cell->type == id_DCM && usr.port == id_CLKIN) ||
                  (usr.cell->type == id_PLL && usr.port == id_CLKIN1) ||
                  (usr.cell->type == id_BUFIO2 && usr.port == id_I)))
                only_clock = false;
        if (!only_clock)
            continue;
        ci->renamePort(id_I, id_GCLK_I);
        if (ci->attrs.count(id_BEL)) {
            BelId pad_bel = ctx->getBelByNameStr(ci->attrs.at(id_BEL).as_string());
            place_clock_tree(ctx->getBelPinWire(pad_bel, id_GCLK_I), ci->getPort(id_GCLK_I), ctx->nameOfBel(pad_bel));
        }
    }
    log_info("Packed %d global buffers.\n", count);
}

// A LUT1 inverter (or INV) on a FF, RAM or SRL clock becomes the cell's
// clock inversion (the slice's CLKINV), as ISE's map does: a global clock
// can't be routed into a LUT input. yosys inverts RAM64M clocks this way.
void Spartan6Impl::absorb_clock_inverters()
{
    struct ClkPin
    {
        IdString port, param;
    };
    auto clk_pin = [&](const CellInfo *ci) -> ClkPin {
        if (ci->type.in(id_FDRE, id_FDSE, id_FDCE, id_FDPE) ||
            ci->type.in(ctx->id("FDRE_1"), ctx->id("FDSE_1"), ctx->id("FDCE_1"), ctx->id("FDPE_1")))
            return {id_C, id_IS_C_INVERTED};
        if (ci->type.in(id_RAM64X1S, id_RAM32M, id_RAM64M))
            return {id_WCLK, id_IS_WCLK_INVERTED};
        if (ci->type.in(id_SRLC32E, id_SRL16E))
            return {id_CLK, id_IS_CLK_INVERTED};
        return {};
    };
    auto is_inverter = [&](const CellInfo *drv) {
        if (drv->type == id_INV)
            return true;
        if (drv->type != id_LUT1 || !drv->params.count(id_INIT))
            return false;
        return (drv->params.at(id_INIT).as_int64() & 3) == 1;
    };
    int n = 0;
    pool<IdString> unused;
    std::vector<CellInfo *> cells;
    for (auto &cell : ctx->cells)
        cells.push_back(cell.second.get());
    for (CellInfo *ci : cells) {
        ClkPin cp = clk_pin(ci);
        if (cp.port == IdString())
            continue;
        NetInfo *clk = ci->getPort(cp.port);
        if (!clk || !clk->driver.cell || !is_inverter(clk->driver.cell))
            continue;
        CellInfo *inv = clk->driver.cell;
        NetInfo *src = inv->getPort(inv->type == id_INV ? id_I : id_I0);
        if (!src)
            continue;
        ci->disconnectPort(cp.port);
        ci->connectPort(cp.port, src);
        bool was = ci->params.count(cp.param) && ci->params.at(cp.param).as_int64() != 0;
        ci->params[cp.param] = Property(was ? 0 : 1, 1);
        n++;
        if (clk->users.entries() == 0)
            unused.insert(inv->name);
    }
    for (IdString name : unused) {
        CellInfo *inv = ctx->cells.at(name).get();
        for (IdString p : {id_I, id_I0, id_O})
            if (inv->ports.count(p))
                inv->disconnectPort(p);
        ctx->cells.erase(name);
    }
    if (n)
        log_info("Absorbed %d clock inverters into slice clock inversion.\n", n);
}

void Spartan6Impl::pack_logic()
{
    int luts = 0, ffs = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type == id_INV) {
            // INV is a LUT1 with INIT = 2'b01.
            ci->renamePort(id_I, id_A1);
            ci->params[id_INIT] = Property(0b01, 2);
            ci->type = id_SLICE_LUT;
            luts++;
        } else if (ci->type.in(id_LUT1, id_LUT2, id_LUT3, id_LUT4, id_LUT5, id_LUT6)) {
            // LUTk I0..I(k-1) -> A1..Ak. Unused inputs stay unconnected; the
            // bitstream writer replicates INIT across them.
            static const IdString in_ports[6] = {id_I0, id_I1, id_I2, id_I3, id_I4, id_I5};
            static const IdString a_ports[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
            for (int i = 0; i < 6; i++)
                rename_if(ci, in_ports[i], a_ports[i]);
            ci->type = id_SLICE_LUT;
            luts++;
        } else if (ci->type.in(id_LDCE, id_LDPE)) {
            // Latch: a slice FF in latch mode (FF_LATCH, slice-wide), gate on
            // the clock pin, gate enable on CE, CLR/PRE asynchronous on SR.
            // The slice latch is transparent while its (inverted-or-not)
            // clock is low, so an active-high gate inverts the clock: ISE
            // writes CLKINV = CLK_B for LDCE.
            bool g_inv = ci->params.count(id_IS_G_INVERTED) && ci->params.at(id_IS_G_INVERTED).as_int64() != 0;
            ci->params.erase(id_IS_G_INVERTED);
            if (!g_inv)
                ci->params[id_CLK_INV] = Property(1, 1);
            ci->params[id_FF_TYPE] = ci->type.str(ctx);
            rename_if(ci, id_G, id_CK);
            rename_if(ci, id_GE, id_CE);
            for (auto sr : {id_CLR, id_PRE})
                rename_if(ci, sr, id_SR);
            NetInfo *ce = ci->getPort(id_CE), *sr = ci->getPort(id_SR);
            if (ce && ce->constant_value == id_VCC)
                ci->disconnectPort(id_CE);
            if (sr && sr->constant_value == id_GND)
                ci->disconnectPort(id_SR);
            ci->type = id_SLICE_FF;
            ffs++;
        } else if (ci->type.in(id_FDRE, id_FDSE, id_FDCE, id_FDPE) ||
                   ci->type.in(ctx->id("FDRE_1"), ctx->id("FDSE_1"), ctx->id("FDCE_1"), ctx->id("FDPE_1"))) {
            // FDxE_1 and IS_C_INVERTED: falling edge. The slice inverts its
            // clock for all its FFs and RAM/SRL LUTs (CLKINV), so it's part
            // of the control set.
            std::string type = ci->type.str(ctx);
            bool inv = false;
            if (type.size() > 2 && type.substr(type.size() - 2) == "_1") {
                type = type.substr(0, type.size() - 2);
                inv = true;
            }
            if (ci->params.count(id_IS_C_INVERTED) && ci->params.at(id_IS_C_INVERTED).as_int64() != 0)
                inv = !inv;
            ci->params.erase(id_IS_C_INVERTED);
            if (inv)
                ci->params[id_CLK_INV] = Property(1, 1);
            ci->params[id_FF_TYPE] = type;
            rename_if(ci, id_C, id_CK);
            for (auto sr : {id_R, id_S, id_CLR, id_PRE})
                rename_if(ci, sr, id_SR);
            // CE tied high and SR tied low are the slice defaults (CEUSED/SRUSED
            // off): don't route constants to them. ISE treats the GND -> SR arcs
            // as excluded, so routing them is also not legal for its tools.
            NetInfo *ce = ci->getPort(id_CE), *sr = ci->getPort(id_SR);
            if (ce && ce->constant_value == id_VCC)
                ci->disconnectPort(id_CE);
            if (sr && sr->constant_value == id_GND)
                ci->disconnectPort(id_SR);
            ci->type = id_SLICE_FF;
            ffs++;
        }
    }
    log_info("Packed %d LUTs and %d FFs.\n", luts, ffs);
}

// LUT6 fracturing: two LUTs with at most 5 distinct inputs between them share
// one LUT6 site. Both get the merged inputs on A1..A5 (tables rewritten to
// match); the LUT6 half (O6) has A6 tied high and the LUT5 half (O5) reads
// the lower half of the table. The pair is a cluster: root on the LUT6 bel,
// child (type SLICE_LUT5) on the LUT5 bel of the same letter.
namespace s6lut {
// Distinct input nets of a LUT in pin order, or false if it can't be paired
// (6 inputs, or a table input left unconnected).
bool inputs(CellInfo *ci, std::vector<NetInfo *> &nets)
{
    static const IdString a_ports[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
    const Property &init = ci->params.at(id_INIT);
    int k = 0;
    while ((1u << k) < init.str.size())
        k++;
    nets.clear();
    for (int i = 0; i < 6; i++) {
        NetInfo *n = ci->getPort(a_ports[i]);
        if (i >= k) {
            if (n)
                return false;
            continue;
        }
        if (!n)
            return false;
        if (std::find(nets.begin(), nets.end(), n) == nets.end())
            nets.push_back(n);
    }
    return nets.size() <= 5;
}

// Rewrite a LUT as a 5-input table over `merged` (on A1..A5).
void remap(Context *ctx, CellInfo *ci, const std::vector<NetInfo *> &merged)
{
    static const IdString a_ports[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
    const Property init = ci->params.at(id_INIT);
    int size = init.str.size();
    std::vector<int> pos; // old input i -> new input index
    for (int i = 0; (1 << i) < size; i++) {
        NetInfo *n = ci->getPort(a_ports[i]);
        pos.push_back(std::find(merged.begin(), merged.end(), n) - merged.begin());
    }
    Property table(0, 32);
    for (int addr = 0; addr < 32; addr++) {
        int old = 0;
        for (size_t i = 0; i < pos.size(); i++)
            if (addr & (1 << pos[i]))
                old |= 1 << i;
        table.str[addr] = init.str.at(old % size) == '1' ? Property::S1 : Property::S0;
    }
    table.update_intval();
    ci->params[id_INIT] = table;
    for (auto p : a_ports)
        ci->disconnectPort(p);
    for (size_t i = 0; i < merged.size(); i++) {
        ci->ports[a_ports[i]].name = a_ports[i];
        ci->ports[a_ports[i]].type = PORT_IN;
        ci->connectPort(a_ports[i], merged[i]);
    }
}
} // namespace s6lut

namespace {
// The FF a LUT drives, if that FF's D is the LUT output's only load.
CellInfo *sole_ff_load(CellInfo *lut)
{
    NetInfo *o = lut->getPort(id_O);
    if (!o || o->users.entries() != 1)
        return nullptr;
    auto &usr = *o->users.begin();
    if (usr.cell->type != id_SLICE_FF || usr.port != id_D || usr.cell->cluster != ClusterId())
        return nullptr;
    return usr.cell;
}

} // namespace

void Spartan6Impl::pack_lut_pairs()
{
    if (ctx->args.options.count("nolut5")) {
        log_info("LUT pairing disabled (-o nolut5).\n");
        return;
    }
    NetInfo *vcc = ctx->nets.at(ctx->id("$PACKER_VCC")).get();
    pool<IdString> done;
    std::vector<NetInfo *> a_in, b_in, merged;
    int pairs = 0, ff5s = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *a = cell.second.get();
        if (a->type != id_SLICE_LUT || a->cluster != ClusterId() || done.count(a->name) || !s6lut::inputs(a, a_in))
            continue;
        // Best partner: most shared inputs, then fewest merged inputs.
        CellInfo *best = nullptr;
        int best_shared = 0, best_union = 0;
        for (NetInfo *n : a_in) {
            if (n->users.entries() > 32 || n->constant_value != IdString())
                continue;
            for (auto &usr : n->users) {
                CellInfo *b = usr.cell;
                if (b == a || b->type != id_SLICE_LUT || b->cluster != ClusterId() || done.count(b->name) ||
                    !s6lut::inputs(b, b_in))
                    continue;
                int shared = 0;
                for (NetInfo *m : b_in)
                    if (std::find(a_in.begin(), a_in.end(), m) != a_in.end())
                        shared++;
                int uni = a_in.size() + b_in.size() - shared;
                if (uni > 5)
                    continue;
                if (shared > best_shared || (shared == best_shared && uni < best_union)) {
                    best = b;
                    best_shared = shared;
                    best_union = uni;
                }
            }
        }
        if (!best)
            continue;
        // The LUT5 half's O5 can feed that letter's 5FF; prefer the LUT whose
        // only load is a FF there.
        if (sole_ff_load(a) && !sole_ff_load(best))
            std::swap(a, best);
        s6lut::inputs(a, a_in);
        s6lut::inputs(best, b_in);
        merged = a_in;
        for (NetInfo *m : b_in)
            if (std::find(merged.begin(), merged.end(), m) == merged.end())
                merged.push_back(m);
        s6lut::remap(ctx, a, merged);
        s6lut::remap(ctx, best, merged);
        a->ports[id_A6].name = id_A6;
        a->ports[id_A6].type = PORT_IN;
        a->connectPort(id_A6, vcc);
        best->type = id_SLICE_LUT5;
        a->cluster = a->name;
        a->constr_children.push_back(best);
        best->cluster = a->name;
        best->constr_x = 0;
        best->constr_y = 0;
        best->constr_z = s6z::LUT5 - s6z::LUT6;
        best->constr_abs_z = false;
        done.insert(a->name);
        done.insert(best->name);
        pairs++;
        // 5FF: D is hardwired to O5 and Q leaves on <L>MUX, so it can only
        // take a FF that is the LUT5 half's sole load.
        // Latches can't be 5FFs (those elements are flip-flops only).
        CellInfo *ff = sole_ff_load(best);
        if (ff && ff->params.at(id_FF_TYPE).as_string().rfind("LD", 0) == 0)
            ff = nullptr;
        if (ff) {
            ff->type = id_SLICE_FF5;
            a->constr_children.push_back(ff);
            ff->cluster = a->name;
            ff->constr_x = 0;
            ff->constr_y = 0;
            ff->constr_z = s6z::FF5 - s6z::LUT6;
            ff->constr_abs_z = false;
            ff5s++;
        }
    }
    log_info("Packed %d LUT pairs into shared LUT6/LUT5 sites, %d with a 5FF.\n", pairs, ff5s);
}

NEXTPNR_NAMESPACE_END
