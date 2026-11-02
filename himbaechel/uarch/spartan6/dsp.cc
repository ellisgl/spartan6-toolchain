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
// input would read 1, holding a used register in reset). The cascade ports
// (BCIN/PCIN/CARRYIN, BCOUT/PCOUT/CARRYOUT) have no bel pins in the
// database, so cascades are not supported.
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
    int n = 0;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_DSP48A1)
            continue;
        // Cascades: only allowed when unused or constant 0.
        for (const char *cas : {"BCIN", "PCIN", "CARRYIN"}) {
            for (int i = -1; i < 48; i++) {
                IdString p = i < 0 ? ctx->id(cas) : ctx->idf("%s[%d]", cas, i);
                NetInfo *net = ci->getPort(p);
                if (net && net->constant_value != id_GND)
                    log_error("DSP48A1 %s: cascade input %s is not supported yet\n", ctx->nameOf(ci), p.c_str(ctx));
                if (ci->ports.count(p)) {
                    ci->disconnectPort(p);
                    ci->ports.erase(p);
                }
            }
        }
        for (const char *cas : {"BCOUT", "PCOUT", "CARRYOUT"}) {
            for (int i = -1; i < 48; i++) {
                IdString p = i < 0 ? ctx->id(cas) : ctx->idf("%s[%d]", cas, i);
                NetInfo *net = ci->getPort(p);
                if (net && net->users.entries())
                    log_error("DSP48A1 %s: cascade output %s is not supported yet\n", ctx->nameOf(ci), p.c_str(ctx));
                if (ci->ports.count(p)) {
                    ci->disconnectPort(p);
                    ci->ports.erase(p);
                }
            }
        }
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
