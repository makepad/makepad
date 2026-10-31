#!/usr/bin/env python3
"""Loopback protocol and CLI integration. Fixture copies and logs survive for inspection."""
import concurrent.futures, json, os, pathlib, shutil, signal, socket, subprocess, sys, tempfile, time
relax = pathlib.Path(sys.argv[1]).resolve()
here = pathlib.Path(__file__).resolve().parent
work = pathlib.Path(tempfile.mkdtemp(prefix='relax-serve-'))
fixture = work / 'fixture'
shutil.copytree(here / 'fixture', fixture)
cache = work / 'cache'
env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=str(cache), MAKEPAD_RUN_CACHE='off',
           MAKEPAD_RUN_STD=str(here.parents[1] / 'std'),
           CARGO_NET_OFFLINE='true', MAKEPAD_RUN_OFFLINE='1')
source = fixture / 'main.rs'
original = source.read_text()
fixed = original.replace('Missing', 'i32')
log = open(work / 'serve.log', 'w')
child = subprocess.Popen([str(relax), 'serve', '--workspace', str(fixture)], env=env, stdout=log, stderr=log)
endpoint = None

def await_endpoint():
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        entries = list(cache.glob('serve/*/endpoint'))
        if entries and len(entries[0].read_text().split()) == 3:
            return entries[0]
        assert child.poll() is None, (child.returncode, (work/'serve.log').read_text())
        time.sleep(.02)
    raise AssertionError('endpoint timeout')

class Client:
    def __init__(self, token=None):
        port, self.token, _ = endpoint.read_text().split()
        if token is not None: self.token = token
        self.socket = socket.create_connection(('127.0.0.1', int(port)), timeout=180)
        self.file = self.socket.makefile('rwb')
        self.id = 0
    def call(self, method, **params):
        self.id += 1
        self.file.write((json.dumps(dict(id=self.id, token=self.token, method=method, params=params))+'\n').encode())
        self.file.flush()
        reply = json.loads(self.file.readline())
        assert reply['id'] == self.id, reply
        assert 'error' not in reply, reply
        return reply['result']
    def close(self):
        self.file.close(); self.socket.close()

def cli(*args):
    p = subprocess.run([str(relax), *args], cwd=fixture, env=env, text=True, capture_output=True, timeout=180)
    return p, [json.loads(s) for s in p.stdout.splitlines()]

def stable(rows):
    rows = [dict(r) for r in rows]
    rows[-1]['ms'] = 0
    return rows

def position(anchor, token):
    lines = fixed.splitlines()
    line = next(i for i,l in enumerate(lines) if anchor in l)
    return dict(path=str(source), line=line+1, col=lines[line].index(token)+1)

try:
    endpoint = await_endpoint()
    assert len(endpoint.parent.name) == 16
    if os.name != 'nt': assert endpoint.stat().st_mode & 0o777 == 0o600
    duplicate = subprocess.run([str(relax), 'serve'], cwd=fixture, env=env, capture_output=True, text=True, timeout=20)
    assert duplicate.returncode == 0 and duplicate.stdout == endpoint.read_text(), duplicate
    client = Client()
    assert client.call('status')['packages'] == []
    initial = client.call('check', pkg='serve_fixture')
    print('initial warm check:', initial['summary'], flush=True)
    one, rows = cli('check', '--no-serve', '--json', 'serve_fixture')
    assert one.returncode == 1 and not one.stderr, one.stderr
    assert initial['diagnostics'] == rows[:-1], (initial, rows)
    assert any(d['code'] == 'E0412' for d in initial['diagnostics'])
    auto, auto_rows = cli('check', '--json', 'serve_fixture')
    assert auto.returncode == one.returncode and stable(auto_rows) == stable(rows), (auto, rows)
    client.call('didChange', path=str(source), text=fixed)
    clean = client.call('check', pkg='serve_fixture')
    print('fixed warm check:', clean['summary'], flush=True)
    assert clean['diagnostics'] == [], clean
    for anchor, token, kind, typ in [('let field', 'local', 'local', 'Counter'), ('let field', 'value', 'field', 'i32'), ('let call', 'read', 'fn', 'i32')]:
        value = client.call('hover', **position(anchor,token))
        assert value and value['kind'] == kind and typ in value['type'], (token,value)
        print('hover', token, value, flush=True)
    defs = client.call('definition', **position('let _result','helper'))
    assert defs and defs[0]['line'] == 13, defs
    schema = client.call('schema', pkg='serve_fixture', path='crate::Counter')
    assert schema['kind'] == 'struct' and 'documented counter' in schema['docs'] and 'Used by' in schema['docs'], schema
    assert schema['fields'][0]['type'] == 'i32' and 'Current count' in schema['fields'][0]['docs'], schema
    assert any(m['name'] == 'read' and 'Read the count' in m['docs'] for m in schema['methods']), schema
    assert client.call('status')['packages'][0]['items'] >= 3
    other = client.call('schema', pkg='serve_other', path='Other')
    assert other['traits'] and any(m['name']=='read' for m in other['methods']), other
    variants = client.call('schema', pkg='serve_other', path='Choice')['variants']
    assert [v['name'] for v in variants] == ['Empty','Value','Named'], variants
    assert variants[1]['fields'][0]['type'] == 'u32'
    assert len(client.call('status')['packages']) == 2
    nested = subprocess.run([str(relax),'check','--json','serve_other'],cwd=fixture/'other',env=env,capture_output=True,text=True,timeout=180)
    assert nested.returncode == 0 and len(list(cache.glob('serve/*/endpoint'))) == 1, nested
    # Loading a second package must not invalidate the first package's editor state.
    assert client.call('hover', **position('let field','value'))['type'] == 'i32'
    # Several connections share the same package writer.
    def parallel(_):
        c = Client()
        try: return c.call('check', pkg='serve_fixture')['diagnostics']
        finally: c.close()
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        assert list(pool.map(parallel, range(4))) == [[],[],[],[]]
    # Unsaved content survives disk edits; text:null restores the disk version.
    source.write_text(original + '\n')
    time.sleep(.25)
    assert client.call('check', pkg='serve_fixture')['diagnostics'] == []
    client.call('didChange', path=str(source), text=None)
    assert client.call('check', pkg='serve_fixture')['diagnostics']
    # Watcher pushes after a real edit; a separate connection avoids event/response mixing.
    subscriber = Client()
    subscriber.call('subscribe', pkg='serve_fixture')
    source.write_text(fixed)
    event = json.loads(subscriber.file.readline())
    assert event['event'] == 'diagnostics' and event['pkg'] == 'serve_fixture' and not event['diagnostics'], event
    print('watch warm check:', event['summary'], flush=True)
    subscriber.close()
    one, rows = cli('check', '--no-serve', '--json', 'serve_fixture')
    auto, auto_rows = cli('check', '--json', 'serve_fixture')
    assert one.returncode == auto.returncode == 0 and stable(rows) == stable(auto_rows)
    for method,args in [('schema',['Counter']),('hover',[str(source),'16','23']),('definition',[str(source),'18','19'])]:
        p,_ = cli(method,'serve_fixture',*args)
        assert p.returncode == 0, (method,p.stderr)
    bad = Client('0'*64)
    bad.file.write(b'{"id":7,"token":"bad","method":"status","params":{}}\n'); bad.file.flush()
    assert json.loads(bad.file.readline())['error']['code'] == -32001
    assert bad.file.readline() == b''
    bad.close()
    malformed = Client()
    malformed.file.write(b'{\n'); malformed.file.flush()
    assert json.loads(malformed.file.readline())['error']['code'] == -32700
    malformed.close()
    client.call('shutdown'); client.close()
    assert child.wait(timeout=15) == 0
    assert not endpoint.exists() and not (endpoint.parent/'lock').exists()
    # Explicit startup and automatic connection use the same endpoint and log.
    started, rows = cli('check', '--serve', '--json', 'serve_fixture')
    assert started.returncode == 0 and 'starting the check server' in started.stderr, started
    c = Client(); c.call('shutdown'); c.close()
    deadline = time.monotonic()+15
    while endpoint.exists() and time.monotonic()<deadline: time.sleep(.02)
    assert not endpoint.exists()
    # SIGTERM cleanup and idle cleanup exercise the exact same lease guard.
    for args, terminate in [([],True),(['--idle-min','0.002'],False)]:
        child = subprocess.Popen([str(relax),'serve','--workspace',str(fixture),*args],env=env,stdout=log,stderr=log)
        endpoint = await_endpoint()
        if terminate: child.send_signal(signal.SIGTERM)
        assert child.wait(timeout=15) == 0
        assert not endpoint.exists() and not (endpoint.parent/'lock').exists()
    print('serve: PASS; artifacts', work)
finally:
    if endpoint is not None and endpoint.exists():
        try:
            cleanup = Client(); cleanup.call('shutdown'); cleanup.close()
        except (OSError, ValueError, AssertionError): pass
    if child.poll() is None:
        child.terminate()
        try: child.wait(timeout=15)
        except subprocess.TimeoutExpired: child.kill(); child.wait()
    log.close()
