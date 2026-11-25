#!/usr/bin/env python3
"""Cross-launch body-query invalidation: run.py /absolute/path/to/makepad.

Code keys and stored borrow-check verdicts are per item (cache/items.rs): a fn's body edit
compiles that fn again (and Gandalf checks it again); a caller that did not take its body in
(here: it calls through a fn pointer) depends only on its interface and keeps its code linked.
An interface edit (an attribute on its header) compiles the callers again too. What each run
compiled or linked is the cache trace's (RELAX_CACHE_TRACE). (The app's own crates are
type-checked whole before main, a crate at a time: a body edit checks the crate again, which is
no per-item question.)"""
import atexit
import os
from pathlib import Path
import signal
import subprocess as sp
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
std = str(Path(__file__).resolve().parents[2] / 'std')
owned = []


def stop(p):
    if p.poll() is None:
        os.killpg(p.pid, signal.SIGTERM)
        try:
            p.wait(timeout=10)
        except sp.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
    try:
        os.killpg(p.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    rows = sp.check_output(['ps', '-eo', 'pid,pgid,stat'], text=True)
    assert not [r for r in rows.splitlines()[1:]
                if r.split()[1] == str(p.pid) and not r.split()[2].startswith('Z')]


def cleanup():
    for p in reversed(owned):
        stop(p)


atexit.register(cleanup)
for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(sig, lambda s, f: sys.exit(128 + s))

source = '''
static mut STATE: u64 = 4;
// called through fn pointers: each keeps its own compiled instance (not inlined)
HEADER
fn value() -> u64 { VALUE }
fn dependent() -> u64 { std::hint::black_box(value as fn() -> u64)() + 1 }
fn uncalled<T>(x: T) -> T { x }
fn main() {
    unsafe { assert_eq!(STATE, 4); STATE = 99; }
    println!("body-verdict {}", std::hint::black_box(dependent as fn() -> u64)());
    // (Gandalf stores the run's verdicts in the background: a run that ends at once leaves its
    // edited body to be checked again by the next)
    std::thread::sleep(std::time::Duration::from_secs(4));
}
'''
with tempfile.TemporaryDirectory(prefix='relax-body-verdict-') as tmp:
    root = Path(tmp)
    (root / 'src').mkdir()
    (root / 'Cargo.toml').write_text('[package]\nname="body-verdict"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    checked = {}
    # (tag, value, header, the fns that must be compiled again, and checked again by Gandalf)
    for tag, value, header, again in [('cold', 7, '', {'value', 'dependent'}), ('warm', 7, '', set()),
                                      ('edit', 19, '', {'value'}), ('warm-edited', 19, '', set()),
                                      ('interface', 19, '#[inline(never)]', {'value', 'dependent'}),
                                      ('warm-interface', 19, '#[inline(never)]', set())]:
        (root / 'src/main.rs').write_text(source.replace('VALUE', str(value)).replace('HEADER', header))
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_NET_OFFLINE='true',
                   RELAX_STD=std, RELAX_CACHE='on', RELAX_GANDALF='1', RELAX_GANDALF_TRACE='1',
                   RELAX_STARTUP_PHASE='0', RELAX_CACHE_DIR=str(root / 'cache'),
                   RELAX_CACHE_TRACE=str(root / (tag + '.trace')))
        with (root / (tag + '.log')).open('w') as log:
            p = sp.Popen([binary, 'run', '--verbose', 'body-verdict'], cwd=root,
                         env=env, stdout=log, stderr=sp.STDOUT, start_new_session=True)
            owned.append(p)
            try:
                p.wait(timeout=120)
            finally:
                stop(p)
        text = (root / (tag + '.log')).read_text()
        assert p.returncode == 0, text
        assert f'body-verdict {value + 1}' in text, text
        # per fn: 'compiled' (a miss: built in this run) or 'linked' (its cached code, a hit)
        seen = {name: set() for name in ['value', 'dependent']}
        for line in (root / (tag + '.trace')).read_text(errors='replace').splitlines():
            f = line.split('\t')
            for name in seen:
                if len(f) >= 6 and (f[4].endswith('body_verdict::' + name) or f[5].endswith('body_verdict::' + name)):
                    seen[name].add(f[1])
        status = {name: 'compiled' if w & {'miss', 'linkfail', 'cert:stale', 'cert:undecodable'} else
                  'linked' if w & {'hit', 'hit:twin', 'prelink'} else None
                  for name, w in seen.items()}
        print(tag, status)
        for name, st in status.items():
            assert st == ('compiled' if name in again else 'linked'), (tag, name, st, seen[name])
        # Gandalf (the lazy borrow checker): a body compiled again is checked again ("check"); a
        # linked one is not handed to it
        verdicts = {name: [line.split()[2] for line in text.splitlines()
                           if line.startswith('gandalf-verdict: ') and line.endswith(' body_verdict::' + name)]
                    for name in ['value', 'dependent']}
        print(tag, 'gandalf', verdicts)
        for name, states in verdicts.items():
            assert ('check' in states) == (name in again), (tag, name, states)
    print('cache_body_verdict: ok (warm runs link both; a body edit compiles and checks that body again, an interface edit its dependent too)')
