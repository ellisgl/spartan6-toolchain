/*
 *  nextpnr -- Next Generation Place and Route
 *
 *  Spartan-6 himbaechel uarch: minimal UCF reader.
 *
 *  Supports NET "<port>" <KEY> = <value> [| <KEY> = <value> ...];
 *  with LOC, IOSTANDARD, DRIVE, SLEW, PULLUP and PULLDOWN. Other statements
 *  (TIMESPEC, PERIOD on NETs, INST constraints) are warned about and ignored.
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
} // namespace

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
    static const pool<std::string> known{"LOC", "IOSTANDARD", "DRIVE", "SLEW", "PULLUP", "PULLDOWN"};

    std::string stmt;
    int n = 0;
    while (std::getline(text, stmt, ';')) {
        stmt = trim(stmt);
        if (stmt.empty())
            continue;
        std::smatch m;
        if (!std::regex_match(stmt, m, net_re)) {
            log_warning("UCF: ignoring unsupported statement: %s\n", stmt.c_str());
            continue;
        }
        // UCF bus syntax led<3> -> nextpnr port bit name led[3].
        std::string port = std::regex_replace(unquote(m[1].str()), std::regex("<(\\d+)>"), "[$1]");
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
            if (!known.count(key)) {
                log_warning("UCF: ignoring %s on NET %s\n", key.c_str(), port.c_str());
                continue;
            }
            std::string value = kv[2].matched ? unquote(kv[2].str()) : "TRUE";
            ucf_attrs[ctx->id(port)][ctx->id(key)] = Property(value);
            n++;
        }
    }
    log_info("Read %d constraints from UCF file '%s'.\n", n, filename.c_str());
}

NEXTPNR_NAMESPACE_END
