#!/usr/bin/env python3
"""A held landing waits; the person's own saves apply live meanwhile.

usage: landing_hold.py [--makepad <makepad binary>]
  (run from the workspace root; the JIT runs through `cargo run --profile host` unless a binary is
  given; RELAX_STD defaults to libs/relax/compiler/std)

devhub holds back a landing in a Mac show's JIT until the landed tip is proven for the app
(`inspect hold files=<path>=<blob id>`, libs/devhub/src/orch/show.rs before_landing), and lets it
in with `hold off`. The hold must never delay the person: only a file that says just what the
landing wrote waits. A program that never presents is written to a fresh dir (its own
RELAX_HOME) and runs live under `makepad run --verbose --ahead=minimal`, printing
`out <landed::value()> <mine::value()> <data.txt, included> <base()>` when that changes:
  1. the landing of src/landed.rs (1 -> 2) is held, then written: the watch says it waits, and
     the program still prints 1 for it;
  2. the person saves src/mine.rs (1 -> 2) meanwhile: it applies at once (`out 1 2`);
  3. `hold off`: the landing goes in (`out 2 2`);
  4. a second landing of src/landed.rs (2 -> 3) is held and written, and the person saves the same
     file with other text (4): that is the person's save, and it applies (`out 4 2 ..`, never 3);
  5. a landing of src/data.txt (1 -> 2), which main.rs includes, is held and written, and the
     person saves main.rs (base 0 -> 5): the save applies and its include reads data.txt as it was
     before the landing (`out 4 2 1 5`, never `4 2 2 5`); `hold off` lets the landing in.
  6. two landings are held and written, P (landed.rs 4 -> 5, data.txt 2 -> 3) and then T (landed.rs
     5 -> 6, mine.rs 2 -> 7); P is proven: `hold take dir=<P's texts> files=<P's files>` lets P in
     (`out 5 2 3 5`, never 6 or 7) and keeps T's landed.rs and mine.rs held (`"held":2`): data.txt
     lands as P has it (its hold goes), landed.rs reads P's text; `hold off` lets T in (`out 6 7 3 5`).
Red without the hold's content check (reload.rs held_back): with the JIT's pin, which held every
file, step 2 waits for step 3. Red without the held reads (vfs::hold_read): step 5 runs data 2.
Red without the take (reload.rs take_landing): step 6 waits for its `hold off`; without the release
looking at a taken file again (LANDING_FORCED), T's landed.rs never comes in.
Prints `landing_hold: ok` or `landing_hold: FAIL <why>`; exit 1 on failure.
LANDING_HOLD_TRACE=1 also prints the run's output.
"""
import hashlib, os, re, shutil, subprocess, sys, tempfile, threading, time

ROOT = os.getcwd()
WAIT = float(os.environ.get("LANDING_HOLD_WAIT", "300"))
MAIN = """mod landed;
mod mine;

use std::time::{Duration, Instant};

#[inline(never)]
fn data() -> u64 {
    include_str!("data.txt").trim().parse().unwrap_or(0)
}

#[inline(never)]
fn base() -> u64 {
    0
}

// prints the values when they change, until LANDING_HOLD_STOP names a file that exists
fn main() {
    let stop = std::env::var("LANDING_HOLD_STOP").unwrap_or_default();
    let start = Instant::now();
    let mut last = (u64::MAX, u64::MAX, u64::MAX, u64::MAX);
    while start.elapsed() < Duration::from_secs(600) && !std::path::Path::new(&stop).exists() {
        let v = (landed::value(), mine::value(), data(), base());
        if v != last {
            println!("out {} {} {} {}", v.0, v.1, v.2, v.3);
            last = v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("out end");
}
"""
BASE = ("fn base() -> u64 {\n    0\n}", "fn base() -> u64 {\n    5\n}")
VALUE = "#[inline(never)]\npub fn value() -> u64 {\n    %d\n}\n"
CARGO = """[package]
name = "relax-landing-hold"
version = "0.1.0"
edition = "2021"
publish = false

[workspace]
"""
BROKE = ("panicked", "internal compiler error", "FATAL", "the watcher stopped")


def holds(said, n):
    """Whether an answer to `hold` says it holds n files (`{"held":n}`, or the CLI's `held: n`)."""
    return re.search(r'"?held"?\s*:\s*%d\b' % n, said) is not None


def blob_id(text):
    data = text.encode()
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


class Run:
    """The live run: its stdout and stderr lines as they arrive."""
    def __init__(self, jit, work, home, pkg):
        self.jit, self.out, self.err = jit, [], []
        self.ready, self.stop = os.path.join(work, "ready"), os.path.join(work, "stop")
        env = dict(os.environ, RELAX_LIVE_READY=self.ready, RELAX_HOME=home, RELAX_CACHE_DIR=os.path.join(home, "cache"),
                   RELAX_TARGET_DIR=os.path.join(home, "target"), MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", LANDING_HOLD_STOP=self.stop)
        env.setdefault("RELAX_STD", os.path.join(ROOT, "libs/relax/compiler/std"))
        self.p = subprocess.Popen(jit + ["run", "--quiet", "--verbose", "--ahead=minimal", "relax-landing-hold"], cwd=pkg, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, errors="replace", start_new_session=True)
        threading.Thread(target=self.pump, args=(self.p.stdout, self.out), daemon=True).start()
        threading.Thread(target=self.pump, args=(self.p.stderr, self.err), daemon=True).start()

    @staticmethod
    def pump(f, acc):
        for line in f:
            acc.append(line.rstrip("\n"))

    def has_err(self, s, since=0):
        return any(s in l for l in self.err[since:])

    def wait(self, pred, secs):
        t = time.time()
        while time.time() - t < secs:
            if pred():
                return True
            if self.p.poll() is not None:
                time.sleep(0.2)
                return pred()
            time.sleep(0.05)
        return False

    def inspect(self, *words):
        m = next((re.search(r"makepad-remote: pid (\d+)", l) for l in self.err if "makepad-remote: pid " in l), None)
        pid = m.group(1) if m else str(self.p.pid)
        c = subprocess.run(self.jit + ["inspect", pid] + list(words), capture_output=True, text=True, errors="replace")
        return c.stdout + c.stderr

    def end(self):
        """Stops the program and waits for the run to end: a line saying it broke, or None."""
        open(self.stop, "w").close()
        self.p.wait()
        time.sleep(0.2)
        return next(("broke: " + l.strip()[:200] for l in self.err if any(b in l for b in BROKE)), None)

    def show(self):
        if os.environ.get("LANDING_HOLD_TRACE"):
            for l in self.err + self.out:
                print("  | %s" % l[:240], flush=True)


def save(path, text):
    # a new mtime even within the same second: write, then bump the time
    with open(path, "w") as f:
        f.write(text)
    st = os.stat(path)
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))


def steps(r, landed, mine, data, main):
    if not r.wait(lambda: os.path.exists(r.ready) and "out 1 1 1 0" in r.out, WAIT):
        return "not up (%s)" % (r.err[-1][:160] if r.err else "no output")
    # 1. a held landing waits
    said = r.inspect("hold", "files=src/landed.rs=%s" % blob_id(VALUE % 2))
    if not holds(said, 1):
        return "the JIT did not hold the landing (%s)" % said.strip()[:200]
    n = len(r.err)
    save(landed, VALUE % 2)
    if not r.wait(lambda: r.has_err("landed.rs waits", n), 20):
        return "the watch did not say the held landing's file waits"
    time.sleep(1.0)
    if "out 2 1 1 0" in r.out:
        return "the held landing reached the program"
    # 2. the person's save applies at once
    t = time.time()
    save(mine, VALUE % 2)
    if not r.wait(lambda: "out 1 2 1 0" in r.out, 20):
        return "the person's save did not apply while a landing was held (out: %s)" % ", ".join(r.out[-4:])
    print("  the person's save applied in %.0f ms with the landing held" % ((time.time() - t) * 1e3), flush=True)
    # 3. released: the landing goes in
    said = r.inspect("hold", "off")
    if not r.wait(lambda: "out 2 2 1 0" in r.out, 20):
        return "`hold off` did not let the landing in (%s; out: %s)" % (said.strip()[:120], ", ".join(r.out[-4:]))
    # 4. the person's own text in a held file is the person's save
    said = r.inspect("hold", "files=src/landed.rs=%s" % blob_id(VALUE % 3))
    if not holds(said, 1):
        return "the JIT did not hold the second landing (%s)" % said.strip()[:200]
    n = len(r.err)
    save(landed, VALUE % 3)
    if not r.wait(lambda: r.has_err("landed.rs waits", n), 20):
        return "the watch did not say the second landing's file waits"
    save(landed, VALUE % 4)
    if not r.wait(lambda: "out 4 2 1 0" in r.out, 20):
        return "the person's save of a held file did not apply (out: %s)" % ", ".join(r.out[-4:])
    if "out 3 2 1 0" in r.out:
        return "the held landing's text reached the program"
    # 5. a save that reads a held file again reads it as it was before the landing
    said = r.inspect("hold", "files=src/data.txt=%s" % blob_id("2\n"))
    if not holds(said, 1):
        return "the JIT did not hold the landing of data.txt (%s)" % said.strip()[:200]
    n = len(r.err)
    save(data, "2\n")
    if not r.wait(lambda: r.has_err("data.txt waits", n), 20):
        return "the watch did not say the held data.txt waits"
    save(main, open(main).read().replace(BASE[0], BASE[1]))
    if not r.wait(lambda: any(o.startswith("out 4 2 ") and o.endswith(" 5") for o in r.out), 30):
        return "the person's save of main.rs did not apply while data.txt was held (out: %s)" % ", ".join(r.out[-4:])
    if "out 4 2 1 5" not in r.out:
        return "the save's include read the held landing's data.txt (out: %s)" % ", ".join(r.out[-4:])
    said = r.inspect("hold", "off")
    if not r.wait(lambda: "out 4 2 2 5" in r.out, 30):
        return "`hold off` did not let the landing of data.txt in (%s; out: %s)" % (said.strip()[:120], ", ".join(r.out[-4:]))
    # 6. a held stream of landings moves to the proven one, the newer one still held
    said = r.inspect("hold", "files=src/landed.rs=%s\nsrc/data.txt=%s" % (blob_id(VALUE % 5), blob_id("3\n")))
    if not holds(said, 2):
        return "the JIT did not hold landing P (%s)" % said.strip()[:200]
    n = len(r.err)
    save(landed, VALUE % 5)
    save(data, "3\n")
    if not r.wait(lambda: r.has_err("landed.rs waits", n) and r.has_err("data.txt waits", n), 20):
        return "the watch did not say landing P's files wait"
    said = r.inspect("hold", "files=src/landed.rs=%s\nsrc/mine.rs=%s" % (blob_id(VALUE % 6), blob_id(VALUE % 7)))
    if not holds(said, 3):
        return "the JIT did not hold landing T (%s)" % said.strip()[:200]
    n = len(r.err)
    save(landed, VALUE % 6)
    save(mine, VALUE % 7)
    if not r.wait(lambda: r.has_err("mine.rs waits", n), 20):
        return "the watch did not say landing T's mine.rs waits"
    time.sleep(1.0)
    if r.out[-1] != "out 4 2 2 5":
        return "a held landing reached the program (out: %s)" % ", ".join(r.out[-4:])
    p_dir = os.path.join(os.path.dirname(r.ready), "take")
    os.makedirs(os.path.join(p_dir, "src"))
    open(os.path.join(p_dir, "src", "landed.rs"), "w").write(VALUE % 5)
    open(os.path.join(p_dir, "src", "data.txt"), "w").write("3\n")
    said = r.inspect("hold", "take", "dir=%s" % p_dir, "files=src/landed.rs=%s\nsrc/data.txt=%s" % (blob_id(VALUE % 5), blob_id("3\n")))
    if not holds(said, 2) or not re.search(r'"?took"?\s*:\s*2\b', said):
        return "`hold take` did not take P's two files and keep T's two held (%s)" % said.strip()[:200]
    shutil.rmtree(p_dir)
    if not r.wait(lambda: "out 5 2 3 5" in r.out, 30):
        return "`hold take` did not let landing P in (out: %s)" % ", ".join(r.out[-4:])
    time.sleep(1.0)
    if any(o.startswith("out 6 ") or o.startswith("out 5 7 ") for o in r.out):
        return "landing T reached the program on P's take (out: %s)" % ", ".join(r.out[-4:])
    said = r.inspect("hold", "off")
    if not r.wait(lambda: "out 6 7 3 5" in r.out, 30):
        return "`hold off` after a take did not let landing T in (%s; out: %s)" % (said.strip()[:120], ", ".join(r.out[-4:]))
    return None


def check(jit, work):
    pkg, home = os.path.join(work, "pkg"), os.path.join(work, "home")
    os.makedirs(os.path.join(pkg, "src"))
    open(os.path.join(pkg, "Cargo.toml"), "w").write(CARGO)
    main = os.path.join(pkg, "src", "main.rs")
    open(main, "w").write(MAIN)
    landed, mine, data = os.path.join(pkg, "src", "landed.rs"), os.path.join(pkg, "src", "mine.rs"), os.path.join(pkg, "src", "data.txt")
    for f in (landed, mine):
        open(f, "w").write(VALUE % 1)
    open(data, "w").write("1\n")
    r = Run(jit, work, home, pkg)
    why = steps(r, landed, mine, data, main)
    broke = r.end()
    r.show()
    return why or broke


args = sys.argv[1:]
if "--help" in args or "-h" in args:
    sys.exit(__doc__)
if "--makepad" in args:
    jit = [os.path.abspath(args[args.index("--makepad") + 1])]
else:
    jit = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
os.makedirs(os.path.join(ROOT, "target"), exist_ok=True)
work = tempfile.mkdtemp(prefix="landing-hold-", dir=os.path.join(ROOT, "target"))
why = check(jit, work)
if why:
    print("landing_hold: FAIL %s (runs in %s)" % (why, work))
    sys.exit(1)
shutil.rmtree(work)
print("landing_hold: ok")
