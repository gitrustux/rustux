# libjpeg-turbo 3.0.4 (https://github.com/libjpeg-turbo/libjpeg-turbo, tag
# 3.0.4; the IJG and BSD licences, src/LICENSE.md): the library's C files
# unchanged, without SIMD (plain C); jconfig.h, jconfigint.h and
# jversion.h written by hand from their .in templates; src/inc/ holds the
# .c files other files include. Some files are
# compiled three times, for 8-, 12- and 16-bit samples, as its CMake does
J16="jcapistd jccolor jcdiffct jclossls jcmainct jcprepct jcsample jdapistd jdcolor jddiffct jdlossls jdmainct jdpostct jdsample jutils"
J12="$J16 jccoefct jcdctmgr jdcoefct jddctmgr jdmerge jfdctfst jfdctint jidctflt jidctfst jidctint jidctred jquant1 jquant2"
compile "$OBJ/8" "-Isrc -Isrc/inc" src/*.c
compile "$OBJ/12" "-Isrc -Isrc/inc -DBITS_IN_JSAMPLE=12" $(for f in $J12; do echo src/$f.c; done)
compile "$OBJ/16" "-Isrc -Isrc/inc -DBITS_IN_JSAMPLE=16" $(for f in $J16; do echo src/$f.c; done)
rm -f "$SYSROOT/lib/libjpeg.a"
# Each width's objects under their own names in the archive
for b in 12 16; do for o in "$OBJ/$b"/*.o; do cp "$o" "$OBJ/$b/j$b-$(basename "$o")"; rm "$o"; done; done
$AR rcs "$SYSROOT/lib/libjpeg.a" "$OBJ"/8/*.o "$OBJ"/12/*.o "$OBJ"/16/*.o
headers . src/jpeglib.h src/jconfig.h src/jmorecfg.h src/jerror.h
