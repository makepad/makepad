#!/usr/bin/env python3
"""Sung-lyric training data: original lyrics -> songs on one fleet node.

Writes original lyrics (a procedural lyric grammar over a phonetically varied
word bank) and a music description, asks one AI Hub node for a song with the
music model, and saves <inbox>/<id>.wav plus <id>.json (lyrics, caption,
seed, model, node). Standard library only. One job at a time; respects the
node's admission (a refused or paused node is retried later, never forced)
and keeps the job lease alive while it runs.

    song_producer.py --node http://10.0.0.123:8123 --inbox DIR [--count N] [--seconds 90]
"""
import argparse
import hashlib
import json
import pathlib
import random
import time
import urllib.error
import urllib.request
import uuid

MODEL = "minimax-music3-q4"

# A word bank chosen for phonetic coverage: every English vowel (short, long,
# diphthongs, r-coloured) and consonant, and clusters (str, spl, nd, mps...).
NOUNS = """heart night light fire river street city ocean shadow window morning summer winter
rain thunder silver mirror garden border story letter feather mountain valley engine
stranger dream echo moment promise season harbour candle velvet thread question answer
treasure pleasure measure freedom kingdom rhythm voice choice journey highway wonder
paper station bridge island signal crystal planet circus forest desert frozen lake
spring stream splinter strength glimpse twelfth sixths breath truth youth judge church
orange purple yellow lightning anthem garage vision azure jewel boy toy coin joy noise
cloud crowd mouth south house tower flower hour earth bird word world circle girl""".split()
VERBS = """run fly burn shine break hold fall rise call wait sing dance breathe dream fade
glow shake chase follow wander whisper scream remember forget believe carry gather
stumble tremble crumble sparkle shatter scatter splash stretch squeeze twist launch
jump climb drift sail turn learn yearn ache wake choose lose move prove leave weave""".split()
ADJS = """bright dark cold warm wild quiet loud golden broken empty endless hollow gentle
heavy lonely restless silent tender burning frozen open distant crimson faded sweet
strange brave careless thirsty lucky smooth rough sharp blue green grey high low""".split()
PLACES = ["in the {adj} {noun}", "across the {noun}", "under the {adj} sky", "through the {noun}",
          "beyond the {noun}", "beneath the {noun}", "along the {adj} {noun}", "inside the {noun}"]
LINES = [
    "I {verb} {place}",
    "we {verb} like a {adj} {noun}",
    "you {verb} and I {verb}",
    "the {noun} will {verb} {place}",
    "{adj} {noun}, {adj} {noun}",
    "hold the {noun} and {verb} with me",
    "every {noun} is {adj} tonight",
    "don't let the {noun} {verb}",
    "I can hear the {noun} {verb}",
    "we were {adj} and {adj}",
    "tell me how the {noun} can {verb}",
    "my {noun} is a {adj} {noun}",
]
GENRES = ["pop", "synth pop", "indie pop", "soft rock", "acoustic pop", "r&b", "folk pop", "electro pop", "ballad"]
KEYS = ["C major", "D major", "E major", "F major", "G major", "A major", "B flat major", "A minor", "E minor", "D minor"]
VOICE_MAIN = "female pop lead vocal, clear diction, upfront dry vocals, every word clearly sung"
VOICE_OTHER = ["male pop lead vocal, clear diction, upfront vocals", "soft female vocal, clear diction",
               "warm male tenor vocal, clear diction"]


def line(rng):
    t = rng.choice(LINES)
    out = ""
    while "{" in t:
        place = rng.choice(PLACES).format(adj=rng.choice(ADJS), noun=rng.choice(NOUNS))
        t = t.replace("{place}", place, 1)
        t = t.replace("{verb}", rng.choice(VERBS), 1).replace("{adj}", rng.choice(ADJS), 1).replace("{noun}", rng.choice(NOUNS), 1)
        out = t
    out = out or t
    # "a" before a vowel sound.
    words = out.split(" ")
    for i in range(len(words) - 1):
        if words[i] == "a" and words[i + 1][:1] in "aeiou":
            words[i] = "an"
    return " ".join(words)


def song(rng):
    verse = lambda: [line(rng) for _ in range(4)]
    chorus = [line(rng) for _ in range(4)]
    parts = ["[Verse]"] + verse() + ["", "[Chorus]"] + chorus
    lyrics = "\n".join(parts)
    voice = VOICE_MAIN if rng.random() < 0.8 else rng.choice(VOICE_OTHER)
    bpm = rng.randrange(80, 128)
    caption = f"{rng.choice(GENRES)}, {bpm} BPM, {rng.choice(KEYS)}, {voice}, simple sparse arrangement"
    return lyrics, caption, voice


def request(base, path, body=None, timeout=15):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def admission_open(base):
    try:
        h = request(base, "/health", timeout=5)
        return h.get("activity", {}).get("admission_open", True) and h.get("jobs_pending", 0) == 0
    except Exception:
        return False


def generate(base, lyrics, caption, seed, seconds, log):
    origin = {"origin_key": uuid.uuid4().hex, "origin_epoch": time.time_ns() // 1_000_000}
    acc = request(base, "/generate", {**origin, "model": MODEL, "prompt": caption, "lyrics": lyrics,
                                      "seconds": seconds, "seed": seed, "queue_policy": "reject"})
    job = acc["job_id"]
    start = time.monotonic()
    last = None
    while True:
        st = request(base, "/job/" + job)
        sig = (st["state"], st.get("stage"))
        if sig != last:
            log(f"  job {job[:8]} {sig[0]} {sig[1] or ''} {st.get('progress', '')}")
            last = sig
        if st["state"] in ("done", "error", "cancelled"):
            break
        beat = request(base, "/job/" + job + "/keepalive", origin)
        if beat.get("ok") is False:
            raise RuntimeError("lease lost: " + str(beat.get("reason")))
        if time.monotonic() - start > 1800:
            request(base, "/job/" + job + "/cancel", origin)
            raise RuntimeError("timeout")
        time.sleep(1.0)
    if st["state"] != "done":
        raise RuntimeError(st.get("error") or st["state"])
    art = [a for a in st["artifacts"] if a.get("url", "").startswith("/artifact/")][0]
    with urllib.request.urlopen(base + art["url"], timeout=60) as r:
        data = r.read()
    if art.get("sha256") and hashlib.sha256(data).hexdigest() != art["sha256"]:
        raise RuntimeError("artifact sha256 mismatch")
    return data, art, time.monotonic() - start


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--node", required=True)
    ap.add_argument("--inbox", required=True)
    ap.add_argument("--count", type=int, default=0, help="0 = forever")
    ap.add_argument("--seconds", type=float, default=35.0)
    ap.add_argument("--seed", type=int, default=int(time.time()))
    a = ap.parse_args()
    inbox = pathlib.Path(a.inbox)
    inbox.mkdir(parents=True, exist_ok=True)
    rng = random.Random(a.seed)
    log = lambda m: print(time.strftime("%H:%M:%S"), m, flush=True)
    made = 0
    while a.count == 0 or made < a.count:
        if not admission_open(a.node):
            log("node busy or not admitting; waiting")
            time.sleep(60)
            continue
        lyrics, caption, voice = song(rng)
        seed = rng.randrange(1 << 31)
        sid = f"{time.strftime('%Y%m%d-%H%M%S')}-{seed:08x}"
        try:
            data, art, secs = generate(a.node, lyrics, caption, seed, a.seconds, log)
        except Exception as e:
            log(f"generation failed: {e}; retrying in 60 s")
            time.sleep(60)
            continue
        ext = ".wav" if data[:4] == b"RIFF" else ".bin"
        (inbox / (sid + ext + ".part")).write_bytes(data)
        (inbox / (sid + ".json")).write_text(json.dumps({"lyrics": lyrics, "caption": caption, "voice": voice, "seed": seed,
                                                       "model": MODEL, "node": a.node, "seconds": a.seconds,
                                                       "content_type": art.get("content_type"), "gen_seconds": secs}, indent=1))
        (inbox / (sid + ".lyrics.txt")).write_text(lyrics + "\n")
        (inbox / (sid + ".caption.txt")).write_text(caption + "\n")
        (inbox / (sid + ext + ".part")).rename(inbox / (sid + ext))
        made += 1
        log(f"song {made}: {sid}{ext} ({len(data) / 1e6:.1f} MB, {secs:.0f} s)")


if __name__ == "__main__":
    main()
