#!/usr/bin/env python3
"""sbchart.py <summary.json> <out.html> [history.jsonl] (Python 3.9+)."""
import argparse
import html
import json
import math
from pathlib import Path


ARCHES = ('x64', 'arm64')
CSS = """
:root{color-scheme:light dark;--bg:#fff;--ink:#192532;--muted:#596674;--grid:#d9dfe5;--panel:#f5f7f9;--alert:#b45a28;--neutral:#788595;--faster:#32857e}
@media(prefers-color-scheme:dark){:root{--bg:#141b23;--ink:#e3eaf1;--muted:#abb8c5;--grid:#374451;--panel:#1d2732;--alert:#eea06e;--neutral:#98a7b8;--faster:#64c1b4}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:14px/1.5 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-variant-numeric:tabular-nums}
main{max-width:1200px;min-width:0;margin:auto;padding:20px}h1{font-size:22px;line-height:1.3;margin:0;overflow-wrap:anywhere}p{margin:8px 0;overflow-wrap:anywhere}.note,time{color:var(--muted)}figure{margin:18px 0}svg{display:block;width:100%;height:auto;font:13px system-ui,sans-serif}svg text{fill:var(--ink)}.bar{stroke-width:12}.alert{stroke:var(--alert)}.neutral{stroke:var(--neutral)}.faster{stroke:var(--faster)}.whisker,.ref,.ship,.trend{fill:none;stroke:var(--ink)}.whisker{stroke-width:1.3}.ref{opacity:.4}.limit{stroke-dasharray:4 3}.ship{stroke-width:2;opacity:.35}.grid{stroke:var(--grid);fill:none}.scroll{max-width:100%;overflow-x:auto;border:1px solid var(--grid)}table{border-collapse:collapse;white-space:nowrap;width:100%;font-size:12px}caption{text-align:left;padding:8px;color:var(--muted)}th,td{text-align:right;padding:6px 9px;border-bottom:1px solid var(--grid)}thead th{background:var(--panel);white-space:normal;min-width:95px}th:first-child,td:nth-child(2){text-align:left}tbody th{font-weight:500}td span{display:block}.bad th{color:var(--alert)}.spark{width:110px;height:32px}.trend{stroke-width:1.5}.arm{stroke-dasharray:4 3}.dot{fill:var(--ink)}details{margin-top:12px;overflow-wrap:anywhere}li{margin:4px 0}@media(max-width:480px){main{padding:12px}h1{font-size:19px}}
"""


def esc(value):
    return html.escape(str(value), quote=True)


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def fmt(value, suffix=''):
    return ('%.2f%s' % (value, suffix)) if number(value) else 'n/a'


def tone(value):
    return 'alert' if value > 1.05 else 'faster' if value < .95 else 'neutral'


def spread(row):
    values = row.get('x_cpu_spread')
    return sorted(values) if isinstance(values, list) and len(values) == 2 and all(number(v) for v in values) else []


def flags(row):
    result = []
    if row.get('fail'):
        result.append('FAIL')
    if row.get('checks_agree') is False:
        result.append('CHECKSUM')
    return result


def chart(workloads, rows):
    # Both columns use the same zero-based scale, including whiskers and ship ticks.
    values = [1.05]
    for by_name in rows.values():
        for row in by_name.values():
            values.extend(v for v in (row.get('x_cpu'), row.get('x_ship')) if number(v))
            values.extend(spread(row))
    maximum = max(values) * 1.08
    scale = 235 / maximum
    bottom = 46 + len(workloads) * 30
    parts = ['<svg viewBox="0 0 900 %d" role="img" aria-labelledby="chart-title"><title id="chart-title">JIT CPU / native-cpu rustc, worst first; lower is faster</title>' % (bottom + 30)]
    for arch, start in zip(ARCHES, (225, 570)):
        parts.append('<text x="%d" y="17">%s</text>' % (start, arch))
        for value, cls in ((0, 'grid'), (1, 'ref'), (1.05, 'ref limit')):
            x = start + value * scale
            parts.append('<path class="%s" d="M%.1f 37V%d"/>' % (cls, x, bottom))
        parts.append('<text x="%d" y="%d">0</text><text x="%.1f" y="%d">1.0</text>' % (start, bottom + 20, start + scale - 9, bottom + 20))
    for index, wl in enumerate(workloads):
        y = 55 + index * 30
        warning = any(flags(rows[a].get(wl, {})) for a in ARCHES)
        label = (wl if len(wl) <= 27 else wl[:26] + '…') + (' !' if warning else '')
        parts.append('<g transform="translate(0 %d)"><text y="4"><title>%s</title>%s</text>' % (y, esc(wl), esc(label)))
        for arch, start in zip(ARCHES, (225, 570)):
            row = rows[arch].get(wl, {})
            value = row.get('x_cpu')
            if number(value):
                end = start + value * scale
                parts.append('<path class="bar %s" d="M%d 0H%.1f"/>' % (tone(value), start, end))
                bounds = spread(row)
                if bounds:
                    lo, hi = (start + v * scale for v in bounds)
                    parts.append('<path class="whisker" d="M%.1f 0H%.1fM%.1f -4V4M%.1f -4V4"/>' % (lo, hi, lo, hi))
                label_x = max(end, start + bounds[-1] * scale if bounds else end) + 5
                parts.append('<text x="%.1f" y="4">%s</text>' % (label_x, fmt(value, 'x')))
            else:
                parts.append('<text x="%d" y="4">n/a</text>' % (start + 5))
            if number(row.get('x_ship')):
                parts.append('<path class="ship" d="M%.1f -9V9"/>' % (start + row['x_ship'] * scale))
        parts.append('</g>')
    parts.append('</svg>')
    return ''.join(parts)


def history_data(path):
    commits, values = {}, {}
    if path:
        with open(path, encoding='utf-8') as source:
            for line in source:
                if not line.strip():
                    continue
                row = json.loads(line)
                commit = row['commit']
                commits.setdefault(commit, len(commits))
                values[(row['wl'], row['arch'], commit)] = row.get('x_cpu')
    return list(commits), values


def sparkline(wl, commits, history):
    series = [[history.get((wl, arch, c)) for c in commits] for arch in ARCHES]
    available = [v for series_values in series for v in series_values if number(v)]
    if not available:
        return 'n/a'
    low, high = min(available + [1.0]), max(available + [1.0])
    padding = max((high - low) * .15, .025)
    low, high = low - padding, high + padding
    def y(value):
        return 29 - 26 * (value - low) / (high - low)
    parts = ['<svg class="spark" viewBox="0 0 110 32" role="img"><title>%s: x64 solid, arm64 dashed; %.2f–%.2fx</title><path class="grid" d="M3 %.1fH107"/>' % (esc(wl), low, high, y(1))]
    for arch, series_values in zip(ARCHES, series):
        segments, segment = [], []
        for i, value in enumerate(series_values):
            if number(value):
                segment.append((3 + 104 * i / (len(commits) - 1), y(value)))
            elif segment:
                segments.append(segment)
                segment = []
        if segment:
            segments.append(segment)
        for points in segments:
            if len(points) == 1:
                parts.append('<circle class="dot" cx="%.1f" cy="%.1f" r="1.5"/>' % points[0])
            else:
                parts.append('<path class="trend%s" d="M%s"/>' % (' arm' if arch == 'arm64' else '', 'L'.join('%.1f %.1f' % p for p in points)))
    return ''.join(parts) + '</svg>'


def render(summary, commits, history):
    arches = summary.get('arch') or {}
    rows = {arch: {r['wl']: r for r in (arches.get(arch) or {}).get('rows', [])} for arch in ARCHES}
    workloads = set().union(*(set(r) for r in rows.values()))
    def worst(wl):
        return max((r[wl]['x_cpu'] for r in rows.values() if wl in r and number(r[wl].get('x_cpu'))), default=-1)
    workloads = sorted(workloads, key=lambda wl: (-worst(wl), wl))
    # start:* rows are idle CPU after the first frame, not per-op work: table only, not the bar chart or headline
    idle = [wl for wl in workloads if wl.startswith('start:')]
    workloads = [wl for wl in workloads if not wl.startswith('start:')]
    title = 'JIT vs rustc -- work %s' % summary.get('commit', 'n/a')
    parts = ['<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>%s</title><style>%s</style></head><body><main><h1>%s</h1><time>%s</time>' % (esc(title), CSS, esc(title), esc(summary.get('date', 'n/a')))]
    for arch in ARCHES:
        data = arches.get(arch) or {}
        measured = [r for r in rows[arch].values() if number(r.get('x_cpu')) and not r['wl'].startswith('start:')]
        worst_row = max(measured, key=lambda r: r['x_cpu']) if measured else None
        worst_text = '%s %s' % (worst_row['wl'], fmt(worst_row['x_cpu'], 'x')) if worst_row else 'n/a'
        parts.append('<p><b>%s</b>: per-op geomean JIT/native-cpu <b>%s</b>; %s/%d within 1.05x; worst: %s.</p>' % (arch, fmt(data.get('geo_cpu'), 'x'), esc(data.get('within', 0)), len(measured), esc(worst_text)))
    parts.append('<figure>' + chart(workloads, rows) + '<figcaption class="note">Lower is faster. Warm: &gt;1.05x; neutral: 0.95–1.05x; teal: &lt;0.95x. Whiskers: observed spread; faint tick: JIT/shipping rustc. Solid line: 1.0x; dashed: 1.05x. ! = failed launch or checksum mismatch.</figcaption></figure>')
    parts.append('<div class="scroll" role="region" aria-label="Workload details" tabindex="0"><table><caption>Timing and RSS cells: x64 first, arm64 second. Native = native-cpu rustc. Warm first is JIT; n/a = missing. First = first frame, or main entered for rw.</caption><thead><tr>')
    headers = ['Workload', 'Unit', 'x64 JIT/ncpu', 'JIT/ship', 'arm64 JIT/ncpu', 'JIT/ship', 'Cold first frame s (JIT / native)', 'Warm first s', 'Time-to-final-perf s (cold/warm)', 'Time-to-idle s (cold/warm)', 'RSS MB (JIT/native)']
    if len(commits) > 1:
        headers.append('CPU history')
    parts.extend('<th scope="col">%s</th>' % h for h in headers)
    parts.append('</tr></thead><tbody>')
    failures = []
    for wl in workloads + idle:
        pair = [rows[a].get(wl, {}) for a in ARCHES]
        status = '; '.join('%s %s' % (a, '/'.join(flags(r))) for a, r in zip(ARCHES, pair) if flags(r))
        parts.append('<tr%s><th scope="row">%s%s</th>' % (' class="bad"' if status else '', esc(wl), ' [' + esc(status) + ']' if status else ''))
        units = list(dict.fromkeys(r['unit'] for r in pair if r.get('unit')))
        parts.append('<td>%s</td>' % esc(' / '.join(units) or 'n/a'))
        for row in pair:
            parts.extend('<td>%s</td>' % fmt(row.get(key), 'x') for key in ('x_cpu', 'x_ship'))
        metrics = [[], [], [], [], []]
        for arch, row in zip(ARCHES, pair):
            jit, native = row.get('jit') or {}, row.get('ncpu') or {}
            values = [(jit.get('cold_first'), native.get('first')), (jit.get('warm_first'),), (jit.get('ttfp_cold'), jit.get('ttfp_warm')), (jit.get('tti_cold'), jit.get('tti_warm')), (jit.get('rss'), native.get('rss'))]
            for cell, value in zip(metrics, values):
                cell.append('<span>%s: %s</span>' % (arch, ' / '.join(fmt(v) for v in value)))
            failures.extend('%s %s: %s' % (wl, arch, failure) for failure in row.get('fail') or [])
        parts.extend('<td>%s</td>' % ''.join(cell) for cell in metrics)
        if len(commits) > 1:
            parts.append('<td>' + sparkline(wl, commits, history) + '</td>')
        parts.append('</tr>')
    parts.append('</tbody></table></div>')
    if len(commits) > 1:
        parts.append('<p class="note">History: x64 solid; arm64 dashed. Per-workload scale; baseline 1.0x; gaps are missing samples. Commits in file order: %s.</p>' % esc(' → '.join(str(c) for c in commits)))
    if failures:
        parts.append('<details><summary>Failed launches (%d)</summary><ul>%s</ul></details>' % (len(failures), ''.join('<li>%s</li>' % esc(f) for f in failures)))
    parts.append('<p class="note">CPU: whole-process time per operation after settling (%s s), median across launches. ncpu: rustc native CPU; ship: shipping rustc. Ratios and spreads come from sbsum.py. Flagged measurements remain visible.</p></main></body></html>' % esc(summary.get('settle', 'n/a')))
    return ''.join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('summary', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('history', nargs='?')
    args = parser.parse_args()
    with args.summary.open(encoding='utf-8') as source:
        summary = json.load(source)
    commits, history = history_data(args.history)
    args.output.write_text(render(summary, commits, history), encoding='utf-8')


if __name__ == '__main__':
    main()
