#!/usr/bin/env python3
"""Hot-reload cases for `makepad live` (local/plans/hotreload.md section 8).

usage: run_cases.py --makepad <makepad binary> [--only name[,name..]] [--verbose] [--keep DIR]

A case is a directory under tests/live/ (the files directly in tests/live/ are the case "body_edit"):
  v0.rs, v1.rs, ..           single-file crate: each edit replaces the crate root
  v0/root.rs + v0/*.rs, v1/.. multi-file crate: v0/ is the crate (root.rs is its root), each later vK/
                              holds only the files that edit K changes (an overlay); the step is passed
                              to `makepad live` as the overlay directory
  expect.txt                 one line per edit (# starts a comment line): `vK <rung>[|<rung>..] [out=oracle|prev|file] [note=<text>]`
                              rung: patch | reinst | restart | refuse | rollback
                                - patch, reinst: the edit applied; the running program prints what a fresh
                                  run of vK prints (out=oracle, the default), or expect-vK.out (out=file)
                                - refuse, restart, rollback: the running program is unchanged; a run after it
                                  prints what the previous segment printed (out=prev, the default)
                              note=<text>: the edit's output must contain <text> (rest of the line)
  expect-v0.out (optional)   what the v0 second run prints, if not what a fresh run of v0 prints
  env (optional)             KEY=VALUE lines for both the live session and the oracle runs
                              MAKEPAD_RUN_LIVE_STREAM=1 feeds single-file edits through apply_stream, 17 ms apart.
  xfail (optional)           the case is expected to FAIL today; its first line names the gap (G1..G10)
  forbid (optional)          substrings (one per line) that must not appear in the live session's output from the
                              first edit on

Program output that is compared: the lines starting with "out " or "checksum ".
The oracle for vK is `makepad test` on vK's sources run fresh (one run).

The live driver's edit lines say how an edit was applied: `rung=<name>` when the driver prints it;
otherwise (the driver before the patch transaction) it is inferred: "patch REFUSED" or an edit line
without "changed fn(s)" = refuse; "rolled back patch" = rollback; "patch compile error"/"patch link
error" = half (a half-applied patch, never acceptable); else patch.

Result per case: PASS, FAIL, XFAIL (fails and is marked xfail), XPASS (passes but is marked xfail:
remove the marker). Exit status 1 on any FAIL or XPASS.
"""
import os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_RE = re.compile(r"^(out |checksum )")


def parse_args(argv):
    a = {"relax": None, "only": None, "verbose": False, "keep": None, "timeout": 180}
    i = 0
    while i < len(argv):
        k = argv[i]
        if k == "--makepad":
            a["relax"] = argv[i + 1]; i += 2
        elif k == "--only":
            a["only"] = set(argv[i + 1].split(",")); i += 2
        elif k == "--keep":
            a["keep"] = argv[i + 1]; i += 2
        elif k == "--timeout":
            a["timeout"] = int(argv[i + 1]); i += 2
        elif k in ("-v", "--verbose"):
            a["verbose"] = True; i += 1
        else:
            sys.exit("unknown argument " + k)
    if not a["relax"]:
        sys.exit(__doc__)
    return a


def cases():
    out = []
    if os.path.exists(os.path.join(HERE, "v0.rs")):
        out.append(("body_edit", HERE))
    for n in sorted(os.listdir(HERE)):
        d = os.path.join(HERE, n)
        if os.path.isdir(d) and (os.path.exists(os.path.join(d, "v0.rs")) or os.path.isdir(os.path.join(d, "v0"))):
            out.append((n, d))
    return out


def versions(d):
    """[(name, path)] in order: files vK.rs or overlay dirs vK/."""
    vs = []
    k = 0
    while True:
        f = os.path.join(d, "v%d.rs" % k)
        g = os.path.join(d, "v%d" % k)
        if os.path.isfile(f):
            vs.append(("v%d" % k, f))
        elif os.path.isdir(g):
            vs.append(("v%d" % k, g))
        else:
            break
        k += 1
    return vs


def read_env(d):
    env = dict(os.environ)
    env.setdefault("MAKEPAD_RUN_CACHE", "0")
    p = os.path.join(d, "env")
    if os.path.exists(p):
        for line in open(p):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                env[k] = v
    return env


def read_expect(d, nedits):
    p = os.path.join(d, "expect.txt")
    ex = {}
    if os.path.exists(p):
        for line in open(p):
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split()
            v, rungs = parts[0], parts[1].split("|")
            e = {"rungs": rungs, "out": None, "note": None}
            rest = line.split(None, 2)[2] if len(parts) > 2 else ""
            m = re.search(r"note=(.*)$", rest)
            if m:
                e["note"] = m.group(1).strip()
                rest = rest[: m.start()]
            m = re.search(r"out=(\S+)", rest)
            if m:
                e["out"] = m.group(1)
            ex[v] = e
    return ex


def run(cmd, env, cwd, timeout):
    try:
        p = subprocess.run(cmd, env=env, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
        return p.returncode, p.stdout.decode("utf-8", "replace")
    except subprocess.TimeoutExpired as e:
        return "timeout", (e.stdout or b"").decode("utf-8", "replace")


def outs(text):
    return [l for l in text.splitlines() if OUT_RE.match(l)]


def materialize(vs, k, tmp):
    """The source tree of version k of a multi-file case: v0 plus overlays 1..k. Returns its root.rs."""
    t = os.path.join(tmp, "tree-v%d" % k)
    shutil.copytree(vs[0][1], t)
    for j in range(1, k + 1):
        src = vs[j][1]
        for base, _, files in os.walk(src):
            rel = os.path.relpath(base, src)
            os.makedirs(os.path.join(t, rel), exist_ok=True)
            for f in files:
                shutil.copy(os.path.join(base, f), os.path.join(t, rel, f))
    return os.path.join(t, "root.rs")


def segments(text):
    """[(edit line, [lines])] after the v0 second run; segment 0 is that run (edit line '')."""
    lines = text.splitlines()
    try:
        i = next(j for j, l in enumerate(lines) if l.startswith("v0: second run"))
    except StopIteration:
        return None
    segs = [("", [])]
    for l in lines[i + 1:]:
        if l.startswith("edit "):
            segs.append((l, []))
        else:
            segs[-1][1].append(l)
    return segs


def rung_of(edit_line, body):
    text = "\n".join(body)
    if "rolled back patch" in text:
        return "rollback"
    m = re.search(r"\brung=(\w+)", edit_line)
    if m:
        return m.group(1)
    if re.search(r"patch (compile|link) error", text):
        return "half"
    if "rolled back patch" in text:
        return "rollback"
    if "patch REFUSED" in edit_line or "changed fn(s)" not in edit_line:
        return "refuse"
    return "patch"


def final_run(body):
    """The program output of the last run in a segment (after a rollback's rerun if any)."""
    k = 0
    for j, l in enumerate(body):
        if "rolled back patch" in l:
            k = j + 1
    return outs("\n".join(body[k:]))


def check_case(name, d, a, tmp):
    vs = versions(d)
    env = read_env(d)
    ex = read_expect(d, len(vs) - 1)
    multi = os.path.isdir(vs[0][1])
    msgs = []
    # the live session
    if multi:
        root = os.path.join(tmp, "live")
        shutil.copytree(vs[0][1], root)
        cmd = [a["relax"], "live", os.path.join(root, "root.rs")] + [p for _, p in vs[1:]]
    else:
        cmd = [a["relax"], "live"] + [p for _, p in vs]
    code, text = run(cmd, env, tmp, a["timeout"])
    if a["verbose"]:
        print("---- %s live (exit %s)\n%s----" % (name, code, text))
    if code != 0:
        msgs.append("live session exit %s%s" % (code, " (crash)" if code not in (1, 2) else ""))
    fp = os.path.join(d, "forbid")
    if os.path.exists(fp):
        for bad in [l.strip() for l in open(fp) if l.strip()]:
            # from the first edit on (warm-up before it is the program's own first run)
            k = text.find("\nedit ")
            if k >= 0 and bad in text[k:]:
                msgs.append("forbidden output: %r" % bad)
    segs = segments(text)
    if segs is None:
        msgs.append("no v0 second run in the output")
        return msgs, text
    if len(segs) != len(vs):
        msgs.append("%d edit segment(s) for %d edit(s)" % (len(segs) - 1, len(vs) - 1))
    # oracles
    oracle_cache = {}

    def oracle(k):
        if k not in oracle_cache:
            src = materialize(vs, k, tmp) if multi else vs[k][1]
            c, t = run([a["relax"], "test", src], env, tmp, a["timeout"])
            if c != 0:
                msgs.append("oracle v%d: makepad test exit %s" % (k, c))
            oracle_cache[k] = outs(t)
        return oracle_cache[k]

    def expected_file(v):
        p = os.path.join(d, "expect-%s.out" % v)
        return [l for l in open(p).read().splitlines() if l.strip()] if os.path.exists(p) else None

    prev = expected_file("v0")
    if prev is None:
        prev = oracle(0)
    got0 = final_run(segs[0][1])
    if got0 != prev:
        msgs.append("v0: second run prints %s, expected %s" % (got0, prev))
    prev = got0
    for k in range(1, len(vs)):
        v = vs[k][0]
        if k >= len(segs):
            break
        edit_line, body = segs[k]
        e = ex.get(v, {"rungs": ["patch"], "out": None, "note": None})
        r = rung_of(edit_line, body)
        if r not in e["rungs"]:
            msgs.append("%s: rung %s, expected %s (%s)" % (v, r, "|".join(e["rungs"]), edit_line.strip()[:200]))
        if e["note"] and e["note"] not in edit_line + "\n" + "\n".join(body):
            msgs.append("%s: missing note %r" % (v, e["note"]))
        got = final_run(body)
        applied = r in ("patch", "reinst")
        mode = e["out"] or ("oracle" if applied else "prev")
        if mode == "oracle":
            want = oracle(k)
        elif mode == "prev":
            want = prev
        else:
            want = expected_file(v)
            if want is None:
                msgs.append("%s: expect-%s.out missing" % (v, v))
                want = []
        if not got and not applied and r != "half":
            # a refused edit may print no run after it: nothing to compare
            pass
        elif got != want:
            msgs.append("%s: prints %s, expected (%s) %s" % (v, got, mode, want))
        if got:
            prev = got
    return msgs, text


def main():
    a = parse_args(sys.argv[1:])
    a["relax"] = os.path.abspath(a["relax"])
    counts = {"PASS": 0, "FAIL": 0, "XFAIL": 0, "XPASS": 0}
    for name, d in cases():
        if a["only"] and name not in a["only"]:
            continue
        tmp = tempfile.mkdtemp(prefix="live-%s-" % name, dir=a["keep"])
        msgs, text = check_case(name, d, a, tmp)
        xf = os.path.join(d, "xfail")
        marked = os.path.exists(xf)
        gap = open(xf).readline().strip() if marked else ""
        if msgs:
            res = "XFAIL" if marked else "FAIL"
        else:
            res = "XPASS" if marked else "PASS"
        counts[res] += 1
        print("%-6s %-20s %s" % (res, name, gap))
        if res in ("FAIL", "XFAIL") or a["verbose"]:
            for m in msgs:
                print("         " + m)
        if res == "FAIL" and not a["verbose"]:
            for l in text.splitlines()[-12:]:
                print("         | " + l)
        if not a["keep"]:
            shutil.rmtree(tmp, ignore_errors=True)
    print("live cases: %d pass, %d fail, %d xfail, %d xpass" % (counts["PASS"], counts["FAIL"], counts["XFAIL"], counts["XPASS"]))
    sys.exit(1 if counts["FAIL"] or counts["XPASS"] else 0)


if __name__ == "__main__":
    main()
