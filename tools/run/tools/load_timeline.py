#!/usr/bin/env python3
"""Convert MAKEPAD_RUN_PROFILE stderr to a Chrome/Perfetto trace and phase summaries.

Durations are inclusive elapsed time, not CPU time. Nested phases must not be
added together. The trace preserves worker ids, start times, waits and paths.
"""
import argparse
import collections
import json
from pathlib import Path
import re


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    pattern = re.compile(r'load-timeline: start_us=(\d+) dur_us=(\d+) thread=ThreadId\((\d+)\) worker=(".*?") phase=(\S+) subject=(".*")$')
    events, threads = [], {}
    phases = collections.defaultdict(list)
    for line in args.log.read_text().splitlines():
        match = pattern.fullmatch(line)
        if not match:
            continue
        start, duration, thread = map(int, match.group(1, 2, 3))
        worker, phase, subject = json.loads(match[4]), match[5], json.loads(match[6])
        threads[thread] = worker
        event = dict(name=phase, cat=phase, ph="X", ts=start, dur=duration,
                     pid=1, tid=thread, args=dict(subject=subject))
        events.append(event)
        phases[phase].append(event)
    metadata = [dict(name="thread_name", ph="M", pid=1, tid=thread, args=dict(name=f"{worker} ({thread})"))
                for thread, worker in sorted(threads.items())]
    args.output.write_text(json.dumps(dict(traceEvents=metadata + events, displayTimeUnit="ms"), separators=(",", ":")) + "\n")
    for phase, items in sorted(phases.items()):
        total = sum(e["dur"] for e in items) / 1000
        largest = max(items, key=lambda e: e["dur"])
        print(f"{phase}: n={len(items)} sum_ms={total:.3f} max_ms={largest['dur']/1000:.3f}")
        for event in sorted(items, key=lambda e: e["dur"], reverse=True)[:5]:
            print(f"  t={event['ts']/1000:.3f}..{(event['ts']+event['dur'])/1000:.3f} thread={event['tid']} {event['args']['subject']}")


if __name__ == "__main__":
    main()
