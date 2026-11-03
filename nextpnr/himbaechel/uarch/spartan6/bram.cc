/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: block RAM (RAMB16BWER, RAMB8BWER).
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

// prjcombine models each BRAM tile as two 9 Kb halves, BRAM[0] (z 5) and
// BRAM[1] (z 6), each with a RAMB8BWER's pins. A RAMB16BWER uses both, wired
// as ISE's naming shows: port bits 15:0 (parity 1:0, WE 1:0) on BRAM[0] and
// 31:16 (3:2) on BRAM[1]; address bits 12:0 on BRAM[0] and bit 13 on
// BRAM[1].ADDR[0]; CLK/EN/REGCE/RST on BRAM[0]. It is packed as a cluster:
// the BRAM[0] cell keeps the parameters (RAMB_KIND = RAMB16), the BRAM[1]
// cell (RAMB_KIND = RAMB16_HI) only carries pins.
//
// Pins whose value doesn't matter are disconnected, as ISE does: all of an
// unused port's pins (DATA_WIDTH 0), address bits below the data width, and
// data bits above it.

namespace {
const int Z_BRAM0 = 5, Z_BRAM1 = 6;

int width_param(const CellInfo *ci, IdString p)
{
    if (!ci->params.count(p))
        return 0;
    const Property &v = ci->params.at(p);
    return v.is_string ? std::stoi(v.as_string()) : int(v.as_int64());
}

// Number of low address bits a port of this width ignores.
int ignored_addr_bits(int w)
{
    switch (w) {
    case 2:
        return 1;
    case 4:
        return 2;
    case 9:
        return 3;
    case 18:
        return 4;
    case 36:
        return 5;
    default:
        return 0;
    }
}
// Data and parity bits used at this width (RAMB16 numbering).
int data_bits(int w) { return w == 9 ? 8 : w == 18 ? 16 : w == 36 ? 32 : w; }
int parity_bits(int w) { return w == 9 ? 1 : w == 18 ? 2 : w == 36 ? 4 : 0; }

void drop(CellInfo *ci, IdString port)
{
    if (ci->ports.count(port)) {
        ci->disconnectPort(port);
        ci->ports.erase(port);
    }
}

// Disconnect don't-care pins of port `ab` ("A"/"B") given its width;
// `addr`/`di`/... are the cell's bus names for that port.
void trim_port(Context *ctx, CellInfo *ci, int w, int addr_bits, int data_w, int par_w, int we_w,
               const std::string &addr, const std::string &di, const std::string &dip, const std::string &dout,
               const std::string &dop, const std::string &we, const std::vector<std::string> &ctrl)
{
    auto bit = [&](const std::string &b, int i) { return ctx->idf("%s[%d]", b.c_str(), i); };
    if (w == 0) {
        for (int i = 0; i < addr_bits; i++)
            drop(ci, bit(addr, i));
        for (int i = 0; i < data_w; i++) {
            drop(ci, bit(di, i));
            drop(ci, bit(dout, i));
        }
        for (int i = 0; i < par_w; i++) {
            drop(ci, bit(dip, i));
            drop(ci, bit(dop, i));
        }
        for (int i = 0; i < we_w; i++)
            drop(ci, bit(we, i));
        for (auto &c : ctrl)
            drop(ci, ctx->id(c));
        return;
    }
    for (int i = 0; i < ignored_addr_bits(w); i++)
        drop(ci, bit(addr, i));
    // Inputs the port does use but yosys left undriven ('x'): tie low, as ISE
    // does (bitgen rejects a configured but unconnected parity input).
    NetInfo *gnd = ctx->nets.at(ctx->id("$PACKER_GND")).get();
    auto tie = [&](IdString p) {
        if (ci->ports.count(p) && ci->getPort(p))
            return;
        ci->ports[p].name = p;
        ci->ports[p].type = PORT_IN;
        ci->connectPort(p, gnd);
    };
    for (int i = ignored_addr_bits(w); i < addr_bits; i++)
        tie(bit(addr, i));
    for (int i = 0; i < std::min(data_bits(w), data_w); i++)
        tie(bit(di, i));
    for (int i = 0; i < std::min(parity_bits(w), par_w); i++)
        tie(bit(dip, i));
    for (int i = std::min(data_bits(w), data_w); i < data_w; i++) {
        drop(ci, bit(di, i));
        drop(ci, bit(dout, i));
    }
    for (int i = std::min(parity_bits(w), par_w); i < par_w; i++) {
        drop(ci, bit(dip, i));
        drop(ci, bit(dop, i));
    }
}
} // namespace

void Spartan6Impl::pack_bram()
{
    int n16 = 0, n8 = 0;
    std::vector<CellInfo *> todo;
    for (auto &cell : ctx->cells)
        if (cell.second->type.in(id_RAMB16BWER, id_RAMB8BWER))
            todo.push_back(cell.second.get());
    for (CellInfo *ci : todo) {
        int wa = width_param(ci, id_DATA_WIDTH_A), wb = width_param(ci, id_DATA_WIDTH_B);
        if (ci->type == id_RAMB16BWER) {
            for (int p = 0; p < 2; p++) {
                std::string ab = p ? "B" : "A";
                trim_port(ctx, ci, p ? wb : wa, 14, 32, 4, 4, "ADDR" + ab, "DI" + ab, "DIP" + ab, "DO" + ab,
                          "DOP" + ab, "WE" + ab, {"CLK" + ab, "EN" + ab, "REGCE" + ab, "RST" + ab});
            }
            CellInfo *hi = ctx->createCell(ctx->idf("%s$hi", ci->name.c_str(ctx)), id_BRAM);
            hi->params[id_RAMB_KIND] = std::string("RAMB16_HI");
            ci->params[id_RAMB_KIND] = std::string("RAMB16");
            // RAMB16 bus bit -> (BRAM[1]?, bit there).
            auto move = [&](const std::string &bus, int n, int split, int hi_off) {
                for (int i = 0; i < n; i++) {
                    IdString from = ctx->idf("%s[%d]", bus.c_str(), i);
                    if (!ci->ports.count(from))
                        continue;
                    if (i < split) {
                        continue; // same name on BRAM[0]
                    }
                    IdString to = ctx->idf("%s[%d]", bus.c_str(), i - hi_off);
                    PortType t = ci->ports.at(from).type;
                    NetInfo *net = ci->getPort(from);
                    drop(ci, from);
                    hi->ports[to].name = to;
                    hi->ports[to].type = t;
                    if (net)
                        hi->connectPort(to, net);
                }
            };
            for (std::string ab : {"A", "B"}) {
                move("ADDR" + ab, 14, 13, 13);
                move("DI" + ab, 32, 16, 16);
                move("DO" + ab, 32, 16, 16);
                move("DIP" + ab, 4, 2, 2);
                move("DOP" + ab, 4, 2, 2);
                move("WE" + ab, 4, 2, 2);
            }
            ci->type = id_BRAM;
            ci->cluster = ci->name;
            ci->constr_abs_z = true;
            ci->constr_z = Z_BRAM0;
            hi->cluster = ci->name;
            ci->constr_children.push_back(hi);
            hi->constr_x = 0;
            hi->constr_y = 0;
            hi->constr_abs_z = true;
            hi->constr_z = Z_BRAM1;
            n16++;
        } else {
            // RAMB8BWER pin names -> BRAM half pins. In SDP mode both ports
            // carry one wide port, so neither is dropped.
            bool sdp = ci->params.count(id_RAM_MODE) && ci->params.at(id_RAM_MODE).as_string() == "SDP";
            static const std::vector<std::pair<std::string, std::string>> buses = {
                    {"ADDRAWRADDR", "ADDRA"}, {"ADDRBRDADDR", "ADDRB"}, {"DIADI", "DIA"}, {"DIBDI", "DIB"},
                    {"DIPADIP", "DIPA"},      {"DIPBDIP", "DIPB"},      {"DOADO", "DOA"}, {"DOBDO", "DOB"},
                    {"DOPADOP", "DOPA"},      {"DOPBDOP", "DOPB"},      {"WEAWEL", "WEA"}, {"WEBWEU", "WEB"}};
            for (auto &b : buses)
                for (int i = 0; i < 16; i++)
                    if (ci->ports.count(ctx->idf("%s[%d]", b.first.c_str(), i)))
                        ci->renamePort(ctx->idf("%s[%d]", b.first.c_str(), i), ctx->idf("%s[%d]", b.second.c_str(), i));
            static const std::vector<std::pair<std::string, std::string>> singles = {
                    {"CLKAWRCLK", "CLKA"}, {"CLKBRDCLK", "CLKB"}, {"ENAWREN", "ENA"},
                    {"ENBRDEN", "ENB"},    {"REGCEBREGCE", "REGCEB"}, {"RSTBRST", "RSTB"}};
            for (auto &s : singles)
                if (ci->ports.count(ctx->id(s.first)))
                    ci->renamePort(ctx->id(s.first), ctx->id(s.second));
            if (!sdp) {
                for (int p = 0; p < 2; p++) {
                    std::string ab = p ? "B" : "A";
                    trim_port(ctx, ci, p ? wb : wa, 13, 16, 2, 2, "ADDR" + ab, "DI" + ab, "DIP" + ab, "DO" + ab,
                              "DOP" + ab, "WE" + ab, {"CLK" + ab, "EN" + ab, "REGCE" + ab, "RST" + ab});
                }
            }
            ci->params[id_RAMB_KIND] = std::string("RAMB8");
            ci->type = id_BRAM;
            n8++;
        }
    }
    // Bel pins are named without brackets (ADDRA5); see the exporter's pin_name.
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_BRAM)
            continue;
        std::vector<IdString> bracketed;
        for (auto &port : ci->ports)
            if (port.first.str(ctx).find('[') != std::string::npos)
                bracketed.push_back(port.first);
        for (IdString from : bracketed) {
            std::string n = from.str(ctx);
            n.erase(std::remove(n.begin(), n.end(), '['), n.end());
            n.erase(std::remove(n.begin(), n.end(), ']'), n.end());
            ci->renamePort(from, ctx->id(n));
        }
    }
    if (n16 || n8)
        log_info("Packed %d RAMB16BWER and %d RAMB8BWER.\n", n16, n8);
}

NEXTPNR_NAMESPACE_END
