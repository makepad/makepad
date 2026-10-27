#!/usr/bin/env python3
# sbstage.py <snapdir> <workload> [--rounds 3] [--settle 60] : ONE scoreboard stage = every launch of one workload,
# interleaved per round: native-cpu, native-ship, JIT. JIT launch 1 is cold (fresh MAKEPAD_RUN_HOME), launches 2..n
# are warm (same home). Run it as one timing stage: `makepad devhub run <lane> timed --node <n> -- python3 tools/run/scoreboard/sbstage.py ...`.
# Results append to <snapdir>/results.jsonl (sb.py lines + variant/commit/run). App and game workloads get a
# private headless weston (Linux) and a private GL shader cache per variant.
import argparse, os, subprocess, sys, time, shutil, signal

HERE = os.path.dirname(os.path.abspath(__file__))
COMMERCIAL_RW = ('sandbox_sim', 'level_walk', 'model_build')
NATIVE_BIN = {'calculator': 'makepad-app-calculator', 'code': 'makepad-app-code', 'amp': 'makepad-app-amp',
              'stage': 'makepad-stage', 'story': 'makepad-story', 'scope': 'scope', 'sandbox': 'makepad-sandbox',
              'box3d': 'makepad-example-box3d'}


def native_bin(snap, variant, wl):
    fam, _, name = wl.partition(':')
    if fam == 'rw':
        b = 'relax-rwbench-commercial' if name in COMMERCIAL_RW else 'relax-rwbench'
    elif fam == 'game':
        b = 'makepad-sandbox'
    elif fam == 'stage':
        b = 'makepad-story'
    else:
        b = NATIVE_BIN.get(name, name)
    kept = os.path.join(snap, 'bins', variant, b)
    return kept if os.path.exists(kept) else os.path.join(snap, variant, 'release', b)


def busy_cores(dt=5.0):
    """Linux: machine-wide busy cores over dt s (/proc/stat); None elsewhere"""
    def rd():
        v = list(map(int, open('/proc/stat').readline().split()[1:]))
        return sum(v) - v[3] - v[4]
    try:
        a = rd(); time.sleep(dt); b = rd()
    except OSError:
        return None
    return (b - a) / os.sysconf('SC_CLK_TCK') / dt


def wait_quiet(limit, max_wait):
    """wait until other load < limit cores (3 checks in a row), at most max_wait s; returns the last reading"""
    t, ok, c = time.time(), 0, None
    while time.time() - t < max_wait:
        c = busy_cores()
        if c is None:
            return None
        ok = ok + 1 if c < limit else 0
        if ok >= 3:
            break
    return c


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('snap')
    ap.add_argument('wl')
    ap.add_argument('--rounds', type=int, default=3)
    ap.add_argument('--settle', type=float, default=60)
    ap.add_argument('--native-settle', type=float, default=0, help='rw: native run length (s); default min(settle, 20)')
    ap.add_argument('--variants', default='ncpu,nship,jit')
    ap.add_argument('--cap', type=float, default=600)
    a = ap.parse_args()
    snap = os.path.abspath(a.snap)
    src = os.path.join(snap, 'src')
    commit = open(os.path.join(snap, 'TREE')).read().split()
    loader = os.path.join(snap, 'bins', 'jit', 'makepad')
    if not os.path.exists(loader):
        loader = os.path.join(snap, 'jit', 'release', 'makepad')
    out = os.environ.get('SB_RESULTS') or os.path.join(snap, 'results.jsonl')
    fam = a.wl.split(':')[0]
    tag = a.wl.replace(':', '-')
    hroot = os.path.join(snap, 'h', tag)
    if os.path.isdir(hroot):
        shutil.rmtree(hroot)  # this stage's own homes only (fresh cold JIT home every stage)
    os.makedirs(hroot)
    env = dict(os.environ)
    wpid = None
    if fam != 'rw' and sys.platform.startswith('linux'):
        sock = 'wayland-sb-%d' % os.getpid()
        env.update(XDG_RUNTIME_DIR=env.get('XDG_RUNTIME_DIR', '/run/user/%d' % os.getuid()), WAYLAND_DISPLAY=sock)
        env.pop('DISPLAY', None)
        wl = subprocess.Popen(['weston', '--backend=headless', '--renderer=gl', '--socket=' + sock, '--width=1600',
                               '--height=900', '--idle-time=0'], stdout=open(os.path.join(hroot, 'weston.log'), 'w'),
                              stderr=subprocess.STDOUT, start_new_session=True, env=env)
        wpid = wl.pid
        time.sleep(2)
    variants = a.variants.split(',')
    # a stopped stage (SIGTERM to its group) still stops its weston (finally below)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit(143)))
    q = wait_quiet(float(os.environ.get('SB_QUIET_CORES', '0.6')), float(os.environ.get('SB_QUIET_WAIT', '900')))
    print('%s: box load before the stage %s cores' % (a.wl, None if q is None else round(q, 2)), flush=True)
    try:
        for r in range(1, a.rounds + 1):
            for v in variants:
                if v == 'jit':
                    binp, kind, home = loader, 'jit', os.path.join(hroot, 'jit')
                    mode = 'cold' if r == 1 else 'warm'
                else:
                    binp, kind, home = native_bin(snap, v, a.wl), 'native', os.path.join(hroot, '%s-%d' % (v, r))
                    mode = '-'
                    if not os.path.exists(binp):
                        print('SKIP %s %s: no %s' % (a.wl, v, binp), flush=True)
                        continue
                settle = a.settle
                if kind == 'native' and fam in ('rw', 'start'):
                    settle = a.native_settle or min(a.settle, 20)
                e = dict(env, __GL_SHADER_DISK_CACHE_PATH=os.path.join(snap, 'gl-' + v), __GL_SHADER_DISK_CACHE_SKIP_CLEANUP='1')
                os.makedirs(e['__GL_SHADER_DISK_CACHE_PATH'], exist_ok=True)
                cmd = [sys.executable, os.path.join(HERE, 'sb.py'), '--wl', a.wl, '--kind', kind, '--bin', binp,
                       '--loader', loader, '--src', src, '--home', home, '--label', '%s-%s-%d' % (tag, v, r),
                       '--settle', str(settle), '--cap', str(a.cap), '--out', out,
                       '--meta', 'variant=' + v, '--meta', 'mode=' + mode, '--meta', 'run=%d' % r,
                       '--meta', 'commit=' + commit[0], '--meta', 'commercial=' + (commit[1] if len(commit) > 1 else '-')]
                t = time.time()
                p = subprocess.run(cmd, env=e, capture_output=True, text=True)
                line = (p.stdout.strip().splitlines() or ['(no output) ' + p.stderr[-300:]])[-1]
                print('%s %s r%d %.0fs %s' % (a.wl, v, r, time.time() - t, line[:300]), flush=True)
    finally:
        if wpid:
            try:
                os.killpg(wpid, signal.SIGTERM)
            except OSError:
                pass


if __name__ == '__main__':
    main()
