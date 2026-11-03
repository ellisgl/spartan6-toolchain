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
            // Feedback through a BUFG: through a BUFIO2FB, as ISE's map does
            // (as for a PLL's CLKFBIN); the DLL then compensates for the same
            // delay as ISE's (general routing would shift CLK0 by its own).
            fb = ci->getPort(id_CLKFB);
            if (fb && fb->driver.cell && fb->driver.cell->type == id_BUFG) {
                CellInfo *f = ctx->createCell(ctx->idf("%s$bufio2fb", ci->name.c_str(ctx)), id_BUFIO2FB);
                f->addInput(id_I);
                f->addOutput(id_O);
                f->connectPort(id_I, fb);
                NetInfo *fo = ctx->createNet(ctx->idf("%s$bufio2fb_out", ci->name.c_str(ctx)));
                f->connectPort(id_O, fo);
                ci->disconnectPort(id_CLKFB);
                ci->connectPort(id_CLKFB, fo);
            }
            unbracket(ctx, ci);
            bypass_bufg(ctx, ci, id_CLKIN);
            // PSCLK from a pad: through general routing from the pad's input
            // buffer, as ISE routes it (the pad's BUFG sits on a global line
            // that can't reach the DCM's GFAN input).
            bypass_bufg(ctx, ci, id_PSCLK);
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
            // Compensation: with CLKFBOUT wired straight to CLKFBIN there is
            // nothing to compensate, and ISE's map switches the PLL to
            // INTERNAL; do the same. With feedback through a BUFG, insert a
            // BUFIO2FB in front of CLKFBIN, as ISE's map does (the BUFG's
            // output reaches it through an unused ILOGIC's clock-to-CFB0
            // route-through).
            std::string comp = ci->params.count(ctx->id("COMPENSATION"))
                                       ? ci->params.at(ctx->id("COMPENSATION")).as_string()
                                       : std::string("SYSTEM_SYNCHRONOUS");
            if (comp != "INTERNAL") {
                NetInfo *fb = ci->getPort(ctx->id("CLKFBIN"));
                if (fb && fb->driver.cell == ci && fb->driver.port == ctx->id("CLKFBOUT")) {
                    log_info("PLL %s: COMPENSATION = %s with direct feedback becomes INTERNAL (as ISE's map does)\n",
                             ctx->nameOf(ci), comp.c_str());
                    ci->params[ctx->id("COMPENSATION")] = std::string("INTERNAL");
                } else if (comp != "SYSTEM_SYNCHRONOUS" && comp != "SOURCE_SYNCHRONOUS") {
                    log_error("PLL %s: COMPENSATION = %s is not supported yet\n", ctx->nameOf(ci), comp.c_str());
                } else if (fb && fb->driver.cell && fb->driver.cell->type == id_BUFG) {
                    CellInfo *f = ctx->createCell(ctx->idf("%s$bufio2fb", ci->name.c_str(ctx)), id_BUFIO2FB);
                    f->addInput(id_I);
                    f->addOutput(id_O);
                    f->connectPort(id_I, fb);
                    NetInfo *fo = ctx->createNet(ctx->idf("%s$bufio2fb_out", ci->name.c_str(ctx)));
                    f->connectPort(id_O, fo);
                    ci->disconnectPort(ctx->id("CLKFBIN"));
                    ci->connectPort(ctx->id("CLKFBIN"), fo);
                } else {
                    log_error("PLL %s: COMPENSATION = %s needs CLKFBIN from CLKFBOUT, directly or through a BUFG\n",
                              ctx->nameOf(ci), comp.c_str());
                }
            }
            // Like ISE's map: a PLL clocked from a clock pad (through the BUFG
            // yosys puts after it) takes it from the pad through an inserted
            // BUFIO2 (divided-clock output); the placer picks CLKIN1 or CLKIN2,
            // whichever it reaches. Otherwise a BUFG feeds CLKIN2 (the global
            // clock input).
            bypass_bufg(ctx, ci, id_CLKIN1);
            NetInfo *in = ci->getPort(id_CLKIN1);
            if (in && in->driver.cell && in->driver.cell->type == id_IOB) {
                CellInfo *b = ctx->createCell(ctx->idf("%s$bufio2", ci->name.c_str(ctx)), id_BUFIO2);
                b->addInput(id_I);
                b->addOutput(id_DIVCLK_CMT);
                b->connectPort(id_I, in);
                NetInfo *o = ctx->createNet(ctx->idf("%s$bufio2_out", ci->name.c_str(ctx)));
                b->connectPort(id_DIVCLK_CMT, o);
                ci->disconnectPort(id_CLKIN1);
                ci->connectPort(id_CLKIN1, o);
                ci->params[id_CLKIN_SOURCE] = std::string("CLKIN1");
            } else if (in && in->driver.cell && in->driver.cell->type == id_BUFG) {
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

// Whether a BUFIO2FB at `bel` reaches the feedback input of the (placed) PLL
// or DCM its output feeds.
bool Spartan6Impl::bufio2fb_reaches_pll(CellInfo *fb, BelId bel)
{
    NetInfo *o = fb->getPort(id_O);
    if (!o)
        return true;
    for (auto &u : o->users) {
        if (!u.cell->type.in(id_PLL, id_DCM) || !u.cell->attrs.count(id_BEL))
            continue;
        BelId pll = ctx->getBelByNameStr(u.cell->attrs.at(id_BEL).as_string());
        WireId target = ctx->getBelPinWire(pll, u.port);
        pool<WireId> seen{ctx->getBelPinWire(bel, id_O)};
        std::vector<WireId> frontier{ctx->getBelPinWire(bel, id_O)};
        for (int d = 1; d <= 8 && !frontier.empty(); d++) {
            std::vector<WireId> next;
            for (WireId w : frontier)
                for (PipId pip : ctx->getPipsDownhill(w)) {
                    WireId dst = ctx->getPipDstWire(pip);
                    if (dst == target)
                        return true;
                    if (seen.insert(dst).second)
                        next.push_back(dst);
                }
            frontier = std::move(next);
        }
        return false;
    }
    return true;
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

// Whether a BUFG's output also goes to logic data inputs (anything on a slice
// but its clock and set/reset, or an I/O) or a DCM's PSCLK (through the
// interconnect's GFAN as well), which need GCLK 0..7.
bool Spartan6Impl::bufg_drives_data(const CellInfo *bufg) const
{
    const NetInfo *o = bufg->getPort(id_O);
    if (!o)
        return false;
    for (auto &u : o->users) {
        if (u.cell->type == id_IOB || (u.cell->type == id_DCM && u.port == id_PSCLK))
            return true;
        if (u.cell->type.in(id_SLICE_LUT, id_SLICE_LUT5, id_SLICE_FF, id_SLICE_FF5, id_SLICE_LUTRAM, id_CARRY4) &&
            !u.port.in(id_CK, id_CLK, id_SR))
            return true;
    }
    return false;
}

// Place the clock loads of `net`, driven from wire `src`: BUFGs on a
// BUFGMUX (input I0 or I1) and DCM/PLL clock inputs on a reachable site;
// then, recursively, the BUFGs a newly placed DCM/PLL drives.
void Spartan6Impl::place_clock_tree(WireId src, NetInfo *net, const std::string &from)
{
    // A BUFIO2FB is reached from a BUFG through the clock network, an I/O
    // clock and an ILOGIC route-through: a longer search.
    bool to_fb = false;
    for (auto &usr : net->users)
        to_fb |= usr.cell->type == id_BUFIO2FB;
    auto dist = wire_distances(ctx, src, to_fb ? 24 : 10);
    std::vector<CellInfo *> placed_cmts, bufgs_to_cmt;
    for (auto &usr : net->users) {
        CellInfo *c = usr.cell;
        if (c->attrs.count(id_BEL))
            continue;
        std::vector<IdString> pins;
        // A BUFG is the BUFGMUX's I0 (the writers don't drive I1 / S yet).
        if (c->type == id_BUFGMUX && usr.port.in(id_I0, id_I1))
            pins = {id_I0};
        else if (c->type == id_DCM && usr.port == id_CLKIN)
            pins = {id_CLKIN};
        else if (c->type == id_BUFIO2 && usr.port == id_I)
            pins = {id_I};
        else if (c->type == id_BUFIO2FB && usr.port == id_I)
            pins = {id_I};
        else if (c->type == id_PLL && usr.port.in(id_CLKIN1, id_CLKIN2))
            pins = net->driver.cell && net->driver.cell->type == id_BUFIO2 ? std::vector<IdString>{id_CLKIN1, id_CLKIN2}
                                                                           : std::vector<IdString>{usr.port};
        else
            continue;
        BelId best;
        IdString best_pin;
        int best_dist = 1000;
        // A global clock reaches logic data inputs only through the
        // interconnect's GFAN muxes, which take GCLK 0..7 alone: a BUFG whose
        // output is also used as data goes on BUFGMUX 0..7 when one is free.
        bool data_gclk = c->type == id_BUFGMUX && bufg_drives_data(c);
        auto gclk_ok = [&](BelId bel) {
            if (!data_gclk)
                return true;
            const std::string name = ctx->getBelName(bel)[1].str(ctx);
            return std::stoi(name.substr(name.find('[') + 1)) < 8;
        };
        for (int pass = 0; pass < 2 && best == BelId(); pass++)
        for (BelId bel : ctx->getBels()) {
            if (ctx->getBelType(bel) != c->type || used_clock_bels.count(bel))
                continue;
            if (pass == 0 && !gclk_ok(bel))
                continue;
            // A BUFIO2FB must also reach its (already placed) PLL's CLKFBIN.
            if (c->type == id_BUFIO2FB && !bufio2fb_reaches_pll(c, bel))
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
        // A BUFG fed from a pin without a dedicated clock path (or from
        // logic): any free BUFGMUX, reached through general routing.
        if (best == BelId() && c->type == id_BUFGMUX) {
            for (int pass = 0; pass < 2 && best == BelId(); pass++)
                for (BelId bel : ctx->getBels())
                    if (ctx->getBelType(bel) == id_BUFGMUX && !used_clock_bels.count(bel) && (pass == 1 || gclk_ok(bel))) {
                        best = bel;
                        best_pin = id_I0;
                        break;
                    }
            if (best != BelId())
                log_info("    %s: no dedicated path from %s, using general routing\n", ctx->nameOf(c), from.c_str());
        }
        if (best == BelId())
            log_error("No %s site is reachable from %s for %s\n", c->type.c_str(ctx), from.c_str(), ctx->nameOf(c));
        if (!gclk_ok(best))
            log_warning("%s: its clock is also used as data, but BUFGMUX 0..7 are taken; it may not route\n",
                        ctx->nameOf(c));
        if (c->type == id_BUFGMUX && best_pin == id_I1)
            c->renamePort(id_I0, id_I1);
        if (c->type == id_PLL && best_pin != usr.port) {
            NetInfo *n = c->getPort(usr.port);
            c->disconnectPort(usr.port);
            c->ports.erase(usr.port);
            c->ports[best_pin].name = best_pin;
            c->ports[best_pin].type = PORT_IN;
            c->connectPort(best_pin, n);
            c->params[id_CLKIN_SOURCE] = best_pin.str(ctx);
        }
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
                    feeds_cmt |= u.cell->type.in(id_DCM, id_PLL, id_BUFIO2FB);
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
