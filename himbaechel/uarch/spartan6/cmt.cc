/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: clock management (DCM_SP, PLL_BASE/PLL_ADV).
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

#include <queue>

#include "log.h"
#include "nextpnr.h"
#include "util.h"

#define HIMBAECHEL_CONSTIDS "uarch/spartan6/constids.inc"
#include "himbaechel_constids.h"

#include "spartan6.h"

NEXTPNR_NAMESPACE_BEGIN

// DCM_SP -> DCM bel, PLL_BASE/PLL_ADV -> PLL bel. Parameters stay on the
// cell for the bitstream and XDL writers.
//
// Clock inputs: yosys feeds a DCM/PLL from the BUFG it puts after the clock
// pad. Like ISE (which inserts a BUFIO2), take the clock straight from the
// pad instead, over the dedicated pad -> BUFIO2 -> CMT path; the BUFG still
// clocks the fabric. Placement follows the clock: pad -> DCM/PLL -> the
// BUFGs they feed, each on a site the previous stage reaches through
// dedicated clock wiring (see place_clock_tree).

namespace {
void drop_port(CellInfo *ci, IdString port)
{
    if (ci->ports.count(port)) {
        ci->disconnectPort(port);
        ci->ports.erase(port);
    }
}

void unbracket(Context *ctx, CellInfo *ci)
{
    std::vector<IdString> bracketed;
    for (auto &port : ci->ports)
        if (port.first.str(ctx).find('[') != std::string::npos)
            bracketed.push_back(port.first);
    for (IdString from : bracketed) {
        std::string s = from.str(ctx);
        s.erase(std::remove(s.begin(), s.end(), '['), s.end());
        s.erase(std::remove(s.begin(), s.end(), ']'), s.end());
        ci->renamePort(from, ctx->id(s));
    }
}

// If `port` is driven by a BUFG fed straight from an input buffer, connect it
// to the input buffer's output instead.
void bypass_bufg(Context *ctx, CellInfo *ci, IdString port)
{
    NetInfo *n = ci->getPort(port);
    if (!n || !n->driver.cell || n->driver.cell->type != id_BUFG)
        return;
    NetInfo *pad = n->driver.cell->getPort(id_I);
    if (!pad || !pad->driver.cell || pad->driver.cell->type != id_IOB)
        return;
    ci->disconnectPort(port);
    ci->connectPort(port, pad);
}
} // namespace

void Spartan6Impl::pack_cmt()
{
    NetInfo *gnd = ctx->nets.at(ctx->id("$PACKER_GND")).get();
    // An open input reads 1: tie resets and phase-shift controls low.
    auto tie_low = [&](CellInfo *ci, std::initializer_list<IdString> ports) {
        for (IdString p : ports) {
            if (ci->getPort(p))
                continue;
            ci->ports[p].name = p;
            ci->ports[p].type = PORT_IN;
            ci->connectPort(p, gnd);
        }
    };
    int dcms = 0, plls = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type == id_DCM_SP) {
            drop_port(ci, id_DSSEN);
            // No feedback: a constant CLKFB is left unconnected, as ISE does.
            NetInfo *fb = ci->getPort(id_CLKFB);
            if (fb && fb->constant_value != IdString())
                drop_port(ci, id_CLKFB);
            unbracket(ctx, ci);
            bypass_bufg(ctx, ci, id_CLKIN);
            // Pad -> BUFIO2 (DIVCLK_CMT, divide 1, bypassed) -> DCM, as ISE's
            // map inserts it.
            NetInfo *in = ci->getPort(id_CLKIN);
            if (in && in->driver.cell && in->driver.cell->type == id_IOB) {
                CellInfo *b = ctx->createCell(ctx->idf("%s$bufio2", ci->name.c_str(ctx)), id_BUFIO2);
                b->addInput(id_I);
                b->addOutput(id_DIVCLK_CMT);
                b->connectPort(id_I, in);
                NetInfo *o = ctx->createNet(ctx->idf("%s$bufio2_out", ci->name.c_str(ctx)));
                b->connectPort(id_DIVCLK_CMT, o);
                ci->disconnectPort(id_CLKIN);
                ci->connectPort(id_CLKIN, o);
            }
            tie_low(ci, {id_RST, id_PSEN, id_PSINCDEC, id_PSCLK});
            ci->type = id_DCM;
            dcms++;
        } else if (ci->type.in(id_PLL_BASE, id_PLL_ADV)) {
            if (ci->type == id_PLL_BASE)
                ci->renamePort(id_CLKIN, id_CLKIN1);
            ci->params[id_PLL_PRIMITIVE] = ci->type.str(ctx);
            unbracket(ctx, ci);
            // CLKINSEL is a static setting (CLKIN1) unless driven.
            NetInfo *sel = ci->getPort(id_CLKINSEL);
            if (sel && sel->constant_value != IdString()) {
                if (sel->constant_value == id_GND)
                    log_error("PLL %s: CLKINSEL = 0 (CLKIN2) is not supported yet\n", ctx->nameOf(ci));
                drop_port(ci, id_CLKINSEL);
            }
            // Like ISE: a PLL clocked from a BUFG takes it on CLKIN2 (the
            // global clock input); CLKIN1 is the BUFIO2 path from a pad.
            NetInfo *in = ci->getPort(id_CLKIN1);
            if (in && in->driver.cell && in->driver.cell->type == id_BUFG) {
                ci->disconnectPort(id_CLKIN1);
                ci->ports.erase(id_CLKIN1);
                ci->ports[id_CLKIN2].name = id_CLKIN2;
                ci->ports[id_CLKIN2].type = PORT_IN;
                ci->connectPort(id_CLKIN2, in);
                ci->params[id_CLKIN_SOURCE] = std::string("CLKIN2");
            } else {
                ci->params[id_CLKIN_SOURCE] = std::string("CLKIN1");
            }
            tie_low(ci, {id_RST});
            ci->type = id_PLL;
            plls++;
        }
    }
    if (dcms || plls)
        log_info("Packed %d DCMs and %d PLLs.\n", dcms, plls);
}

// Breadth-first distances over the routing graph from `src`, up to `depth`.
static dict<WireId, int> wire_distances(Context *ctx, WireId src, int depth)
{
    dict<WireId, int> dist{{src, 0}};
    std::vector<WireId> frontier{src};
    for (int d = 1; d <= depth && !frontier.empty(); d++) {
        std::vector<WireId> next;
        for (WireId w : frontier)
            for (PipId pip : ctx->getPipsDownhill(w)) {
                WireId dst = ctx->getPipDstWire(pip);
                if (!dist.count(dst)) {
                    dist[dst] = d;
                    next.push_back(dst);
                }
            }
        frontier = std::move(next);
    }
    return dist;
}

// Place the clock loads of `net`, driven from wire `src`: BUFGs on a
// BUFGMUX (input I0 or I1) and DCM/PLL clock inputs on a reachable site;
// then, recursively, the BUFGs a newly placed DCM/PLL drives.
void Spartan6Impl::place_clock_tree(WireId src, NetInfo *net, const std::string &from)
{
    auto dist = wire_distances(ctx, src, 10);
    std::vector<CellInfo *> placed_cmts, bufgs_to_cmt;
    for (auto &usr : net->users) {
        CellInfo *c = usr.cell;
        if (c->attrs.count(id_BEL))
            continue;
        std::vector<IdString> pins;
        if (c->type == id_BUFGMUX && usr.port.in(id_I0, id_I1))
            pins = {id_I0, id_I1};
        else if (c->type == id_DCM && usr.port == id_CLKIN)
            pins = {id_CLKIN};
        else if (c->type == id_BUFIO2 && usr.port == id_I)
            pins = {id_I};
        else if (c->type == id_PLL && usr.port.in(id_CLKIN1, id_CLKIN2))
            pins = {usr.port};
        else
            continue;
        BelId best;
        IdString best_pin;
        int best_dist = 1000;
        for (BelId bel : ctx->getBels()) {
            if (ctx->getBelType(bel) != c->type || used_clock_bels.count(bel))
                continue;
            for (IdString pin : pins) {
                auto it = dist.find(ctx->getBelPinWire(bel, pin));
                if (it != dist.end() && it->second < best_dist) {
                    best = bel;
                    best_pin = pin;
                    best_dist = it->second;
                }
            }
        }
        if (best == BelId())
            log_error("No %s site is reachable from %s for %s\n", c->type.c_str(ctx), from.c_str(), ctx->nameOf(c));
        if (c->type == id_BUFGMUX && best_pin == id_I1)
            c->renamePort(id_I0, id_I1);
        c->attrs[id_BEL] = ctx->getBelName(best).str(ctx);
        used_clock_bels.insert(best);
        log_info("    %s -> %s.%s (from %s)\n", ctx->nameOf(c), ctx->nameOfBel(best), best_pin.c_str(ctx),
                 from.c_str());
        if (c->type.in(id_DCM, id_PLL, id_BUFIO2))
            placed_cmts.push_back(c);
        // A BUFG may in turn clock a DCM/PLL (PLL CLKIN2).
        if (c->type == id_BUFGMUX) {
            NetInfo *o = c->getPort(id_O);
            bool feeds_cmt = false;
            if (o)
                for (auto &u : o->users)
                    feeds_cmt |= u.cell->type.in(id_DCM, id_PLL);
            if (feeds_cmt)
                bufgs_to_cmt.push_back(c);
        }
    }
    for (CellInfo *b : bufgs_to_cmt) {
        BelId bel = ctx->getBelByNameStr(b->attrs.at(id_BEL).as_string());
        place_clock_tree(ctx->getBelPinWire(bel, id_O), b->getPort(id_O), ctx->nameOf(b));
    }
    // Then the BUFGs each DCM/PLL output feeds.
    for (CellInfo *c : placed_cmts) {
        BelId bel = ctx->getBelByNameStr(c->attrs.at(id_BEL).as_string());
        for (auto &port : c->ports) {
            if (port.second.type != PORT_OUT || !port.second.net)
                continue;
            WireId w = ctx->getBelPinWire(bel, port.first);
            if (w == WireId())
                continue;
            place_clock_tree(w, port.second.net, stringf("%s.%s", ctx->nameOf(c), port.first.c_str(ctx)));
        }
    }
}

NEXTPNR_NAMESPACE_END
