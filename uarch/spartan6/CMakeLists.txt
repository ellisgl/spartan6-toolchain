set(SOURCES
    constids.inc
    pack.cc
    spartan6.cc
    spartan6.h
    ucf.cc
)

add_nextpnr_himbaechel_microarchitecture(${uarch}
    CORE_SOURCES ${SOURCES}
)

# The chip database is built outside nextpnr (see the top-level Makefile:
# prjcombine -> s6-export -> s6_gen.py -> bbasm). Install it as
# <share>/himbaechel/spartan6/chipdb-xc6slx9.bin, or pass --chipdb.
