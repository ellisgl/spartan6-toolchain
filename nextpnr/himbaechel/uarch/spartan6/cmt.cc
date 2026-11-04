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
            // A constant CLKINSEL only says which input carries the clock
            // (1 or unconnected: CLKIN1, 0: CLKIN2). Keep that one, as
            // CLKIN1; the other is never selected. Which physical input it
            // ends up on is chosen below (CLKIN_SOURCE, the bitstream's
            // CLKINSEL_STATIC_VAL), as for PLL_BASE.
            NetInfo *sel = ci->getPort(id_CLKINSEL);
            // CLKINSEL from logic switches between both inputs, so they keep
            // their pins. ISE requires each from a clock pad through its own
            // BUFIO2 (placed so both reach this PLL; see place_clock_tree).
            bool dynamic = sel && sel->constant_value == IdString();
            // PLL_ADV's dynamic reconfiguration port (DRP). Tied off (constant
            // DCLK/DEN/DWE/DADDR/DI, DO/DRDY unused) it does nothing: drop it.
            // Used from logic, its pins stay and are routed like any other
            // (constant ones from the tie-offs).
            {
                bool drp_used = false;
                std::vector<IdString> drp;
                for (auto &[name, port] : ci->ports) {
                    std::string n = name.str(ctx);
                    bool in = n == "DCLK" || n == "DEN" || n == "DWE" || n.rfind("DADDR", 0) == 0 ||
                              (n.rfind("DI", 0) == 0 && n.size() > 2 && isdigit(n[2]));
                    bool out = n == "DRDY" || (n.rfind("DO", 0) == 0 && n.size() > 2 && isdigit(n[2]));
                    if (!in && !out)
                        continue;
                    drp.push_back(name);
                    if (port.net && ((in && port.net->constant_value == IdString()) || (out && port.net->users.entries())))
                        drp_used = true;
                }
                if (!drp_used)
                    for (IdString p : drp)
                        drop_port(ci, p);
                // DCLK is reached through the interconnect (GFAN), so its BUFG
                // needs BUFGMUX 0..7 (see bufg_drives_data and pack_clocking).
            }
            // PLL_ADV's reserved REL pin has no site pin.
            if (NetInfo *rel = ci->getPort(ctx->id("REL"))) {
                if (rel->constant_value == IdString())
                    log_error("PLL %s: REL driven from logic is not supported (the site has no REL pin)\n",
                              ctx->nameOf(ci));
                drop_port(ci, ctx->id("REL"));
            }
            if (dynamic) {
                ci->params[ctx->id("CLKINSEL_DYNAMIC")] = std::string("TRUE");
                ci->params[id_CLKIN_SOURCE] = std::string("CLKIN1");
                for (IdString pin : {id_CLKIN1, id_CLKIN2}) {
                    bypass_bufg(ctx, ci, pin);
                    NetInfo *in = ci->getPort(pin);
                    if (!in || !in->driver.cell || in->driver.cell->type != id_IOB)
                        log_error("PLL %s: with CLKINSEL from logic, %s must come from a clock pin (through a BUFG "
                                  "or directly)\n",
                                  ctx->nameOf(ci), pin.c_str(ctx));
                    CellInfo *b = ctx->createCell(ctx->idf("%s$bufio2_%s", ci->name.c_str(ctx), pin.c_str(ctx)), id_BUFIO2);
                    b->addInput(id_I);
                    b->addOutput(id_DIVCLK_CMT);
                    b->connectPort(id_I, in);
                    NetInfo *o = ctx->createNet(ctx->idf("%s$bufio2_%s_out", ci->name.c_str(ctx), pin.c_str(ctx)));
                    b->connectPort(id_DIVCLK_CMT, o);
                    ci->disconnectPort(pin);
                    ci->connectPort(pin, o);
                }
                tie_low(ci, {id_RST});
                ci->type = id_PLL;
                plls++;
                continue;
            }
            bool use2 = sel && sel->constant_value == id_GND;
            drop_port(ci, id_CLKINSEL);
            drop_port(ci, use2 ? id_CLKIN1 : id_CLKIN2);
            if (use2) {
                if (!ci->getPort(id_CLKIN2))
                    log_error("PLL %s: CLKINSEL = 0 selects CLKIN2, which is not connected\n", ctx->nameOf(ci));
                ci->renamePort(id_CLKIN2, id_CLKIN1);
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
    // A BUFIO2FB's output is O; a BUFIO2's CMT output is DIVCLK_CMT.
    IdString out = fb->type == id_BUFIO2 ? id_DIVCLK_CMT : id_O;
    NetInfo *o = fb->getPort(out);
    if (!o)
        return true;
    for (auto &u : o->users) {
        if (!u.cell->type.in(id_PLL, id_DCM) || !u.cell->attrs.count(id_BEL))
            continue;
        BelId pll = ctx->getBelByNameStr(u.cell->attrs.at(id_BEL).as_string());
        WireId target = ctx->getBelPinWire(pll, u.port);
        pool<WireId> seen{ctx->getBelPinWire(bel, out)};
        std::vector<WireId> frontier{ctx->getBelPinWire(bel, out)};
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

static dict<WireId, int> wire_distances(Context *ctx, WireId src, int depth);

// A PLL with CLKINSEL from logic takes CLKIN1 and CLKIN2 through one pair
// mux in its tile, whose only two-input settings from BUFIO2s are
// DIVCLK_CMT_V[n] on CLKIN1 with DIVCLK_CMT_V[n + 4] on CLKIN2 (n = 0..3).
// So CLKIN1's BUFIO2 goes on index 0..3 and CLKIN2's four above it, in the
// same tile.
bool Spartan6Impl::dynamic_pll_pair_ok(CellInfo *b, BelId bel)
{
    NetInfo *o = b->getPort(id_DIVCLK_CMT);
    if (!o)
        return true;
    auto index = [&](BelId x) {
        const std::string name = ctx->getBelName(x)[1].str(ctx);
        return std::stoi(name.substr(name.find('[') + 1));
    };
    for (auto &u : o->users) {
        if (u.cell->type != id_PLL || !u.cell->params.count(ctx->id("CLKINSEL_DYNAMIC")))
            continue;
        if (bel == BelId())
            return false; // asked whether b feeds such a PLL: yes
        bool is1 = u.port == id_CLKIN1;
        int i = index(bel);
        if (is1 ? i >= 4 : i < 4)
            return false;
        NetInfo *other = u.cell->getPort(is1 ? id_CLKIN2 : id_CLKIN1);
        if (!other || !other->driver.cell)
            return true;
        CellInfo *ob_cell = other->driver.cell;
        Loc la = ctx->getBelLocation(bel);
        int want = is1 ? i + 4 : i - 4;
        // The other input's BUFIO2, if already placed: the partner site.
        if (ob_cell->attrs.count(id_BEL)) {
            BelId ob = ctx->getBelByNameStr(ob_cell->attrs.at(id_BEL).as_string());
            Loc lb = ctx->getBelLocation(ob);
            return la.x == lb.x && la.y == lb.y && index(ob) == want;
        }
        // Not yet: its pad must reach the partner site.
        NetInfo *pin = ob_cell->getPort(id_I);
        if (!pin || !pin->driver.cell || !pin->driver.cell->attrs.count(id_BEL))
            return true;
        BelId pad = ctx->getBelByNameStr(pin->driver.cell->attrs.at(id_BEL).as_string());
        auto dist = wire_distances(ctx, ctx->getBelPinWire(pad, id_GCLK_I), 10);
        for (BelId x : ctx->getBelsByTile(la.x, la.y))
            if (ctx->getBelType(x) == id_BUFIO2 && index(x) == want && !used_clock_bels.count(x) &&
                dist.count(ctx->getBelPinWire(x, id_I)))
                return true;
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
        // A DCM's PSCLK and a PLL's DRP clock are reached through the
        // interconnect (GFAN), like data inputs.
        if (u.cell->type == id_IOB || (u.cell->type == id_DCM && u.port == id_PSCLK) ||
            (u.cell->type == id_PLL && u.port == ctx->id("DCLK")))
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
            pins = net->driver.cell && net->driver.cell->type == id_BUFIO2 && !c->params.count(ctx->id("CLKINSEL_DYNAMIC"))
                           ? std::vector<IdString>{id_CLKIN1, id_CLKIN2}
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
            // A BUFIO2FB (or a BUFIO2) must also reach its already placed PLL's pin.
            if (c->type.in(id_BUFIO2FB, id_BUFIO2) && !bufio2fb_reaches_pll(c, bel))
                continue;
            if (c->type == id_BUFIO2 && !dynamic_pll_pair_ok(c, bel))
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
        if (best == BelId() && c->type == id_BUFIO2 && !dynamic_pll_pair_ok(c, BelId()))
            log_error("No BUFIO2 site for %s: a PLL with CLKINSEL from logic needs its CLKIN1 and CLKIN2 from "
                      "the two pins of one clock pair on its edge (e.g. P56 / P55 on the TQG144's bottom edge), "
                      "which reach BUFIO2 n and n + 4 (the PLL's input pair mux takes no other combination)\n",
                      ctx->nameOf(c));
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
