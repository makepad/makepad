#!/usr/bin/env python3
"""Generate bounded real Flux images on explicit AIHub nodes, with lease/progress evidence.

Uses only the standard library. Never overrides a node's activity policy or
cancels work it did not create. Output PNGs and JSON report stay in --output.
"""
import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import struct
import time
import urllib.error
import urllib.request
import uuid


def request(base, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
    attempts = 4 if body is None or path.endswith(("/keepalive", "/cancel")) else 1
    for attempt in range(attempts):
        try:
            with urllib.request.urlopen(req, timeout=10) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if error.code not in (429, 502, 503, 504) or attempt + 1 == attempts:
                raise RuntimeError(f"{path}: HTTP {error.code}: " + error.read(500).decode(errors="replace")) from error
            emit({"node": base, "path": path, "transport_retry": attempt + 1, "status": error.code})
            time.sleep(0.1 * (attempt + 1))


def emit(record):
    print(json.dumps(record, separators=(",", ":")), flush=True)


def exercise(base, index, args):
    origin = {"origin_key": uuid.uuid4().hex, "origin_epoch": time.time_ns() // 1_000_000}
    label = base.removeprefix("http://").replace(":", "-") + f"-{index}"
    report = {"node": base, "index": index, "model": args.model, "events": []}
    owned = None
    terminal = False
    start = time.monotonic()
    try:
        accepted = request(base, "/generate", {
            **origin, "model": args.model,
            "prompt": ["An old moss green 1960s saloon car, three quarter front view, studio photograph",
                       "An old cream 1950s coupe car, three quarter front view, studio photograph",
                       "An old burgundy 1930s touring car, three quarter front view, studio photograph"][index % 3],
            "width": 512, "height": 512, "steps": 4, "seed": 910600 + index,
            "queue_policy": "queue",
        })
        owned = accepted["job_id"]
        report["job_id"] = owned
        previous = None
        while time.monotonic() - start < args.timeout:
            status = request(base, "/job/" + owned)
            report["final"] = status
            signature = (status["state"], status.get("stage"), status.get("progress"))
            if signature != previous:
                event = {"seconds": round(time.monotonic() - start, 2), "state": signature[0], "stage": signature[1], "progress": signature[2]}
                report["events"].append(event)
                emit({"node": base, "index": index, **event})
                previous = signature
            if status["state"] in ("done", "error", "cancelled"):
                terminal = True
                if status["state"] != "done":
                    raise RuntimeError(status.get("error") or status["state"])
                pngs = []
                for artifact in status["artifacts"]:
                    path = artifact["url"]
                    if not path.startswith("/artifact/"):
                        raise RuntimeError("unexpected artifact path")
                    with urllib.request.urlopen(base + path, timeout=15) as response:
                        data = response.read(32 * 1024 * 1024 + 1)
                    digest = hashlib.sha256(data).hexdigest()
                    if artifact.get("sha256") and artifact["sha256"] != digest:
                        raise RuntimeError("artifact SHA256 mismatch")
                    if artifact.get("byte_len") is not None and artifact["byte_len"] != len(data):
                        raise RuntimeError("artifact byte length mismatch")
                    if not data.startswith(b"\x89PNG\r\n\x1a\n") or len(data) < 1000 or data[12:16] != b"IHDR" or struct.unpack(">II", data[16:24]) != (512, 512):
                        raise RuntimeError("expected a complete 512x512 PNG image")
                    target = args.output / f"{label}-{len(pngs)}.png"
                    target.write_bytes(data)
                    pngs.append({"path": str(target), "bytes": len(data), "sha256": digest})
                if not pngs:
                    raise RuntimeError("done without an image artifact")
                report.update(ok=True, images=pngs)
                return report
            beat = request(base, "/job/" + owned + "/keepalive", origin)
            if not beat.get("renewed"):
                raise RuntimeError("job lease was not renewed: " + str(beat.get("reason")))
            time.sleep(1)
        raise TimeoutError("generation deadline reached")
    except Exception as error:
        report.update(ok=False, error=str(error))
        return report
    finally:
        if owned and not terminal:
            try:
                request(base, "/job/" + owned + "/cancel", {})
            except Exception as error:
                report["cleanup_error"] = str(error)
        try:
            # Each request owns a unique origin, including an admission whose
            # HTTP reply was lost before we learned its job id.
            request(base, "/bye", {"origin_key": origin["origin_key"]})
        except Exception as error:
            report["lease_cleanup_error"] = str(error)
        report["seconds"] = round(time.monotonic() - start, 2)
        (args.output / f"{label}.json").write_text(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("nodes", nargs="+", help="Explicit http://host:port nodes")
    parser.add_argument("--model", default="flux1-schnell")
    parser.add_argument("--count", type=int, default=1, help="Concurrent requests per node (1..3)")
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.count <= 3 or not 30 <= args.timeout <= 900:
        parser.error("count must be 1..3 and timeout 30..900 seconds")
    args.output.mkdir(parents=True, exist_ok=True)
    pending = []
    reports = []
    for base in dict.fromkeys(node.rstrip("/") for node in args.nodes):
        try:
            health = request(base, "/health")
            if health.get("activity") and not health["activity"].get("admission_open"):
                raise RuntimeError("node respects local-use pause: " + health["activity"].get("reason", "unknown"))
            if request(base, "/jobs").get("jobs"):
                raise RuntimeError("node has existing work")
            pending.extend((base, index) for index in range(args.count))
        except Exception as error:
            reports.append({"node": base, "ok": False, "skipped": True, "error": str(error)})
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(12, max(1, len(pending)))) as pool:
        reports.extend(pool.map(lambda item: exercise(*item, args), pending))
    (args.output / "report.json").write_text(json.dumps(reports, indent=2))
    emit({"passed": sum(report["ok"] for report in reports), "total": len(reports), "report": str(args.output / "report.json")})
    return 0 if reports and all(report["ok"] for report in reports) else 1


if __name__ == "__main__":
    raise SystemExit(main())
