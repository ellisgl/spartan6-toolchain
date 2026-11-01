# Spartan-6 open-source P&R: chip database + nextpnr himbaechel uarch.
#
#   make                 # chip database (build/chipdb-xc6slx9.bin) and nextpnr
#   make check           # routing/bel sanity checks on the exported database
#   make blinky          # synthesize, place and route tests/blinky for the Mojo V3
#
# nextpnr and prjcombine are pinned. nextpnr is cloned into build/nextpnr and
# our uarch (uarch/spartan6) is linked into its source tree.

DEVICE         ?= xc6slx9
PACKAGE        ?= tqg144
PRJCOMBINE_REV := 2b3317db3799b82b3ef5606438145ba76b9b4485
NEXTPNR_REV    := a2c6479977d64d312686d853a93941744fdbeacf
PYTHON         ?= python3
YOSYS          ?= yosys
JOBS           ?= $(shell nproc)
# Until the packer supports them: no carry chains, wide muxes, SRLs, LUTRAM, BRAM or DSP.
SYNTH_FLAGS    ?= -nocarry -nowidelut -nosrl -nolutram -nobram -nodsp

BUILD    := build
EXPORTER := $(BUILD)/target/release/s6-export
DB       := $(BUILD)/spartan6-$(PRJCOMBINE_REV).zstd
NEXTPNR  := $(BUILD)/nextpnr
NPNR_BLD := $(BUILD)/nextpnr-build
NPNR_BIN := $(NPNR_BLD)/nextpnr-himbaechel
BBASM    := $(NPNR_BLD)/bba/bbasm
CHIPDB   := $(BUILD)/chipdb-$(DEVICE).bin
UARCH_SRC := $(wildcard uarch/spartan6/*)

all: $(CHIPDB) $(NPNR_BIN)

# --- nextpnr ---------------------------------------------------------------

$(NEXTPNR)/.fetched:
	rm -rf $(NEXTPNR)
	git init -q $(NEXTPNR)
	git -C $(NEXTPNR) fetch -q --depth 1 https://github.com/YosysHQ/nextpnr.git $(NEXTPNR_REV)
	git -C $(NEXTPNR) checkout -q FETCH_HEAD
	ln -s $(abspath uarch/spartan6) $(NEXTPNR)/himbaechel/uarch/spartan6
	# Register the uarch (nextpnr keeps a fixed list).
	sed -i 's/^set(HIMBAECHEL_UARCHES \(.*\))$$/set(HIMBAECHEL_UARCHES \1 spartan6)/' $(NEXTPNR)/himbaechel/CMakeLists.txt
	grep -q 'HIMBAECHEL_UARCHES .* spartan6)' $(NEXTPNR)/himbaechel/CMakeLists.txt
	touch $@

$(NPNR_BLD)/Makefile: $(NEXTPNR)/.fetched
	cmake -S $(NEXTPNR) -B $(NPNR_BLD) -DARCH=himbaechel -DHIMBAECHEL_UARCH=spartan6 \
		-DCMAKE_BUILD_TYPE=Release -DBUILD_GUI=OFF -DBUILD_PYTHON=OFF -DBUILD_TESTS=OFF

$(NPNR_BIN): $(NPNR_BLD)/Makefile $(UARCH_SRC)
	cmake --build $(NPNR_BLD) -j$(JOBS) --target nextpnr-himbaechel

$(BBASM): $(NPNR_BLD)/Makefile
	cmake --build $(NPNR_BLD) -j$(JOBS) --target bbasm

# --- chip database -----------------------------------------------------------

# The database is pinned to the same revision as the exporter's prjcombine crates.
$(DB):
	@mkdir -p $(BUILD)
	curl -fsSL -o $@.tmp https://codeberg.org/prjunnamed/prjcombine/raw/commit/$(PRJCOMBINE_REV)/databases/spartan6.zstd
	mv $@.tmp $@

$(EXPORTER): exporter/Cargo.toml $(wildcard exporter/src/*.rs)
	CARGO_TARGET_DIR=$(abspath $(BUILD)/target) cargo build --release --manifest-path exporter/Cargo.toml

$(BUILD)/$(DEVICE).json: $(EXPORTER) $(DB)
	$(EXPORTER) --db $(DB) --device $(DEVICE) --out $@

# The uarch's constids are preloaded so string indices match the compiled uarch.
$(BUILD)/$(DEVICE).bba: $(BUILD)/$(DEVICE).json gen/s6_gen.py uarch/spartan6/constids.inc $(NEXTPNR)/.fetched
	$(PYTHON) -I gen/s6_gen.py --nextpnr $(NEXTPNR) --constids uarch/spartan6/constids.inc --json $< --bba $@

$(CHIPDB): $(BUILD)/$(DEVICE).bba $(BBASM)
	$(BBASM) --l $< $@

check: $(BUILD)/$(DEVICE).json
	$(PYTHON) -I gen/check_db.py $<

# --- test designs ------------------------------------------------------------

$(BUILD)/blinky/synth.json: tests/blinky/blinky.v
	@mkdir -p $(dir $@)
	$(YOSYS) -q -l $(BUILD)/blinky/yosys.log -p "synth_xilinx -family xc6s -top blinky $(SYNTH_FLAGS) -json $@" $<

$(BUILD)/blinky/routed.json: $(BUILD)/blinky/synth.json tests/blinky/mojo.ucf $(CHIPDB) $(NPNR_BIN)
	$(NPNR_BIN) --device $(DEVICE)-2$(PACKAGE) --chipdb $(CHIPDB) --json $< \
		-o ucf=tests/blinky/mojo.ucf --write $@ --log $(BUILD)/blinky/nextpnr.log

blinky: $(BUILD)/blinky/routed.json

clean:
	rm -rf $(BUILD)/*.json $(BUILD)/*.bba $(BUILD)/*.bin $(BUILD)/blinky

.PHONY: all check blinky clean
