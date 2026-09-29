#!/usr/bin/env python3
"""Keep the raw model's touch in a fitted calibration table.

The fitter targets the native Salamander layers, which are recorded without
their SFZ amp_veltrack, so a fitted table also pulls each key's pianissimo up
and fortissimo down toward the layers' near-equal levels. This shifts every
partial's pp and ff gain of a key by one constant each, so the calibrated
level at velocity 28 and 112 (stereo RMS, 0-2 s after onset, re velocity 68)
equals the raw model's. The mf gains and every spectral shape are unchanged.
Run it on renders of the raw model and of the calibrated table, render the
result and repeat until the shifts are ~0 (two or three rounds; nonlinear
attack noise makes it inexact at the top keys).

  preserve_touch.py RAW_RENDERS CALIBRATED_RENDERS TABLE.csv OUT.csv
"""
import csv
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acoustic  # noqa: E402

KNOTS = (28, 68, 112)


def level_db(directory, key, velocity):
    rate, x = acoustic.read_wav(Path(directory) / f"note_{key:03d}_vel_{velocity:03d}.wav")
    onset = acoustic.onset_index(x, rate)
    return 20 * math.log10(float(np.sqrt(np.mean(x[onset:onset + 2 * rate] ** 2))))


def main():
    if len(sys.argv) != 5:
        raise SystemExit(__doc__)
    raw, calibrated, table, out = sys.argv[1:]
    rows = list(csv.reader(open(table)))
    if rows[0] != ["key", "partial", "pp_db", "mf_db", "ff_db", "decay_scale"]:
        raise SystemExit("unexpected calibration CSV header")
    shift = {}
    for key in sorted({int(r[0]) for r in rows[1:]}):
        r = {v: level_db(raw, key, v) for v in KNOTS}
        c = {v: level_db(calibrated, key, v) for v in KNOTS}
        shift[key] = {v: (r[v] - r[68]) - (c[v] - c[68]) for v in (28, 112)}
        print(f"{key}: pp {shift[key][28]:+.2f} dB, ff {shift[key][112]:+.2f} dB")
    with open(out, "x") as f:
        f.write(",".join(rows[0]) + "\n")
        for r in rows[1:]:
            key = int(r[0])
            pp = min(24.0, max(-36.0, float(r[2]) + shift[key][28]))
            ff = min(24.0, max(-36.0, float(r[4]) + shift[key][112]))
            f.write(f"{r[0]},{r[1]},{pp:.6f},{r[3]},{ff:.6f},{r[5]}\n")


if __name__ == "__main__":
    main()
