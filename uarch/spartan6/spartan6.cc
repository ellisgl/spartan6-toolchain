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
}

void Spartan6Impl::pack()
{
    const ArchArgs &args = ctx->args;
    if (args.options.count("ucf"))
        read_ucf(args.options.at("ucf").as<std::string>());
    pack_io();
    h.replace_constants(CellTypePort(id_PSEUDO_VCC, id_Y), CellTypePort(id_PSEUDO_GND, id_Y), {}, {}, id_VCC,
                        id_GND);
    pack_clocking();
    pack_logic();
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
    if (ctx->getBelType(bel) != id_SLICE_FF)
        return true;
    // All storage elements in a slice share CLK, CE and SR, and the slice-wide
    // FF_LATCH / FF_SR_SYNC settings.
    Loc l = ctx->getBelLocation(bel);
    int base = s6z::slice_base(l.z);
    const FFInfo *ref = nullptr;
    for (int letter = 0; letter < 4; letter++) {
        for (int k : {s6z::FF, s6z::FF5}) {
            BelId ff_bel = ctx->getBelByLocation(Loc(l.x, l.y, base + letter * 8 + k));
            if (ff_bel == BelId())
                continue;
            const CellInfo *ci = ctx->getBoundBelCell(ff_bel);
            if (!ci)
                continue;
            const FFInfo &fi = ff_info.at(ci->flat_index);
            if (!ref) {
                ref = &fi;
                continue;
            }
            if (fi.clk != ref->clk || fi.ce != ref->ce || fi.sr != ref->sr || fi.async != ref->async ||
                fi.latch != ref->latch) {
                if (explain_invalid)
                    log_nonfatal_error("slice at X%dY%d: FF %s has a different control set\n", l.x, l.y,
                                       ctx->nameOf(ci));
                return false;
            }
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
