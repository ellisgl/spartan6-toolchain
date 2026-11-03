# Spartan-6 open-source P&R: chip database + nextpnr himbaechel uarch.
#
#   make                 # chip database (build/chipdb-xc6slx9.bin) and nextpnr
#   make check           # routing/bel sanity checks on the exported database
#   make blinky          # synthesize, place and route tests/blinky for the Mojo V3
#
# nextpnr and prjcombine are pinned. nextpnr, with the spartan6 uarch, comes
# from the fork's spartan6 branch and is cloned into build/nextpnr. To work on
# the uarch, point NEXTPNR_SRC at your own checkout of that branch (built in
# build/nextpnr-build-dev):
#   make NEXTPNR_SRC=../nextpnr-fork pnr TEST=cylon

DEVICE         ?= xc6slx9
PACKAGE        ?= tqg144
PRJCOMBINE_REV := 2b3317db3799b82b3ef5606438145ba76b9b4485
NEXTPNR_REPO   := https://github.com/ellisgl/nextpnr.git
NEXTPNR_REV    := 799b32a715d59b284b92dbb3ae94fac3ed386705
PYTHON         ?= python3
YOSYS          ?= yosys
JOBS           ?= $(shell nproc)
# Everything synth_xilinx emits for xc6s is supported (see README for limits).
SYNTH_FLAGS    ?=

BUILD    := build
EXPORTER := $(BUILD)/target/release/s6-export
DB       := $(BUILD)/spartan6-$(PRJCOMBINE_REV).zstd
NEXTPNR_SRC ?=
ifeq ($(NEXTPNR_SRC),)
NEXTPNR  := $(BUILD)/nextpnr
NPNR_BLD := $(BUILD)/nextpnr-build
NPNR_FETCHED := $(NEXTPNR)/.fetched-$(NEXTPNR_REV)
else
NEXTPNR  := $(NEXTPNR_SRC)
NPNR_BLD := $(BUILD)/nextpnr-build-dev
NPNR_FETCHED :=
endif
NPNR_BIN := $(NPNR_BLD)/nextpnr-himbaechel
BBASM    := $(NPNR_BLD)/bba/bbasm
CHIPDB   := $(BUILD)/chipdb-$(DEVICE).bin
UARCH_DIR := $(NEXTPNR)/himbaechel/uarch/spartan6
UARCH_SRC := $(wildcard $(UARCH_DIR)/*)

all: $(CHIPDB) $(NPNR_BIN)

# --- nextpnr ---------------------------------------------------------------

ifeq ($(NEXTPNR_SRC),)
# The stamp names the revision, so changing NEXTPNR_REV re-clones.
$(NPNR_FETCHED):
	rm -rf $(NEXTPNR) $(NPNR_BLD)
	git init -q $(NEXTPNR)
	git -C $(NEXTPNR) fetch -q --depth 1 $(NEXTPNR_REPO) $(NEXTPNR_REV)
	git -C $(NEXTPNR) checkout -q FETCH_HEAD
	test -f $(UARCH_DIR)/spartan6.cc
	touch $@

# .SECONDARY (below) would otherwise let make skip a missing stamp.
.NOTINTERMEDIATE: $(NPNR_FETCHED)
endif

$(NPNR_BLD)/Makefile: $(NPNR_FETCHED)
	cmake -S $(NEXTPNR) -B $(NPNR_BLD) -DARCH=himbaechel -DHIMBAECHEL_UARCH=spartan6 \
		-DCMAKE_BUILD_TYPE=Release -DBUILD_GUI=OFF -DBUILD_PYTHON=OFF -DBUILD_TESTS=OFF

# With NEXTPNR_SRC, any file in the checkout may change (not just the uarch),
# so always let cmake decide.
$(NPNR_BIN): $(NPNR_BLD)/Makefile $(UARCH_SRC) $(if $(NEXTPNR_SRC),FORCE)
	cmake --build $(NPNR_BLD) -j$(JOBS) --target nextpnr-himbaechel

.PHONY: FORCE
FORCE:

$(BBASM): $(NPNR_BLD)/Makefile
	cmake --build $(NPNR_BLD) -j$(JOBS) --target bbasm

# --- chip database -----------------------------------------------------------

# The database is pinned to the same revision as the exporter's prjcombine crates.
$(DB):
	@mkdir -p $(BUILD)
	curl -fsSL -o $@.tmp https://codeberg.org/prjunnamed/prjcombine/raw/commit/$(PRJCOMBINE_REV)/databases/spartan6.zstd
	mv $@.tmp $@

$(EXPORTER): exporter/Cargo.toml $(shell find exporter/src exporter/vendor -name "*.rs")
	CARGO_TARGET_DIR=$(abspath $(BUILD)/target) cargo build --release --manifest-path exporter/Cargo.toml

$(BUILD)/$(DEVICE).json: $(EXPORTER) $(DB)
	$(EXPORTER) --db $(DB) --device $(DEVICE) --out $@

# The uarch's constids are preloaded so string indices match the compiled uarch.
# The delay model (data/timing-xc6slx9-2.json) is committed; `make timing-data`
# regenerates it with ISE.
TIMING_DATA := data/timing-$(DEVICE)-2.json
$(BUILD)/$(DEVICE).bba: $(BUILD)/$(DEVICE).json gen/s6_gen.py gen/s6_timing.py $(NPNR_FETCHED) \
		$(wildcard $(UARCH_DIR)/constids.inc) $(wildcard $(TIMING_DATA))
	$(PYTHON) -I gen/s6_gen.py --nextpnr $(NEXTPNR) --constids $(UARCH_DIR)/constids.inc --json $< --bba $@ \
		$(if $(wildcard $(TIMING_DATA)),--timing $(TIMING_DATA))

$(CHIPDB): $(BUILD)/$(DEVICE).bba $(BBASM)
	$(BBASM) --l $< $@

check: $(BUILD)/$(DEVICE).json
	$(PYTHON) -I gen/check_db.py $<

# --- ISE names (for XDL) -----------------------------------------------------
# Needs ISE (tools/ise-install.sh). s6-rawdump runs ISE's xdl/partgen through
# the tools/ise-bin wrappers; prjcombine's xrd2geom turns the dump into the
# naming database s6-xdl uses.

XRD2GEOM := $(BUILD)/tools/bin/xrd2geom
RAWDUMP  := $(BUILD)/$(DEVICE)-rawdump.zstd
GEOM     := $(BUILD)/$(DEVICE)-geom.zstd
S6XDL    := $(BUILD)/target/release/s6-xdl
ISE      := tools/ise.sh

$(XRD2GEOM):
	cargo install -q --git https://codeberg.org/prjunnamed/prjcombine.git --rev $(PRJCOMBINE_REV) \
		prjcombine-re-xilinx-rd2geom --root $(BUILD)/tools --target-dir $(BUILD)/target

$(BUILD)/ise-toolchain.kdl:
	@mkdir -p $(BUILD)
	printf 'env PATH="%s"\n' "$(abspath tools/ise-bin)" > $@

# Order-only: the dump depends on ISE, not on our code.
$(RAWDUMP): | $(EXPORTER) $(BUILD)/ise-toolchain.kdl
	mkdir -p $(BUILD)/ise-tmp
	TMPDIR=$(abspath $(BUILD)/ise-tmp) ISE_MOUNT=$(abspath $(BUILD)/ise-tmp) \
		$(BUILD)/target/release/s6-rawdump $(BUILD)/ise-toolchain.kdl $(DEVICE) $@

$(GEOM): $(RAWDUMP) $(XRD2GEOM)
	$(XRD2GEOM) convert $@ $<

# s6-rawdump and s6-xdl are built with the exporter.
$(S6XDL): $(EXPORTER)

# --- test designs ------------------------------------------------------------
#
#   make pnr       TEST=cylon  # yosys + nextpnr               -> build/cylon/routed.json
#   make bit       TEST=cylon  # + s6-bitgen (no ISE)          -> build/cylon/design.bin
#   make load      TEST=cylon  # + load into the Mojo's FPGA RAM
#   make ise-check TEST=cylon  # ISE cross-check: s6-xdl -> xdl2ncd -> drc -> bitgen,
#                              # which must give a byte-identical bitstream
#
# tests/<name>/*.v with top module <name>; pins from tests/<name>/pins.ucf if
# present, else tests/mojo_v3.ucf.
# XDL_MODE=routed (default) checks our routing; XDL_MODE=pins lets ISE route.

TEST ?= blinky
T    := $(BUILD)/$(TEST)
S6BITGEN := $(BUILD)/target/release/s6-bitgen
XDL_MODE ?= routed

# Keep intermediate files (synth.json, design.xdl) for inspection.
.SECONDARY:
.SECONDEXPANSION:
# Re-synthesize when SYNTH_FLAGS changes: the stamp is rewritten only then.
SYNTH_STAMP := $(BUILD)/.synth-flags
$(shell mkdir -p $(BUILD); echo '$(SYNTH_FLAGS)' | cmp -s - $(SYNTH_STAMP) 2>/dev/null || echo '$(SYNTH_FLAGS)' > $(SYNTH_STAMP))

# tests/common/*.v (shared UART helpers) is read for every design; yosys drops
# the modules a design doesn't use.
$(BUILD)/%/synth.json: $$(wildcard tests/$$*/*.v) $(wildcard tests/common/*.v) $(SYNTH_STAMP)
	@mkdir -p $(dir $@)
	$(YOSYS) -q -l $(dir $@)yosys.log -p "synth_xilinx -family xc6s -top $* $(SYNTH_FLAGS) -json $@" $(filter %.v,$^)

ucf_for = $(if $(wildcard tests/$(1)/pins.ucf),tests/$(1)/pins.ucf,tests/mojo_v3.ucf)

$(BUILD)/%/routed.json: $(BUILD)/%/synth.json $$(call ucf_for,$$*) $(CHIPDB) $(NPNR_BIN)
	$(NPNR_BIN) --device $(DEVICE)-2$(PACKAGE) --chipdb $(CHIPDB) --json $< \
		-o ucf=$(call ucf_for,$*) --write $@ --log $(dir $@)nextpnr.log

$(S6BITGEN): $(EXPORTER)

$(BUILD)/%/design.bin: $(BUILD)/%/routed.json $(S6BITGEN) $(DB) $(BUILD)/$(DEVICE).json
	$(S6BITGEN) --db $(DB) gen --routed $< --chipdb-json $(BUILD)/$(DEVICE).json --out $@

# --- ISE cross-check ---------------------------------------------------------

XDL_ARGS_routed := --rawdump $(RAWDUMP) --chipdb-json $(BUILD)/$(DEVICE).json
XDL_ARGS_pins   := --pins-only

$(BUILD)/%/design.xdl: $(BUILD)/%/routed.json $(S6XDL) $(GEOM) $(DB) $(RAWDUMP) $(BUILD)/$(DEVICE).json
	$(S6XDL) --db $(DB) --geom $(GEOM) $(XDL_ARGS_$(XDL_MODE)) --routed $< --package $(PACKAGE) --out $@

# ISE: XDL -> NCD, (route with par in pins mode), DRC, bitstream.
$(BUILD)/%/ise.bin: $(BUILD)/%/design.xdl
	cd $(dir $@) && ISE_MOUNT=$(abspath $(dir $@)) $(abspath $(ISE)) xdl -xdl2ncd -force design.xdl design.ncd > xdl2ncd.log
	if grep -q '^ERROR' $(dir $@)xdl2ncd.log; then grep '^ERROR' $(dir $@)xdl2ncd.log; exit 1; fi
ifeq ($(XDL_MODE),pins)
	cd $(dir $@) && ISE_MOUNT=$(abspath $(dir $@)) $(abspath $(ISE)) par -w design.ncd design_routed.ncd > par.log
else
	cp $(dir $@)design.ncd $(dir $@)design_routed.ncd
endif
	cd $(dir $@) && ISE_MOUNT=$(abspath $(dir $@)) $(abspath $(ISE)) drc design_routed.ncd > drc.log
	grep -q '<0> errors' $(dir $@)drc.log || { grep ERROR $(dir $@)drc.log; exit 1; }
	cd $(dir $@) && ISE_MOUNT=$(abspath $(dir $@)) $(abspath $(ISE)) bitgen -w -g Binary:Yes -g UnusedPin:Pullnone \
		design_routed.ncd ise.bit > bitgen.log

# --- delay model (needs ISE) ---------------------------------------------------
#
#   make timing-data   # ISE timing (netgen SDF) for the test designs, fitted
#                      # into data/timing-xc6slx9-2.json (see s6-timing).
#                      # Uses the current routes, so run it after `make`.

TIMING_DESIGNS ?= uart_echo alu rom bram dsp fill lutram clocks tristate speed
# DSP48A1 characterization (tools/gen_dsp_char.py): cell timing only.
TIMING_CELL_DESIGNS ?= $(notdir $(wildcard tests/dsp_char*))
S6TIMING := $(BUILD)/target/release/s6-timing
$(S6TIMING): $(EXPORTER)

# ISE's timing simulation SDF for the NCD built from our XDL by ise-check.
$(BUILD)/%/timing_sim.sdf: $(BUILD)/%/ise.bin
	cd $(dir $@) && ISE_MOUNT=$(abspath $(dir $@)) $(abspath $(ISE)) netgen -sim -ofmt verilog -w \
		design_routed.ncd timing_sim.v > netgen.log

# Not a file rule: the chip database depends on the committed data file, and
# make must never try to regenerate it (that needs ISE).
timing-data: $(foreach t,$(TIMING_DESIGNS) $(TIMING_CELL_DESIGNS),$(BUILD)/$(t)/timing_sim.sdf) $(S6TIMING) $(BUILD)/$(DEVICE).json
	@mkdir -p data
	$(S6TIMING) --chipdb-json $(BUILD)/$(DEVICE).json --out $(TIMING_DATA) \
		$(foreach t,$(TIMING_DESIGNS),--design $(BUILD)/$(t)/routed.json:$(BUILD)/$(t)/timing_sim.sdf) \
		$(foreach t,$(TIMING_CELL_DESIGNS),--cells-only $(BUILD)/$(t)/routed.json:$(BUILD)/$(t)/timing_sim.sdf)
.PHONY: timing-data

pnr:  $(T)/routed.json
bit:  $(T)/design.bin
load: $(T)/design.bin
	$(PYTHON) -I tools/mojo-load.py $<
ise-check: $(T)/design.bin $(T)/ise.bin
	$(S6BITGEN) --db $(DB) diff $(T)/ise.bin $(T)/design.bin | tail -1
	cmp $(T)/ise.bin $(T)/design.bin && echo "$(TEST): s6-bitgen output is identical to ISE bitgen"

# Old name for the first milestone.
blinky:
	$(MAKE) pnr TEST=blinky

clean:
	rm -rf $(BUILD)/*.json $(BUILD)/*.bba $(BUILD)/*.bin $(BUILD)/blinky $(BUILD)/cylon $(BUILD)/uart_echo

.PHONY: all check pnr bit load ise-check blinky clean
