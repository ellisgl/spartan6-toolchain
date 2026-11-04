/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: minimal UCF reader.
 *
 *  Supports NET "<net>" <KEY> = <value> [| <KEY> = <value> ...];
 *  with LOC, IOSTANDARD, DRIVE, SLEW, PULLUP and PULLDOWN on top-level ports,
 *  and clock periods, either on the net:
 *      NET "<net>" PERIOD = <time> [HIGH|LOW <duty>];
 *  or through a timing group:
 *      NET "<net>" TNM_NET = <group>;
 *      TIMESPEC <TS_name> = PERIOD "<group>" <time> [HIGH|LOW <duty>];
 *  Other statements (derived TIMESPECs, OFFSET, FROM/TO, INST constraints)
 *  are warned about and ignored.
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

#include <fstream>
#include <regex>
#include <sstream>

#include "log.h"
#include "nextpnr.h"
#include "util.h"

#define HIMBAECHEL_CONSTIDS "uarch/spartan6/constids.inc"
#include "himbaechel_constids.h"

#include "spartan6.h"

NEXTPNR_NAMESPACE_BEGIN

namespace {
std::string trim(const std::string &s)
{
    size_t b = s.find_first_not_of(" \t\r\n"), e = s.find_last_not_of(" \t\r\n");
    return b == std::string::npos ? "" : s.substr(b, e - b + 1);
}

std::string unquote(std::string s)
{
    s = trim(s);
    if (s.size() >= 2 && s.front() == '"' && s.back() == '"')
        s = s.substr(1, s.size() - 2);
    return s;
}

std::string upper(std::string s)
{
    for (auto &c : s)
        c = toupper(c);
    return s;
}

// A time (ps, ns, us, ms) or a frequency (Hz, kHz, MHz, GHz), in ns.
bool to_ns(double v, const std::string &unit, double &ns)
{
    static const dict<std::string, double> times{{"PS", 1e-3}, {"NS", 1}, {"US", 1e3}, {"MS", 1e6}};
    static const dict<std::string, double> freqs{{"HZ", 1e-9}, {"KHZ", 1e-6}, {"MHZ", 1e-3}, {"GHZ", 1}};
    std::string u = upper(unit);
    if (times.count(u))
        ns = v * times.at(u);
    else if (freqs.count(u) && v > 0)
        ns = 1.0 / (v * freqs.at(u));
    else
        return false;
    return ns > 0;
}

struct Period
{
    double ns = 0, high = 0.5; // period; fraction of it spent high
};

// "<value> <unit> [HIGH|LOW <duty> [%|<unit>]] [INPUT_JITTER ...]"
bool parse_period(const std::string &text, Period &out)
{
    static const std::regex re(R"(^([0-9]*\.?[0-9]+(?:[eE][-+]?[0-9]+)?)\s*([A-Za-z]+)(?:\s+(HIGH|LOW)\s+([0-9]*\.?[0-9]+)\s*(%|[A-Za-z]+)?)?(?:\s+INPUT_JITTER\b.*)?$)",
                               std::regex::icase);
    std::smatch m;
    if (!std::regex_match(text, m, re) || !to_ns(std::stod(m[1].str()), m[2].str(), out.ns))
        return false;
    out.high = 0.5;
    if (m[3].matched) {
        double v = std::stod(m[4].str()), frac;
        std::string unit = m[5].matched ? m[5].str() : "%";
        if (unit == "%") {
            frac = v / 100;
        } else {
            double ns;
            if (!to_ns(v, unit, ns))
                return false;
            frac = ns / out.ns;
        }
        if (frac <= 0 || frac >= 1)
            return false;
        out.high = upper(m[3].str()) == "HIGH" ? frac : 1 - frac;
    }
    return true;
}

// UCF bus syntax led<3> -> nextpnr bit name led[3].
std::string net_name(const std::string &ucf) { return std::regex_replace(unquote(ucf), std::regex("<(\\d+)>"), "[$1]"); }
} // namespace

void Spartan6Impl::set_clock(NetInfo *net, double period_ns, double high)
{
    auto cc = std::make_unique<ClockConstraint>();
    cc->period = DelayPair(ctx->getDelayFromNS(period_ns));
    cc->high = DelayPair(ctx->getDelayFromNS(period_ns * high));
    cc->low = DelayPair(ctx->getDelayFromNS(period_ns * (1 - high)));
    net->clkconstr = std::move(cc);
}

void Spartan6Impl::read_ucf(const std::string &filename)
{
    std::ifstream in(filename);
    if (!in)
        log_error("Failed to open UCF file '%s'\n", filename.c_str());

    // Strip comments, then split into ';'-terminated statements.
    std::stringstream text;
    std::string line;
    while (std::getline(in, line)) {
        auto hash = line.find('#');
        text << (hash == std::string::npos ? line : line.substr(0, hash)) << ' ';
    }
    static const std::regex net_re(R"(^NET\s+("[^"]+"|\S+)\s+(.*)$)", std::regex::icase);
    static const std::regex kv_re(R"(^([A-Za-z_]+)\s*(?:=\s*(.+))?$)");
    static const std::regex ts_re(R"(^TIMESPEC\s+("[^"]+"|\S+)\s*=\s*PERIOD\s+("[^"]+"|\S+)\s+(.*)$)",
                                  std::regex::icase);
    static const pool<std::string> known{"LOC", "IOSTANDARD", "DRIVE", "SLEW", "PULLUP", "PULLDOWN"};

    std::vector<std::pair<std::string, Period>> net_periods; // NET ... PERIOD
    dict<std::string, std::vector<std::string>> groups;      // TNM_NET group -> nets
    std::vector<std::tuple<std::string, std::string, Period>> timespecs; // TS, group, period

    std::string stmt;
    int n = 0;
    while (std::getline(text, stmt, ';')) {
        stmt = trim(stmt);
        if (stmt.empty())
            continue;
        std::smatch m;
        if (std::regex_match(stmt, m, ts_re)) {
            std::string ts = unquote(m[1].str()), group = unquote(m[2].str());
            Period p;
            if (!parse_period(trim(m[3].str()), p)) {
                log_warning("UCF: ignoring %s: cannot parse period '%s' (derived TIMESPECs are not supported)\n",
                            ts.c_str(), trim(m[3].str()).c_str());
                continue;
            }
            timespecs.emplace_back(ts, group, p);
            continue;
        }
        if (!std::regex_match(stmt, m, net_re)) {
            log_warning("UCF: ignoring unsupported statement: %s\n", stmt.c_str());
            continue;
        }
        std::string port = net_name(m[1].str());
        std::stringstream rest(m[2].str());
        std::string item;
        while (std::getline(rest, item, '|')) {
            item = trim(item);
            std::smatch kv;
            if (!std::regex_match(item, kv, kv_re)) {
                log_warning("UCF: cannot parse '%s' on NET %s\n", item.c_str(), port.c_str());
                continue;
            }
            std::string key = upper(kv[1].str());
            std::string value = kv[2].matched ? unquote(kv[2].str()) : "TRUE";
            if (key == "TNM_NET") {
                groups[value].push_back(port);
                continue;
            }
            if (key == "PERIOD") {
                Period p;
                if (!parse_period(value, p))
                    log_warning("UCF: cannot parse PERIOD '%s' on NET %s\n", value.c_str(), port.c_str());
                else
                    net_periods.emplace_back(port, p);
                continue;
            }
            if (!known.count(key)) {
                log_warning("UCF: ignoring %s on NET %s\n", key.c_str(), port.c_str());
                continue;
            }
            ucf_attrs[ctx->id(port)][ctx->id(key)] = Property(value);
            n++;
        }
    }

    for (auto &[ts, group, p] : timespecs) {
        if (!groups.count(group)) {
            log_warning("UCF: %s: no NET has TNM_NET = %s; ignoring it\n", ts.c_str(), group.c_str());
            continue;
        }
        for (auto &net : groups.at(group))
            net_periods.emplace_back(net, p);
    }
    for (auto &[name, p] : net_periods) {
        IdString id = ctx->id(name);
        NetInfo *net = nullptr;
        if (ctx->nets.count(id))
            net = ctx->nets.at(id).get();
        else if (ctx->net_aliases.count(id))
            net = ctx->getNetByAlias(id);
        else if (ctx->ports.count(id))
            net = ctx->ports.at(id).net;
        if (!net) {
            log_warning("UCF: clock net '%s' does not exist in the design; ignoring its period\n", name.c_str());
            continue;
        }
        set_clock(net, p.ns, p.high);
        log_info("    clock %s: period %.3f ns (%.2f MHz), %.0f%% high\n", name.c_str(), p.ns, 1000 / p.ns,
                 p.high * 100);
        n++;
    }
    log_info("Read %d constraints from UCF file '%s'.\n", n, filename.c_str());
}

// Carry clock periods from where the UCF puts them (usually the pad) through
// the clock buffers and clock managers to the nets the flip-flops see.
void Spartan6Impl::propagate_clock_constraints()
{
    auto param = [&](const CellInfo *ci, const char *name, double def) {
        auto it = ci->params.find(ctx->id(name));
        if (it == ci->params.end())
            return def;
        const Property &p = it->second;
        if (p.is_string) {
            try {
                return std::stod(p.as_string());
            } catch (...) {
                return def;
            }
        }
        return double(p.as_int64());
    };
    auto first_constrained = [&](const CellInfo *ci, std::initializer_list<IdString> ports) -> const NetInfo * {
        for (IdString p : ports) {
            const NetInfo *n = ci->getPort(p);
            if (n && n->clkconstr)
                return n;
        }
        return nullptr;
    };

    bool changed = true;
    while (changed) {
        changed = false;
        for (auto &cell : ctx->cells) {
            CellInfo *ci = cell.second.get();
            // The constrained input net, and (output port, period multiplier) pairs.
            std::vector<std::pair<IdString, double>> outs;
            const NetInfo *src = nullptr;
            if (ci->type == id_IOB) {
                src = first_constrained(ci, {id_I});
                outs = {{id_GCLK_I, 1}};
            } else if (ci->type == id_BUFGMUX) {
                src = first_constrained(ci, {id_I0, id_I1});
                outs = {{id_O, 1}};
            } else if (ci->type.in(id_BUFIO2, id_BUFIO2FB)) {
                src = first_constrained(ci, {id_I});
                double div = ci->type == id_BUFIO2 ? param(ci, "DIVIDE", 1) : 1;
                outs = {{id_DIVCLK, div}, {id_DIVCLK_CMT, 1}, {ctx->id("IOCLK"), 1}, {id_O, 1}};
            } else if (ci->type == id_DCM) {
                src = first_constrained(ci, {id_CLKIN});
                if (src) {
                    double in = (upper(ci->params.count(ctx->id("CLKIN_DIVIDE_BY_2"))
                                               ? ci->params.at(ctx->id("CLKIN_DIVIDE_BY_2")).as_string()
                                               : "FALSE") == "TRUE")
                                        ? 2
                                        : 1;
                    double fx = in * param(ci, "CLKFX_DIVIDE", 1) / param(ci, "CLKFX_MULTIPLY", 4);
                    double dv = in * param(ci, "CLKDV_DIVIDE", 2);
                    for (const char *p : {"CLK0", "CLK90", "CLK180", "CLK270"})
                        outs.emplace_back(ctx->id(p), in);
                    for (const char *p : {"CLK2X", "CLK2X180"})
                        outs.emplace_back(ctx->id(p), in / 2);
                    outs.emplace_back(ctx->id("CLKDV"), dv);
                    for (const char *p : {"CLKFX", "CLKFX180"})
                        outs.emplace_back(ctx->id(p), fx);
                }
            } else if (ci->type == id_PLL) {
                src = first_constrained(ci, {id_CLKIN1, id_CLKIN2});
                if (src) {
                    double vco = param(ci, "DIVCLK_DIVIDE", 1) / param(ci, "CLKFBOUT_MULT", 1);
                    outs.emplace_back(ctx->id("CLKFBOUT"), vco * param(ci, "CLKFBOUT_MULT", 1));
                    for (int i = 0; i < 6; i++)
                        outs.emplace_back(ctx->idf("CLKOUT%d", i),
                                          vco * param(ci, stringf("CLKOUT%d_DIVIDE", i).c_str(), 1));
                }
            }
            if (!src)
                continue;
            delay_t period = src->clkconstr->period.minDelay();
            double high = double(src->clkconstr->high.minDelay()) / period;
            for (auto &[port, mul] : outs) {
                NetInfo *o = ci->getPort(port);
                if (!o || o->clkconstr || o == src)
                    continue;
                // Buffers keep the duty cycle; clock managers make 50% clocks.
                bool buf = ci->type.in(id_IOB, id_BUFGMUX, id_BUFIO2, id_BUFIO2FB);
                double ns = ctx->getDelayNS(period) * mul;
                set_clock(o, ns, buf ? high : 0.5);
                if (!buf)
                    log_info("Derived clock %s.%s: period %.3f ns (%.2f MHz)\n", ctx->nameOf(ci), port.c_str(ctx), ns,
                             1000 / ns);
                changed = true;
            }
        }
    }
}

// Auto-named nets ($...) on clock paths get names from where they come from,
// so timing reports say 'clk$bufg' rather than '$abc$...$aiger$o100':
// <port>$ibuf after an input pad, <src>$bufg after a BUFG, <src>$bufio2
// after a BUFIO2 / BUFIO2FB, <src>$<pin> on DCM and PLL outputs, where
// <src> is the input net's name up to its first '$'. Named nets are kept.
void Spartan6Impl::name_clock_nets()
{
    auto root = [&](const NetInfo *n) -> std::string {
        if (!n)
            return "";
        std::string s = n->name.str(ctx);
        return s.empty() || s[0] == '$' ? "" : s.substr(0, s.find('$'));
    };
    auto rename = [&](NetInfo *n, const std::string &base) {
        if (!n || base.empty() || n->name.str(ctx)[0] != '$')
            return;
        std::string name = base;
        for (int i = 1; ctx->nets.count(ctx->id(name)); i++)
            name = stringf("%s%d", base.c_str(), i);
        ctx->renameNet(n->name, ctx->id(name));
    };
    // Pads first: the buffers' names build on theirs.
    for (auto &cell : ctx->cells) {
        CellInfo *ci = cell.second.get();
        if (ci->type != id_IOB || !iob_port.count(ci->name))
            continue;
        std::string port = iob_port.at(ci->name).str(ctx);
        rename(ci->getPort(id_I), port + "$ibuf");
        rename(ci->getPort(id_GCLK_I), port + "$ibuf");
    }
    // Buffers and clock managers, until nothing changes (chains of them).
    for (bool changed = true; changed;) {
        changed = false;
        for (auto &cell : ctx->cells) {
            CellInfo *ci = cell.second.get();
            std::vector<std::pair<IdString, std::string>> outs; // output port, suffix
            std::string src;
            if (ci->type == id_BUFGMUX) {
                src = root(ci->getPort(id_I0));
                if (src.empty())
                    src = root(ci->getPort(id_I1));
                outs = {{id_O, "$bufg"}};
            } else if (ci->type.in(id_BUFIO2, id_BUFIO2FB)) {
                src = root(ci->getPort(id_I));
                for (IdString p : {id_DIVCLK, id_DIVCLK_CMT, ctx->id("IOCLK"), id_O})
                    outs.emplace_back(p, "$bufio2");
            } else if (ci->type.in(id_DCM, id_PLL)) {
                for (IdString p : {id_CLKIN, id_CLKIN1, id_CLKIN2})
                    if (src.empty())
                        src = root(ci->getPort(p));
                for (auto &[name, port] : ci->ports)
                    if (port.type == PORT_OUT && name.str(ctx).rfind("CLK", 0) == 0)
                        outs.emplace_back(name, "$" + name.str(ctx));
            }
            if (src.empty())
                continue;
            for (auto &[port, suffix] : outs) {
                NetInfo *n = ci->getPort(port);
                if (n && n->name.str(ctx)[0] == '$') {
                    rename(n, src + suffix);
                    changed = true;
                }
            }
        }
    }
}

NEXTPNR_NAMESPACE_END
