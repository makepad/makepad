"""A stale access with somebody to decide (MAKEPAD_RUN_STALE_PARK=1): the faulting thread parks,
the app pauses, the access is listed as a parked panic, `panic` is refused, `stop` ends the process."""
import os
import re
import subprocess
import sys
import time

mk = os.path.abspath(sys.argv[1])
root = os.path.dirname(os.path.abspath(__file__))
env = dict(os.environ, MAKEPAD_RUN_STALE_PARK="1", SAFE2_UNLOAD_CRASH="parked")
p = subprocess.Popen([mk, "test", os.path.join(root, "unload_inside.rs")], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
err = []
pid = None
ident = None
t0 = time.time()
try:
    while time.time() - t0 < 120:
        line = p.stderr.readline()
        if not line:
            break
        err.append(line)
        m = re.search(r"stale access parked as (P\d+) -- makepad inspect (\d+) panics", line)
        if m:
            ident, pid = m.group(1), m.group(2)
            break
    assert ident, "no park line"
    assert p.poll() is None, "the process ended instead of parking"
    def inspect(*args):
        r = subprocess.run([mk, "inspect", pid, *args], capture_output=True, text=True, timeout=60)
        return r.stdout + r.stderr
    listed = inspect("panics")
    assert ident in listed and "stale access" in listed and "parker" in listed, f"not listed: {listed[:600]}"
    told = inspect("stale")
    # the parked call returns into the module's code first: its code is invalidated at the unload
    # (invalid.rs), so the return parks before any access to the window's data
    assert "stale access: thread" in told and "unloaded module `parker`" in told and ("shadow stack" in told or "heap" in told or "RETURNED into" in told), f"stale: {told[:800]}"
    stacks = inspect("stacks", "thread=all")
    assert "parker::parked" in stacks, f"stacks: {stacks[:800]}"
    ev = inspect("eval", "in=test_crate", "crate::PARKED.load(std::sync::atomic::Ordering::SeqCst)")
    assert "true" in ev, f"eval while paused: {ev[:600]}"
    refused = inspect("resume", ident, "panic")
    assert "cannot go on" in refused, f"panic was not refused: {refused[:400]}"
    time.sleep(0.5)
    assert p.poll() is None, "ended before the decision"
    inspect("resume", ident, "stop")
    rc = p.wait(timeout=60)
    out = p.stdout.read()
    assert rc != 0 and "out parked call" not in out, f"stop: rc {rc}, out {out[:300]}"
    print("modules stale access parks until stop: PASS")
    # a trusted module's fn called through a kept fn pointer after its unload: parked, named
    env2 = dict(env, SAFE2_UNLOAD_CRASH="trusted_call", MAKEPAD_RUN_MODULES_TRUSTED="1")
    q = subprocess.Popen([mk, "test", os.path.join(root, "unload_inside.rs")], env=env2, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    p = q
    err = []
    ident = None
    t0 = time.time()
    while time.time() - t0 < 120:
        line = q.stderr.readline()
        if not line:
            break
        err.append(line)
        m = re.search(r"stale access parked as (P\d+) -- makepad inspect (\d+) panics", line)
        if m:
            ident, pid = m.group(1), m.group(2)
            break
    assert ident, "trusted_call: no park line"
    told = inspect("stale")
    assert "unloaded module `parker`" in told and "CALLED" in told and "Module::unload" in told, f"trusted_call stale: {told[:1500]}"
    inspect("resume", ident, "stop")
    rc = q.wait(timeout=60)
    out = q.stdout.read()
    assert rc != 0 and "out called an unloaded" not in out, f"trusted_call stop: rc {rc}, out {out[:300]}"
    print("modules stale call into an unloaded trusted module parks: PASS")
except Exception as e:
    if p.poll() is None:
        p.kill()
    print("".join(err)[-3000:])
    sys.exit(f"modules stale access park: FAIL ({e})")
