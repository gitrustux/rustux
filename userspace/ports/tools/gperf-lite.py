#!/usr/bin/env python3
# gperf-lite: what the ported libraries need of GNU gperf, without gperf:
# reads a gperf input with %struct-type (keywords and the struct's other
# fields on each line) and writes C with the same lookup function, over a
# table sorted by key (binary search) instead of a perfect hash. The table
# is named wordlist, as with %global-table; %ignore-case compares without
# case. Used for hubbub's element types and libsvgtiny's colours
# (2026-11-26, phase W).
#
#   gperf-lite.py INPUT.gperf OUTPUT.c
import re
import sys

src, out = sys.argv[1], sys.argv[2]
text = open(src).read()
decl, rest = text.split('%%', 1)
body = rest.split('%%', 1)[0]
opts = {}
for m in re.finditer(r'^%define\s+(\S+)\s+(\S+)', decl, re.M):
    opts[m.group(1)] = m.group(2)
ignore_case = re.search(r'^%ignore-case', decl, re.M) is not None
prologue = ''
m = re.search(r'%\{(.*?)%\}', decl, re.S)
if m:
    prologue = m.group(1)
# The struct's name: "struct NAME;" or "struct NAME { ... };" after %}
sm = re.search(r'struct\s+(\w+)\s*[;{]', decl[m.end() if m else 0:])
struct = sm.group(1)
lookup = opts.get('lookup-function-name', 'in_word_set')

rows = []
for line in body.splitlines():
    line = line.strip()
    if not line or line.startswith('#'):
        continue
    parts = [p.strip() for p in line.split(',')]
    key = parts[0]
    if key.startswith('"') and key.endswith('"'):
        key = key[1:-1]
    rows.append((key, parts[1:]))
norm = (lambda s: s.lower()) if ignore_case else (lambda s: s)
rows.sort(key=lambda r: norm(r[0]))

o = []
o.append('/* Made by ports/tools/gperf-lite.py from %s: do not edit */' % src.split('/')[-1])
o.append(prologue.strip('\n'))
o.append('#include <stddef.h>')
o.append('#include <string.h>')
o.append('#include <strings.h>')
o.append('static const struct %s wordlist[] = {' % struct)
for key, fields in rows:
    o.append('    { "%s"%s },' % (key, ''.join(', ' + f for f in fields)))
o.append('};')
cmp = 'strncasecmp' if ignore_case else 'strncmp'
o.append('''
const struct %(s)s *%(f)s(const char *str, size_t len);
const struct %(s)s *%(f)s(const char *str, size_t len)
{
    size_t lo = 0, hi = sizeof(wordlist) / sizeof(wordlist[0]);
    while (lo < hi) {
        size_t mid = (lo + hi) / 2;
        const char *k = wordlist[mid].name;
        size_t kl = strlen(k);
        int c = %(cmp)s(str, k, len < kl ? len : kl);
        if (c == 0)
            c = len < kl ? -1 : len > kl ? 1 : 0;
        if (c == 0)
            return &wordlist[mid];
        if (c < 0)
            hi = mid;
        else
            lo = mid + 1;
    }
    return NULL;
}''' % {'s': struct, 'f': lookup, 'cmp': cmp})
open(out, 'w').write('\n'.join(o) + '\n')
