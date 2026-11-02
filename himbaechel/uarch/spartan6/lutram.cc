/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: distributed RAM and shift registers (SLICEM).
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

// SLICEM LUTs as RAM or shift registers (cell type SLICE_LUTRAM, only on
// SLICEM LUT6 bels, which have the extra pins). In a SLICEM all four LUTs
// take their write address from the D LUT's input pins (WA = D1..D6), share
// WE (WE or CE pin) and the slice clock. So:
//   RAM64X1S: up to four with the same address, clock and WE share a slice;
//             one of them is always the D LUT (its read address is the
//             write address). RAMMODE RAM64.
//   RAM32M:   the whole slice; per letter bit 0 is the DI1 -> O5 half and
//             bit 1 the DI2 -> O6 half (A6 tied high). RAMMODE RAM32.
//   RAM128X1S: D LUT (lower 64) and C LUT (upper 64), both written from DX
//             (C through CDI1MUX = DX), A6 on CX (WA7 and the F7BMUX select),
//             read through F7BMUX (out on CMUX), as ISE's map does.
//   RAM64M:   the whole slice; per letter one 64-deep bit, DI1 -> O6.
//             RAMMODE RAM64. (Both: letter x reads at ADDRx, all write at
//             ADDRD, which is also the D LUT's read address.)
//   SRLC32E:  any SLICEM LUT6; address on A2..A6 (A1 tied high), D on DI1,
//             CE on WE. RAMMODE SRL32. Cascades (Q31) are not supported yet.
// An inverted clock (IS_*CLK_INVERTED) becomes CLK_INV = 1 on the LUT cells;
// the slice inverts its clock for all of them and its FFs (CLKINV).
//   SRL16E:   address on A2..A5 (A1, A6 tied high), D on DI2, CE on WE.
//             RAMMODE SRL16.

namespace {
const int Z_SLICEM = s6z::BASE; // SLICE[0]
int lut6_z(int letter) { return Z_SLICEM + letter * 8 + s6z::LUT6; }

void move_port(CellInfo *ci, IdString from, IdString to)
{
    if (ci->ports.count(from))
        ci->renamePort(from, to);
}

void add_in(CellInfo *ci, IdString port, NetInfo *net)
{
    ci->ports[port].name = port;
    ci->ports[port].type = PORT_IN;
    if (net)
        ci->connectPort(port, net);
}

std::string init_bits(const CellInfo *ci, IdString p, int n)
{
    std::string s(n, '0');
    if (ci->params.count(p)) {
        const Property &v = ci->params.at(p);
        for (int i = 0; i < n && i < int(v.str.size()); i++)
            s[n - 1 - i] = v.str[i] == Property::S1 ? '1' : '0';
    }
    return s;
}
} // namespace

void Spartan6Impl::pack_lutram()
{
    NetInfo *vcc = ctx->nets.at(ctx->id("$PACKER_VCC")).get();
    int n64 = 0, n32m = 0, n64m = 0, n128 = 0, nsrl = 0;
    std::vector<CellInfo *> ram64, ram32m, ram128, srl;
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type == id_RAM64X1S)
            ram64.push_back(ci);
        else if (ci->type.in(id_RAM32M, id_RAM64M))
            ram32m.push_back(ci);
        else if (ci->type == id_RAM128X1S)
            ram128.push_back(ci);
        else if (ci->type.in(id_SRLC32E, id_SRL16E))
            srl.push_back(ci);
        else if (ci->type.in(id_RAM32X1S, id_RAM32X1D, id_RAM64X1D, id_RAM128X1D))
            log_error("Cell %s: %s is not supported yet\n", ctx->nameOf(ci), ci->type.c_str(ctx));
    }

    // Inverted write clock (the slice's CLKINV): CLK_INV on the LUT cells.
    auto clk_inv = [&](const CellInfo *ci, IdString param) {
        return ci->params.count(param) && ci->params.at(param).as_int64() != 0;
    };
    auto to_lutram = [&](CellInfo *ci, const char *mode) {
        ci->type = id_SLICE_LUTRAM;
        ci->params[id_RAM_MODE] = std::string(mode);
    };

    // RAM64X1S: group by address, clock and WE.
    dict<std::string, std::vector<CellInfo *>> groups;
    for (CellInfo *ci : ram64) {
        std::string key = clk_inv(ci, id_IS_WCLK_INVERTED) ? "inv|" : "|";
        for (IdString p : {id_A0, id_A1, id_A2, id_A3, id_A4, id_A5, id_WCLK, id_WE}) {
            NetInfo *n = ci->getPort(p);
            key += n ? n->name.str(ctx) : "-";
            key += "|";
        }
        groups[key].push_back(ci);
    }
    for (auto &g : groups) {
        auto &cells = g.second;
        for (size_t base = 0; base < cells.size(); base += 4) {
            CellInfo *root = nullptr;
            for (size_t k = 0; k < 4 && base + k < cells.size(); k++) {
                CellInfo *ci = cells[base + k];
                static const IdString a[6] = {id_A0, id_A1, id_A2, id_A3, id_A4, id_A5};
                static const IdString la[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
                static const IdString wa[6] = {id_WA1, id_WA2, id_WA3, id_WA4, id_WA5, id_WA6};
                std::vector<NetInfo *> addr;
                for (int i = 0; i < 6; i++)
                    addr.push_back(ci->getPort(a[i]));
                for (int i = 0; i < 6; i++)
                    ci->disconnectPort(a[i]);
                for (int i = 0; i < 6; i++)
                    ci->ports.erase(a[i]);
                for (int i = 0; i < 6; i++) {
                    add_in(ci, la[i], addr[i]);
                    add_in(ci, wa[i], addr[i]);
                }
                move_port(ci, id_D, id_DI1);
                move_port(ci, id_WCLK, id_CLK);
                if (clk_inv(ci, id_IS_WCLK_INVERTED))
                    ci->params[id_CLK_INV] = Property(1, 1);
                ci->params.erase(id_IS_WCLK_INVERTED);
                to_lutram(ci, "RAM64");
                int letter = 3 - int(k); // D first, then C, B, A
                if (!root) {
                    root = ci;
                    ci->cluster = ci->name;
                    ci->constr_abs_z = true;
                    ci->constr_z = lut6_z(letter);
                } else {
                    ci->cluster = root->name;
                    root->constr_children.push_back(ci);
                    ci->constr_x = 0;
                    ci->constr_y = 0;
                    ci->constr_abs_z = true;
                    ci->constr_z = lut6_z(letter);
                }
                n64++;
            }
        }
    }

    // RAM32M / RAM64M: four new LUT cells (D first), then the original goes.
    for (CellInfo *ci : ram32m) {
        bool deep = ci->type == id_RAM64M;
        int abits = deep ? 6 : 5;
        bool inv = clk_inv(ci, id_IS_WCLK_INVERTED);
        NetInfo *we = ci->getPort(id_WE), *wclk = ci->getPort(id_WCLK);
        std::vector<NetInfo *> waddr;
        for (int i = 0; i < abits; i++)
            waddr.push_back(ci->getPort(ctx->idf("ADDRD[%d]", i)));
        static const IdString la[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
        static const IdString wa[6] = {id_WA1, id_WA2, id_WA3, id_WA4, id_WA5, id_WA6};
        CellInfo *root = nullptr;
        for (int letter = 3; letter >= 0; letter--) {
            char x = "ABCD"[letter];
            CellInfo *lut = ctx->createCell(ctx->idf("%s$%c", ci->name.c_str(ctx), x), id_SLICE_LUTRAM);
            for (int i = 0; i < abits; i++) {
                add_in(lut, la[i], ci->getPort(ctx->idf("ADDR%c[%d]", x, i)));
                add_in(lut, wa[i], waddr[i]);
            }
            add_in(lut, id_WE, we);
            add_in(lut, id_CLK, wclk);
            if (inv)
                lut->params[id_CLK_INV] = Property(1, 1);
            if (deep) {
                add_in(lut, id_DI1, ci->getPort(ctx->idf("DI%c", x)));
                IdString from = ctx->idf("DO%c", x);
                NetInfo *n = ci->getPort(from);
                lut->addOutput(id_O);
                if (n) {
                    ci->disconnectPort(from);
                    lut->connectPort(id_O, n);
                }
                lut->params[id_INIT] = Property::from_string(init_bits(ci, ctx->idf("INIT_%c", x), 64));
                lut->params[id_RAM_MODE] = std::string("RAM64");
            } else {
                add_in(lut, id_A6, vcc);
                add_in(lut, id_WA6, vcc);
                add_in(lut, id_DI1, ci->getPort(ctx->idf("DI%c[0]", x)));
                add_in(lut, id_DI2, ci->getPort(ctx->idf("DI%c[1]", x)));
                for (auto &o : {std::make_pair(0, id_O5), std::make_pair(1, id_O)}) {
                    IdString from = ctx->idf("DO%c[%d]", x, o.first);
                    NetInfo *n = ci->getPort(from);
                    lut->addOutput(o.second);
                    if (n) {
                        ci->disconnectPort(from);
                        lut->connectPort(o.second, n);
                    }
                }
                // INIT_x interleaves the halves: bit 2i -> O5 (lower half of the
                // LUT table), bit 2i+1 -> O6 (upper half).
                std::string init = init_bits(ci, ctx->idf("INIT_%c", x), 64); // MSB first
                std::string lut_init(64, '0');
                for (int i = 0; i < 32; i++) {
                    lut_init[63 - i] = init[63 - 2 * i];
                    lut_init[63 - (32 + i)] = init[63 - (2 * i + 1)];
                }
                lut->params[id_INIT] = Property::from_string(lut_init);
                lut->params[id_RAM_MODE] = std::string("RAM32");
            }
            if (!root) {
                root = lut;
                lut->cluster = lut->name;
                lut->constr_abs_z = true;
                lut->constr_z = lut6_z(letter);
            } else {
                lut->cluster = root->name;
                root->constr_children.push_back(lut);
                lut->constr_x = 0;
                lut->constr_y = 0;
                lut->constr_abs_z = true;
                lut->constr_z = lut6_z(letter);
            }
        }
        std::vector<IdString> ports;
        for (auto &p : ci->ports)
            ports.push_back(p.first);
        for (IdString p : ports)
            ci->disconnectPort(p);
        ctx->cells.erase(ci->name);
        (deep ? n64m : n32m)++;
    }

    // RAM128X1S: two RAM64 LUTs (D low, C high) and the F7BMUX, one cluster.
    for (CellInfo *ci : ram128) {
        bool inv = clk_inv(ci, id_IS_WCLK_INVERTED);
        static const IdString la[6] = {id_A1, id_A2, id_A3, id_A4, id_A5, id_A6};
        static const IdString wa[6] = {id_WA1, id_WA2, id_WA3, id_WA4, id_WA5, id_WA6};
        std::vector<NetInfo *> addr;
        for (int i = 0; i < 7; i++)
            addr.push_back(ci->getPort(ctx->idf("A%d", i)));
        NetInfo *d = ci->getPort(id_D), *we = ci->getPort(id_WE), *wclk = ci->getPort(id_WCLK);
        std::string init = init_bits(ci, id_INIT, 128); // MSB first
        CellInfo *root = nullptr;
        NetInfo *half[2];
        for (int h = 0; h < 2; h++) { // 0: low half (D LUT), 1: high half (C LUT)
            int letter = h ? 2 : 3;
            CellInfo *lut = ctx->createCell(ctx->idf("%s$%s", ci->name.c_str(ctx), h ? "HIGH" : "LOW"), id_SLICE_LUTRAM);
            for (int i = 0; i < 6; i++) {
                add_in(lut, la[i], addr[i]);
                add_in(lut, wa[i], addr[i]);
            }
            add_in(lut, id_WA7, addr[6]);
            add_in(lut, id_DI1, d);
            add_in(lut, id_WE, we);
            add_in(lut, id_CLK, wclk);
            if (inv)
                lut->params[id_CLK_INV] = Property(1, 1);
            half[h] = ctx->createNet(ctx->idf("%s$%s_O", ci->name.c_str(ctx), h ? "HIGH" : "LOW"));
            lut->addOutput(id_O);
            lut->connectPort(id_O, half[h]);
            lut->params[id_INIT] = Property::from_string(h ? init.substr(0, 64) : init.substr(64, 64));
            lut->params[id_RAM_MODE] = std::string("RAM64");
            lut->constr_abs_z = true;
            lut->constr_z = lut6_z(letter);
            if (!root) {
                root = lut;
                lut->cluster = lut->name;
            } else {
                lut->cluster = root->name;
                root->constr_children.push_back(lut);
                lut->constr_x = 0;
                lut->constr_y = 0;
            }
        }
        CellInfo *mux = ctx->createCell(ctx->idf("%s$F7", ci->name.c_str(ctx)), id_SLICE_F7MUX);
        add_in(mux, id_I0, half[0]);
        add_in(mux, id_I1, half[1]);
        add_in(mux, id_S, addr[6]);
        mux->addOutput(id_O);
        if (NetInfo *o = ci->getPort(id_O)) {
            ci->disconnectPort(id_O);
            mux->connectPort(id_O, o);
        }
        mux->cluster = root->name;
        root->constr_children.push_back(mux);
        mux->constr_x = 0;
        mux->constr_y = 0;
        mux->constr_abs_z = true;
        mux->constr_z = Z_SLICEM + s6z::F7BMUX;
        std::vector<IdString> ports;
        for (auto &p : ci->ports)
            ports.push_back(p.first);
        for (IdString p : ports)
            ci->disconnectPort(p);
        ctx->cells.erase(ci->name);
        n128++;
    }

    // SRLC32E / SRL16E: one LUT anywhere in a SLICEM.
    for (CellInfo *ci : srl) {
        bool srl16 = ci->type == id_SRL16E;
        if (clk_inv(ci, id_IS_CLK_INVERTED))
            ci->params[id_CLK_INV] = Property(1, 1);
        ci->params.erase(id_IS_CLK_INVERTED);
        if (srl16) {
            static const IdString a16[4] = {id_A0, id_A1, id_A2, id_A3};
            static const IdString la16[4] = {id_A2, id_A3, id_A4, id_A5};
            std::vector<NetInfo *> addr;
            for (int i = 0; i < 4; i++)
                addr.push_back(ci->getPort(a16[i]));
            for (int i = 0; i < 4; i++)
                if (ci->ports.count(a16[i])) {
                    ci->disconnectPort(a16[i]);
                    ci->ports.erase(a16[i]);
                }
            for (int i = 0; i < 4; i++)
                add_in(ci, la16[i], addr[i]);
            add_in(ci, id_A1, vcc);
            add_in(ci, id_A6, vcc);
            move_port(ci, id_D, id_DI2);
            move_port(ci, id_CE, id_WE);
            move_port(ci, id_Q, id_O);
            to_lutram(ci, "SRL16");
            nsrl++;
            continue;
        }
        NetInfo *q31 = ci->getPort(id_Q31);
        if (q31 && q31->users.entries())
            log_error("SRLC32E %s: cascades (Q31) are not supported yet\n", ctx->nameOf(ci));
        static const IdString a[5] = {ctx->id("A[0]"), ctx->id("A[1]"), ctx->id("A[2]"), ctx->id("A[3]"),
                                      ctx->id("A[4]")};
        static const IdString la[5] = {id_A2, id_A3, id_A4, id_A5, id_A6};
        std::vector<NetInfo *> addr;
        for (int i = 0; i < 5; i++)
            addr.push_back(ci->getPort(a[i]));
        for (int i = 0; i < 5; i++)
            if (ci->ports.count(a[i])) {
                ci->disconnectPort(a[i]);
                ci->ports.erase(a[i]);
            }
        for (int i = 0; i < 5; i++)
            add_in(ci, la[i], addr[i]);
        add_in(ci, id_A1, vcc);
        move_port(ci, id_D, id_DI1);
        move_port(ci, id_CE, id_WE);
        move_port(ci, id_Q, id_O);
        if (ci->ports.count(id_Q31)) {
            ci->disconnectPort(id_Q31);
            ci->ports.erase(id_Q31);
        }
        // INIT stays 32 bits; the writers place it in the LUT.
        to_lutram(ci, "SRL32");
        nsrl++;
    }
    // LUT address pins can be tied high (TIE_1) but some can't reach ground,
    // and a RAM / SRL address can't be folded into the table the way a
    // logic LUT's can. Constant-0 address bits (an SRL's fixed tap, say)
    // come from one shared LUT whose output is 0.
    NetInfo *lut_gnd = nullptr;
    int ngnd = 0;
    static const IdString addr_ports[] = {id_A1,  id_A2,  id_A3,  id_A4,  id_A5,  id_A6,
                                          id_WA1, id_WA2, id_WA3, id_WA4, id_WA5, id_WA6};
    std::vector<CellInfo *> lutram_cells;
    for (auto &cell : ctx->cells)
        if (cell.second->type == id_SLICE_LUTRAM)
            lutram_cells.push_back(cell.second.get());
    for (CellInfo *ci : lutram_cells)
        for (IdString p : addr_ports) {
            NetInfo *n = ci->getPort(p);
            if (!n || n->constant_value != id_GND)
                continue;
            if (!lut_gnd) {
                lut_gnd = ctx->createNet(ctx->id("$lutram_gnd"));
                CellInfo *drv = ctx->createCell(ctx->id("$lutram_gnd_lut"), id_LUT1);
                drv->params[id_INIT] = Property(0, 2);
                drv->addOutput(id_O);
                drv->connectPort(id_O, lut_gnd);
            }
            ci->disconnectPort(p);
            ci->connectPort(p, lut_gnd);
            ngnd++;
        }
    // ISE requires a RAM / SRL LUT's data input to be connected; yosys can
    // leave one undriven (an unused bit of a RAM64M, say).
    for (CellInfo *ci : lutram_cells) {
        const std::string &mode = ci->params.at(id_RAM_MODE).as_string();
        std::vector<IdString> di;
        if (mode == "RAM64" || mode == "SRL32")
            di = {id_DI1};
        else if (mode == "RAM32")
            di = {id_DI1, id_DI2};
        else
            di = {id_DI2};
        for (IdString p : di) {
            NetInfo *n = ci->getPort(p);
            if (n && n->driver.cell)
                continue;
            if (!lut_gnd) {
                lut_gnd = ctx->createNet(ctx->id("$lutram_gnd"));
                CellInfo *drv = ctx->createCell(ctx->id("$lutram_gnd_lut"), id_LUT1);
                drv->params[id_INIT] = Property(0, 2);
                drv->addOutput(id_O);
                drv->connectPort(id_O, lut_gnd);
            }
            if (n)
                ci->disconnectPort(p);
            add_in(ci, p, lut_gnd);
            ngnd++;
        }
    }
    if (ngnd)
        log_info("%d constant-0 RAM/SRL address or undriven data pins driven from a LUT.\n", ngnd);
    if (n64 || n32m || n64m || n128 || nsrl)
        log_info("Packed %d RAM64X1S, %d RAM32M, %d RAM64M, %d RAM128X1S and %d SRLs into SLICEM LUTs.\n", n64, n32m,
                 n64m, n128, nsrl);
}

NEXTPNR_NAMESPACE_END
