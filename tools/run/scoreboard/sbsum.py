#!/usr/bin/env python3
# sbsum.py <outdir> <arch>=<results.jsonl>... : the scoreboard from sb.py result lines.
#   <outdir>/summary.json      per arch, per workload: medians + spreads per variant, JIT/native ratios
#   <outdir>/scoreboard.md     answer-first markdown (worst gaps first)
#   <outdir>/history.jsonl     one line per (commit, arch, workload) appended (deduplicated by commit+arch+wl)
# Variants: ncpu = rustc -C target-cpu=native, nship = rustc shipping (default CPU), jit = `makepad run`
# (run 1 cold, runs 2.. warm). Settled cpu = whole-process CPU per op after the settle time (median of launches).
import json, os, sys, statistics, time


def med(v):
    v = [x for x in v if x is not None]
    return statistics.median(v) if v else None


def spread(v):
    v = [x for x in v if x is not None]
    return [min(v), max(v)] if v else None


def load(path):
    rows = []
    for l in open(path):
        l = l.strip()
        if l:
            try:
                rows.append(json.loads(l))
            except ValueError:
                pass
    return rows


def summarize(rows):
    # a stage that was run again (a failed queue job re-run) repeats its labels: keep the last launch per label
    last = {}
    for r in rows:
        last[(r['wl'], r['label'])] = r
    rows = list(last.values())
    by = {}
    for r in rows:
        by.setdefault(r['wl'], {}).setdefault(r.get('variant', r['kind']), []).append(r)
    out = []
    for wl, vs in by.items():
        row = {'wl': wl, 'unit': None, 'fail': []}
        trees = sorted(set('%s+%s' % (r.get('commit'), r.get('commercial')) for rs in vs.values() for r in rs))
        row['tree'] = trees[0] if len(trees) == 1 else ','.join(trees)
        for v, rs in vs.items():
            good = [r for r in rs if r.get('ok') and r.get('cpu') is not None]
            for r in rs:
                if not r.get('ok'):
                    row['fail'].append('%s r%s: %s' % (v, r.get('run'), str(r.get('err', 'exit %s' % r.get('exit')))[:120]))
            if not good:
                continue
            row['unit'] = good[0].get('unit')
            s = {'n': len(good), 'cpu': med([r['cpu'] for r in good]), 'cpu_spread': spread([r['cpu'] for r in good]),
                 'first': med([r.get('first') for r in good]), 'rss': med([r.get('rss') for r in good]),
                 'rss_peak': med([r.get('rss_peak') for r in good]), 'other_cores': max((r.get('other_cores') or 0) for r in good)}
            if v == 'jit':
                cold = [r for r in good if r.get('mode') == 'cold']
                warm = [r for r in good if r.get('mode') == 'warm']
                s.update({'cold_first': med([r.get('first') for r in cold]), 'warm_first': med([r.get('first') for r in warm]),
                          'ttfp_cold': med([r.get('ttfp') for r in cold]), 'ttfp_warm': med([r.get('ttfp') for r in warm]),
                          'tti_cold': med([r.get('tti') for r in cold]), 'tti_warm': med([r.get('tti') for r in warm]),
                          'cti_cold': med([r.get('cpu_to_idle') for r in cold]), 'cti_warm': med([r.get('cpu_to_idle') for r in warm])})
            if good[0].get('tick_ins') is not None:
                s['tick_ins'] = med([r.get('tick_ins') for r in good])
                s['draw_ins'] = med([r.get('draw_ins') for r in good])
            # exact checksums only for rwbench (game DET digests depend on which windows settled, and FMA
            # contraction makes native-cpu and shipping floats differ)
            checks = sorted(set(str(r.get('check')) for r in good if r.get('check')))
            s['checks'] = checks
            row[v] = s
        j = row.get('jit')
        for base, key in (('ncpu', 'x_cpu'), ('nship', 'x_ship')):
            b = row.get(base)
            if j and b and b['cpu']:
                row[key] = round(j['cpu'] / b['cpu'], 3)
                if j['cpu_spread'] and b['cpu_spread'] and b['cpu_spread'][0] > 0:
                    row[key + '_spread'] = [round(j['cpu_spread'][0] / b['cpu_spread'][1], 3), round(j['cpu_spread'][1] / b['cpu_spread'][0], 3)]
        if j and row.get('ncpu') and row['ncpu'].get('rss'):
            row['x_rss'] = round(j['rss'] / row['ncpu']['rss'], 2) if j.get('rss') else None
        oth = [x.get('other_cores') for v, rs in vs.items() for x in rs if x.get('ok') and x.get('other_cores') is not None and x.get('mode') != 'cold']
        row['other_range'] = round(max(oth) - min(oth), 3) if oth else 0
        cs = set()
        for v in ('ncpu', 'nship', 'jit'):
            if v in row:
                cs.update(row[v]['checks'])
        row['checks_agree'] = len(cs) <= 1
        out.append(row)
    out.sort(key=lambda r: -(r.get('x_cpu') or 0))
    return out


def geomean(v):
    v = [x for x in v if x]
    if not v:
        return None
    p = 1.0
    for x in v:
        p *= x
    return round(p ** (1.0 / len(v)), 3)


def f(x, d=2):
    return '-' if x is None else ('%.*f' % (d, x))


def table(rows):
    L = ['| workload | unit | JIT/ncpu | spread | JIT/ship | cpu ms/op JIT | ncpu | first s cold / warm / native | ttfp s cold / warm | idle s cold / warm | compile CPU-s cold | RSS MB JIT / native | ok |',
         '|---|---|--:|---|--:|--:|--:|---|---|---|--:|---|---|']
    for r in rows:
        j, n = r.get('jit') or {}, r.get('ncpu') or {}
        ok = 'yes' if not r['fail'] and r['checks_agree'] else ('CHECKSUM' if not r['checks_agree'] else 'FAIL %d' % len(r['fail']))
        sp = r.get('x_cpu_spread')
        L.append('| %s | %s | **%s** | %s | %s | %s | %s | %s / %s / %s | %s / %s | %s / %s | %s | %s / %s | %s |' % (
            r['wl'], r['unit'] or '-', f(r.get('x_cpu')), ('%s..%s' % (f(sp[0]), f(sp[1]))) if sp else '-', f(r.get('x_ship')),
            f(j.get('cpu'), 3), f(n.get('cpu'), 3), f(j.get('cold_first')), f(j.get('warm_first')), f(n.get('first')),
            f(j.get('ttfp_cold'), 0), f(j.get('ttfp_warm'), 0), f(j.get('tti_cold'), 0), f(j.get('tti_warm'), 0),
            f(j.get('cti_cold'), 1), f(j.get('rss'), 0), f(n.get('rss'), 0), ok))
    return '\n'.join(L)


def md(summary):
    L = []
    c = summary['commit']
    L.append('# JIT vs rustc scoreboard -- work %s (commercial %s), %s' % (c, summary['commercial'], summary['date']))
    L.append('')
    for arch, A in summary['arch'].items():
        allrows = A['rows']
        rows = [r for r in allrows if not r['wl'].startswith('start:')]
        L.append('## %s (%s): per-op geomean JIT/native-cpu **%s**, JIT/shipping %s; %d/%d within 1.05x; worst: %s' % (
            arch, A['host'], f(A['geo_cpu'], 3), f(A['geo_ship'], 3), A['within'], len([r for r in rows if r.get('x_cpu')]),
            ', '.join('%s %sx' % (r['wl'], f(r['x_cpu'])) for r in rows[:5] if r.get('x_cpu'))))
        L.append('')
        L.append(table(rows))
        idle = [r for r in allrows if r['wl'].startswith('start:')]
        if idle:
            L.append('')
            L.append('Apps left alone after start (unit s: CPU ms per second of idle after the first frame; not in the geomean):')
            L.append('')
            L.append(table(idle))
        other = sorted(set(r['tree'] for r in allrows if r['tree'] != '%s+%s' % (summary['commit'], summary['commercial'])))
        if other:
            L.append('')
            L.append('Rows from another tree: ' + '; '.join('%s (%s)' % (', '.join(r['wl'] for r in allrows if r['tree'] == t), t) for t in other))
        rows = allrows
        # other load = machine CPU outside the launch's tree; UI drivers (inspect calls) add a constant share, so
        # flag a workload when its launches differ by > 0.5 cores
        noisy = [r['wl'] for r in rows if r.get('other_range', 0) > 0.5]
        if noisy:
            L.append('')
            L.append('Noisy (other load > 0.5 cores during a launch): ' + ', '.join(noisy))
        fails = [(r['wl'], x) for r in rows for x in r['fail']]
        if fails:
            L.append('')
            L.append('Failed launches: ' + '; '.join('%s %s' % x for x in fails[:12]))
        L.append('')
    L.append('Method: sb.py (tools/run/scoreboard). cpu = whole-process CPU (all threads) per op, settled (windows starting >= %ss after launch), median of launches; JIT = 1 cold + warm launches, ncpu/nship = rustc release (-C target-cpu=native / RUSTFLAGS=""), same tree. first = first frame (apps; JIT --report-to present, native FIRST PRESENT ON GLASS) or main entered (rw). ttfp = first op window within 5%% of the settled level. idle = last stretch where JIT compile threads (run-*) stay < 2%% of a core. compile CPU-s = whole-process CPU from launch to idle.' % summary.get('settle', 60))
    return '\n'.join(L) + '\n'


def main():
    od = os.path.abspath(sys.argv[1])
    os.makedirs(od, exist_ok=True)
    summary = {'arch': {}, 'date': time.strftime('%Y-%m-%d %H:%M')}
    for spec in sys.argv[2:]:
        arch, path = spec.split('=', 1)
        rows = load(path)
        if not rows:
            continue
        # the board's tree = the most common commit pair of its launches (a few stages may come from another pair:
        # their rows say so in 'tree')
        pairs = {}
        for r in rows:
            pairs[(r.get('commit'), r.get('commercial'))] = pairs.get((r.get('commit'), r.get('commercial')), 0) + 1
        summary['commit'], summary['commercial'] = max(pairs, key=pairs.get)
        summary['settle'] = rows[-1].get('settle')
        sm = summarize(rows)
        # per-op workloads only: start:* rows are idle CPU after the first frame (a different quantity)
        xs = [r.get('x_cpu') for r in sm if not r['wl'].startswith('start:')]
        summary['arch'][arch] = {'host': rows[-1].get('host'), 'rows': sm, 'geo_cpu': geomean(xs),
                                 'geo_ship': geomean([r.get('x_ship') for r in sm if not r['wl'].startswith('start:')]),
                                 'within': len([x for x in xs if x and x <= 1.05])}
    json.dump(summary, open(os.path.join(od, 'summary.json'), 'w'), indent=1)
    open(os.path.join(od, 'scoreboard.md'), 'w').write(md(summary))
    hp = os.path.join(od, 'history.jsonl')
    seen = set()
    if os.path.exists(hp):
        for l in open(hp):
            try:
                h = json.loads(l)
                seen.add((h['commit'], h['arch'], h['wl']))
            except (ValueError, KeyError):
                pass
    with open(hp, 'a') as hf:
        for arch, A in summary['arch'].items():
            for r in A['rows']:
                k = (summary['commit'], arch, r['wl'])
                if k in seen:
                    continue
                j = r.get('jit') or {}
                hf.write(json.dumps({'commit': summary['commit'], 'commercial': summary['commercial'], 'date': summary['date'],
                                     'arch': arch, 'wl': r['wl'], 'x_cpu': r.get('x_cpu'), 'x_ship': r.get('x_ship'),
                                     'jit_cpu': j.get('cpu'), 'ncpu_cpu': (r.get('ncpu') or {}).get('cpu'),
                                     'cold_first': j.get('cold_first'), 'warm_first': j.get('warm_first'),
                                     'ttfp_warm': j.get('ttfp_warm'), 'tti_cold': j.get('tti_cold'), 'rss': j.get('rss')}) + '\n')
    print(open(os.path.join(od, 'scoreboard.md')).read())


if __name__ == '__main__':
    main()
