"""Run each case in isolation; preserve failures and raw output with the table."""
import hashlib
import platform
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys

host, target, relax = sys.argv[1:]
bench = Path(__file__).resolve().parent
logs = bench / ('logs-' + host)
names = (bench / 'names.txt').read_text().splitlines()
# BENCH_ONLY=a,b runs just those rows and writes results-<host>-only.md (the full table stays).
only = os.environ.get('BENCH_ONLY', '')
if only:
    names = [n for n in names if n in only.split(',')]
out_name = 'results-' + host + ('-only' if only else '') + '.md'
results = {}
failures = []
fallbacks = {
    'hashset_string': 'shim String lacks the host Hash implementation',
    'rc_refcell_churn': 'optimized shim panics: RefCell already borrowed; its UnsafeCell is not the rustc interior-mutability lang item',
}
batches = dict(re.findall(r'"([a-z0-9_]+)" => (\d+)', (bench / 'src/benches.rs').read_text()))
metadata = {
    'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'platform': platform.platform(),
    'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
    'relax': relax,
    'relax_sha256': hashlib.sha256(Path(relax).read_bytes()).hexdigest(),
    'source_sha256': {str(p.relative_to(bench)): hashlib.sha256(p.read_bytes()).hexdigest()
                      for p in bench.rglob('*.rs') if 'logs-' not in str(p)},
    'batches': {n: int(batches.get(n, 1)) for n in names},
}
(logs / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
for name in names:
    results[name] = {}
    for column, binary in [('A', str(Path(target) / 'std-bench-real')),
                           ('B', str(Path(target) / 'release/std-bench-shim')),
                           ('C', relax)]:
        if column == 'C':
            case = logs / name
            case.mkdir(exist_ok=True)
            # Same version/roots/unit/target/dependency schema as /tmp/rp/runone.sh.
            # Direct entry point ensures an unsupported case cannot suppress others.
            source = case / 'std_bench.rs'
            source.write_text('#[path = "../../src/prelude_real.rs"] mod prelude;\n'
                              'use prelude::s;\n'
                              '#[path = "../../src/benches.rs"] mod benches;\n'
                              'fn main() { benches::measure("' + name + '", benches::' + name + '); }\n')
            units = case / 'units.json'
            units.write_text(json.dumps({'version': 1, 'roots': [0], 'units': [{
                'pkg_id': 'path+file://' + str(case) + '#std_bench@0.0.0',
                'target': {'kind': ['bin'], 'name': 'std_bench', 'src_path': str(source), 'edition': '2021'},
                'mode': 'build', 'features': [], 'dependencies': []}]}))
            command = ['nice', '-n', '10', relax, 'run', '--quiet', '--units', str(units), 'std_bench']
            env = dict(os.environ, MAKEPAD_RUN_CACHE_DIR=str(Path(target) / 'relax' / name))
        else:
            command = ['nice', '-n', '10', binary, name]
            env = os.environ.copy()
        print(f'{name}: {column}', flush=True)
        log = logs / (name + '-' + column + '.log')
        with log.open('w') as stream:
            completed = subprocess.run(command, env=env, stdout=stream, stderr=subprocess.STDOUT, text=True)
        output = log.read_text()
        match = re.search(r'^\| ' + re.escape(name) + r' ([0-9.]+) ([0-9]+)$', output, re.M)
        if completed.returncode == 0 and match:
            results[name][column] = (float(match[1]), match[2])
        else:
            failures.append((name, column, completed.returncode, output))
    # Write incrementally so interrupted runs still retain completed evidence.
    lines = ['| bench | A ms | B ms | C ms | B/A | C/B | checksums equal |',
             '|---|---:|---:|---:|---:|---:|---|']
    for n, row in results.items():
        def ms(c):
            return f'{row[c][0]:.2f}' if c in row else 'ERROR'
        def ratio(c, d):
            return f'{row[c][0]/row[d][0]:.2f}' if c in row and d in row and row[d][0] else '—'
        equal = ('yes' if len({v[1] for v in row.values()}) == 1 else '**MISMATCH**') if len(row) == 3 else ('A=B; C unavailable' if 'A' in row and 'B' in row and row['A'][1] == row['B'][1] else '**INCOMPLETE / MISMATCH**')
        lines.append(f'| {n + (" (B=real)" if n in fallbacks else "")} | {ms("A")} | {ms("B")} | {ms("C")} | {ratio("B", "A")} | {ratio("C", "B")} | {equal} |')
    lines += ['', 'Milliseconds per fixed batch: one warm-up batch, best of five batches. Includes input construction, allocation, checksumming and destruction. All columns use identical batch counts. B uses host iterator traits and thread spawning with Relax mpsc. Default hashers intentionally differ.',
              '', 'Batch counts: ' + ', '.join(n + '=' + str(metadata['batches'][n]) for n in names) + '.',
              '', 'B=real fallbacks: ' + '; '.join(n + ': ' + reason for n, reason in fallbacks.items()) + '.',
              '', 'Environment: ' + metadata['platform'] + '; ' + metadata['rustc'] + '. Binary/source hashes and batch counts: logs-' + host + '/metadata.json.']
    if any('setpriority: Operation not permitted' in p.read_text() for p in logs.glob('*-A.log')):
        lines += ['', 'Sandbox limitation: nice -n 10 was invoked, but setpriority was denied; the measured processes ran at the inherited priority.']
    lines += ['', 'Execution failures: ' + ('none.' if not failures else '')]
    for n, c, code, output in failures:
        lines += ['', f'### {n}, {c}, exit {code}', '```text', output.rstrip(), '```']
    (bench / out_name).write_text('\n'.join(lines) + '\n')

(logs / 'measurements.json').write_text(json.dumps(results, indent=2) + '\n')
if failures or any(len(row) != 3 or len({v[1] for v in row.values()}) != 1 for row in results.values()):
    sys.exit(1)
