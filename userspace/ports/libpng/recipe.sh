# libpng 1.6.44 (https://github.com/pnggroup/libpng, tag v1.6.44; the libpng
# licence, src/LICENSE): the library unchanged, pnglibconf.h the prebuilt
# one from scripts/; no SIMD
compile "$OBJ" "-Isrc -DPNG_ARM_NEON_OPT=0 -DPNG_INTEL_SSE_OPT=0 -DPNG_POWERPC_VSX_OPT=0 -DPNG_MIPS_MSA_OPT=0" src/*.c
archive png "$OBJ"
headers . src/png.h src/pngconf.h src/pnglibconf.h
