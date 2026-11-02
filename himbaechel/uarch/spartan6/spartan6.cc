/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch, built on the prjcombine database.
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

#include <regex>

#include "himbaechel_api.h"
#include "log.h"
#include "nextpnr.h"
#include "util.h"

#define GEN_INIT_CONSTIDS
#define HIMBAECHEL_CONSTIDS "uarch/spartan6/constids.inc"
#include "himbaechel_constids.h"

#include "spartan6.h"

NEXTPNR_NAMESPACE_BEGIN

po::options_description Spartan6Impl::getUArchOptions()
{
    po::options_description specific("Spartan-6 specific options");
    specific.add_options()("ucf", po::value<std::string>(), "UCF constraints file (LOC, IOSTANDARD, ...)");
    specific.add_options()("nolut5", "don't pair LUTs into shared LUT6/LUT5 sites");
    return specific;
}

void Spartan6Impl::init_database(Arch *arch)
{
    const ArchArgs &args = arch->args;
    init_uarch_constids(arch);
    // Accept xc6slx9-tqg144, xc6slx9-2tqg144 and xc6slx9-2tqg144c.
    std::smatch m;
    static const std::regex devre("(xc6slx\\d+)-(\\d?)([a-z]+\\d+)[cil]?");
    if (!std::regex_match(args.device, m, devre))
        log_error("Invalid Spartan-6 device '%s' (expected e.g. xc6slx9-2tqg144)\n", args.device.c_str());
    std::string die = m[1].str();
    if (die != "xc6slx9")
        log_error("Only xc6slx9 is supported so far (got %s)\n", die.c_str());
    arch->load_chipdb(stringf("spartan6/chipdb-%s.bin", die.c_str()));
    std::string pkg = m[3].str();
    for (auto &c : pkg)
        c = toupper(c);
    arch->set_package(pkg);
}

void Spartan6Impl::init(Context *ctx)
{
    h.init(ctx);
    HimbaechelAPI::init(ctx);
    // Only bonded IOB sites can hold an I/O.
    for (const auto &pad : ctx->package_info->pads) {
        BelId bel = ctx->getBelByName(IdStringList::concat(IdString(pad.tile), IdString(pad.bel)));
        NPNR_ASSERT(bel != BelId());
        bonded_iobs.insert(bel);
    }
    init_carry_next();
}

void Spartan6Impl::pack()
{
    const ArchArgs &args = ctx->args;
    if (args.options.count("ucf"))
        read_ucf(args.options.at("ucf").as<std::string>());
    pack_io();
    h.replace_constants(CellTypePort(id_PSEUDO_VCC, id_Y), CellTypePort(id_PSEUDO_GND, id_Y), {}, {}, id_VCC,
                        id_GND);
    pack_cmt();
    pack_clocking();
    pack_bram();
    pack_dsp();
    pack_lutram();
    pack_logic();
    pack_carries();
    pack_wide();
    pack_lut_pairs();
}

void Spartan6Impl::prePlace()
{
    ff_info.assign(ctx->cells.size(), FFInfo());
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (!ci->type.in(id_SLICE_FF, id_SLICE_FF5))
            continue;
        auto &fi = ff_info.at(ci->flat_index);
        fi.clk = ci->getPort(id_CK);
        fi.ce = ci->getPort(id_CE);
        fi.sr = ci->getPort(id_SR);
        IdString kind = ctx->id(ci->params.at(id_FF_TYPE).as_string());
        fi.async = kind.in(id_FDCE, id_FDPE);
        fi.latch = kind.in(id_LDCE, id_LDPE);
    }
}

// The LUT5 half of a fractured LUT6 (see pack_lut_pairs) has its own cell
// type so that only paired LUTs land on LUT5 bels.
BelBucketId Spartan6Impl::getBelBucketForCellType(IdString cell_type) const
{
    if (cell_type == id_SLICE_LUT5)
        return id_SLICE_LUT;
    if (cell_type == id_SLICE_FF5)
        return id_SLICE_FF;
    if (cell_type == id_SLICE_LUTRAM)
        return id_SLICE_LUT;
    return cell_type;
}

bool Spartan6Impl::isValidBelForCellType(IdString cell_type, BelId bel) const
{
    IdString bel_type = ctx->getBelType(bel);
    int z = ctx->getBelLocation(bel).z;
    if (cell_type == id_SLICE_LUT5)
        return bel_type == id_SLICE_LUT && s6z::kind(z) == s6z::LUT5;
    if (cell_type == id_SLICE_FF5)
        return bel_type == id_SLICE_FF && s6z::kind(z) == s6z::FF5;
    // RAM/SRL LUTs: SLICEM LUT6 bels (the ones with a WE pin).
    if (cell_type == id_SLICE_LUTRAM)
        return bel_type == id_SLICE_LUT && s6z::kind(z) == s6z::LUT6 && ctx->getBelPinWire(bel, id_WE) != WireId();
    if (bel_type != cell_type)
        return false;
    if (bel_type == id_IOB)
        return bonded_iobs.count(bel);
    // LUT5 and 5FF bels only take cells clustered there by pack_lut_pairs
    // (types SLICE_LUT5 / SLICE_FF5).
    if (bel_type == id_SLICE_LUT && s6z::kind(z) == s6z::LUT5)
        return false;
    if (bel_type == id_SLICE_FF && s6z::kind(z) == s6z::FF5)
        return false;
    return true;
}

bool Spartan6Impl::isBelLocationValid(BelId bel, bool explain_invalid) const
{
    IdString type = ctx->getBelType(bel);
    if (!type.in(id_SLICE_FF, id_SLICE_LUT, id_CARRY4, id_SLICE_F7MUX, id_SLICE_F8MUX))
        return true;
    Loc l = ctx->getBelLocation(bel);
    int base = s6z::slice_base(l.z);
    return slice_controls_ok(l.x, l.y, base, explain_invalid) && slice_bypass_ok(l.x, l.y, base, explain_invalid);
}

const CellInfo *Spartan6Impl::cell_at(int x, int y, int z) const
{
    BelId b = ctx->getBelByLocation(Loc(x, y, z));
    return b == BelId() ? nullptr : ctx->getBoundBelCell(b);
}

// All storage elements in a slice share CLK, CE and SR, and the slice-wide
// FF_LATCH / FF_SR_SYNC settings.
bool Spartan6Impl::slice_controls_ok(int x, int y, int base, bool explain_invalid) const
{
    // RAM/SRL LUTs share the slice's WE and clock.
    const NetInfo *ram_clk = nullptr, *ram_we = nullptr;
    bool have_ram = false;
    for (int letter = 0; letter < 4; letter++) {
        const CellInfo *ci = cell_at(x, y, base + letter * 8 + s6z::LUT6);
        if (!ci || ci->type != id_SLICE_LUTRAM)
            continue;
        if (have_ram && (ci->getPort(id_CLK) != ram_clk || ci->getPort(id_WE) != ram_we)) {
            if (explain_invalid)
                log_nonfatal_error("slice at X%dY%d: RAM/SRL LUTs with different clock or WE\n", x, y);
            return false;
        }
        have_ram = true;
        ram_clk = ci->getPort(id_CLK);
        ram_we = ci->getPort(id_WE);
    }
    const FFInfo *ref = nullptr;
    for (int letter = 0; letter < 4; letter++) {
        for (int k : {s6z::FF, s6z::FF5}) {
            const CellInfo *ci = cell_at(x, y, base + letter * 8 + k);
            if (!ci)
                continue;
            const FFInfo &fi = ff_info.at(ci->flat_index);
            if (have_ram && fi.clk != ram_clk) {
                if (explain_invalid)
                    log_nonfatal_error("slice at X%dY%d: FF and RAM/SRL clocks differ\n", x, y);
                return false;
            }
            if (!ref) {
                ref = &fi;
                continue;
            }
            if (fi.clk != ref->clk || fi.ce != ref->ce || fi.sr != ref->sr || fi.async != ref->async ||
                fi.latch != ref->latch) {
                if (explain_invalid)
                    log_nonfatal_error("slice at X%dY%d: FF %s has a different control set\n", x, y, ctx->nameOf(ci));
                return false;
            }
        }
    }
    return true;
}

// Each letter's bypass pin (AX..DX) carries one net. Its users: the main FF's
// D unless it comes from inside the letter (its LUTs, or the carry XOR/CY),
// carry DI unless fed from the letter's LUT5, and (letter A) a chain's
// initial carry-in from a signal (PRECYINIT = AX).
bool Spartan6Impl::slice_bypass_ok(int x, int y, int base, bool explain_invalid) const
{
    static const IdString di_ports[4] = {id_DI0, id_DI1, id_DI2, id_DI3};
    static const IdString o_ports[4] = {id_O0, id_O1, id_O2, id_O3};
    static const IdString co_ports[4] = {id_CO0, id_CO1, id_CO2, id_CO3};
    const CellInfo *carry = cell_at(x, y, base + s6z::CARRY4);
    if (carry && carry->type != id_CARRY4)
        carry = nullptr;
    // Wide mux feeding each letter's FF/<L>MUX (A: F7AMUX, B: F8MUX, C: F7BMUX)
    // and taking its select from that letter's bypass pin.
    const CellInfo *wide[4] = {cell_at(x, y, base + s6z::F7AMUX), cell_at(x, y, base + s6z::F8MUX),
                               cell_at(x, y, base + s6z::F7BMUX), nullptr};
    const CellInfo *f8 = wide[1];
    for (int letter = 0; letter < 4; letter++) {
        int zl = base + letter * 8;
        const NetInfo *want = nullptr;
        bool conflict = false;
        auto need = [&](const NetInfo *n) {
            if (want && want != n)
                conflict = true;
            want = n;
        };
        auto from = [&](const NetInfo *n, int z) {
            const CellInfo *c = cell_at(x, y, z);
            return c && n->driver.cell == c;
        };
        if (const CellInfo *ff = cell_at(x, y, zl + s6z::FF)) {
            const NetInfo *d = ff->getPort(id_D);
            bool internal = d && (from(d, zl + s6z::LUT6) || from(d, zl + s6z::LUT5) ||
                                  (carry && d->driver.cell == carry &&
                                   d->driver.port.in(o_ports[letter], co_ports[letter])) ||
                                  (wide[letter] && d->driver.cell == wide[letter]));
            if (d && !internal)
                need(d);
        }
        if (wide[letter]) {
            if (const NetInfo *sel = wide[letter]->getPort(id_S))
                need(sel);
        }
        // RAM/SRL data in (DI1) through the bypass pin.
        const CellInfo *lr = cell_at(x, y, zl + s6z::LUT6);
        if (lr && lr->type == id_SLICE_LUTRAM)
            if (const NetInfo *di = lr->getPort(id_DI1))
                need(di);
        if (carry) {
            const NetInfo *di = carry->getPort(di_ports[letter]);
            if (di && !from(di, zl + s6z::LUT5))
                need(di);
            if (letter == 0) {
                const NetInfo *ci = carry->getPort(id_CI);
                if (ci && ci->constant_value == IdString() && !(ci->driver.cell && ci->driver.cell->type == id_CARRY4))
                    need(ci);
            }
        }
        if (conflict) {
            if (explain_invalid)
                log_nonfatal_error("slice at X%dY%d: bypass pin %cX needed by two nets\n", x, y, 'A' + letter);
            return false;
        }

        // <L>MUX output pin: the only way out for O5, the 5FF's Q and the
        // carry XOR/CY outputs (CO3 also reaches the next CIN directly).
        want = nullptr;
        const CellInfo *ff = cell_at(x, y, zl + s6z::FF);
        auto leaves = [&](const NetInfo *n, bool to_cin) {
            if (!n)
                return false;
            for (auto &usr : n->users) {
                if (ff && usr.cell == ff && usr.port == id_D)
                    continue; // through the FF's D mux
                if (to_cin && usr.cell->type == id_CARRY4 && usr.port == id_CI)
                    continue;
                if (carry && usr.cell == carry && usr.port == di_ports[letter])
                    continue; // O5 -> MUX_<L>CY0
                const CellInfo *ff5 = cell_at(x, y, zl + s6z::FF5);
                if (ff5 && usr.cell == ff5)
                    continue; // O5 -> 5FF D
                if (f8 && usr.cell == f8 && usr.port.in(id_I0, id_I1))
                    continue; // F7 -> F8 inside the slice
                return true;
            }
            return false;
        };
        if (const CellInfo *l5 = cell_at(x, y, zl + s6z::LUT5)) {
            const NetInfo *o = l5->getPort(id_O);
            if (leaves(o, false))
                need(o);
        }
        if (const CellInfo *lr6 = cell_at(x, y, zl + s6z::LUT6); lr6 && lr6->type == id_SLICE_LUTRAM)
            if (const NetInfo *o5 = lr6->getPort(id_O5); o5 && o5->users.entries())
                need(o5);
        if (const CellInfo *ff5 = cell_at(x, y, zl + s6z::FF5)) {
            const NetInfo *q = ff5->getPort(id_Q);
            if (q && q->users.entries())
                need(q);
        }
        if (wide[letter]) {
            const NetInfo *o = wide[letter]->getPort(id_O);
            if (leaves(o, false))
                need(o);
        }
        if (carry) {
            const NetInfo *o = carry->getPort(o_ports[letter]);
            if (leaves(o, false))
                need(o);
            const NetInfo *co = carry->getPort(co_ports[letter]);
            if (leaves(co, letter == 3))
                need(co);
        }
        if (conflict) {
            if (explain_invalid)
                log_nonfatal_error("slice at X%dY%d: output pin %cMUX needed by two nets\n", x, y, 'A' + letter);
            return false;
        }
    }
    return true;
}

namespace {
struct Spartan6Arch : HimbaechelArch
{
    Spartan6Arch() : HimbaechelArch("spartan6") {};
    bool match_device(const std::string &device) override { return device.rfind("xc6s", 0) == 0; }
    std::unique_ptr<HimbaechelAPI> create(const std::string &device) override
    {
        return std::make_unique<Spartan6Impl>();
    }
} spartan6Arch;
} // namespace

NEXTPNR_NAMESPACE_END
