#!/usr/bin/env python3
"""edit.py <tree> <kind> [n]: one edit of the series (run.sh), a real body change of the copied
workspace: vm = a line in the script VM's eval, hal = a line in the HAL's hal_open (the line is
inserted the first time, its literal changed after); closure+ / closure- = a closure added at the
start of eval, before the file's other closures (their expression numbers shift), and removed again."""
import re, sys
tree, kind = sys.argv[1], sys.argv[2]
if kind in ("closure+", "closure-"):
    p = tree + "/platform/script/src/vm.rs"
    s = open(p).read()
    anchor = "pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {"
    add = "\n        let warm_edit_z = |x: u32| x + 9; std::hint::black_box(warm_edit_z(1)); // WARM-EDIT-closure"
    if kind == "closure+":
        assert add not in s
        s = s.replace(anchor, anchor + add, 1)
    else:
        assert add in s
        s = s.replace(add, "", 1)
    open(p, "w").write(s)
    sys.exit(0)
n = int(sys.argv[3])
path, anchor = {
    "vm": ("platform/script/src/vm.rs", "pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {"),
    "hal": ("platform/hal/src/lib.rs", "pub fn hal_open(req: &OpenReq) -> HalInfo {"),
}[kind]
p = tree + "/" + path
s = open(p).read()
mark = "// WARM-EDIT-" + kind
line = "std::hint::black_box(%du32); %s" % (n, mark)
if mark in s:
    s = re.sub(r"std::hint::black_box\(\d+u32\); " + re.escape(mark), line, s)
else:
    i = s.index(anchor) + len(anchor)
    ind = re.match(r"\s*", s[i + 1:]).group(0)
    s = s[:i] + "\n" + ind + line + s[i:]
open(p, "w").write(s)
