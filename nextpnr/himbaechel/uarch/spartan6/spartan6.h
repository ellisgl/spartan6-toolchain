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

// LUT helpers shared by LUT pairing (pack.cc) and carry packing (carry.cc).
namespace s6lut {
// Distinct input nets of a LUT in pin order; false if it has 6 or a table
// input is unconnected.
bool inputs(CellInfo *ci, std::vector<NetInfo *> &nets);
// Rewrite a LUT as a 5-input table over `merged` (connected to A1..A5).
void remap(Context *ctx, CellInfo *ci, const std::vector<NetInfo *> &merged);
} // namespace s6lut

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
    const CellInfo *cell_at(int x, int y, int z) const;
    bool slice_controls_ok(int x, int y, int base, bool explain_invalid) const;
    bool slice_bypass_ok(int x, int y, int base, bool explain_invalid) const;
    bool getClusterPlacement(ClusterId cluster, BelId root_bel,
                             std::vector<std::pair<CellInfo *, BelId>> &placement) const override;

    std::string getDefaultRouter() const override { return "router2"; }

    // Timing: LUT inputs the table doesn't depend on (shared LUT6 sites wire
    // both halves to the merged inputs) have no arc.
    bool getCellDelay(const CellInfo *cell, IdString fromPort, IdString toPort, DelayQuad &delay) const override;
    TimingPortClass getPortTimingClass(const CellInfo *cell, IdString port, int &clockInfoCount) const override;
    std::vector<uint8_t> lut_used_inputs; // per cell flat index: bit i = A(i+1) matters
    bool lut_input_unused(const CellInfo *cell, IdString port) const;
    // CLK_INV = 1: the cell's slice clock is inverted (falling edge).
    bool clk_inverted(const CellInfo *cell) const;
    // Pip flag 1 (chip database): the C LUT's DI1 from DX, allowed only when
    // that LUT is a RAM (ISE rejects it for SRLs).
    bool checkPipAvail(PipId pip) const override;
    // Pip flag 2: IMUX_CLK from general routing; not for global clock nets
    // (driven by a BUFGMUX), which must reach clock pins through HCLK.
    bool checkPipAvailForNet(PipId pip, const NetInfo *net) const override;
    void absorb_clock_inverters();
    void pack_oddr();
    void pack_iddr();
    bool bufio2fb_reaches_pll(CellInfo *fb, BelId bel);
    bool bufg_drives_data(const CellInfo *bufg) const;
    TimingClockingInfo getPortClockingInfo(const CellInfo *cell, IdString port, int index) const override;
    // DSP timing is stored per bus on bit 0 (A0 -> P0); M is looked up as P.
    IdString dsp_timing_pin(IdString port) const;

    // Flow helpers (pack.cc / ucf.cc)
    void read_ucf(const std::string &filename);
    void set_clock(NetInfo *net, double period_ns, double high);
    void propagate_clock_constraints();
    void name_clock_nets();
    // IOB cell -> the top-level port it was packed from (pack_io).
    dict<IdString, IdString> iob_port;
    void pack_io();
    void pack_clocking();
    void pack_logic();
    void pack_lut_pairs();
    void pack_ff5_passthrough();
    void pack_carries();
    void pack_wide();
    void pack_bram();
    void pack_dsp();
    void pack_lutram();
    void init_carry_next();
    void pack_cmt();
    void place_clock_tree(WireId src, NetInfo *net, const std::string &from);
    pool<BelId> used_clock_bels;

    // Per-cell data cached for fast validity checks.
    struct FFInfo
    {
        const NetInfo *clk = nullptr, *ce = nullptr, *sr = nullptr;
        bool async = false, latch = false, clk_inv = false;
    };
    std::vector<FFInfo> ff_info;

    // Constraints read from UCF, keyed by top-level port name.
    dict<IdString, dict<IdString, Property>> ucf_attrs;
    pool<BelId> bonded_iobs;

    // Carry chains (carry.cc): CARRY4 bel -> the CARRY4 bel its CO3 feeds,
    // and each chain's members as (cell, step along the chain, absolute z).
    dict<BelId, BelId> carry_next;
    struct CarryMember
    {
        CellInfo *cell;
        int step;
        int z;
    };
    struct CarryChain
    {
        std::vector<CarryMember> members;
    };
    dict<ClusterId, CarryChain> carry_chains;

    // Lone MUXF7 clusters (wide.cc): the LUTs on its I1 and I0, which go to
    // LUTs A/B or C/D depending on which F7 mux the root lands on.
    struct F7Cluster
    {
        CellInfo *lut1, *lut0;
    };
    dict<ClusterId, F7Cluster> f7_clusters;
    bool place_f7_cluster(ClusterId cluster, BelId root_bel,
                          std::vector<std::pair<CellInfo *, BelId>> &placement) const;

    HimbaechelHelpers h;
};

NEXTPNR_NAMESPACE_END

#endif
