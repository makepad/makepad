#!/usr/bin/env python3
"""A static nobody thought about is the next run's wrong state: every static of a `makepad run`
process goes back to its start value when a run ends in place (`relax_compiler::run_end`), or is
named on process_statics.tsv with the reason it stays.

usage: restart_statics.py   (from the workspace root)

Each `static` in the crates below, thread-locals and tests aside, must be one of:
  - module level, named by its module's `reset_statics` or `end_run`;
  - on process_statics.tsv: `path::of::NAME<tab>why it may stay`.
A row whose static is gone fails too, so the list stays true. Prints "restart_statics: OK <counts>"
or the rows to settle and "restart_statics: FAIL"; exit 1 on FAIL.
"""
import os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "../../.."))
CRATES = {
    "relax_session": "libs/relax/session/src/lib.rs",
    "relax_base": "libs/relax/base/src/lib.rs",
    "relax_front": "libs/relax/front/src/lib.rs",
    "relax_types": "libs/relax/types/src/lib.rs",
    "relax_backend": "libs/relax/backend/src/lib.rs",
    "relax_codegen": "libs/relax/codegen/src/lib.rs",
    "relax_compiler": "libs/relax/compiler/src/lib.rs",
    "relax_runner": "tools/run/runner/src/lib.rs",
    "makepad_run": "tools/run/src/lib.rs",
    "makepad_hal": "platform/hal/src/lib.rs",
    "makepad_script": "platform/script/src/lib.rs",
    "makepad_script_compute": "platform/script/compute/src/lib.rs",
    "makepad_relax_api": "libs/relax_api/src/lib.rs",
    "makepad_remote": "libs/remote/src/lib.rs",
    "makepad_font_core": "libs/font_core/src/lib.rs",
    "makepad_cpu_budget": "platform/cpu_budget/src/lib.rs",
    "makepad_shared_bytes": "libs/shared_bytes/src/lib.rs",
    "makepad_alloc_phase": "libs/relax_api/phase/src/lib.rs",
    "makepad_live_id": "libs/live_id/src/lib.rs",
    "makepad_shader": "platform/shader/src/lib.rs",
    "makepad_script_std": "platform/script/std/src/lib.rs",
    "makepad_script_census": "platform/script/census/src/lib.rs",
    "makepad_tween_script": "libs/tween/script/src/lib.rs",
    "makepad_script_audio_aot": "platform/script/audio_aot/src/lib.rs",
}
RESET_FNS = ("reset_statics", "end_run", "reset_stores", "next_run")
MOD = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z_0-9]*)\s*(;|\{)")
STATIC = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:")
FN = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_0-9]+)')
TLS = re.compile(r"\b(?:thread_local|memo_local)!")
EXTERN = re.compile(r'^\s*(?:unsafe\s+)?extern\s*(?:"[^"]*")?\s*\{')
INCLUDE = re.compile(r'^\s*include!\("([^"]+\.rs)"\)')


# Crates whose code runs inside an op of the run's (makepad_relax_api run_op, or called from one):
# what they allocate is in the run's heap unless they say otherwise.
RUN_REACHED = (
    "platform/script/src",
    "platform/script/std/src",
    "platform/script/census/src",
    "platform/script/compute/src",
    "platform/shader/src",
    "libs/live_id/src",
    "libs/tween/script/src",
    "libs/error_log/src",
)
STD_LOCK = re.compile(r"\bstd::sync::(?:Mutex|Condvar)\b|\bstd::sync::\{[^}]*\b(?:Mutex|Condvar)\b|^\s*sync::(?:Mutex|Condvar),")


def std_locks():
    """On macOS std's Mutex and Condvar allocate a block at their first lock: in an op it is the run's,
    and the lock is gone with the run though what holds it stays (makepad_relax_api::sync)."""
    out = []
    for d in RUN_REACHED:
        for dirpath, _, names in os.walk(os.path.join(ROOT, d)):
            for name in names:
                if not name.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, name)
                lines = open(path, errors="replace").read().split("\n")
                for i, line in enumerate(lines):
                    code = line.split("//")[0]
                    if STD_LOCK.search(code) and not any("wasm32" in l for l in lines[max(0, i - 4):i]):
                        out.append("std's lock in a crate an op reaches: %s:%d" % (os.path.relpath(path, ROOT), i + 1))
    return out


def code_of(line, st):
    """The line without comments and without what strings hold, for brace counting."""
    out, i, n = [], 0, len(line)
    while i < n:
        if st["raw"] is not None:
            j = line.find('"' + "#" * st["raw"], i)
            if j < 0:
                return "".join(out)
            i, st["raw"] = j + 1 + st["raw"], None
        elif st["block"]:
            if line.startswith("*/", i):
                st["block"] -= 1
                i += 2
            elif line.startswith("/*", i):
                st["block"] += 1
                i += 2
            else:
                i += 1
        elif st["str"]:
            if line[i] == "\\":
                i += 2
                continue
            st["str"] = line[i] != '"'
            i += 1
        elif line.startswith("//", i):
            break
        elif line.startswith("/*", i):
            st["block"] += 1
            i += 2
        else:
            m = re.match(r'b?r(#*)"', line[i:])
            if m and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == "_")):
                st["raw"] = len(m.group(1))
                i += len(m.group(0))
            elif line[i] == '"':
                st["str"] = True
                i += 1
            elif line[i] == "'" and re.match(r"'(\\.[^']*|[^\\'])'", line[i:]):
                i += len(re.match(r"'(\\.[^']*|[^\\'])'", line[i:]).group(0))
            else:
                out.append(line[i])
                i += 1
    return "".join(out)


def scan_file(path, modpath, found, resets):
    """Each static of the file and the modules it declares: found[key] = where; resets: per module, what
    its reset functions name."""
    st = {"raw": None, "block": 0, "str": False}
    src = open(path, errors="replace").read().split("\n")
    stack = [{"kind": "mod", "depth": 0, "path": modpath}]
    depth, pending, path_attr, test_attr = 0, None, None, False
    for i, line in enumerate(src):
        code = code_of(line, st)
        t = code.strip()
        m = re.match(r'#\[path\s*=\s*"([^"]+)"\]', line.strip())
        if m:
            path_attr = m.group(1)
        if t.startswith("#[") and "test" in t and t.startswith("#[cfg("):
            test_attr = True
        in_fn = any(s["kind"] == "fn" for s in stack)
        hidden = any(s["kind"] in ("tls", "test") for s in stack)
        m = INCLUDE.match(code_of(line.replace('"', "\x00"), dict(st)).replace("\x00", '"'))
        if m and not hidden and not test_attr:
            scan_file(os.path.join(os.path.dirname(path), m.group(1)), stack[-1]["path"], found, resets)
        m = MOD.match(code)
        if m and not in_fn:
            sub = stack[-1]["path"] + [m.group(1)]
            if m.group(2) == ";":
                if not test_attr and not hidden:
                    base = os.path.dirname(path)
                    stem = os.path.splitext(os.path.basename(path))[0]
                    inner = stack[-1]["path"][len(modpath):]
                    d = os.path.join(base, *([] if stem in ("lib", "mod", "main") else [stem]), *inner)
                    cands = [os.path.join(base, path_attr)] if path_attr else [os.path.join(d, m.group(1) + ".rs"), os.path.join(d, m.group(1), "mod.rs")]
                    for c in cands:
                        if os.path.exists(c):
                            scan_file(c, sub, found, resets)
                            break
            else:
                stack.append({"kind": "test" if test_attr else "mod", "depth": depth, "path": sub})
                depth += code.count("{") - code.count("}")
                path_attr, test_attr = None, False
                continue
            path_attr, test_attr = None, False
        f = FN.match(code)
        if f and "{" in code and not code.rstrip().endswith(";"):
            stack.append({"kind": "fn", "name": f.group(1), "depth": depth, "path": stack[-1]["path"]})
            test_attr = False
        elif f and not code.rstrip().endswith(";"):
            pending = f.group(1)
        elif pending is not None:
            if "{" in code:
                stack.append({"kind": "fn", "name": pending, "depth": depth, "path": stack[-1]["path"]})
                pending = None
            elif code.rstrip().endswith(";"):
                pending = None
        if TLS.search(code) or EXTERN.match(code):
            stack.append({"kind": "tls", "depth": depth, "path": stack[-1]["path"]})
        m = STATIC.match(code)
        if m and not any(s["kind"] in ("tls", "test") for s in stack):
            fns = [s["name"] for s in stack if s["kind"] == "fn"]
            found.setdefault("::".join(stack[-1]["path"] + fns + [m.group(1)]), []).append("%s:%d" % (os.path.relpath(path, ROOT), i + 1))
        for s in stack:
            if s["kind"] == "fn" and s["name"] in RESET_FNS:
                resets.setdefault("::".join(s["path"]), set()).update(re.findall(r"[A-Z_][A-Z0-9_]*", code))
                break
        if t and not t.startswith("#["):
            test_attr = False
        depth += code.count("{") - code.count("}")
        while len(stack) > 1 and depth <= stack[-1]["depth"] and "}" in code:
            stack.pop()


def main():
    found, resets = {}, {}
    for name, lib in CRATES.items():
        scan_file(os.path.join(ROOT, lib), [name], found, resets)
    listed = {}
    for line in open(os.path.join(HERE, "process_statics.tsv"), errors="replace"):
        if line.strip() and not line.startswith("#"):
            key, _, why = line.rstrip("\n").partition("\t")
            listed[key] = why
    restored, staying, bad = 0, 0, []
    for key, where in sorted(found.items()):
        module, name = key.rsplit("::", 1)
        if name in resets.get(module, ()):
            restored += 1
        elif listed.get(key, "").strip():
            staying += 1
        else:
            bad.append("no reset and no reason: %s (%s)" % (key, where[0]))
    bad += ["on the list, gone from the source: " + k for k in sorted(listed) if k not in found and "::" in k and k.split("::")[0] in CRATES]
    bad += std_locks()
    for line in bad:
        print(line)
    print("restart_statics: %s %d restored, %d stay with a reason, %d to settle" % ("FAIL" if bad else "OK", restored, staying, len(bad)))
    sys.exit(1 if bad else 0)


main()
