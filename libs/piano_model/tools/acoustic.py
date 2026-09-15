#!/usr/bin/env python3
"""Offline native Salamander comparison; NumPy FFT, stdlib RIFF parsing, no playback."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import sys

try:
    import numpy as np
except ImportError:
    raise SystemExit("acoustic: NumPy is required for offline analysis; no corpus was measured")

ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit"
NOTES = [21, 24, 30, 33, 36, 45, 48, 60, 69, 72, 84, 96]
VELOCITIES = [28, 68, 112]
SFZ = "SalamanderGrandPianoV3.sfz"
BANDS = [(20, 500), (500, 2000), (2000, 8000)]
# Provisional diagnostic tolerances, chosen before model measurement. They are
# fixed physical-unit budgets, NOT current model errors times a margin.
LIMITS = {
    "early_mid_share_db": 6.0, "early_high_share_db": 6.0,
    "late_mid_share_db": 6.0, "late_high_share_db": 6.0,
    "p1_cluster_50_300_db": 6.0, "p1_cluster_1_2_db": 6.0,
    "rms_register_db": 6.0, "onset_energy_5_over_50": 0.15,
    "decay_low_db_s": 8.0, "decay_mid_db_s": 8.0, "decay_high_db_s": 8.0,
}
METHOD = {
    "version": 1,
    "stereo": "mean of independent channel powers; never FFT of L+R",
    "onset": "first nonoverlapping 1ms RMS frame > -40dB re peak frame in first 0.5s; align to frame start",
    "spectrum": "periodic Hann; next-power-of-two FFT; one-sided power / (Nfft * sum(window^2))",
    "bands_hz": BANDS, "band_edges": "lower inclusive, upper exclusive",
    "share_denominator_hz": [20, 20000],
    "share_windows_s": [[0.05, 0.1], [1.0, 2.0]],
    "partial_windows_s": [[0.05, 0.3], [1.0, 2.0]],
    "partial_bands": "[(n-0.4)*f0,(n+0.4)*f0), n=1..6, equal-tempered MIDI f0; 10log10(P1/max(P2..P6))",
    "rms": "unwindowed stereo RMS 0..2s; absolute dB re native C4 at SAME velocity plus within-set C4 register dB",
    "onset_energy": "unwindowed stereo sum-of-squares 0..5ms / 0..50ms",
    "decay": "negative OLS slope of 10log10 band power, 100ms Hann windows starting 0.1..0.9s in 50ms hops; positive means decay, dB/s",
    "floor": "power ratios floored at 1e-15; decay floor = whole-note 0..2s mean square * 1e-15",
    "sfz": "native attack-region selection only; no transposition, velocity gain, envelopes, loops or release/noise layers applied",
    "acceptance": "provisional diagnostics only; ignored Rust comparison is not final acceptance",
}


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_wav(path):
    """RIFF PCM16/24 or IEEE float32, including corresponding extensible GUIDs."""
    blob = Path(path).read_bytes()
    if len(blob) < 12 or blob[:4] != b"RIFF" or blob[8:12] != b"WAVE":
        raise ValueError(f"not a little-endian RIFF WAV: {path}")
    end = struct.unpack_from("<I", blob, 4)[0] + 8
    if end > len(blob):
        raise ValueError(f"truncated RIFF: {path}")
    fmt = payload = None
    pos = 12
    while pos + 8 <= end:
        tag, size = struct.unpack_from("<4sI", blob, pos)
        pos += 8
        if pos + size > end:
            raise ValueError(f"truncated WAV chunk: {path}")
        if tag == b"fmt ":
            fmt = blob[pos:pos + size]
        elif tag == b"data":
            if payload is not None:
                raise ValueError(f"multiple WAV data chunks: {path}")
            payload = blob[pos:pos + size]
        pos += size + (size & 1)
    if fmt is None or len(fmt) < 16 or payload is None:
        raise ValueError(f"missing WAV fmt/data: {path}")
    kind, channels, rate, byte_rate, align, bits = struct.unpack_from("<HHIIHH", fmt)
    if kind == 0xFFFE:
        if len(fmt) < 40 or struct.unpack_from("<H", fmt, 16)[0] < 22:
            raise ValueError(f"short extensible WAV format: {path}")
        valid_bits = struct.unpack_from("<H", fmt, 18)[0]
        if valid_bits != bits or fmt[28:40] != bytes.fromhex("00001000800000aa00389b71"):
            raise ValueError(f"unsupported extensible WAV encoding: {path}")
        kind = struct.unpack_from("<I", fmt, 24)[0]
    if channels not in (1, 2) or not rate or align != channels * (bits // 8) or byte_rate != rate * align:
        raise ValueError(f"invalid WAV channel/rate/alignment: {path}")
    if not payload or len(payload) % align:
        raise ValueError(f"empty or partial WAV frame: {path}")
    if kind == 1 and bits == 16:
        samples = np.frombuffer(payload, dtype="<i2").astype(np.float64) / 32768.0
    elif kind == 1 and bits == 24:
        octets = np.frombuffer(payload, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        packed = octets[:, 0] | (octets[:, 1] << 8) | (octets[:, 2] << 16)
        samples = ((packed ^ 0x800000) - 0x800000).astype(np.float64) / 8388608.0
    elif kind == 3 and bits == 32:
        samples = np.frombuffer(payload, dtype="<f4").astype(np.float64)
    else:
        raise ValueError(f"unsupported WAV format {kind}/{bits}: {path}")
    samples = samples.reshape(-1, channels)
    if not np.isfinite(samples).all():
        raise ValueError(f"non-finite WAV samples: {path}")
    return rate, samples


def sfz_regions(text):
    """Small parser for this corpus's numeric-key, whitespace-separated SFZ."""
    text = re.sub(r"//[^\n]*", "", text)
    group, global_values, regions = {}, {}, []
    for match in re.finditer(r"<(\w+)>([^<]*)", text):
        tag, body = match.groups()
        values = dict(re.findall(r"(\w+)\s*=\s*([^\s]+)", body))
        if tag == "global":
            global_values = values
            group = {}
        elif tag == "group":
            group = values
        elif tag == "region":
            regions.append(global_values | group | values)
        else:
            raise ValueError(f"unsupported SFZ header <{tag}>; inspect mapping before extending parser")
    return regions


def native_region(regions, note, velocity):
    matches = []
    for region in regions:
        if region.get("trigger", "attack") != "attack":
            continue
        # C4 regions in the actual SFZ omit pitch_keycenter: its default is 60.
        if (int(region.get("pitch_keycenter", 60)) == note
                and int(region.get("lokey", 0)) <= note <= int(region.get("hikey", 127))
                and int(region.get("lovel", 1)) <= velocity <= int(region.get("hivel", 127))):
            matches.append(region)
    if len(matches) != 1:
        raise ValueError(f"expected one native attack region for MIDI {note} velocity {velocity}, got {len(matches)}")
    name = re.search(r"([A-G])(#?)(-?\d+)v\d+\.wav$", matches[0].get("sample", ""))
    if not name:
        raise ValueError("native region must name a pitched velocity recording")
    pitch = 12 * (int(name[3]) + 1) + {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}[name[1]] + bool(name[2])
    if pitch != note:
        raise ValueError(f"native filename pitch {pitch} disagrees with SFZ pitch center {note}")
    return matches[0]


def onset_index(x, rate):
    frame = max(1, round(rate * 0.001))
    count = min(len(x), round(rate * 0.5)) // frame
    if count == 0:
        raise ValueError("audio too short to locate onset")
    powers = np.mean(x[:count * frame].reshape(count, frame, -1) ** 2, axis=(1, 2))
    peak = float(powers.max())
    if peak <= 0:
        raise ValueError("silent first 0.5s; cannot align onset")
    return int(np.flatnonzero(powers > peak * 1e-4)[0]) * frame


def spectrum(x, rate):
    size = len(x)
    nfft = 1 << (size - 1).bit_length()
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(size) / size)
    channels = np.fft.rfft(x * window[:, None], n=nfft, axis=0)
    power = np.mean(np.abs(channels) ** 2, axis=1) / (nfft * np.sum(window ** 2))
    power[1:-1] *= 2
    return np.fft.rfftfreq(nfft, 1.0 / rate), power


def band(spec, lo, hi):
    freq, power = spec
    return float(power[(freq >= lo) & (freq < hi)].sum())


def ratio_db(numerator, denominator):
    if denominator <= 0:
        raise ValueError("zero analysis denominator")
    return 10 * np.log10(max(numerator / denominator, 1e-15))


def measure(x, rate, note):
    if rate < 16000:
        raise ValueError("analysis requires at least 16000 Hz (8kHz upper band)")
    onset = onset_index(x, rate)
    x = x[onset:]
    if len(x) < round(2 * rate):
        raise ValueError("need at least 2s of audio AFTER aligned onset")

    def section(a, b):
        return x[round(a * rate):round(b * rate)]

    metrics = {}
    for label, a, b in [("early", 0.05, 0.1), ("late", 1, 2)]:
        spec = spectrum(section(a, b), rate)
        total = band(spec, 20, min(20000, rate / 2))
        for name, limits in zip(["mid", "high"], BANDS[1:]):
            metrics[f"{label}_{name}_share_db"] = float(ratio_db(band(spec, *limits), total))
    f0 = 440 * 2 ** ((note - 69) / 12)
    for label, a, b in [("50_300", 0.05, 0.3), ("1_2", 1, 2)]:
        spec = spectrum(section(a, b), rate)
        partials = [band(spec, (n - 0.4) * f0, (n + 0.4) * f0) for n in range(1, 7)]
        metrics[f"p1_cluster_{label}_db"] = float(ratio_db(partials[0], max(partials[1:])))
    mean_square = float(np.mean(section(0, 2) ** 2))
    metrics["onset_energy_5_over_50"] = float(np.sum(section(0, 0.005) ** 2) / np.sum(section(0, 0.05) ** 2))
    times = np.array([0.1 + i * 0.05 for i in range(17)])
    spectra = [spectrum(section(t, t + 0.1), rate) for t in times]
    for name, limits in zip(["low", "mid", "high"], BANDS):
        db = np.array([10 * np.log10(max(band(spec, *limits), mean_square * 1e-15)) for spec in spectra])
        centered = times - times.mean()
        metrics[f"decay_{name}_db_s"] = float(-np.dot(centered, db - db.mean()) / np.dot(centered, centered))
    return {"onset_sample": onset, "rate_hz": rate, "rms_0_2": mean_square ** 0.5, "metrics": metrics}


def load_measurement(path, note):
    rate, samples = read_wav(path)
    return measure(samples, rate, note) | {"sha256": sha256(path), "channels": samples.shape[1]}


def reference(root, notes, velocities):
    if not (root / SFZ).is_file() or not (root / "README").is_file():
        raise ValueError(f"missing Salamander corpus at {root}; need {SFZ}, README and native 48khz24bit WAVs")
    readme = (root / "README").read_text()
    if "Author: Alexander Holm" not in readme or "creativecommons.org/licenses/by/3.0/" not in readme:
        raise ValueError("unexpected corpus attribution/license; inspect README")
    regions = sfz_regions((root / SFZ).read_text())
    rows = []
    for note in sorted(set(notes) | {60}):
        for velocity in velocities:
            region = native_region(regions, note, velocity)
            name = region["sample"].replace("\\", "/")
            path = root / name
            if not path.is_file():
                raise ValueError(f"missing native reference recording: {path}")
            layer_match = re.search(r"v(\d+)\.wav$", name)
            if not layer_match:
                raise ValueError(f"not a native velocity recording: {name}")
            row = load_measurement(path, note)
            if row["channels"] != 2:
                raise ValueError(f"expected stereo reference recording: {path}")
            rows.append(row | {"note": note, "velocity": velocity, "native_file": name,
                               "layer": int(layer_match[1]), "lovel": int(region.get("lovel", 1)),
                               "hivel": int(region.get("hivel", 127)), "pitch_keycenter": int(region.get("pitch_keycenter", 60)),
                               "sfz_amp_veltrack_unapplied": float(region.get("amp_veltrack", 100))})
    normalize(rows, rows)
    provenance = {"corpus": "Salamander Grand Piano V3 48kHz/24bit (README heading says V2)",
                  "instrument": "Yamaha C5", "author": "Alexander Holm", "license": "CC BY 3.0",
                  "license_url": "http://creativecommons.org/licenses/by/3.0/",
                  "sfz_file": SFZ, "sfz_sha256": sha256(root / SFZ),
                  "readme_file": "README", "readme_sha256": sha256(root / "README")}
    return provenance, rows


def normalize(rows, references):
    own_c4 = {r["velocity"]: r["rms_0_2"] for r in rows if r["note"] == 60}
    ref_c4 = {r["velocity"]: r["rms_0_2"] for r in references if r["note"] == 60}
    for row in rows:
        velocity = row["velocity"]
        row["rms_rel_reference_c4_db"] = float(20 * np.log10(row["rms_0_2"] / ref_c4[velocity]))
        row["metrics"]["rms_register_db"] = float(20 * np.log10(row["rms_0_2"] / own_c4[velocity]))


def model_directory(path, notes, velocities, references):
    rows = []
    manifest = path / "render.json"
    if not manifest.is_file():
        raise ValueError(f"missing completed renderer manifest: {manifest}")
    for note in sorted(set(notes) | {60}):
        for velocity in velocities:
            name = f"note_{note:03}_vel_{velocity:03}.wav"
            file = path / name
            if not file.is_file():
                raise ValueError(f"missing model WAV: {file} (include MIDI 60 for same-velocity C4 normalization)")
            row = load_measurement(file, note)
            if row["channels"] != 2:
                raise ValueError(f"expected stereo model recording: {file}")
            rows.append(row | {"note": note, "velocity": velocity, "file": name})
    normalize(rows, references)
    by_key = {(r["note"], r["velocity"]): r for r in references}
    failures = []
    for row in rows:
        ref = by_key[row["note"], row["velocity"]]
        row["delta_vs_reference"] = {key: row["metrics"][key] - ref["metrics"][key] for key in LIMITS}
        if row["note"] in notes:
            failures.extend({"note": row["note"], "velocity": row["velocity"], "metric": key,
                             "model": row["metrics"][key], "reference": ref["metrics"][key],
                             "delta": delta, "limit_abs": LIMITS[key]}
                            for key, delta in row["delta_vs_reference"].items() if abs(delta) > LIMITS[key])
    anchors = [{key: row[key] for key in ("note", "velocity", "file", "sha256", "rms_0_2")}
               for row in rows if row["note"] == 60]
    return {"render": json.loads(manifest.read_text()), "render_manifest_sha256": sha256(manifest),
            "c4_normalization_anchors": anchors,
            "rows": [r for r in rows if r["note"] in notes], "outside_provisional_tolerance": failures}


def fixture_text(provenance, rows):
    # TSV needs no serde/runtime dependency in the opt-in Rust test. Comments
    # carry JSON metadata; rows contain ONLY measurements from the recordings.
    lines = ["# salamander-acoustic-v1", "# provenance " + json.dumps(provenance, sort_keys=True),
             "# method " + json.dumps(METHOD, sort_keys=True),
             "# thresholds_abs\t" + "\t".join(str(value) for value in LIMITS.values())]
    fields = ["note", "velocity", "layer", "lovel", "hivel", "native_file", "sha256", "onset_sample", "rate_hz", "rms_0_2"]
    lines.append("\t".join(fields + list(LIMITS)))
    for row in rows:
        lines.append("\t".join([str(row[key]) if key != "rms_0_2" else format(row[key], ".10g") for key in fields]
                               + [format(row["metrics"][key], ".10g") for key in LIMITS]))
    return "\n".join(lines) + "\n"


def midi_list(text):
    values = [int(value) for value in text.split(",")]
    if not values or len(set(values)) != len(values):
        raise argparse.ArgumentTypeError("expected distinct comma-separated MIDI integers")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference-root", type=Path, default=CORPUS)
    parser.add_argument("--notes", type=midi_list, default=NOTES)
    parser.add_argument("--velocities", type=midi_list, default=VELOCITIES)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--out", type=Path, help="new JSON report; refuses overwriting")
    parser.add_argument("--fixture-out", type=Path, help="new reference-only TSV; no model inputs allowed")
    args = parser.parse_args()
    try:
        if not args.out and not args.fixture_out:
            raise ValueError("provide --out and/or --fixture-out")
        if any(not 21 <= n <= 108 for n in args.notes) or any(not 1 <= v <= 127 for v in args.velocities):
            raise ValueError("notes must be 21..108; velocities must be 1..127")
        if args.fixture_out and (args.baseline or args.candidate):
            raise ValueError("fixture generation accepts reference inputs only; compare models in a separate command")
        for path in (args.out, args.fixture_out):
            if path and path.exists():
                raise ValueError(f"refusing to overwrite {path}")
        provenance, refs = reference(args.reference_root, args.notes, args.velocities)
        selected = [r for r in refs if r["note"] in args.notes]
        report = {"schema": "salamander-acoustic-v1", "analysis": METHOD, "provenance": provenance,
                  "tool_sha256": sha256(__file__), "numpy_version": np.__version__,
                  "thresholds_abs": LIMITS, "reference": refs}
        for label, path in [("baseline", args.baseline), ("candidate", args.candidate)]:
            if path:
                report[label] = model_directory(path, args.notes, args.velocities, refs)
                failures = report[label]["outside_provisional_tolerance"]
                print(f"{label}: {len(failures)} metric deviations outside provisional tolerance")
        if args.baseline and args.candidate:
            baseline = {(r["note"], r["velocity"]): r for r in report["baseline"]["rows"]}
            for row in report["candidate"]["rows"]:
                prior = baseline[row["note"], row["velocity"]]
                row["delta_vs_baseline"] = {key: row["metrics"][key] - prior["metrics"][key] for key in LIMITS}
                row["absolute_error_change"] = {key: abs(row["delta_vs_reference"][key]) - abs(prior["delta_vs_reference"][key]) for key in LIMITS}
        if args.fixture_out:
            with args.fixture_out.open("x") as stream:
                stream.write(fixture_text(provenance, refs))
        if args.out:
            with args.out.open("x") as stream:
                json.dump(report, stream, indent=2, allow_nan=False)
                stream.write("\n")
        print(f"measured {len(selected)} native reference note/velocity pairs; diagnostics are not final acceptance")
    except (ValueError, OSError) as error:
        parser.exit(1, f"acoustic: {error}\n")


if __name__ == "__main__":
    main()
