#!/usr/bin/env python3
"""Functional + auth tests for the Qwen3.8-27B llama-server endpoint on .217.

Usage: QWEN38_KEY=... python3 tools/qwen38/test_api.py [--host 10.0.0.217] [--port 8090]
Prints PASS/FAIL lines; exit 1 if any FAIL.
"""
import argparse, base64, json, os, struct, sys, time, urllib.request, urllib.error, zlib


def make_png():
    # 64x64: left half red, right half blue, 8px white square center
    w = h = 64
    rows = []
    for y in range(h):
        row = bytearray([0])  # filter type 0
        for x in range(w):
            if 28 <= x < 36 and 28 <= y < 36:
                px = (255, 255, 255)
            elif x < w // 2:
                px = (220, 30, 30)
            else:
                px = (30, 30, 220)
            row += bytes(px)
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(typ, data):
        c = struct.pack(">I", len(data)) + typ + data
        return c + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def req(base, key, path, body=None, timeout=180, method=None, raw=False):
    url = base + path
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method or ("POST" if data else "GET"))
    r.add_header("Content-Type", "application/json")
    if key is not None:
        r.add_header("Authorization", "Bearer " + key)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            b = resp.read()
            return resp.status, (b if raw else json.loads(b))
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")[:300]
    except Exception as e:
        return -1, str(e)


results = []

def check(name, ok, detail=""):
    results.append((name, ok))
    print(("PASS " if ok else "FAIL ") + name + ("  | " + str(detail)[:400] if detail else ""))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="10.0.0.217")
    ap.add_argument("--port", default="8090")
    args = ap.parse_args()
    key = os.environ.get("QWEN38_KEY")
    if not key:
        print("set QWEN38_KEY"); sys.exit(2)
    base = f"http://{args.host}:{args.port}"

    st, _ = req(base, None, "/health")
    check("health_no_auth", st == 200, st)

    st, body = req(base, None, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "hi"}], "max_tokens": 4})
    check("unauth_rejected_401", st == 401, st)

    st, body = req(base, "wrong-key-123", "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "hi"}], "max_tokens": 4})
    check("wrongkey_rejected_401", st == 401, st)

    st, body = req(base, key, "/v1/models")
    ok = st == 200 and any("qwen3.8" in m.get("id", "").lower() for m in body.get("data", []))
    check("models_lists_qwen38", ok, body if not ok else body["data"][0]["id"])

    # thinking (default) math — low effort to keep it quick, greedy
    st, body = req(base, key, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "What is 84 * 3 / 2? Reply with just the number."}],
        "max_tokens": 1200, "temperature": 0,
        "chat_template_kwargs": {"reasoning_effort": "low"}})
    if st == 200:
        msg = body["choices"][0]["message"]
        content = msg.get("content") or ""
        rc = msg.get("reasoning_content") or ""
        check("think_math_126", "126" in content, content[:120])
        check("think_reasoning_present", len(rc) > 0, ("rc_len=" + str(len(rc))))
        t = body.get("timings", {})
        print(f"  timings: prompt_n={t.get('prompt_n')} prompt_ms={t.get('prompt_ms')} "
              f"pred_n={t.get('predicted_n')} pred_per_sec={round(t.get('predicted_per_second') or 0,1)}")
    else:
        check("think_math_126", False, body)

    # non-thinking
    st, body = req(base, key, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "What is 84 * 3 / 2? Reply with just the number."}],
        "max_tokens": 60, "temperature": 0,
        "chat_template_kwargs": {"enable_thinking": False}})
    if st == 200:
        msg = body["choices"][0]["message"]
        content = msg.get("content") or ""
        rc = msg.get("reasoning_content") or ""
        check("nothink_math_126", "126" in content, content[:120])
        check("nothink_no_reasoning", len(rc.strip()) == 0 and "<think>" not in content, "rc_len=" + str(len(rc)))
    else:
        check("nothink_math_126", False, body)

    # xhigh (default) effort quick probe — verify template accepts default path
    st, body = req(base, key, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "Say OK."}],
        "max_tokens": 2500, "temperature": 0})
    ok = st == 200 and (body["choices"][0]["message"].get("content") or "").strip() != ""
    check("default_effort_completes", ok,
          (body["choices"][0]["message"].get("content") or "")[:80] if st == 200 else body)

    # tool call
    tools = [{
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "Get current weather for a city",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string", "description": "City name"}},
                "required": ["city"]}}}]
    st, body = req(base, key, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": "What's the weather in Amsterdam right now? Use the tool."}],
        "tools": tools, "max_tokens": 1200, "temperature": 0,
        "chat_template_kwargs": {"reasoning_effort": "low"}})
    if st == 200:
        msg = body["choices"][0]["message"]
        tcs = msg.get("tool_calls") or []
        okname = tcs and tcs[0]["function"]["name"] == "get_weather"
        argok = False
        if okname:
            try:
                a = json.loads(tcs[0]["function"]["arguments"])
                argok = "amsterdam" in json.dumps(a).lower()
            except Exception:
                argok = False
        check("toolcall_parsed", bool(okname), json.dumps(tcs)[:300] if tcs else ("content=" + (msg.get("content") or "")[:200]))
        check("toolcall_args_city", argok, tcs[0]["function"]["arguments"][:160] if tcs else "")
        check("toolcall_finish_reason", body["choices"][0].get("finish_reason") == "tool_calls",
              body["choices"][0].get("finish_reason"))
    else:
        check("toolcall_parsed", False, body)

    # vision
    png64 = base64.b64encode(make_png()).decode()
    st, body = req(base, key, "/v1/chat/completions", {
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "Left half color and right half color of this image, one word each."},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64," + png64}}]}],
        "max_tokens": 1200, "temperature": 0,
        "chat_template_kwargs": {"reasoning_effort": "low"}}, timeout=300)
    if st == 200:
        content = (body["choices"][0]["message"].get("content") or "").lower()
        check("vision_red_blue", ("red" in content and "blue" in content), content[:200])
    else:
        check("vision_red_blue", False, body)

    n_fail = sum(1 for _, ok in results if not ok)
    print(f"== {len(results) - n_fail}/{len(results)} passed ==")
    sys.exit(1 if n_fail else 0)


if __name__ == "__main__":
    main()
