#!/usr/bin/env python3
"""asmdiff: Relax's machine code for a function next to rustc --release's, normalised, with tallies.

  asmdiff.py [options] <fn path>          e.g.  asmdiff.py relax_bench_rt::sort_ints

Relax side: runs the program under `makepad run` (default: this bench_rt crate, kernel --kernel, steady
state BENCH_WARM_MS) with MAKEPAD_RUN_DUMP=<fn> MAKEPAD_RUN_PROFILE (per-instruction samples of the fn), and takes
the O3 code (.t1.bin, else O1). rustc side: the symbol of that path in --native (a release build;
default: built here into $CARGO_TARGET_DIR or <workspace>/target/asmdiff), with the same kernel profile
unavailable (no profiler on macOS) -- only static code.

Options:
  --makepad <bin>        makepad binary (default: env MAKEPAD_RUN_BIN, else `makepad` on PATH)
  --native <bin>       rustc release binary holding the symbol
  --kernel <name>      bench_rt kernel to run (default: the fn's last path segment)
  --run "<args>"       instead of a kernel: `makepad run --quiet <args>` (cwd: --dir), e.g. "makepad-app-amp"
  --dir <dir>          cwd for makepad run (default: this bench_rt dir)
  --dumps <dir>        use existing dumps in <dir> (no run): relax_dump_<fn>.t1.bin / .bin / .prof
  --warm <ms>          BENCH_WARM_MS (default 3000)
  --width <n>          column width (default 64)
  --raw                no register renaming
  --only relax|rustc   one side only
Normalisation: registers renamed per side in order of first appearance (r0.. ints, f0.. float/vector;
sp/fp/lr/rsp/rbp kept), absolute addresses -> ADDR, branch targets -> block labels (.B<n>).
Tallies: instructions, loads, stores (stack-relative ones split out: spills/slots), reg-reg moves,
immediate moves, address arithmetic (lea/adr/add of a shifted index), compares, cond/uncond branches,
calls, setcc/csel/cmov; per block: instruction count and (Relax) sample share.
"""
import os, re, sys, subprocess, platform, struct, collections, shutil, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import demangle_v0

HERE = os.path.dirname(os.path.abspath(__file__))
ARCH = "arm64" if platform.machine() in ("arm64", "aarch64") else "x64"


# ---------------------------------------------------------------- disassembly
def disasm(code, base=0):
    """[(offset, mnemonic, operands)]"""
    try:
        import capstone
        if ARCH == "arm64":
            md = capstone.Cs(capstone.CS_ARCH_ARM64, capstone.CS_MODE_ARM)
        else:
            md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
            md.syntax = capstone.CS_OPT_SYNTAX_INTEL
        out = []
        off = 0
        while off < len(code):
            got = False
            for i in md.disasm(code[off:], base + off):
                out.append((i.address - base, i.mnemonic, i.op_str))
                off = i.address - base + i.size
                got = True
            if off < len(code):
                # undecodable (data / literal pool): stop at a 4-byte boundary step
                out.append((off, ".word", code[off:off + 4].hex()))
                off += 4 if ARCH == "arm64" else 1
            if not got and off >= len(code):
                break
        return out
    except ImportError:
        pass
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as t:
        t.write(code)
    m = ["-maarch64"] if ARCH == "arm64" else ["-mi386:x86-64", "-M", "intel"]
    txt = subprocess.run(["objdump", "-D", "-b", "binary"] + m + ["--no-show-raw-insn", t.name], capture_output=True, text=True).stdout
    os.unlink(t.name)
    out = []
    for l in txt.splitlines():
        mm = re.match(r"\s*([0-9a-f]+):\s+(\S+)\s*(.*)", l)
        if mm:
            ops = re.sub(r"\s+#.*$", "", mm.group(3)).lower()
            out.append((int(mm.group(1), 16), mm.group(2).lower(), ops))
    return out


# ---------------------------------------------------------------- rustc symbols
def demangle_legacy(s):
    s = s.lstrip("_")
    if not s.startswith("ZN"):
        return None
    i = 2
    parts = []
    while i < len(s) and s[i] != "E":
        j = i
        while j < len(s) and s[j].isdigit():
            j += 1
        if j == i:
            return None
        n = int(s[i:j])
        parts.append(s[j:j + n])
        i = j + n
    if parts and re.match(r"h[0-9a-f]{16}$", parts[-1]):
        parts.pop()
    out = "::".join(parts)
    for a, b in (("$LT$", "<"), ("$GT$", ">"), ("$RF$", "&"), ("$BP$", "*"), ("$u20$", " "), ("$u27$", "'"), ("$u5b$", "["), ("$u5d$", "]"), ("$u7b$", "{"), ("$u7d$", "}"), ("$C$", ","), ("..", "::"), ("$u7e$", "~"), ("$LP$", "("), ("$RP$", ")")):
        out = out.replace(a, b)
    return out.replace("::_", "::")


def rust_symbols(binary):
    """{demangled: (file offset, size)} for the text symbols of a native binary"""
    nm = subprocess.run(["nm", "-n", binary], capture_output=True, text=True).stdout
    syms = []
    for l in nm.splitlines():
        p = l.split()
        if len(p) >= 3 and p[1] in ("t", "T"):
            syms.append((int(p[0], 16), p[2]))
    data = open(binary, "rb").read()
    if data[:4] == b"\xcf\xfa\xed\xfe":  # Mach-O 64
        ncmds = struct.unpack_from("<I", data, 16)[0]
        off = 32
        segs = []
        for _ in range(ncmds):
            cmd, sz = struct.unpack_from("<II", data, off)
            if cmd == 0x19:  # LC_SEGMENT_64
                vmaddr, vmsize, fileoff = struct.unpack_from("<QQQ", data, off + 24)
                segs.append((vmaddr, vmsize, fileoff))
            off += sz
        def foff(a):
            for v, n, f in segs:
                if v <= a < v + n:
                    return a - v + f
            return None
    else:  # ELF 64
        shoff = struct.unpack_from("<Q", data, 0x28)[0]
        shentsize, shnum = struct.unpack_from("<HH", data, 0x3a)
        secs = []
        for k in range(shnum):
            b = shoff + k * shentsize
            typ = struct.unpack_from("<I", data, b + 4)[0]
            addr, offs, size = struct.unpack_from("<QQQ", data, b + 16)
            if typ == 1 and addr:
                secs.append((addr, size, offs))
        def foff(a):
            for v, n, f in secs:
                if v <= a < v + n:
                    return a - v + f
            return None
    out = {}
    for k, (a, name) in enumerate(syms):
        end = syms[k + 1][0] if k + 1 < len(syms) else a
        d = demangle_legacy(name) or demangle_v0.demangle(name) or name.lstrip("_")
        fo = foff(a)
        if fo is not None and end > a and d not in out:
            out[d] = (fo, end - a, data)
    return out


# ---------------------------------------------------------------- normalisation + tallies
X64_GPR = {}
for k, names in enumerate([("rax", "eax", "ax", "al"), ("rcx", "ecx", "cx", "cl"), ("rdx", "edx", "dx", "dl"), ("rbx", "ebx", "bx", "bl"),
                           ("rsi", "esi", "si", "sil"), ("rdi", "edi", "di", "dil")]):
    for n in names:
        X64_GPR[n] = names[0]
for r in range(8, 16):
    for suf in ("", "d", "w", "b"):
        X64_GPR["r%d%s" % (r, suf)] = "r%d" % r
KEEP = {"rsp", "rbp", "esp", "ebp", "sp", "fp", "lr", "xzr", "wzr", "x29", "x30", "rip", "w29", "w30"}


def canon_reg(t):
    """canonical register for renaming: (kind, name) or None"""
    if t in KEEP:
        return None
    if ARCH == "x64":
        if t in X64_GPR:
            return ("r", X64_GPR[t])
        m = re.match(r"[xy]mm(\d+)$", t)
        if m:
            return ("f", "xmm" + m.group(1))
        return None
    m = re.match(r"[xw](\d+)$", t)
    if m:
        return ("r", "x" + m.group(1))
    m = re.match(r"[qdsbhv](\d+)$", t)
    if m:
        return ("f", "v" + m.group(1))
    return None


BR_X64 = re.compile(r"^j[a-z]+$")


def is_branch(op):
    if ARCH == "x64":
        return bool(BR_X64.match(op))
    return op in ("b", "br", "cbz", "cbnz", "tbz", "tbnz") or op.startswith("b.")


def is_uncond(op):
    return op in ("jmp", "b", "br", "ret", "ud2", "brk")


def target(op, ops):
    """branch target offset (direct branches), else None"""
    if not is_branch(op) or op == "br":
        return None
    last = ops.split(",")[-1].strip().lstrip("#")
    m = re.match(r"^(0x)?([0-9a-f]+)$", last)
    if not m:
        return None
    return int(m.group(2), 16)


def classify(op, ops):
    """class name of one instruction"""
    mem = "[" in ops
    stack = bool(re.search(r"\[(rbp|rsp|sp|x29|fp)\b", ops))
    if op in ("call", "bl", "blr"):
        return "call"
    if op in ("ret",):
        return "ret"
    if is_branch(op):
        return "branch" if is_uncond(op) else "condbranch"
    if ARCH == "x64":
        if op in ("push", "pop"):
            return "push/pop"
        if op == "lea":
            return "addr (lea)"
        if op in ("cmp", "test", "ucomiss", "ucomisd", "comiss", "comisd"):
            return "cmp/test" if not mem else "load (cmp mem)"
        if op.startswith("set") or op.startswith("cmov"):
            return "setcc/cmov"
        if mem:
            dst = ops.split(",")[0]
            if "[" in dst:
                return "store (stack)" if stack else "store"
            return "load (stack)" if stack else "load"
        if op.startswith("mov") and "," in ops:
            src = ops.split(",", 1)[1].strip()
            if re.match(r"^-?(0x)?[0-9a-f]+$", src):
                return "mov imm"
            return "mov reg"
        return "alu"
    # arm64
    if op in ("ldr", "ldrb", "ldrh", "ldrsb", "ldrsh", "ldrsw", "ldur", "ldurb", "ldurh", "ldursw", "ldp", "ldar", "ldaxr", "ldxr", "ld1", "ldnp"):
        return "load (stack)" if stack else "load"
    if op in ("str", "strb", "strh", "stur", "sturb", "sturh", "stp", "stlr", "stlxr", "stxr", "st1", "stnp"):
        return "store (stack)" if stack else "store"
    if op in ("mov", "fmov") and re.match(r"^[a-z]+\d+, [a-z]+\d+$", ops):
        return "mov reg"
    if op in ("mov", "movz", "movk", "movn", "fmov"):
        return "mov imm"
    if op in ("adr", "adrp") or (op == "add" and ("lsl" in ops or "sxtw" in ops or "uxtw" in ops)):
        return "addr (lea)"
    if op in ("cmp", "cmn", "tst", "fcmp", "ccmp", "fccmp"):
        return "cmp/test"
    if op in ("csel", "cset", "csinc", "csneg", "csinv", "fcsel", "csetm"):
        return "setcc/cmov"
    return "alu"


def normalise(ins, raw):
    """-> list of (offset, text, class, block index)"""
    # blocks: leaders
    leaders = {0}
    for k, (o, op, ops) in enumerate(ins):
        t = target(op, ops)
        if t is not None:
            leaders.add(t)
        if (is_branch(op) or op in ("ret", "ud2", "brk")) and k + 1 < len(ins):
            leaders.add(ins[k + 1][0])
    order = sorted(l for l in leaders if any(o == l for o, _, _ in ins))
    bidx = {o: k for k, o in enumerate(order)}
    names = {}
    cnt = {"r": 0, "f": 0}

    def ren(m):
        t = m.group(0)
        c = canon_reg(t)
        if c is None or raw:
            return t
        if c not in names:
            names[c] = "%s%d" % (c[0], cnt[c[0]])
            cnt[c[0]] += 1
        # keep the width hint on x64 (32-bit forms) / arm64 (w/x, s/d/q)
        w = ""
        if ARCH == "x64" and t[0] == "e" or re.match(r"r\d+d$", t):
            w = ".32"
        elif ARCH == "arm64" and t[0] in "wsdqbh":
            w = "." + t[0]
        return names[c] + w

    out = []
    b = -1
    for o, op, ops in ins:
        if o in bidx:
            b = bidx[o]
        t = target(op, ops)
        pre = ops
        lab = None
        if t is not None and t in bidx:
            pre = ",".join(ops.split(",")[:-1])
            lab = ".B%d" % bidx[t]
        o2 = re.sub(r"\b0xffffffff[0-9a-f]{8}\b", lambda m: str(int(m.group(0), 16) - (1 << 64)), pre)
        o2 = re.sub(r"\b0x[0-9a-f]{8,}\b", "ADDR", o2)
        o2 = re.sub(r"\b[a-z]{1,3}\d{0,2}[a-z]?\b", ren, o2)
        if lab:
            o2 = (o2 + ", " + lab) if o2.strip() else lab
        txt = ("%s %s" % (op, o2)).strip()
        out.append((o, txt, classify(op, ops), b))
    return out, len(order)


# ---------------------------------------------------------------- driver
def relax_side(fn, a):
    base = "relax_dump_" + fn.replace("::", "_")
    d = a["dumps"]
    if not d:
        d = tempfile.mkdtemp(prefix="asmdiff-")
        env = dict(os.environ, MAKEPAD_RUN_DUMP=fn, MAKEPAD_RUN_DUMP_DIR=d, MAKEPAD_RUN_PROFILE=os.path.join(d, "profile.tsv"))
        env.setdefault("SANDBOX_MUTE", "1")
        if a["run"]:
            cmd = [a["relax"], "run", "--quiet"] + a["run"].split()
        else:
            env["BENCH_WARM_MS"] = a["warm"]
            env.setdefault("BENCH_REPS", "20")
            env.setdefault("BENCH_FONT", os.path.normpath(os.path.join(HERE, "../../../../widgets/resources/IBMPlexSans-Text.ttf")))
            env.setdefault("MAKEPAD_RUN_STD", os.path.join(HERE, "../std"))
            cmd = [a["relax"], "run", "--quiet", "relax-bench-rt", "--", a["kernel"] or fn.split("::")[-1]]
        r = subprocess.run(["nice", "-n", "10"] + cmd, cwd=a["dir"], env=env, capture_output=True, text=True)
        if r.returncode != 0:
            sys.stderr.write(r.stderr[-1500:])
    for suf in (".t1", ""):
        p = os.path.join(d, base + suf + ".bin")
        if os.path.exists(p):
            prof = {}
            pp = os.path.join(d, base + suf + ".prof")
            if os.path.exists(pp):
                for l in open(pp):
                    o, n = l.split()
                    prof[int(o, 16)] = int(n)
            return open(p, "rb").read(), prof, "O3" if suf else "O1", d
    sys.exit("no Relax dump of %s in %s (is the name right? does the run reach it?)" % (fn, d))


def native_bin(a):
    if a["native"]:
        return a["native"]
    tgt = os.environ.get("CARGO_TARGET_DIR") or os.path.normpath(os.path.join(HERE, "../../../../target/asmdiff"))
    env = dict(os.environ, CARGO_INCREMENTAL="0", CARGO_TARGET_DIR=tgt)
    subprocess.run(["nice", "-n", "10", "cargo", "build", "--release", "-q", "-j", "4"], cwd=HERE, env=env, check=True)
    return os.path.join(tgt, "release", "relax-bench-rt")


def rustc_side(fn, a):
    binary = native_bin(a)
    syms = rust_symbols(binary)
    if fn in syms:
        fo, n, data = syms[fn]
        return data[fo:fo + n], fn
    last = fn.split("::")[-1]
    cands = [s for s in syms if s.split("::")[-1] == last or s.endswith("::" + last)]
    if len(cands) == 1:
        fo, n, data = syms[cands[0]]
        return data[fo:fo + n], cands[0]
    sys.stderr.write("rustc: no symbol %s in %s%s\n" % (fn, binary, (" (candidates: %s)" % ", ".join(cands[:8])) if cands else " (inlined?)"))
    return None, None


CLASSES = ["load", "load (stack)", "load (cmp mem)", "store", "store (stack)", "mov reg", "mov imm", "addr (lea)", "alu", "cmp/test", "setcc/cmov", "condbranch", "branch", "call", "ret", "push/pop"]


def tally(rows):
    c = collections.Counter(r[2] for r in rows)
    return c


def main():
    args = sys.argv[1:]
    a = {"relax": os.environ.get("MAKEPAD_RUN_BIN", "makepad"), "native": None, "kernel": None, "run": None, "dir": HERE, "dumps": None, "warm": "3000", "width": 64, "raw": False, "only": None}
    fn = None
    i = 0
    while i < len(args):
        x = args[i]
        if x in ("--makepad", "--native", "--kernel", "--run", "--dir", "--dumps", "--warm", "--only"):
            a[x[2:]] = args[i + 1]
            i += 2
        elif x == "--width":
            a["width"] = int(args[i + 1])
            i += 2
        elif x == "--raw":
            a["raw"] = True
            i += 1
        elif x in ("-h", "--help"):
            print(__doc__)
            return
        else:
            fn = x
            i += 1
    if not fn:
        sys.exit(__doc__)
    sides = []
    if a["only"] != "rustc":
        code, prof, tier, d = relax_side(fn, a)
        rows, nb = normalise(disasm(code), a["raw"])
        tot = sum(prof.values()) or 1
        sides.append(("Relax %s (%d B)" % (tier, len(code)), rows, nb, {o: 100.0 * n / tot for o, n in prof.items()}))
    if a["only"] != "relax":
        code, sym = rustc_side(fn, a)
        if code is not None:
            rows, nb = normalise(disasm(code), a["raw"])
            sides.append(("rustc %s (%d B)" % (sym if sym != fn else "", len(code)), rows, nb, {}))
    # tallies
    print("== %s (%s)" % (fn, ARCH))
    print("%-16s" % "" + "".join("%14s" % s[0].split()[0] for s in sides) + ("%10s" % "ratio" if len(sides) == 2 else ""))
    tals = [tally(s[1]) for s in sides]
    for k in ["instructions"] + CLASSES + ["blocks"]:
        vals = [len(s[1]) if k == "instructions" else s[2] if k == "blocks" else t.get(k, 0) for s, t in zip(sides, tals)]
        if not any(vals):
            continue
        line = "%-16s" % k + "".join("%14d" % v for v in vals)
        if len(vals) == 2:
            line += "%10s" % ("%.2f" % (vals[0] / vals[1]) if vals[1] else "-")
        print(line)
    # hot blocks of the Relax side (sample shares)
    if sides and sides[0][3]:
        pb = collections.Counter()
        nb_ins = collections.Counter()
        for o, t, c, b in sides[0][1]:
            pb[b] += sides[0][3].get(o, 0.0)
            nb_ins[b] += 1
        print("Relax hot blocks: " + ", ".join(".B%d %.0f%% (%d insts)" % (b, p, nb_ins[b]) for b, p in pb.most_common(8) if p >= 1.0))
    # side by side
    w = a["width"]
    cols = []
    for title, rows, nb, prof in sides:
        lines = [title[:w]]
        bins = collections.Counter(r[3] for r in rows)
        last = None
        for o, t, c, b in rows:
            if b != last:
                ps = sum(prof.get(o2, 0.0) for o2, _, _, b2 in rows if b2 == b) if prof else None
                lines.append((".B%d: %d insts%s" % (b, bins[b], (" %.1f%%" % ps) if ps is not None else ""))[:w])
                last = b
            p = prof.get(o)
            lines.append(("%5.1f " % p if p else "      " if prof else "") + "  " + t)
        cols.append(lines)
    n = max(len(c) for c in cols)
    for k in range(n):
        print("".join(("%-*s" % (w, (c[k] if k < len(c) else "")[:w - 1])) for c in cols).rstrip())


if __name__ == "__main__":
    main()
