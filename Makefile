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

$(EXPORTER): exporter/Cargo.toml $(shell find exporter/src -name "*.rs")
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
$(BUILD)/%/synth.json: $$(wildcard tests/$$*/*.v)
	@mkdir -p $(dir $@)
	$(YOSYS) -q -l $(dir $@)yosys.log -p "synth_xilinx -family xc6s -top $* $(SYNTH_FLAGS) -json $@" $^

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
