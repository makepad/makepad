#!/usr/bin/env python3
"""Five warm edits against isolated one-shot references; original app is read-only.
Spec is a JSON array of five {name, replace: [old,new]} or {name, append: text}.
Each edit starts from the initial text; {name: 'revert'} restores it.
"""
import json, os, pathlib, queue, re, shutil, subprocess, sys, tempfile, threading, time

relax, app, pkg, edit, spec = sys.argv[1:]
relax, app, edit = map(lambda p: pathlib.Path(p).resolve(), (relax, app, edit))
steps = json.loads(pathlib.Path(spec).read_text())
assert len(steps) == 5, 'expected body error, revert, signature, new fn, unresolved name'
tmp = pathlib.Path(tempfile.mkdtemp(prefix='relax-check-watch-'))
relative = edit.relative_to(app)
for name in ('warm', 'reference'):
    dst = tmp / name
    shutil.copytree(app, dst, ignore=shutil.ignore_patterns('target', '.git'))
    manifest = dst / 'Cargo.toml'
    source = manifest.read_text()
    source = re.sub(r'(\bpath\s*=\s*")([^"]+)(")', lambda m: m[0] if (app / m[2]).resolve().is_relative_to(app) else m[1] + str((app / m[2]).resolve()) + m[3], source)
    if '[workspace]' not in source:
        source += '\n[workspace]\n'
    manifest.write_text(source)
base = (tmp / 'warm' / relative).read_text()
env = dict(os.environ, MAKEPAD_RUN_STD=str(pathlib.Path(__file__).resolve().parents[2] / 'std'),
           MAKEPAD_RUN_CACHE_DIR=str(tmp / 'target'), MAKEPAD_RUN_CACHE='off', CARGO_NET_OFFLINE='true')
# The unit-graph resolver must not fetch missing registry packages in this gate.
env['MAKEPAD_RUN_OFFLINE'] = '1'
err = open(tmp / 'watch.stderr', 'w')
child = subprocess.Popen([str(relax), 'check', '--watch', '--json', pkg], cwd=tmp / 'warm', env=env,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err, text=True, bufsize=1)
q = queue.Queue()
def read():
    for line in child.stdout: q.put(line)
    q.put(None)
threading.Thread(target=read, daemon=True).start()
def batch():
    rows = []
    deadline = time.monotonic() + 180
    while True:
        line = q.get(timeout=max(0.01, deadline-time.monotonic()))
        assert line is not None, f'watch exited {child.poll()}; see {tmp}'
        row = json.loads(line)
        if row.get('type') == 'summary': return rows, row
        rows.append(row)
def normalized(rows, root):
    # Also normalize suggestion replacement paths and any source paths in notes/messages.
    return sorted(json.dumps(row, sort_keys=True).replace(str(root), '<app>') for row in rows)
def reference(text):
    (tmp / 'reference' / relative).write_text(text)
    r = subprocess.run([str(relax), 'check', '--json', pkg], cwd=tmp / 'reference', env=env,
                       capture_output=True, text=True, timeout=180)
    (tmp / 'reference.stderr').write_text(r.stderr)
    assert r.returncode in (0, 1), (r.returncode, r.stderr, tmp)
    rows = [json.loads(line) for line in r.stdout.splitlines()]
    assert rows and rows[-1].get('type') == 'summary', (r.stdout, r.stderr, tmp)
    assert rows[-1]['errors'] == len(rows)-1
    return rows[:-1]
def compare(rows, ref, label):
    got, want = normalized(rows, tmp/'warm'), normalized(ref, tmp/'reference')
    if got != want:
        (tmp / (label+'.actual.json')).write_text('\n'.join(got))
        (tmp / (label+'.expected.json')).write_text('\n'.join(want))
        raise AssertionError(f'{label}: diagnostics differ; artifacts {tmp}')
try:
    rows, summary = batch()
    compare(rows, reference(base), 'initial')
    baseline = normalized(rows, tmp/'warm')
    print('initial: ' + json.dumps(summary), flush=True)
    slow = []
    for n, step in enumerate(steps):
        text = base
        if 'replace' in step:
            old, new = step['replace']
            assert old in text, (step['name'], 'anchor not found')
            text = text.replace(old, new, 1)
        text += step.get('append', '')
        child.stdin.write(json.dumps({'method':'didChange','path':str(tmp/'warm'/relative),'text':text})+'\n')
        child.stdin.flush()
        rows, summary = batch()
        assert summary['errors'] == len(rows)
        (tmp / (str(n)+'-'+step['name']+'.summary.json')).write_text(json.dumps(summary))
        print(f"{step['name']}: {summary['ms']:.3f} ms, {summary['rechecked']} rechecked, RSS {summary['rss_before_trim_mb']:.1f} -> {summary['rss_mb']:.1f} MB", flush=True)
        compare(rows, reference(text), str(n)+'-'+step['name'])
        if n == 0: assert normalized(rows, tmp/'warm') != baseline, 'body-error edit did not change diagnostics'
        if n < 2 and summary['ms'] >= 100: slow.append((step['name'], summary['ms']))
    child.stdin.write('{"method":"shutdown"}\n'); child.stdin.flush()
    assert child.wait(timeout=10) == 0
    assert not slow, f'all diagnostics matched, but body edits exceeded 100 ms: {slow}; artifacts {tmp}'
    print('PASS: all five edits equal one-shot diagnostics; artifacts ' + str(tmp))
finally:
    if child.poll() is None:
        child.terminate()  # This child PID only.
        try: child.wait(timeout=5)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
    err.close()
