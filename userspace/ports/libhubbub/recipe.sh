# libhubbub (https://github.com/netsurf-browser/libhubbub, 68989e6 2026-11-15; MIT, src/COPYING):
# the sources its own build compiles (taken from a host build's list), with
# the files that build generates (gperf through ports/tools/gperf-lite.py),
# and the flags NetSurf's build system gives GCC (_ALIGNED, STMTEXPR) and NDEBUG, as its release builds
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -Dinline=__inline__ -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -DNDEBUG -Isrc/include -Isrc/src -Isrc "
compile "$OBJ" "$F" src/src/parser.c src/src/charset/detect.c src/src/tokeniser/entities.c src/src/tokeniser/tokeniser.c src/src/treebuilder/treebuilder.c src/src/treebuilder/initial.c src/src/treebuilder/before_html.c src/src/treebuilder/before_head.c src/src/treebuilder/in_head.c src/src/treebuilder/in_head_noscript.c src/src/treebuilder/after_head.c src/src/treebuilder/in_body.c src/src/treebuilder/in_table.c src/src/treebuilder/in_caption.c src/src/treebuilder/in_column_group.c src/src/treebuilder/in_table_body.c src/src/treebuilder/in_row.c src/src/treebuilder/in_cell.c src/src/treebuilder/in_select.c src/src/treebuilder/in_select_in_table.c src/src/treebuilder/in_foreign_content.c src/src/treebuilder/after_body.c src/src/treebuilder/in_frameset.c src/src/treebuilder/after_frameset.c src/src/treebuilder/after_after_body.c src/src/treebuilder/after_after_frameset.c src/src/treebuilder/generic_rcdata.c src/src/treebuilder/element-type.c src/src/utils/errors.c src/src/utils/string.c
archive hubbub "$OBJ"
headers . src/include/hubbub
