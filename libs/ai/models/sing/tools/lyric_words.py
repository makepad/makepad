#!/usr/bin/env python3
"""Per kept lyric line, the timed prompted words (from the harvester's
alignment.json) and where the line's segment starts in its take, so
`sing_prep lyrics` can rebuild each training segment as a score-aligned item
and `sing_eval` can score held-out songs against their lyric.

  python3 lyric_words.py <out dir> [--exclude MODEL]...
  python3 lyric_words.py --count-secs [--exclude MODEL]...
      (reads ~/nv1/lyric/out, ~/nv1/prep/lyrics, ~/nv1/lyric/inbox)

--exclude MODEL leaves out every song group made by that music model (the
producer's request, `<inbox>/<take>.json`, names it). --count-secs prints the
harvested clean seconds (harvest.tsv) of the groups kept.

Out: <out>/<group>.tsv with
  seg <cand.mksdat path> <item k> <f0a frame> <p0> <p1> <f0 match err>
  w <t0 s from segment start> <t1> <midi or 0> <ipa> <word>
"""
import json, os, struct, sys, glob

OUT_DIR = os.path.expanduser("~/nv1/lyric/out")
SHARDS = os.path.expanduser("~/nv1/prep/lyrics")
INBOX = os.path.expanduser("~/nv1/lyric/inbox")
args = sys.argv[1:]
exclude = [args[i + 1] for i, a in enumerate(args) if a == "--exclude"]


def made_by_excluded(take):
    try:
        req = open(os.path.join(INBOX, take + ".json")).read()
    except OSError:
        return False
    return any(m in req for m in exclude)


def group_excluded(g):
    takes = [t for t in (g, g + "-t0", g + "-t1") if os.path.exists(os.path.join(INBOX, t + ".json"))]
    return any(made_by_excluded(t) for t in takes)


if "--count-secs" in args:
    s = 0.0
    for l in open(os.path.join(SHARDS, "harvest.tsv")):
        c = l.rstrip("\n").split("\t")
        if not group_excluded(c[0]):
            s += float(c[4])
    print(int(s))
    sys.exit(0)

dst = args[0]
os.makedirs(dst, exist_ok=True)


def read_items_f0(path):
    out = []
    with open(path, "rb") as f:
        b = f.read()
    assert b[:8] == b"MKSDAT01"
    at = 8
    while at + 28 <= len(b):
        kind, spk, ns, nf, nt, nn = struct.unpack_from("<6I", b, at)
        at += 28
        at += nt
        at += 4 * nn
        f0 = struct.unpack_from("<%df" % nf, b, at)
        at += 4 * nf
        at += 2 * ns
        out.append((spk, f0))
    return out


groups = {}
for d in sorted(os.listdir(OUT_DIR)):
    if not os.path.exists(os.path.join(OUT_DIR, d, "cand.tsv")):
        continue
    g = d.rsplit("-t", 1)[0] if "-t" in d[-4:] else d
    groups.setdefault(g, []).append(d)

stats = dict(groups=0, segs=0, null_word=0, bad_match=0, no_line=0)
for g, takes in sorted(groups.items()):
    if not os.path.exists(os.path.join(SHARDS, g + ".picked")) or group_excluded(g):
        continue
    best = {}
    for d in takes:
        for k, l in enumerate(open(os.path.join(OUT_DIR, d, "cand.tsv")).read().splitlines()):
            c = l.split("\t")
            key = (int(c[0]), int(c[1]))
            wb = (float(c[2]), float(c[3]))
            if key not in best or wb < best[key][0]:
                best[key] = (wb, d, k)
    if not best:
        continue
    lines_out = []
    cache = {}
    for (p0, p1), (_, d, k) in sorted(best.items()):
        if d not in cache:
            al = json.load(open(os.path.join(OUT_DIR, d, "alignment.json")))
            kar = json.load(open(os.path.join(OUT_DIR, d, "karaoke.json")))
            items = read_items_f0(os.path.join(OUT_DIR, d, "cand.mksdat"))
            cache[d] = (al, kar, items)
        al, kar, items = cache[d]
        words = al["words"][p0:p1]
        if any(w.get("heard") is None for w in words):
            stats["null_word"] += 1
            continue
        song_f0 = al["f0_hz_10ms"]
        spk, f0 = items[k]
        wt0 = words[0]["t0"]
        cands = [ln for ln in kar["lines"] if ln["t0"] - 0.05 <= wt0 <= ln["t1"] + 0.05]
        if not cands:
            stats["no_line"] += 1
            continue
        est = int(max(cands[0]["t0"] - 0.15, 0.0) * 100)

        def err(off):
            n = min(len(f0), len(song_f0) - off)
            if off < 0 or n <= 0:
                return 1e9
            return sum(abs(f0[i] - song_f0[off + i]) for i in range(n)) / n

        bestoff = min(range(est - 3, est + 4), key=err)
        e = err(bestoff)
        if e > 0.5:
            bestoff = min(range(max(est - 60, 0), est + 61), key=err)
            e = err(bestoff)
        if e > 0.5:
            stats["bad_match"] += 1
            continue
        s0 = bestoff / 100.0
        lines_out.append("seg\t%s\t%d\t%d\t%d\t%d\t%.3f" % (os.path.join(OUT_DIR, d, "cand.mksdat"), k, bestoff, p0, p1, e))
        for w in words:
            lines_out.append("w\t%.3f\t%.3f\t%d\t%s\t%s" % (w["t0"] - s0, w["t1"] - s0, w.get("note") or 0, w["ipa"], w["word"]))
        stats["segs"] += 1
    if lines_out:
        stats["groups"] += 1
        open(os.path.join(dst, g + ".tsv"), "w").write("\n".join(lines_out) + "\n")
print(stats)
