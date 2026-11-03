/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: DSP48A1.
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

#include <tuple>

#include "log.h"
#include "nextpnr.h"
#include "util.h"

#define HIMBAECHEL_CONSTIDS "uarch/spartan6/constids.inc"
#include "himbaechel_constids.h"

#include "spartan6.h"

NEXTPNR_NAMESPACE_BEGIN

// DSP48A1 -> the DSP bel. Bus ports lose their brackets (A[0] -> A0, as the
// bel pins are named). Like ISE, every input is connected: inputs yosys
// left open are tied (clock enables high, everything else low; an open DSP
// input would read 1, holding a used register in reset).
//
// Cascades: PCOUT -> PCIN, CARRYOUT -> CARRYIN, and a B bus fed by another
// DSP's BCOUT (which becomes the dedicated BCIN with B_INPUT = CASCADE, as
// ISE's XST makes it) run on hardwired wires to the DSP one site (4 rows)
// up, so a chain is a cluster with each link 4 rows above the last.
void Spartan6Impl::pack_dsp()
{
    NetInfo *vcc = ctx->nets.at(ctx->id("$PACKER_VCC")).get();
    NetInfo *gnd = ctx->nets.at(ctx->id("$PACKER_GND")).get();
    struct Bus
    {
        const char *name;
        int width;
    };
    static const Bus in_buses[] = {{"A", 18}, {"B", 18}, {"C", 48}, {"D", 18}, {"OPMODE", 8}};
    static const char *ce[] = {"CEA", "CEB", "CEC", "CED", "CEOPMODE", "CECARRYIN", "CEM", "CEP"};
    static const char *rst[] = {"RSTA", "RSTB", "RSTC", "RSTD", "RSTOPMODE", "RSTCARRYIN", "RSTM", "RSTP"};
    auto bit = [&](const char *bus, int i, int width) {
        return width > 1 ? ctx->idf("%s[%d]", bus, i) : ctx->id(bus);
    };
    // The DSP driving a cascade input bus (bit i from output bus bit i), or
    // nullptr when the input is unused / constant 0.
    auto cascade_from = [&](CellInfo *ci, const char *in, const char *out, int width) -> CellInfo * {
        CellInfo *src = nullptr;
        bool any = false, all = true;
        for (int i = 0; i < width; i++) {
            NetInfo *n = ci->getPort(bit(in, i, width));
            if (!n || n->constant_value == id_GND) {
                all = false;
                continue;
            }
            any = true;
            CellInfo *d = n->driver.cell;
            if (!d || d->type != id_DSP48A1 || n->driver.port != bit(out, i, width) || (src && d != src)) {
                all = false;
                continue;
            }
            src = d;
        }
        if (!any)
            return nullptr;
        if (!all)
            return reinterpret_cast<CellInfo *>(-1);
        return src;
    };
    std::vector<CellInfo *> dsps;
    for (auto &cell : ctx->cells)
        if (cell.second->type == id_DSP48A1)
            dsps.push_back(cell.second.get());
    dict<IdString, CellInfo *> below; // DSP -> the DSP cascading into it
    for (CellInfo *ci : dsps) {
        CellInfo *bad = reinterpret_cast<CellInfo *>(-1);
        CellInfo *p = cascade_from(ci, "PCIN", "PCOUT", 48);
        CellInfo *c = cascade_from(ci, "CARRYIN", "CARRYOUT", 1);
        CellInfo *b = cascade_from(ci, "B", "BCOUT", 18);
        if (p == bad)
            log_error("DSP48A1 %s: PCIN must be another DSP48A1's PCOUT, bit for bit\n", ctx->nameOf(ci));
        if (c == bad)
            log_error("DSP48A1 %s: CARRYIN must be another DSP48A1's CARRYOUT\n", ctx->nameOf(ci));
        if (b == bad)
            b = nullptr; // B from fabric (partly from BCOUT is just routing)
        CellInfo *src = nullptr;
        for (CellInfo *x : {p, c, b}) {
            if (!x)
                continue;
            if (src && x != src)
                log_error("DSP48A1 %s: cascades from two different DSPs\n", ctx->nameOf(ci));
            src = x;
        }
        if (b) {
            // B from the DSP below's BCOUT: the dedicated BCIN.
            for (int i = 0; i < 18; i++) {
                IdString bp = ctx->idf("B[%d]", i), cp = ctx->idf("BCIN[%d]", i);
                NetInfo *n = ci->getPort(bp);
                ci->disconnectPort(bp);
                ci->ports.erase(bp);
                ci->ports[cp].name = cp;
                ci->ports[cp].type = PORT_IN;
                ci->connectPort(cp, n);
            }
            ci->params[ctx->id("B_INPUT")] = std::string("CASCADE");
        }
        // Unused cascade inputs go.
        for (auto [bus, width, used] : {std::tuple<const char *, int, bool>{"PCIN", 48, p != nullptr},
                                        {"CARRYIN", 1, c != nullptr}, {"BCIN", 18, b != nullptr}}) {
            if (used)
                continue;
            for (int i = 0; i < width; i++) {
                IdString pn = bit(bus, i, width);
                if (ci->ports.count(pn)) {
                    ci->disconnectPort(pn);
                    ci->ports.erase(pn);
                }
            }
        }
        if (src)
            below[ci->name] = src;
    }
    // Cascade outputs: only to the DSP above (the wires reach nothing else).
    for (CellInfo *ci : dsps) {
        for (auto [bus, width] : {std::pair<const char *, int>{"PCOUT", 48}, {"BCOUT", 18}, {"CARRYOUT", 1}}) {
            for (int i = 0; i < width; i++) {
                IdString pn = bit(bus, i, width);
                NetInfo *n = ci->getPort(pn);
                if (!n || n->users.entries() == 0) {
                    if (ci->ports.count(pn)) {
                        ci->disconnectPort(pn);
                        ci->ports.erase(pn);
                    }
                    continue;
                }
                for (auto &u : n->users)
                    if (u.cell->type != id_DSP48A1 || !below.count(u.cell->name) || below.at(u.cell->name) != ci)
                        log_error("DSP48A1 %s: %s drives something other than the next DSP's cascade input\n",
                                  ctx->nameOf(ci), pn.c_str(ctx));
            }
        }
    }
    // Chains: each link one DSP site (4 rows) above the one below it.
    dict<IdString, CellInfo *> above;
    for (auto &b : below) {
        if (above.count(b.second->name))
            log_error("DSP48A1 %s cascades into two DSPs\n", ctx->nameOf(b.second));
        above[b.second->name] = ctx->cells.at(b.first).get();
    }
    for (CellInfo *ci : dsps) {
        if (below.count(ci->name) || !above.count(ci->name))
            continue; // not the bottom of a chain
        ci->cluster = ci->name;
        int dy = 0;
        for (CellInfo *c = above.at(ci->name);; c = above.at(c->name)) {
            dy += 4;
            c->cluster = ci->name;
            ci->constr_children.push_back(c);
            c->constr_x = 0;
            c->constr_y = dy;
            c->constr_z = 0;
            c->constr_abs_z = false;
            if (!above.count(c->name))
                break;
        }
        log_info("DSP48A1 cascade of %d starting at %s\n", dy / 4 + 1, ctx->nameOf(ci));
    }
    int n = 0;
    for (CellInfo *ci : dsps) {
        // Bus ports: drop brackets.
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
        auto tie = [&](IdString p, NetInfo *net) {
            if (ci->getPort(p))
                return;
            ci->ports[p].name = p;
            ci->ports[p].type = PORT_IN;
            ci->connectPort(p, net);
        };
        // (B too with BCIN in use: ISE's DRC wants B tied to ground then.)
        for (auto &b : in_buses)
            for (int i = 0; i < b.width; i++)
                tie(ctx->idf("%s%d", b.name, i), gnd);
        for (const char *p : ce)
            tie(ctx->id(p), vcc);
        for (const char *p : rst)
            tie(ctx->id(p), gnd);
        if (!ci->getPort(id_CLK))
            log_warning("DSP48A1 %s has no clock\n", ctx->nameOf(ci));
        ci->type = id_DSP;
        n++;
    }
    if (n)
        log_info("Packed %d DSP48A1.\n", n);
}

NEXTPNR_NAMESPACE_END
