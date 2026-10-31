#!/usr/bin/env python3
"""edit.py <pkg dir> <step>: one edit of the series (run.sh). Each is a real change of the source
text the identities of the code after it depend on (offsets, closure expression numbers).
  line N     : a line `std::hint::black_box(N);` in wf-edit's eval (inserted, then its literal changed)
  host N     : the same in wf-host's host_open
  closure+   : a closure added before eval's existing closures
  closure-   : that closure removed again
  move       : Item::later moved above callbacks (its offset and its closures' numbers change)"""
import re, sys
pkg, step = sys.argv[1], sys.argv[2]

def body(path, anchor, tag, n):
    s = open(path).read()
    mark = "// WF-" + tag
    line = "std::hint::black_box(%du32); %s" % (n, mark)
    if mark in s:
        s = re.sub(r"std::hint::black_box\(\d+u32\); " + re.escape(mark), line, s)
    else:
        s = s.replace(anchor, anchor + "\n    " + line, 1)
    open(path, "w").write(s)

edit = pkg + "/edit/src/lib.rs"
host = pkg + "/host/src/lib.rs"
kind, _, arg = step.partition(" ")
if kind == "line":
    body(edit, "// EDIT-ANCHOR", "line", int(arg))
elif kind == "host":
    body(host, "// HOST-EDIT", "host", int(arg))
elif kind in ("closure+", "closure-"):
    s = open(edit).read()
    add = "    let z = |x: u64| x + 9; // WF-closure\n    std::hint::black_box(z(1));\n"
    anchor = "    let a = |x: u64| x + 1;\n"
    if kind == "closure+":
        assert add not in s
        s = s.replace(anchor, add + anchor, 1)
    else:
        assert add in s
        s = s.replace(add, "", 1)
    open(edit, "w").write(s)
elif kind == "move":
    s = open(edit).read()
    i = s.index("pub struct Item(pub u64);")
    item = s[i:]
    s = s[:i].rstrip("\n") + "\n"
    j = s.index("pub fn callbacks()")
    s = s[:j] + item.rstrip("\n") + "\n\n" + s[j:]
    open(edit, "w").write(s)
else:
    sys.exit("unknown step " + step)
