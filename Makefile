# Build the nextpnr himbaechel chip database for a Spartan-6 device.
#
#   make NEXTPNR=/path/to/nextpnr          # -> build/xc6slx9.bba
#   make check                             # routing/bel sanity checks
#
# NEXTPNR is a nextpnr source checkout; only himbaechel/himbaechel_dbgen is used.

DEVICE         ?= xc6slx9
NEXTPNR        ?= $(error set NEXTPNR=/path/to/nextpnr source tree)
PRJCOMBINE_REV := 2b3317db3799b82b3ef5606438145ba76b9b4485
PYTHON         ?= python3

BUILD    := build
EXPORTER := $(BUILD)/target/release/s6-export
DB       := $(BUILD)/spartan6-$(PRJCOMBINE_REV).zstd

all: $(BUILD)/$(DEVICE).bba

# The database is pinned to the same revision as the exporter's prjcombine crates.
$(DB):
	@mkdir -p $(BUILD)
	curl -fsSL -o $@.tmp https://codeberg.org/prjunnamed/prjcombine/raw/commit/$(PRJCOMBINE_REV)/databases/spartan6.zstd
	mv $@.tmp $@

$(EXPORTER): exporter/Cargo.toml exporter/src/main.rs
	CARGO_TARGET_DIR=$(abspath $(BUILD)/target) cargo build --release --manifest-path exporter/Cargo.toml

$(BUILD)/$(DEVICE).json: $(EXPORTER) $(DB)
	$(EXPORTER) --db $(DB) --device $(DEVICE) --out $@

$(BUILD)/$(DEVICE).bba: $(BUILD)/$(DEVICE).json gen/s6_gen.py
	$(PYTHON) -I gen/s6_gen.py --nextpnr $(NEXTPNR) --json $< --bba $@

check: $(BUILD)/$(DEVICE).json
	$(PYTHON) -I gen/check_db.py $<

clean:
	rm -rf $(BUILD)/*.json $(BUILD)/*.bba

.PHONY: all check clean
