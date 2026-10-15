#!/usr/bin/env python3
"""Disjoint main-thread load/tail accounting and per-derive elapsed/lock-wait totals.

Input is the Chrome trace from load_timeline.py. Tail starts at the final lowering's
completion. Timings are elapsed (not CPU); derive totals across workers overlap.
"""
import argparse
import collections
import json
from pathlib import Path

CATEGORIES = [
    ('wait-macro-unit', 'JIT unit lock waits'),
    ('wait-macro-world', 'Macro world waits'),
    ('wait-parse', 'Worker parse waits'),
    ('wait-scan', 'Scan waits'),
    ('macro-jit', 'JIT macro execution / bridge'),
    ('macro-native', 'Linked macro execution'),
    ('macro-rules', 'macro_rules expansion / output parse'),
    ('macro-rules-output', 'macro_rules expansion / output parse'),
    ('macro-rules-parse', 'macro_rules expansion / output parse'),
    ('derive-builtin', 'Built-in derive generation / output parse'),
    ('derive', 'Derive input / render / dispatch'),
    ('derive-output-parse', 'Proc-derive output parse'),
    ('prepare-file', 'File preparation'),
    ('file-publication', 'Source / macro / imports publication'),
    ('publish', 'Prescan publication'),
    ('crate-macro-publication', 'Crate macro / imports publication'),
    ('parse-lowered', 'Lowered-file parse, remaining syntax'),
    ('parse-rs', 'Rust parse, remaining syntax'),
    ('lower', 'Loader lowering'),
    ('schema', 'Loader schema'),
    ('scope-collect', 'Scope / item collection'),
]

def intersect(e, a, b):
    return max(0, min(e['ts'] + e['dur'], b) - max(e['ts'], a))

def in_window(e, a, b):
    # Zero-microsecond spans still represent a call (sub-microsecond native macros).
    return e['ts'] < b and e['ts'] + e['dur'] >= a

def partition(events, a, b):
    names = {phase: (len(CATEGORIES) - i, name) for i, (phase, name) in enumerate(CATEGORIES)}
    names.update({'derive-jit': (0, 'Derive dispatch'), 'derive-native': (0, 'Derive dispatch')})
    points = [(a, 0, -1), (b, 0, -1)]
    for i, e in enumerate(events):
        if e['name'] in names and intersect(e, a, b):
            points += [(max(a, e['ts']), 1, i), (min(b, e['ts'] + e['dur']), -1, i)]
    active, out, previous = set(), collections.Counter(), a
    for at, change, index in sorted(points):
        if at > previous:
            phase = max((names[events[i]['name']] for i in active), default=(0, 'Other loader work'))[1]
            routes = {events[i]['name'] for i in active}
            if phase == 'JIT macro execution / bridge':
                phase = 'JIT derive execution / bridge' if 'derive-jit' in routes else 'JIT attribute / function macro execution'
            elif phase == 'Linked macro execution':
                phase = 'Linked derive execution' if 'derive-native' in routes else 'Linked attribute / function macro execution'
            out[phase] += at - previous
        if change == 1: active.add(index)
        if change == -1: active.discard(index)
        previous = at
    assert sum(out.values()) == b - a
    return out

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    parser.add_argument('--json', type=Path)
    args = parser.parse_args()
    events = [e for e in json.loads(args.trace.read_text())['traceEvents'] if e['ph'] == 'X']
    load, = [e for e in events if e['name'] == 'load-parse']
    start, end = load['ts'], load['ts'] + load['dur']
    lowers = [e for e in events if e['name'] == 'lower' and intersect(e, start, end)]
    tail = max(e['ts'] + e['dur'] for e in lowers)
    main_events = [e for e in events if e['tid'] == load['tid']]
    totals, tails = partition(main_events, start, end), partition(main_events, tail, end)
    print(f'Load {load["dur"]/1000:.3f} ms; last lowering {(tail-start)/1000:.3f} ms; tail {(end-tail)/1000:.3f} ms')
    print('| Main-thread disjoint elapsed | Entire load ms | Tail ms |')
    print('|---|---:|---:|')
    for name in sorted(totals, key=totals.get, reverse=True):
        print(f'| {name} | {totals[name]/1000:.3f} | {tails[name]/1000:.3f} |')
    print(f'| **Total** | **{sum(totals.values())/1000:.3f}** | **{sum(tails.values())/1000:.3f}** |')
    derives = collections.defaultdict(lambda: dict(count=0, elapsed_us=0, unit_wait_us=0, tail_us=0, lowered_count=0, lowered_us=0))
    waits = collections.defaultdict(list)
    lowered_parses = collections.defaultdict(list)
    for e in events:
        if e['name'] == 'wait-macro-unit': waits[e['tid']].append(e)
        if e['name'] == 'parse-lowered': lowered_parses[e['tid']].append(e)
    for e in events:
        if e['name'] not in ('derive-jit', 'derive-native', 'derive-builtin-generate') or not in_window(e, start, end): continue
        key = e['name'] + ':' + e['args']['subject']
        row = derives[key]
        row['count'] += 1
        row['elapsed_us'] += intersect(e, start, end)
        if any(p['ts'] <= e['ts'] and e['ts'] + e['dur'] <= p['ts'] + p['dur'] for p in lowered_parses[e['tid']]):
            row['lowered_count'] += 1
            row['lowered_us'] += intersect(e, start, end)
        row['tail_us'] += intersect(e, tail, end)
        row['unit_wait_us'] += sum(intersect(w, max(start, e['ts']), min(end, e['ts'] + e['dur'])) for w in waits[e['tid']])
    print('\n| Derive / route (all threads) | Calls | Elapsed ms | Unit wait ms | Tail elapsed ms | Lowered calls | Lowered ms |')
    print('|---|---:|---:|---:|---:|---:|---:|')
    for key, row in sorted(derives.items()):
        print(f'| {key} | {row["count"]} | {row["elapsed_us"]/1000:.3f} | {row["unit_wait_us"]/1000:.3f} | {row["tail_us"]/1000:.3f} | {row["lowered_count"]} | {row["lowered_us"]/1000:.3f} |')
    print('\n| Phase | Calls | First start ms | Last finish ms | Summed elapsed ms |')
    print('|---|---:|---:|---:|---:|')
    phases = ('sources-batch', 'schema', 'lower', 'lower-token-input', 'parse-lowered', 'parse-rs', 'derive-native', 'derive-jit', 'macro-native', 'macro-jit', 'publish')
    for name in phases:
        es = [e for e in events if e['name'] == name and in_window(e, start, end)]
        if es: print(f'| {name} | {len(es)} | {(min(e["ts"] for e in es)-start)/1000:.3f} | {(max(e["ts"]+e["dur"] for e in es)-start)/1000:.3f} | {sum(intersect(e,start,end) for e in es)/1000:.3f} |')
    if args.json:
        args.json.write_text(json.dumps(dict(load_us=end-start, lower_end_us=tail-start, tail_us=end-tail, main=dict(totals), tail=dict(tails), derives=dict(derives)), indent=2)+'\n')

if __name__ == '__main__':
    main()
