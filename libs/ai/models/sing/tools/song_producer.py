#!/usr/bin/env python3
"""Sung-lyric training data: original lyrics -> songs on one fleet node.

Writes original lyrics (a procedural lyric grammar over a phonetically varied
word bank) and a music description, asks one AI Hub node for --takes songs
(seeds) of each with the music model, separates their vocals in a batch, and
saves <inbox>/<group>-t<k>.wav, .vocals.wav.v and .json (lyrics, caption,
seed, model, node, group, takes). Standard library only. One job at a time;
respects the node's admission (a refused or paused node is retried later,
never forced) and keeps the job lease alive while it runs.

    song_producer.py --node http://10.0.0.123:8123 --inbox DIR [--takes 2] [--batch 4] [--model ace-step-1.5-xl]
"""
import argparse
import array
import base64
import hashlib
import json
import pathlib
import random
import time
import http.client
import os
import ssl
import urllib.parse
import uuid

MODEL = "minimax-music3-q4"  # --model overrides (e.g. ace-step-1.5-xl)

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
# Every major and minor key, so the sung range moves from set to set.
TONICS = ["C", "C sharp", "D", "E flat", "E", "F", "F sharp", "G", "A flat", "A", "B flat", "B"]
KEYS = [f"{t} {m}" for t in TONICS for m in ("major", "minor")]
VOICE_MAIN = "female pop lead vocal, clear diction, upfront dry vocals, every word clearly sung"
# Every song asks for one clear solo voice (style "b"; "a", a sparse
# arrangement alone, kept more ad-libs and backing vocals).
STYLES = {
    "b": "solo lead vocal only, no backing vocals, no harmonies, no ad-libs, sparse arrangement",
}
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
    style = rng.choice(sorted(STYLES))
    caption = f"{rng.choice(GENRES)}, {bpm} BPM, {rng.choice(KEYS)}, {voice}, {STYLES[style]}"
    return lyrics, caption, voice, style


HUB = pathlib.Path(os.path.expanduser("~/.makepad/ai-hub"))


def fetch(base, path, body=None, timeout=15):
    """One request to a node: http://host:port as is; https://host:port over TLS
    pinned to the node's certificate (fleet-pins.txt), checked before the
    credential (client.token) is sent as a Bearer header."""
    u = urllib.parse.urlsplit(base)
    data = None if body is None else json.dumps(body).encode()
    headers = {"Content-Type": "application/json"}
    if u.scheme == "https":
        pins = dict(l.split()[:2] for l in (HUB / "fleet-pins.txt").read_text().splitlines() if l.strip() and not l.startswith("#"))
        pin = pins[f"{u.hostname}:{u.port}"]
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        ctx.minimum_version = ssl.TLSVersion.TLSv1_2
        conn = http.client.HTTPSConnection(u.hostname, u.port, context=ctx, timeout=timeout)
        conn.connect()
        if hashlib.sha256(conn.sock.getpeercert(binary_form=True)).hexdigest() != pin:
            conn.close()
            raise RuntimeError(f"{u.hostname}:{u.port}: certificate does not match its pin")
        headers["Authorization"] = "Bearer " + (HUB / "client.token").read_text().strip()
    else:
        conn = http.client.HTTPConnection(u.hostname, u.port, timeout=timeout)
    try:
        conn.request("GET" if data is None else "POST", path, body=data, headers=headers)
        r = conn.getresponse()
        out = r.read()
        if r.status >= 400:
            raise RuntimeError(f"HTTP {r.status}: {out[:200].decode(errors='replace')}")
        return out
    finally:
        conn.close()


def request(base, path, body=None, timeout=15):
    return json.loads(fetch(base, path, body, timeout))


def admission_open(base):
    try:
        h = request(base, "/health", timeout=5)
        return h.get("activity", {}).get("admission_open", True) and h.get("jobs_pending", 0) == 0
    except Exception:
        return False


def generate(base, lyrics, caption, seed, seconds, log, body=None):
    origin = {"origin_key": uuid.uuid4().hex, "origin_epoch": time.time_ns() // 1_000_000}
    body = body or {"model": MODEL, "prompt": caption, "lyrics": lyrics, "seconds": seconds, "seed": seed, "queue_policy": "reject"}
    acc = request(base, "/generate", {**origin, **body}, timeout=120)
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
    data = fetch(base, art["url"], timeout=60)
    if art.get("sha256") and hashlib.sha256(data).hexdigest() != art["sha256"]:
        raise RuntimeError("artifact sha256 mismatch")
    return data, art, time.monotonic() - start


def to_44k1(wav):
    """16-bit stereo WAV at any rate -> 44.1 kHz (the separator's rate), by
    linear interpolation; unchanged when already 44.1 kHz or not 16-bit stereo."""
    if wav[:4] != b"RIFF":
        return wav
    i, rate, ch, bits, pcm = 12, 0, 0, 0, None
    while i + 8 <= len(wav):
        cid, size = wav[i:i + 4], int.from_bytes(wav[i + 4:i + 8], "little")
        if cid == b"fmt ":
            ch = int.from_bytes(wav[i + 10:i + 12], "little")
            rate = int.from_bytes(wav[i + 12:i + 16], "little")
            bits = int.from_bytes(wav[i + 22:i + 24], "little")
        elif cid == b"data":
            pcm = wav[i + 8:i + 8 + size]
            break
        i += 8 + size + (size & 1)
    if pcm is None or rate == 44100 or ch != 2 or bits != 16:
        return wav
    src = array.array("h")
    src.frombytes(pcm[:len(pcm) // 4 * 4])
    n = len(src) // 2
    m = int(n * 44100 / rate)
    out = array.array("h", [0]) * (2 * m)
    step = rate / 44100
    for j in range(m):
        x = j * step
        k = int(x)
        f = x - k
        k1 = min(k + 1, n - 1)
        out[2 * j] = int(src[2 * k] * (1 - f) + src[2 * k1] * f)
        out[2 * j + 1] = int(src[2 * k + 1] * (1 - f) + src[2 * k1 + 1] * f)
    body = out.tobytes()
    return b"RIFF" + (36 + len(body)).to_bytes(4, "little") + b"WAVEfmt " + (16).to_bytes(4, "little") + (1).to_bytes(2, "little") + (2).to_bytes(2, "little") \
        + (44100).to_bytes(4, "little") + (44100 * 4).to_bytes(4, "little") + (4).to_bytes(2, "little") + (16).to_bytes(2, "little") + b"data" + len(body).to_bytes(4, "little") + body


def separate(node, wav, log):
    """The vocal of a 44.1 kHz stereo WAV via a node's four-stem separator:
    16-bit stereo WAV bytes."""
    body = {"model": "bs-roformer-4stem", "input_b64": base64.b64encode(wav).decode(), "queue_policy": "queue"}
    data, art, secs = generate(node, None, None, None, None, log, body=body)
    if data[:4] != b"MPST":
        raise RuntimeError("not a stems artifact")
    rate = int.from_bytes(data[8:12], "little")
    frames = int.from_bytes(data[12:20], "little")
    planar = array.array("f")
    planar.frombytes(data[20:20 + 8 * frames * 4])
    left, right = planar[6 * frames:7 * frames], planar[7 * frames:8 * frames]
    pcm = array.array("h", [0]) * (2 * frames)
    for i in range(frames):
        pcm[2 * i] = max(-32768, min(32767, int(left[i] * 32767)))
        pcm[2 * i + 1] = max(-32768, min(32767, int(right[i] * 32767)))
    body = pcm.tobytes()
    hdr = b"RIFF" + (36 + len(body)).to_bytes(4, "little") + b"WAVEfmt " + (16).to_bytes(4, "little") + (1).to_bytes(2, "little") + (2).to_bytes(2, "little") \
        + rate.to_bytes(4, "little") + (rate * 4).to_bytes(4, "little") + (4).to_bytes(2, "little") + (16).to_bytes(2, "little") + b"data" + len(body).to_bytes(4, "little")
    return hdr + body, secs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--node", required=True)
    ap.add_argument("--inbox", required=True)
    ap.add_argument("--count", type=int, default=0, help="lyric sets to make; 0 = forever")
    ap.add_argument("--seconds", type=float, default=35.0)
    ap.add_argument("--seed", type=int, default=int(time.time()))
    ap.add_argument("--model", default=MODEL)
    ap.add_argument("--takes", type=int, default=2, help="songs (seeds) per lyric set; the harvester keeps each line's best take")
    ap.add_argument("--batch", type=int, default=4, help="lyric sets generated before the batch is separated")
    ap.add_argument("--sep-node", default=None, help="the node that separates the vocals (default: --node)")
    a = ap.parse_args()
    sep_node = a.sep_node or a.node
    inbox = pathlib.Path(a.inbox)
    inbox.mkdir(parents=True, exist_ok=True)
    rng = random.Random(a.seed)
    log = lambda m: print(time.strftime("%H:%M:%S"), m, flush=True)
    made = 0
    while a.count == 0 or made < a.count:
        # A batch: every take of every lyric set on the music model (resident
        # across the batch), then every vocal on the separator, so each model
        # loads once per batch rather than once per song.
        batch = []
        for _ in range(a.batch if a.count == 0 else min(a.batch, a.count - made)):
            lyrics, caption, voice, style = song(rng)
            group = f"{time.strftime('%Y%m%d-%H%M%S')}-{rng.randrange(1 << 31):08x}"
            takes = []
            while len(takes) < a.takes:
                if not admission_open(a.node):
                    log("node busy or not admitting; waiting")
                    time.sleep(60)
                    continue
                seed = rng.randrange(1 << 31)
                try:
                    body = {"model": a.model, "prompt": caption, "lyrics": lyrics, "seconds": a.seconds, "seed": seed, "queue_policy": "reject"}
                    data, art, secs = generate(a.node, lyrics, caption, seed, a.seconds, log, body=body)
                    data = to_44k1(data)
                except Exception as e:
                    log(f"generation failed: {e}; retrying in 60 s")
                    time.sleep(60)
                    continue
                takes.append((seed, data, art, secs))
                log(f"{group} take {len(takes)}/{a.takes} ({len(data) / 1e6:.1f} MB, {secs:.0f} s)")
            batch.append((group, lyrics, caption, voice, style, takes))
        for group, lyrics, caption, voice, style, takes in batch:
            for k, (seed, data, art, secs) in enumerate(takes):
                sid = f"{group}-t{k}"
                ext = ".wav" if data[:4] == b"RIFF" else ".bin"
                if ext == ".wav":
                    # The vocal first: the harvester picks a song up when its .wav appears.
                    try:
                        vocals, ssecs = separate(sep_node, data, log)
                        (inbox / (sid + ".vocals.part")).write_bytes(vocals)
                        (inbox / (sid + ".vocals.part")).rename(inbox / (sid + ".vocals.wav.v"))
                        log(f"  {sid} separated on {sep_node} in {ssecs:.0f} s")
                    except Exception as e:
                        log(f"  {sid} separation failed ({e}); the harvester will separate")
                (inbox / (sid + ".json")).write_text(json.dumps({"lyrics": lyrics, "caption": caption, "voice": voice, "seed": seed,
                                                               "model": a.model, "node": a.node, "seconds": a.seconds, "style": style,
                                                               "group": group, "take": k, "takes": len(takes),
                                                               "content_type": art.get("content_type"), "gen_seconds": secs}, indent=1))
                (inbox / (sid + ".lyrics.txt")).write_text(lyrics + "\n")
                (inbox / (sid + ".caption.txt")).write_text(caption + "\n")
                (inbox / (sid + ext + ".part")).write_bytes(data)
                (inbox / (sid + ext + ".part")).rename(inbox / (sid + ext))
            made += 1
            log(f"set {made}: {group} ({len(takes)} takes)")


if __name__ == "__main__":
    main()
