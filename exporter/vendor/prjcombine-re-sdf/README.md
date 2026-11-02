# prjcombine-re-sdf (vendored)

prjcombine's SDF parser (`re/sdf` at prjcombine rev `2b3317d`,
https://codeberg.org/prjunnamed/prjcombine, 0BSD OR Apache-2.0; licence
files alongside), copied here with fixes so it reads the timing-simulation
SDF that ISE 14.7 `netgen` writes. `s6-timing` uses it.

Changes from upstream (`src/parse.rs`):

- `VOLTAGE` / `TEMPERATURE` / `PROCESS` header items are skipped;
- `PATHPULSE` / `PATHPULSEPERCENT` inside `DELAY` are skipped;
- `RECREM` timing checks are parsed (into `RecRem`, recovery and removal);
- `PORT` and `IOPATH` accept a single delay value for both edges;
- identifiers may start with an escaped character (`\$abc\$12`);
- bus ranges (`A[17:0]`) are read as part of the identifier.

These are reported upstream as an issue (text in `docs/prjcombine-sdf-issue.md`).
Once upstream handles ISE's output, this copy can go and `exporter/Cargo.toml`
can depend on the upstream crate again.
