#!/usr/bin/env python3
"""Every interpreter handler must end in a tail jump (a missed tail call grows the native stack per
instruction). usage: tailcheck.py <makepad binary>. Disassembles with otool (macOS) or objdump and
lists handlers (relax_codegen::interp::h_*) that dispatch with a call (`blr`) instead of a tail jump
(`br`): a handler that ends the chain (returns, h_call's unwinding exit) may `ret`, but the next
handler is never called."""
import re, subprocess, sys
exe = sys.argv[1]
if sys.platform == "darwin":
    dis = subprocess.run(["otool", "-tV", exe], capture_output=True, text=True).stdout
else:
    dis = subprocess.run(["objdump", "-d", "--no-show-raw-insn", exe], capture_output=True, text=True).stdout
fns, cur = {}, None
for line in dis.splitlines():
    m = re.match(r"^(?:[0-9a-f]+ <)?(_?_R\w+)>?:$", line)
    if m:
        cur = m.group(1)
        fns[cur] = []
    elif cur:
        parts = line.split("\t")
        if len(parts) > 1:
            fns[cur].append(parts[1].strip() if sys.platform == "darwin" else parts[-1].split()[0] if parts[-1].split() else "")
hs = {n: o for n, o in fns.items() if re.search(r"6interp(\d+vx)?\d+h_", n)}
# a handler that does not end the chain must reach the next one with `br`; one that also
# calls something (`bl` / `blr`, a frame) must still not dispatch by call: no `blr` to the next
# handler is visible statically, so require the tail `br` and no `ret` outside the known exits
bad = [n for n, o in hs.items() if not re.search(r"h_ret|h_trap|h_resume|h_call$|h_const_call$", n) and ("br" not in o or "ret" in o)]
bad += [n for n, o in hs.items() if re.search(r"h_call$|h_const_call$", n) and "br" not in o]
# no handler calls through a register: that is how a lost tail dispatch looks (`blr` to the next
# handler); out-of-line helpers are called with `bl`
bad += [n for n, o in hs.items() if "blr" in o and n not in bad]
sizes = sorted(len(o) for o in hs.values())
print("%d handlers, median %d instructions; not tail-calling: %s" % (len(hs), sizes[len(sizes) // 2] if sizes else 0, bad or "none"))
sys.exit(1 if bad or not hs else 0)
