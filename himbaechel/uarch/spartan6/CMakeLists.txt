set(SOURCES
    bram.cc
    carry.cc
    cmt.cc
    constids.inc
    dsp.cc
    lutram.cc
    pack.cc
    spartan6.cc
    spartan6.h
    ucf.cc
    wide.cc
)

add_nextpnr_himbaechel_microarchitecture(${uarch}
    CORE_SOURCES ${SOURCES}
)

# The chip database is built outside nextpnr, from prjcombine's Spartan-6
# database, by the xc6slx9-pnr tools (s6-export -> s6_gen.py -> bbasm).
# Install it as <share>/himbaechel/spartan6/chipdb-xc6slx9.bin, or pass
# --chipdb. See README.md.
