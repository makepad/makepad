#!/usr/bin/env python3
"""One file declared as two modules, through the check server's edits: run.py [/path/to/makepad]
(without one, from a makepad checkout's workspace root: makepad's host build). Relax issue 135.

src/main.rs declares shared/value.rs as `first` and as `second` (`#[path]`); the file imports a
path dependency's exported macro, `use macro_dep::{Value, json};`, and calls `json!`. main.rs's
`here!` is a `&str` before `mod first;` and a `u32` before `mod second;`: in the file, each module
has its own. Two servers
run the steps: the default one, whose edits load the package again (the loader's module walk), and
one with RELAX_CHECK_EAGER=1, whose edits that add an item collect the crate again from a new scan
of its modules (preload.rs check_macros; a body-only edit would not).
1. The first check has no diagnostics.
2. An edit of a body of shared/value.rs, `json!(here!())`: the edit reaches each module's version,
   and `first`'s `here!` is a `&str` where `Value` holds a `u32` (E0308, `first`'s alone; the eager
   server parsed the file's last module only, and `first` kept the old body).
3. An edit of shared/value.rs that adds an item (`fn other`, called by `value`, calls `json!`): the
   import is still in scope in `second` (the scan walked the file once, under `first`: "cannot
   find macro").
4. An edit that makes shared/value.rs include itself (`#[path = "value.rs"] mod again;`, rustc's
   circular-modules error): the walk stops at the repeat and the check answers (no time limit: a
   walk that does not end stalls the job, and devhub's stall report names it).
5. The edit undone: no diagnostics again.
Prints "two_module_file: ok", or fails with the step's assertion."""
import json, os, pathlib, socket, subprocess, sys, tempfile, time
if len(sys.argv) > 1:
    relax = pathlib.Path(sys.argv[1]).resolve()
else:
    subprocess.run(['cargo', 'build', '--profile', 'host', '-q', '-p', 'makepad-run', '--bin', 'makepad'], check=True)
    relax = pathlib.Path(os.environ.get('CARGO_TARGET_DIR', 'target')).resolve() / 'host' / ('makepad.exe' if os.name == 'nt' else 'makepad')
here = pathlib.Path(__file__).resolve().parent
FILES = {
    'Cargo.toml': '[package]\nname = "two_module_file"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n\n[dependencies]\nmacro_dep = { path = "dep" }\n\n[workspace]\n',
    'dep/Cargo.toml': '[package]\nname = "macro_dep"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n',
    'dep/src/lib.rs': 'pub struct Value(pub u32);\n\n#[macro_export]\nmacro_rules! json {\n    ($v:expr) => {\n        $crate::Value($v)\n    };\n}\n',
    'src/main.rs': 'macro_rules! here {\n    () => {\n        "one"\n    };\n}\n#[path = "../shared/value.rs"]\nmod first;\nmacro_rules! here {\n    () => {\n        2u32\n    };\n}\n#[path = "../shared/value.rs"]\nmod second;\n\nfn main() {\n    let _ = (first::value(), second::value(), first::label(), second::label(), here!());\n}\n',
    'shared/value.rs': 'use macro_dep::{Value, json};\n\npub fn value() -> Value {\n    json!(3)\n}\n\npub fn label() -> impl std::fmt::Debug {\n    here!()\n}\n',
}
def steps(mode, extra):
    with tempfile.TemporaryDirectory(prefix='two-module-file-') as tmp:
        work = pathlib.Path(tmp)
        ws = work / 'ws'
        for path, text in FILES.items():
            (ws / path).parent.mkdir(parents=True, exist_ok=True)
            (ws / path).write_text(text)
        cache = work / 'cache'
        env = dict(os.environ, RELAX_CACHE_DIR=str(cache), RELAX_CACHE='off', RELAX_STD=str(here.parents[1] / 'std'),
                   CARGO_NET_OFFLINE='true', RELAX_OFFLINE='1', **extra)
        log = open(work / 'serve.log', 'w')
        child = subprocess.Popen([str(relax), 'serve', '--workspace', str(ws)], env=env, stdout=log, stderr=log)
        client = None
        try:
            endpoint = None
            while endpoint is None:
                entries = [e for e in cache.glob('serve/*/endpoint') if len(e.read_text().split()) == 3]
                if entries:
                    endpoint = entries[0]
                    break
                assert child.poll() is None, 'no check server endpoint: ' + (work / 'serve.log').read_text()
                time.sleep(.05)
            port, token, _ = endpoint.read_text().split()
            sock = socket.create_connection(('127.0.0.1', int(port)))
            client = sock.makefile('rwb')
            calls = [0]

            def call(method, **params):
                calls[0] += 1
                client.write((json.dumps(dict(id=calls[0], token=token, method=method, params=params)) + '\n').encode())
                client.flush()
                reply = json.loads(client.readline())
                assert reply['id'] == calls[0] and 'error' not in reply, reply
                return reply['result']

            def diagnostics():
                return [(d.get('code'), d.get('message')) for d in call('check', pkg='two_module_file')['diagnostics']]

            assert diagnostics() == [], (mode, 'the first check', diagnostics())
            value = ws / 'shared/value.rs'
            call('didChange', path=str(value), text=FILES['shared/value.rs'].replace('json!(3)', 'json!(here!())'))
            got = diagnostics()
            assert len(got) == 1 and 'E0308' in (got[0][0] or '') + (got[0][1] or ''), (mode, "an edit of a body, in each module's version", got)
            call('didChange', path=str(value), text=FILES['shared/value.rs'].replace('json!(3)', 'other()') + '\nfn other() -> Value {\n    json!(4)\n}\n')
            got = diagnostics()
            assert got == [], (mode, 'after an edit of the file both modules read', got)
            call('didChange', path=str(value), text=FILES['shared/value.rs'] + '\n#[path = "value.rs"]\nmod again;\n')
            got = diagnostics()
            assert not any('json' in (m or '') for _, m in got), (mode, 'the file that includes itself', got)
            call('didChange', path=str(value), text=None)
            got = diagnostics()
            assert got == [], (mode, 'the edit undone', got)
            call('shutdown')
        except BaseException:
            print(mode, (work / 'serve.log').read_text()[-4000:], file=sys.stderr)
            raise
        finally:
            if client is not None: client.close()
            if child.poll() is None:
                child.terminate()
                try: child.wait(timeout=10)
                except subprocess.TimeoutExpired: child.kill(); child.wait()


steps('reload', {})
steps('eager', dict(RELAX_CHECK_EAGER='1'))
print('two_module_file: ok')
