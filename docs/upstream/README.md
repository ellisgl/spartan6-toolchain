# Upstream contributions (drafts)

Pull request texts for the parts of this toolchain that belong upstream.
Nothing here has been submitted. Before opening any of these, check the
project's current contribution policy, in particular on AI-assisted code
(Codeberg's prjcombine forbids it, which is why the SDF parser fix is only
an issue draft, `../prjcombine-sdf-issue.md`).

| Draft | Project | Branch / source | Size |
|---|---|---|---|
| [yosys-dsp48a-preadd.md](yosys-dsp48a-preadd.md) | YosysHQ/yosys | `xc6s-preadd-swap` on ellisgl/yosys | 59 lines (+ test) |
| [nextpnr-router2-fixes.md](nextpnr-router2-fixes.md) | YosysHQ/nextpnr | `nextpnr/common/route/router2.cc` here | 9 lines |
| [nextpnr-himbaechel-spartan6.md](nextpnr-himbaechel-spartan6.md) | YosysHQ/nextpnr | `nextpnr/himbaechel/uarch/spartan6` here | ~4,400 lines |

Suggested order: the router2 fixes first (small, general, and the uarch
relies on them), then the uarch. The yosys change is independent.

To open the nextpnr ones from this repository's subtree:

```
git subtree split --prefix=nextpnr -b nextpnr-upstream   # nextpnr history only
# then, in a nextpnr clone: fetch that branch and rebase it onto YosysHQ main,
# one branch per PR (router2 fixes; uarch + the CMake line)
```
