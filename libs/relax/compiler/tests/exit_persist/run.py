#!/usr/bin/env python3
"""The exit never waits, and persistence does not depend on it: run.py /absolute/path/to/makepad.

1. kill: a program idles after its start; once the run has written its cache in the background
   (the mid-run write, MAKEPAD_RUN_CACHE_TRACE) it is SIGKILLed -- no exit work at all. The next run of
   the same program is warm: its code comes from the cache and its program from the collected
   store (written in the background too, not only by the exit).
2. busy: a thread of the program keeps calling functions that were never compiled (each call a
   lazy compile) while main exits. The process ends within a bounded time with code 0 (no wait
   for that thread, its compiles or the background compilers), and the next run is warm.
Each run owns a process group; it is stopped by group and checked empty."""
import atexit
import os
from pathlib import Path
import re
import signal
import subprocess as sp
import sys
import tempfile
import time

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
        os.killpg(p.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    for _ in range(100):
        rows = sp.check_output(['ps', '-eo', 'pid,pgid,stat'], text=True)
        if not [r for r in rows.splitlines()[1:] if r.split()[1] == str(p.pid) and not r.split()[2].startswith('Z')]:
            return
        time.sleep(.05)
    raise AssertionError('process group left: %d' % p.pid)


def cleanup():
    for p in reversed(owned):
        stop(p)


atexit.register(cleanup)
for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(sig, lambda s, f: sys.exit(128 + s))

# `step::<N>`: one more never-compiled instance per call (the busy thread's lazy compiles)
N = 4000
calls = '\n'.join(f'        acc = acc.wrapping_add(step::<{n}>(acc));' for n in range(N))
source = '''
#[inline(never)]
fn step<const K: u64>(x: u64) -> u64 { x.rotate_left((K % 63) as u32 + 1) ^ K.wrapping_mul(0x9e3779b97f4a7c15) }
#[inline(never)]
fn work(seed: u64) -> u64 {
    let mut acc = seed;
    for i in 0..1000u64 { acc = acc.wrapping_mul(6364136223846793005).wrapping_add(i); }
    acc
}
fn busy() -> u64 {
    let mut acc = 1u64;
CALLS
    acc
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    println!("ready {}", work(3));
    match mode.as_str() {
        "idle" => loop { std::thread::sleep(std::time::Duration::from_millis(100)); },
        "busy" => {
            std::thread::spawn(|| loop { std::hint::black_box(busy()); });
            std::thread::sleep(std::time::Duration::from_millis(1500));
            println!("main exits");
            std::process::exit(0);
        }
        _ => println!("quick {}", work(5)),
    }
}
'''.replace('CALLS', calls)

with tempfile.TemporaryDirectory(prefix='relax-exit-persist-') as tmp:
    root = Path(tmp)
    (root / 'src').mkdir()
    (root / 'Cargo.toml').write_text('[package]\nname="exit-persist"\nversion="0.0.0"\nedition="2021"\n[workspace]\n')
    (root / 'src/main.rs').write_text(source)

    def launch(tag, mode, cache):
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_NET_OFFLINE='true', MAKEPAD_RUN_STD=std, MAKEPAD_RUN_CACHE='on',
                   MAKEPAD_RUN_EXIT_TRACE='1', MAKEPAD_RUN_CACHE_TRACE=str(root / (tag + '.cache.tsv')),
                   MAKEPAD_RUN_CACHE_DIR=str(root / cache), MAKEPAD_RUN_TARGET_DIR=str(root / ('target-' + cache)))
        log = (root / (tag + '.log')).open('w')
        p = sp.Popen([binary, 'run', '--verbose', 'exit-persist', '--', mode], cwd=root, env=env, stdout=log, stderr=sp.STDOUT,
                     start_new_session=True)
        owned.append(p)
        return p

    def text(tag):
        return (root / (tag + '.log')).read_text(errors='replace')

    def warm(tag):
        p = launch(tag, 'quick', tag.split('-')[0])
        try:
            p.wait(timeout=300)
        finally:
            stop(p)
        t = text(tag)
        assert p.returncode == 0 and 'quick' in t, t[-3000:]
        m = re.search(r'makepad-run cache: (\d+) hits, (\d+) misses', t)
        assert m, t[-3000:]
        hits, misses = int(m.group(1)), int(m.group(2))
        # warm: the code came from the cache -- linked one by one (hits) or with the startup image
        # (prelinked: nothing left to compile)
        compiled = re.search(r'compiled (\d+) fns', t)
        assert (hits > 0 and hits > misses) or (compiled and int(compiled.group(1)) <= 5), (tag, hits, misses, compiled and compiled.group(0), t[-3000:])
        # a live session (the default: edits patch in place) uses no collected program store
        return hits, misses, 'makepad-run collected: restored' in t or 'makepad live: watching' in t

    # 1. killed after the background write: the next run is warm
    p = launch('kill-cold', 'idle', 'kill')
    t0 = time.monotonic()
    while 'mid-run write' not in text('kill-cold'):
        assert p.poll() is None, text('kill-cold')[-3000:]
        assert time.monotonic() - t0 < 300, 'no mid-run write: ' + text('kill-cold')[-3000:]
        time.sleep(.1)
    time.sleep(.5)  # the write's own thread finishes its files (append is atomic per store)
    os.killpg(p.pid, signal.SIGKILL)
    p.wait()
    stop(p)
    kill = warm('kill-warm')
    # the collected program too: written in the background, not only at the exit
    assert kill[2], 'the run after a kill did not restore the collected program: ' + text('kill-warm')[-3000:]
    print('exit_persist: killed run -> warm run: %d hits, %d misses, collected program restored (or a live session: none used)' % kill[:2])

    # 2. main exits while a thread compiles: bounded, code 0, and the next run is warm
    p = launch('busy-cold', 'busy', 'busy')
    t0 = time.monotonic()
    while 'main exits' not in text('busy-cold'):
        assert p.poll() is None, text('busy-cold')[-3000:]
        assert time.monotonic() - t0 < 300, text('busy-cold')[-3000:]
        time.sleep(.01)
    t1 = time.monotonic()
    try:
        p.wait(timeout=20)
    except sp.TimeoutExpired:
        raise AssertionError('the process did not exit within 20 s of main exiting: ' + text('busy-cold')[-3000:])
    gone = (time.monotonic() - t1) * 1e3
    stop(p)
    assert p.returncode == 0, (p.returncode, text('busy-cold')[-3000:])
    busy = warm('busy-warm')
    print('exit_persist: busy exit -> gone in %.0f ms, code 0; warm run: %d hits, %d misses' % ((gone,) + busy[:2]))
    print('exit_persist: ok')
