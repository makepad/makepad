#!/usr/bin/env python3
"""Benchmarks against the Qwen3.8-27B endpoint: TTFT (streamed), decode tok/s, ~30K prefill sustained.

Usage: QWEN38_KEY=... python3 tools/qwen38/bench_api.py [--host 10.0.0.217] [--port 8090] [--runs 3]
"""
import argparse, json, os, sys, time, urllib.request

WORDS = ("the quick brown fox jumps over the lazy dog while seventeen engineers "
         "review the quarterly telemetry report and annotate every anomaly with "
         "careful notes about thermal drift, memory pressure, and scheduling jitter ").split()


def build_long_text(target_chars):
    out, i = [], 0
    while sum(len(w) + 1 for w in out) < target_chars:
        out.append(WORDS[i % len(WORDS)])
        if i % 29 == 0:
            out.append(f"[section-{i}]")
        i += 1
    return " ".join(out)


def post(base, key, body, timeout=600):
    r = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(body).encode())
    r.add_header("Content-Type", "application/json")
    r.add_header("Authorization", "Bearer " + key)
    t0 = time.time()
    with urllib.request.urlopen(r, timeout=timeout) as resp:
        b = json.loads(resp.read())
    return time.time() - t0, b


def stream_ttft(base, key, body, timeout=600):
    body = dict(body); body["stream"] = True
    r = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(body).encode())
    r.add_header("Content-Type", "application/json")
    r.add_header("Authorization", "Bearer " + key)
    t0 = time.time()
    first = None
    with urllib.request.urlopen(r, timeout=timeout) as resp:
        for line in resp:
            if line.startswith(b"data: ") and b"[DONE]" not in line:
                try:
                    d = json.loads(line[6:])
                    delta = d["choices"][0]["delta"]
                    if delta.get("content") or delta.get("reasoning_content"):
                        first = time.time() - t0
                        break
                except Exception:
                    pass
    return first


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="10.0.0.217")
    ap.add_argument("--port", default="8090")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--skip-long", action="store_true")
    args = ap.parse_args()
    key = os.environ.get("QWEN38_KEY")
    if not key:
        print("set QWEN38_KEY"); sys.exit(2)
    base = f"http://{args.host}:{args.port}"

    short = {"messages": [{"role": "user", "content": "Write one sentence about mountains."}],
             "max_tokens": 48, "temperature": 0,
             "chat_template_kwargs": {"enable_thinking": False}}

    print("== TTFT (stream, short prompt, no-think) ==")
    for i in range(args.runs):
        t = stream_ttft(base, key, short)
        print(f"  run{i+1}: ttft={round(t*1000,1) if t else None} ms")

    print("== decode tok/s (512-token gen, no-think) ==")
    gen = {"messages": [{"role": "user", "content":
           "Write a long detailed essay about the history of shipbuilding."}],
           "max_tokens": 512, "temperature": 0,
           "chat_template_kwargs": {"enable_thinking": False}}
    for i in range(args.runs):
        wall, b = post(base, key, gen)
        t = b.get("timings", {})
        print(f"  run{i+1}: wall={round(wall,2)}s prompt_n={t.get('prompt_n')} "
              f"prompt_tps={round(t.get('prompt_per_second') or 0,1)} "
              f"gen_n={t.get('predicted_n')} gen_tps={round(t.get('predicted_per_second') or 0,2)}")

    if not args.skip_long:
        print("== sustained ~30K-token prefill + 128 gen (no-think) ==")
        long_text = build_long_text(118000)
        body = {"messages": [
            {"role": "user", "content": "Here is a document:\n" + long_text +
             "\nHow many words roughly? Reply briefly."}],
            "max_tokens": 128, "temperature": 0, "cache_prompt": False,
            "chat_template_kwargs": {"enable_thinking": False}}
        for i in range(min(args.runs, 2)):
            wall, b = post(base, key, body, timeout=1200)
            t = b.get("timings", {})
            print(f"  run{i+1}: wall={round(wall,2)}s prompt_n={t.get('prompt_n')} "
                  f"prefill_tps={round(t.get('prompt_per_second') or 0,1)} "
                  f"prompt_ms={round(t.get('prompt_ms') or 0)} "
                  f"gen_tps@32k={round(t.get('predicted_per_second') or 0,2)}")


if __name__ == "__main__":
    main()
