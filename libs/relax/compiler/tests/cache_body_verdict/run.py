#!/usr/bin/env python3
"""Cross-launch body-query invalidation: run.py /absolute/path/to/makepad.

Code keys and stored borrow-check verdicts are per item (cache/items.rs): a fn's body edit
re-checks that fn (typeck + Gandalf); a caller that did not take its body in (here: it calls
through a fn pointer) depends only on its interface and keeps its code and verdict. An interface
edit (an attribute on its header) re-checks the callers too."""
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
    std::thread::sleep(std::time::Duration::from_secs(4));
}
'''
with tempfile.TemporaryDirectory(prefix='relax-body-verdict-') as tmp:
    root = Path(tmp)
    (root / 'src').mkdir()
    (root / 'Cargo.toml').write_text('[package]\nname="body-verdict"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    checked = {}
    # (tag, value, header, the fns that must be checked again: typeck and Gandalf)
    for tag, value, header, again in [('cold', 7, '', {'value', 'dependent'}), ('warm', 7, '', set()),
                                      ('edit', 19, '', {'value'}), ('warm-edited', 19, '', set()),
                                      ('interface', 19, '#[inline(never)]', {'value', 'dependent'}),
                                      ('warm-interface', 19, '#[inline(never)]', set())]:
        (root / 'src/main.rs').write_text(source.replace('VALUE', str(value)).replace('HEADER', header))
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_NET_OFFLINE='true',
                   MAKEPAD_RUN_STD=std, MAKEPAD_RUN_CACHE='on', MAKEPAD_RUN_GANDALF='1', MAKEPAD_RUN_GANDALF_TRACE='1',
                   MAKEPAD_RUN_STARTUP_PHASE='0', MAKEPAD_RUN_CACHE_DIR=str(root / 'cache'),
                   MAKEPAD_RUN_FN_COST=str(root / tag))
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
        rows = (root / f'{tag}.{p.pid}').read_text().splitlines()
        checked[tag] = [r.split('\t')[5] for r in rows if r.startswith('type\t')]
        print(tag, {name: sum(n.endswith('body_verdict::' + name) for n in checked[tag])
                    for name in ['value', 'dependent']})
        for name in ['value', 'dependent']:
            found = any(n.endswith('body_verdict::' + name) for n in checked[tag])
            assert found == (name in again), (tag, name, checked[tag], text)
        # Gandalf (the lazy borrow checker): every body handed to it names the verdict an
        # earlier run stored for it ("check": none, checked in this run).
        verdicts = {name: [line.split()[2] for line in text.splitlines()
                           if line.startswith('gandalf-verdict: ') and line.endswith(' body_verdict::' + name)]
                    for name in ['value', 'dependent']}
        print(tag, 'gandalf', verdicts)
        for name, states in verdicts.items():
            assert states, (tag, name, text)
            # checked in this run, or the verdict an earlier run stored for the same body and
            # interface (unchanged since, or since the edit run that stored new verdicts)
            assert ('check' in states) == (name in again), (tag, name, states)
    print('cache_body_verdict: ok (warm reuse; a body edit re-checks that body, an interface edit its dependent too, by typeck and Gandalf; their new verdicts reused)')
