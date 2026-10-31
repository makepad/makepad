#!/usr/bin/env python3
# sb.py (lane BENCHALL): ONE measured launch of one scoreboard workload, native rustc or JIT (`makepad run`).
# Linux (/proc) and macOS (ps); Python 3.9 (the mini's). Prints one JSON line (stdout + --out).
#
#   sb.py --wl <workload> --kind native|jit --bin <exe> --src <tree> --home <dir> --label L
#         [--loader <makepad>] [--settle 60] [--out f.jsonl] [--cap 900]
#
# Workloads (the adapter decides the command, the end of the run and the per-op unit):
#   rw:<name>    rwbench workload (relax-rwbench / relax-rwbench-commercial). The run lasts --settle s
#                (RW_MS); op = one iteration. cpu/op = steady_ms * process cores over the last 10 s.
#   game:<name>  Sandbox game resources/games/<name> under SANDBOX_DET (view/det.rs); op = one frame.
#                cpu/op = whole-process CPU over the DET windows starting >= --settle s / their frames.
#
# Every launch records, from a 1-s sampler over the launch's own process tree (never by name):
#   cpu      whole-process CPU per op (ms), settled (>= --settle s after launch)
#   ttfp     time-to-final-perf: first op window within 5% of the settled level (s since launch)
#   tti      time-to-idle: start of the last stretch whose 10-s mean CPU of the non-program threads stays
#            under 2% of a core (s since launch; None = never idle)
#   first    first useful work (s): rw = main entered (process age at main); game = first DET window start
#   rss      settled RSS (MB, the tree's sum at the last sample) and peak RSS (MB, sampled)
#   cpu_to_idle  whole-process CPU seconds from launch to tti (the compile bill)
# The program thread = the thread with the most CPU in the settled window (Linux); on macOS (no per-thread
# CPU via ps) tti uses whole-process CPU above the settled level.
import argparse, json, os, re, signal, subprocess, sys, threading, time

IS_LINUX = os.path.isdir('/proc/self/task')


def tree(root):
    r = subprocess.run(['ps', '-A', '-o', 'pid=,ppid='], capture_output=True, text=True).stdout
    kids = {}
    for line in r.splitlines():
        f = line.split()
        if len(f) == 2:
            kids.setdefault(int(f[1]), []).append(int(f[0]))
    out, todo = [], [root]
    while todo:
        q = todo.pop()
        if q in out:
            continue
        out.append(q)
        todo += kids.get(q, [])
    return out


def sample_linux(pids):
    """({(pid,tid): (name, cpu s)}, rss MB)"""
    hz = os.sysconf('SC_CLK_TCK')
    th, rss = {}, 0.0
    for q in pids:
        try:
            for l in open('/proc/%d/status' % q):
                if l.startswith('VmRSS:'):
                    rss += int(l.split()[1]) / 1024.0
            # reaped children's CPU (short-lived helpers that exit between samples: linkers, tools)
            st = open('/proc/%d/stat' % q).read()
            f = st[st.rindex(')') + 2:].split()
            th[(q, -1)] = ('children', (int(f[13]) + int(f[14])) / hz)
            tids = os.listdir('/proc/%d/task' % q)
        except OSError:
            continue
        for t in tids:
            try:
                st = open('/proc/%d/task/%s/stat' % (q, t)).read()
            except OSError:
                continue
            i = st.rindex(')')
            f = st[i + 2:].split()
            th[(q, int(t))] = (st[st.index('(') + 1:i], (int(f[11]) + int(f[12])) / hz)
    return th, rss


def ps_time(s):
    # macOS ps cputime: [[dd-]hh:]mm:ss.cc
    d = 0
    if '-' in s:
        d, s = s.split('-', 1)
    p = [float(x) for x in s.split(':')]
    while len(p) < 3:
        p.insert(0, 0.0)
    return int(d) * 86400 + p[0] * 3600 + p[1] * 60 + p[2]


def sample_ps(pids):
    r = subprocess.run(['ps', '-o', 'pid=,rss=,cputime=', '-p', ','.join(map(str, pids))], capture_output=True, text=True).stdout
    th, rss = {}, 0.0
    for line in r.splitlines():
        f = line.split()
        if len(f) == 3:
            rss += int(f[1]) / 1024.0
            th[(int(f[0]), 0)] = ('proc', ps_time(f[2]))
    return th, rss


class Sampler:
    """per-second series: [t, total cores-s in the second, {thread key: cpu s}, rss MB]"""
    def __init__(self, p, t0):
        self.p, self.t0, self.series, self.stop = p, t0, [], threading.Event()
        self.th = threading.Thread(target=self.run, daemon=True)
        self.th.start()

    def sys_cpu(self):
        # Linux: whole-machine busy CPU seconds (/proc/stat); the launch's own share is subtracted -> other load
        try:
            f = open('/proc/stat').readline().split()[1:]
            v = list(map(int, f))
            return (sum(v) - v[3] - v[4]) / os.sysconf('SC_CLK_TCK')
        except (OSError, ValueError, IndexError):
            return None

    def run(self):
        prev = {}
        sp = self.sys_cpu()
        self.other = []
        while not self.stop.wait(1.0):
            if self.p.poll() is not None:
                return
            pids = tree(self.p.pid)
            cur, rss = (sample_linux if IS_LINUX else sample_ps)(pids)
            per = {}
            for k, (name, c) in cur.items():
                if k in prev:
                    d = c - prev[k][1]
                    if d > 0:
                        per['%s/%d' % (name, k[1])] = round(d, 4)
            prev = cur
            sc = self.sys_cpu()
            if sc is not None and sp is not None:
                self.other.append(max(0.0, sc - sp - sum(per.values())))
            sp = sc
            self.series.append([round(time.time() - self.t0, 2), round(sum(per.values()), 4), per, round(rss, 1)])

    def done(self):
        self.stop.set()
        self.th.join(3)


def cum_at(series, t):
    """whole-process CPU seconds used from launch to time t (linear between samples; exact at sample times)"""
    prev_t, acc = 0.0, 0.0
    for st, c, _, _ in series:
        if st >= t:
            return acc + c * (t - prev_t) / max(1e-6, st - prev_t)
        acc += c
        prev_t = st
    return acc


def window_cpu(series, a, b):
    """whole-process CPU s in (a, b] (interpolated) and per-thread CPU s of the samples inside it"""
    per = {}
    for t, c, th, _ in series:
        if a < t <= b:
            for k, v in th.items():
                per[k] = per.get(k, 0.0) + v
    return cum_at(series, b) - cum_at(series, a), per


def jit_cpu(th):
    return sum(v for k, v in th.items() if k.startswith('run-'))


def idle_metrics(series, kind, settled_level=None, end=None):
    """tti: start of the last stretch where the 10-s mean JIT compile CPU (Linux: threads `run-*`; macOS: the
    whole process above its settled level) stays < 0.02 cores. Native: None (nothing to compile)."""
    if kind != 'jit':
        return None, None
    s = [x for x in series if end is None or x[0] <= end]
    if IS_LINUX:
        bg = [(t, jit_cpu(th)) for t, c, th, _ in s]
    else:
        lvl = settled_level or 0.0
        bg = [(t, max(0.0, c - lvl)) for t, c, th, _ in s]
    tti = None
    for k in range(len(bg)):
        w = bg[k:k + 10]
        m = sum(v for _, v in w) / max(1, len(w))
        if m >= 0.02:
            tti = None
        elif tti is None:
            tti = bg[k][0]
    if tti is not None and s and tti >= s[-1][0] - 5:
        tti = None  # idle only in the last few samples: not established
    cti = round(sum(c for t, c, _, _ in s if t <= tti), 2) if tti is not None else None
    return tti, cti


def common_env(home):
    env = dict(os.environ)
    for d in ('t', 'userhome', 'root/local'):
        os.makedirs(os.path.join(home, d), exist_ok=True)
    env.update({'SANDBOX_MUTE': '1', 'MUSIC_MUTE': '1', 'MAKEPAD_GGML_NO_CUDA': '1', 'MAKEPAD_SEED': '1',
                'TMPDIR': os.path.join(home, 't'), 'HOME': os.path.join(home, 'userhome'),
                'MAKEPAD_ROOT': os.path.join(home, 'root'), 'RUST_MIN_STACK': '16777216'})
    # Linux: hidden windows on the bench box's private headless weston. macOS (the mini, a bench box with no user):
    # visible windows -- a hidden macOS window is throttled to ~10 Hz. SB_VISIBLE=0/1 overrides.
    if os.environ.get('SB_VISIBLE', '1' if sys.platform == 'darwin' else '0') != '1':
        env['MAKEPAD_HIDE_WINDOWS'] = '1'
    return env


def jit_env(env, home, src):
    rh = os.path.join(home, 'run')
    env.update({'MAKEPAD_RUN_HOME': rh, 'MAKEPAD_RUN_TARGET_DIR': rh + '/target', 'MAKEPAD_RUN_CACHE_DIR': rh + '/cache',
                'MAKEPAD_RUN_STD': os.path.join(src, 'libs/relax/compiler/std'),
                'MAKEPAD_RUN_O3_PROGRESS': os.path.join(home, 'o3-progress')})
    return rh


RWLINE = re.compile(r"^\| (\w+) steady_ms=([\d.]+) tts_main_ms=([\d.]+) tts_proc_ms=([\d.]+) iters=(\d+) first_ms=([\d.]+) o3_ms=(-?\d+) (\d+)", re.M)
DET = re.compile(r"DET window (\d+) frames (\d+) ticks (\d+) tick_ins (\d+) tick_cyc (\d+) draw_ins (\d+) draw_cyc (\d+) ui_ins (\d+) ui_cyc (\d+) wall_ms (\d+) digest ([0-9a-f]+)")
DETSTART = re.compile(r"DET window (\d+) start frame")


class Ctx:
    """what an adapter sees: the launch's env (edit it in prepare), paths, and helpers while it drives."""
    def __init__(self, a, src, home, binp, loader, env):
        self.a, self.src, self.home, self.bin, self.loader, self.env = a, src, home, binp, loader, env
        self.kind, self.settle, self.cap = a.kind, a.settle, a.cap
        self.p = self.t0 = self.logp = None
        self.res = {}
        self._pos = 0

    def now(self):
        return time.time() - self.t0

    def alive(self):
        return self.p.poll() is None and self.now() < self.cap

    def idle_start(self):
        """live: when the JIT went (and stayed) idle, s since launch; None while it compiles. Native: 0.
        Linux: the 10-s mean CPU of the compile threads (run-*) < 0.02 cores. macOS: MAKEPAD_RUN_O3_PROGRESS
        reads 0 pending with an unchanged install count for 10 s."""
        if self.kind != 'jit':
            return 0.0
        if IS_LINUX:
            S = list(self.smp.series)
            if len(S) < 10:
                return None
            start = None
            for k in range(len(S) - 9):
                if sum(jit_cpu(x[2]) for x in S[k:k + 10]) / 10.0 < 0.02:
                    if start is None:
                        start = S[k][0]
                else:
                    start = None
            return start
        try:
            f = open(self.env['MAKEPAD_RUN_O3_PROGRESS']).read().split()
            cur = (int(f[0]), int(f[1]))
        except (OSError, ValueError, IndexError, KeyError):
            return None
        if cur[0] != 0 or cur != getattr(self, '_prog', None):
            self._prog, self._prog_t = cur, self.now()
            return None
        return self._prog_t if self.now() - self._prog_t >= 10 else None

    def settled_from(self):
        """live: settled windows start after both the settle time and the JIT's idle start (None: not yet)"""
        i = self.idle_start()
        if i is not None and getattr(self, 'idle_seen', None) is None:
            self.idle_seen = i  # macOS: the live progress-file reading is the only idle signal
        if i is None and self.now() > self.settle + float(os.environ.get('SB_IDLE_WAIT', '240')):
            return self.now()  # never idle: measure from here on (sb.py flags never_idle)
        return None if i is None else max(self.settle, i)

    def new_log(self):
        """log text appended since the last call"""
        try:
            with open(self.logp, errors='replace') as f:
                f.seek(self._pos)
                chunk = f.read()
                self._pos = f.tell()
            return chunk
        except OSError:
            return ''

    def log_text(self):
        return open(self.logp, errors='replace').read()

    def wait_log(self, pat, timeout):
        """seconds since launch when regex pat first appears in the log (polled every 50 ms), or None"""
        rx = re.compile(pat)
        end = time.time() + timeout
        buf = ''
        while time.time() < end and self.alive():
            buf += self.new_log()
            if rx.search(buf):
                return round(self.now(), 2)
            time.sleep(0.05)
        return None

    def app_pids(self):
        """this launch's process tree (the only source of app pids: never `inspect list`)"""
        return tree(self.p.pid)

    def inspect(self, args, timeout=60):
        """`makepad inspect <pid> args...` against the deepest live process of this launch's tree that answers"""
        ienv = dict(os.environ, TMPDIR=self.env['TMPDIR'], HOME=self.env['HOME'])
        last = 'NO ANSWER'
        for q in self.app_pids()[::-1]:
            try:
                r = subprocess.run([self.loader, 'inspect', str(q)] + list(args), capture_output=True, text=True, timeout=timeout, env=ienv)
            except subprocess.TimeoutExpired:
                last = 'TIMEOUT'
                continue
            o = r.stdout + r.stderr
            if r.returncode == 0 and 'no running app' not in o and 'cannot connect' not in o:
                return o
            last = o
        return last


# ---- adapters: prepare(ctx) -> (cmd, cwd); drive(ctx) -> {'windows': [{t0, t1, ops}], 'first': s, ...} ----
# windows: consecutive measured op windows (s since launch); cpu/op = whole-process CPU in the window / ops.

def rw_prepare(c, name):
    commercial = name in ('sandbox_sim', 'level_walk', 'model_build')
    c.env.update({'RW_MS': str(int(c.settle * 1000)), 'RW_WALL0': str(time.time_ns()), 'RW_ROOT': c.src})
    cwd = os.path.join(c.src, 'libs/relax/compiler/rwbench')
    if c.kind == 'jit':
        jit_env(c.env, c.home, c.src)
        c.env.update({'MAKEPAD_RUN_CACHE': 'on', 'MAKEPAD_HAL_BACKEND': 'headless', 'RW_O3_PROGRESS': c.env['MAKEPAD_RUN_O3_PROGRESS']})
        pkg = ['relax-commercial-tests', '--bin', 'relax-rwbench-commercial'] if commercial else ['relax-rwbench']
        return [c.bin, 'run', '--quiet', '--verbose'] + pkg + ['--', name], cwd
    return [c.bin, name], cwd


def rw_drive(c, name):
    while c.alive():
        time.sleep(0.25)
    return {}


def game_prepare(c, name):
    sb = os.path.join(c.home, 'sb')
    # 60 warm frames after each window's reset, 600 counted (10 s at 60 Hz); windows past the settle time: >= 5
    # 60 warm frames after each window's reset, 600 counted (10 s at 60 Hz); the drive loop ends the launch once
    # SB_WINDOWS (4) windows started after max(settle, JIT idle); DET's own window count is only the cap
    det = c.a.det or 'warm=60 frames=600 windows=80'
    c.env.update({'SANDBOX_HOME': sb, 'SANDBOX_CONFIG_DIR': os.path.join(sb, 'config'), 'SANDBOX_PERF_METER': '0',
                  'SANDBOX_GAME': os.path.join(c.src, 'apps/commercial/sandbox/resources/games/%s/game.splash' % name),
                  'SANDBOX_DET': det + ' quit=1', 'MAKEPAD_REMOTE': '1', 'MAKEPAD_TRACE': 'startup'})
    c.res['det'] = det
    if c.kind == 'jit':
        rh = jit_env(c.env, c.home, c.src)
        for d in (rh + '/cache/windows', rh + '/windows'):
            os.makedirs(d, exist_ok=True)
            open(d + '/d41beb9aac4e544a.geom', 'w').write('100 100 1360 860\n')
        return [c.bin, 'run', 'makepad-sandbox'], c.src
    return [c.bin], os.path.join(c.src, 'apps/commercial/sandbox')


def game_drive(c, name):
    first = c.wait_log('FIRST PRESENT ON GLASS', c.cap)
    wins, wstart = [], {}
    c._pos = 0
    while c.alive():
        time.sleep(0.25)
        for line in c.new_log().splitlines():
            m = DETSTART.search(line)
            if m:
                wstart[int(m.group(1))] = round(c.now(), 2)
            m = DET.search(line)
            if m:
                g = list(map(int, m.groups()[:10]))
                if g[0] in wstart:
                    wins.append({'w': g[0], 'ops': g[1], 'tick_ins': g[3], 'draw_ins': g[5], 'digest': m.group(11),
                                 't0': wstart[g[0]], 't1': round(c.now(), 2)})
        sf = c.settled_from()
        if sf is not None and sum(w['t0'] >= sf for w in wins) >= int(os.environ.get('SB_WINDOWS', '4')):
            break
    return {'windows': wins, 'first': first, 'unit': 'frame'}


# start:<pkg> -- an app launched and left alone: first frame, then idle CPU per second in 10-s windows until
# settle + 30 s. JIT first frame = the `--report-to` present line; native = MAKEPAD_TRACE=startup's
# FIRST PRESENT ON GLASS (both: when sb.py sees it, 50 ms polls).
START_PKGS = {'calculator': 'makepad-app-calculator', 'code': 'makepad-app-code', 'amp': 'makepad-app-amp',
              'stage': 'makepad-stage', 'story': 'makepad-story', 'scope': 'makepad-scope', 'sandbox': 'makepad-sandbox', 'box3d': 'makepad-example-box3d'}


def app_env(c):
    d = c.env['MAKEPAD_ROOT'] + '/local'
    c.env.update({'MAKEPAD_REMOTE': '1', 'MAKEPAD_TRACE': 'startup', 'STAGE_DATA_ROOT': d, 'MAKEPAD_CEF_OFFLINE': '1',
                  'VJ_ARTNET_DISABLE': '1', 'MUSIC_SPOTIFY_CLI': '/nonexistent/spotify_cli'})


def start_prepare(c, name):
    app_env(c)
    pkg = START_PKGS.get(name, name)
    if c.kind == 'jit':
        jit_env(c.env, c.home, c.src)
        c.report = os.path.join(c.home, 'report.' + c.a.label)
        open(c.report, 'w').close()
        return [c.bin, 'run', '--report-to', c.report, pkg], c.src
    return [c.bin], c.src


def first_present(c):
    if c.kind == 'jit':
        end = time.time() + c.cap
        while time.time() < end and c.alive():
            try:
                t = open(c.report).read()
            except OSError:
                t = ''
            if 'present' in t:
                return round(c.now(), 2)
            if 'error' in t:
                return None
            time.sleep(0.02)
        return None
    return c.wait_log('FIRST PRESENT ON GLASS', c.cap)


def start_drive(c, name):
    first = first_present(c)
    wins = []
    if first is None:
        return {'first': None, 'windows': [], 'err': 'no first present'}
    t = first
    while c.alive():
        sf = c.settled_from()
        if sf is not None and sum(w['t0'] >= sf for w in wins) >= 3:
            break
        time.sleep(max(0.0, t + 10 - c.now()))
        wins.append({'t0': t, 't1': round(c.now(), 2), 'ops': round(c.now() - t, 2)})
        t = wins[-1]['t1']
    return {'first': first, 'windows': wins, 'unit': 's'}


ADAPTERS = {'rw': (rw_prepare, rw_drive), 'game': (game_prepare, game_drive), 'start': (start_prepare, start_drive)}
try:  # sb_apps.py: UI-interaction and media adapters (ui:*, stage:*), same interface
    import sb_apps
    ADAPTERS.update(sb_apps.ADAPTERS)
except ImportError:
    pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--wl', required=True)
    ap.add_argument('--kind', choices=['native', 'jit'], required=True)
    ap.add_argument('--bin', required=True)
    ap.add_argument('--loader', default='')
    ap.add_argument('--src', required=True)
    ap.add_argument('--home', required=True)
    ap.add_argument('--label', required=True)
    ap.add_argument('--settle', type=float, default=60)
    ap.add_argument('--det', default='')
    ap.add_argument('--out', default='')
    ap.add_argument('--cap', type=float, default=900)
    ap.add_argument('--env', action='append', default=[])
    ap.add_argument('--meta', action='append', default=[], help='k=v copied into the result (variant, commit, run)')
    a = ap.parse_args()
    src, home, binp = os.path.abspath(a.src), os.path.abspath(a.home), os.path.abspath(a.bin)
    os.makedirs(home, exist_ok=True)
    c = Ctx(a, src, home, binp, os.path.abspath(a.loader or a.bin), common_env(home))
    fam, _, name = a.wl.partition(':')
    if fam not in ADAPTERS:
        sys.exit('unknown workload family: ' + fam)
    prepare, drive = ADAPTERS[fam]
    c.res.update({'wl': a.wl, 'kind': a.kind, 'label': a.label, 'bin': binp, 'settle': a.settle,
                  'host': os.uname()[1], 'arch': os.uname()[4], 't_launch': time.strftime('%Y-%m-%dT%H:%M:%S')})
    for kv in a.meta:
        k, v = kv.split('=', 1)
        c.res[k] = v
    cmd, cwd = prepare(c, name)
    for kv in a.env:
        k, v = kv.split('=', 1)
        c.env[k] = v
    c.logp = os.path.join(home, 'log.' + a.label)
    lf = open(c.logp, 'w')
    c.t0 = time.time()
    if fam == 'rw':
        c.env['RW_WALL0'] = str(time.time_ns())
    c.p = subprocess.Popen(cmd, cwd=cwd, env=c.env, stdout=lf, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True)
    res = c.res
    res['pid'] = c.p.pid
    smp = c.smp = Sampler(c.p, c.t0)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(SystemExit(143)))
    d = {}
    try:
        d = drive(c, name) or {}
    except Exception as e:  # an adapter failure is a failed launch, never a hang
        d = {'err': 'adapter: %r' % (e,)}
    finally:
        smp.done()
        if c.p.poll() is None:
            res['stopped'] = True  # app-like adapters end the launch themselves
            try:
                os.killpg(c.p.pid, signal.SIGTERM)
                c.p.wait(30)
            except (OSError, subprocess.TimeoutExpired):
                try:
                    os.killpg(c.p.pid, signal.SIGKILL)
                except OSError:
                    pass
                c.p.wait()
        lf.close()
    res['exit'] = c.p.returncode
    res['wall_s'] = round(c.now(), 1)
    S = smp.series
    out = c.log_text()
    res['ok'] = 'err' not in d
    if 'err' in d:
        res['err'] = d['err']
    for k, v in d.items():
        if k not in ('windows', 'err'):
            res[k] = v
    if fam == 'rw':
        m = RWLINE.search(out)
        if c.p.returncode != 0 or not m:
            res['ok'] = False
            res['err'] = out[-400:]
        else:
            steady, tts_main, tts_proc = float(m.group(2)), float(m.group(3)), float(m.group(4))
            end = S[-1][0] if S else 0
            tot, per = window_cpu(S, end - 11, end - 1)
            cores = tot / 10.0 if tot else None  # rw: the program thread runs flat out; cores = all threads' load
            res.update({'unit': 'iter', 'steady_ms': steady, 'cores': cores and round(cores, 3),
                        'cpu': cores and round(steady * cores, 4), 'ttfp': round(tts_proc / 1000, 2),
                        'first': round((tts_proc - tts_main) / 1000, 3), 'iters': int(m.group(5)),
                        'o3_ms': int(m.group(7)), 'check': m.group(8)})
            res['tti'], res['cpu_to_idle'] = idle_metrics(S, a.kind, cores, end)
    else:
        wins = [w for w in d.get('windows', []) if w.get('t0') is not None and w.get('ops')]
        res['windows'] = wins
        tti_all, _ = idle_metrics(S, a.kind, end=wins[-1]['t1'] if wins else None)
        if a.kind == 'jit' and not IS_LINUX:
            tti_all = getattr(c, 'idle_seen', None)
        sfrom = max(a.settle, tti_all or 0.0)
        res['settled_from'] = sfrom
        if a.kind == 'jit' and tti_all is None:
            res['never_idle'] = True
            late = [w['t0'] for w in wins if w['t0'] >= a.settle]
            sfrom = late[-4] if len(late) >= 4 else a.settle  # the last 4 windows
        sett = [w for w in wins if w['t0'] >= sfrom]
        if any(w.get('seg') for w in wins):  # adapters that replay a fixed segment: settled = that segment
            sett = [w for w in wins if w.get('seg')]
        # what a user sees one minute in: the windows starting in [60, 90) s
        m60 = [w for w in wins if 60 <= w['t0'] < 90]
        def wcpu(w):
            return window_cpu(S, w['t0'], w['t1'])[0] * 1000.0 / w['ops']
        for w in wins:
            w['cpu'] = round(wcpu(w), 4)
        if sett:
            # settled level = all settled windows' CPU / their ops (window edges interpolated between samples)
            lv = sorted(w['cpu'] for w in sett)
            res['cpu'] = round(sum(window_cpu(S, w['t0'], w['t1'])[0] for w in sett) * 1000.0 / sum(w['ops'] for w in sett), 4)
            res['cpu_win_spread'] = [lv[0], lv[-1]]
            if 'tick_ins' in sett[0]:
                fr = sum(w['ops'] for w in sett)
                res['tick_ins'] = round(sum(w['tick_ins'] for w in sett) / fr / 1e6, 3)
                res['draw_ins'] = round(sum(w['draw_ins'] for w in sett) / fr / 1e6, 3)
                res['digests'] = sorted(set(w['digest'] for w in sett))
            if m60:
                res['cpu60'] = round(sum(window_cpu(S, w['t0'], w['t1'])[0] for w in m60) * 1000.0 / sum(w['ops'] for w in m60), 4)
            tol = max(0.05 * res['cpu'], 0.05)  # idle apps: 0.05 ms CPU per s floor
            res['ttfp'] = next((w['t0'] for w in wins if w['cpu'] <= res['cpu'] + tol), None)
            span = max(1e-6, sett[-1]['t1'] - sett[0]['t0'])
            lvl = window_cpu(S, sett[0]['t0'], sett[-1]['t1'])[0] / span
            res['tti'], res['cpu_to_idle'] = idle_metrics(S, a.kind, lvl, sett[-1]['t1'])
            if a.kind == 'jit' and not IS_LINUX:
                res['tti'] = tti_all
                res['cpu_to_idle'] = round(cum_at(S, tti_all), 2) if tti_all is not None else None
        elif res['ok']:
            res['ok'] = False
            res['err'] = 'no settled window (got %d); log tail: %s' % (len(wins), out[-300:])
    if S:
        res['rss'] = max(x[3] for x in S[-5:])  # settled RSS: the last samples (one may catch the exit)
        res['rss_peak'] = max(x[3] for x in S)
    oth = getattr(smp, 'other', [])
    if oth:
        oth = sorted(oth)
        res['other_cores'] = round(sum(oth) / len(oth), 3)
        res['other_p90'] = round(oth[int(len(oth) * 0.9)], 3)
    res['series'] = [[x[0], x[1], round(jit_cpu(x[2]), 4), x[3]] for x in S]
    line = json.dumps(res)
    print(json.dumps({k: v for k, v in res.items() if k not in ('series', 'windows')}))
    if a.out:
        open(a.out, 'a').write(line + '\n')


if __name__ == '__main__':
    main()
