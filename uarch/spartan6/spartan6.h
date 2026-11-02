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

#ifndef HIMBAECHEL_SPARTAN6_H
#define HIMBAECHEL_SPARTAN6_H

#include "himbaechel_api.h"
#include "himbaechel_helpers.h"
#include "nextpnr.h"

NEXTPNR_NAMESPACE_BEGIN

// Slice sub-bel z layout. Must match exporter/src/slice.rs.
namespace s6z {
static constexpr int BASE = 0x100;
static constexpr int SLICE_STRIDE = 0x40;
static constexpr int LUT6 = 0, LUT5 = 1, FF = 2, FF5 = 3; // + letter * 8
static constexpr int F7AMUX = 0x20, F7BMUX = 0x21, F8MUX = 0x22, CARRY4 = 0x23;

inline bool is_slice(int z) { return z >= BASE && z < BASE + 2 * SLICE_STRIDE; }
inline int slice_base(int z) { return BASE + ((z - BASE) / SLICE_STRIDE) * SLICE_STRIDE; }
inline int kind(int z) { return (z - BASE) % 8; } // only meaningful below F7AMUX
inline int letter(int z) { return ((z - BASE) % SLICE_STRIDE) / 8; }
} // namespace s6z

struct Spartan6Impl : HimbaechelAPI
{
    ~Spartan6Impl() {};
    po::options_description getUArchOptions() override;
    void init_database(Arch *arch) override;
    void init(Context *ctx) override;

    void pack() override;
    void prePlace() override;

    bool isValidBelForCellType(IdString cell_type, BelId bel) const override;
    BelBucketId getBelBucketForCellType(IdString cell_type) const override;
    bool isBelLocationValid(BelId bel, bool explain_invalid = false) const override;

    std::string getDefaultRouter() const override { return "router2"; }

    // Flow helpers (pack.cc / ucf.cc)
    void read_ucf(const std::string &filename);
    void pack_io();
    void pack_clocking();
    void pack_logic();
    void pack_lut_pairs();
    void place_bufgs_for_pad(CellInfo *iob);
    pool<BelId> used_bufgmux;

    // Per-cell data cached for fast validity checks.
    struct FFInfo
    {
        const NetInfo *clk = nullptr, *ce = nullptr, *sr = nullptr;
        bool async = false, latch = false;
    };
    std::vector<FFInfo> ff_info;

    // Constraints read from UCF, keyed by top-level port name.
    dict<IdString, dict<IdString, Property>> ucf_attrs;
    pool<BelId> bonded_iobs;

    HimbaechelHelpers h;
};

NEXTPNR_NAMESPACE_END

#endif
