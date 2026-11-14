"""replace_root.py NAME ROWS.rs: replaces the `debs` of LINUX_ROOTS' root NAME
in xtask/src/main.rs with ROWS.rs (its comment kept)."""
import sys
name, rows = sys.argv[1], open(sys.argv[2]).read()
import os
p = os.path.join(os.path.dirname(__file__), '../../xtask/src/main.rs')
s = open(p).read()
k = s.index(f'        name: "{name}",')
d = s.index('        debs: &[\n', k)
end = s.index('\n        ],\n    },\n', d)
s = s[:d] + '        debs: &[\n' + rows + s[end + 1:]
open(p, 'w').write(s)
