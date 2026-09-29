#!/usr/bin/env python3
"""Give a fitted calibration table the reference instrument's touch.

The fitter matches the native Salamander layers' spectra, but the layers are
recorded without their SFZ amp_veltrack (73): played through its SFZ, the
reference is quieter at pianissimo and louder at fortissimo than its layer
files. This shifts every partial's pp and ff gain of a key by one constant
each, so the calibrated level at velocity 28 and 112 (stereo RMS, 0-2 s
after onset) re velocity 68 equals the reference's: the native layer's level
times the SFZ velocity gain (1 - 0.73) + 0.73 (v/127)^2. The mf gains and
every spectral shape are unchanged. Render the result and repeat until the
shifts are ~0 (two or three rounds).

  preserve_touch.py CORPUS CALIBRATED_RENDERS TABLE.csv OUT.csv
"""
import csv
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acoustic  # noqa: E402
import fit_voicing  # noqa: E402

KNOTS = (28, 68, 112)
VELTRACK = 0.73


def level_db(x, rate):
    onset = acoustic.onset_index(x, rate)
    return 20 * math.log10(float(np.sqrt(np.mean(x[onset:onset + 2 * rate] ** 2))))


def reference_db(regions, key, velocity):
    region = fit_voicing.select_region(regions, key, velocity)
    rate, x = acoustic.read_wav(region.sample)
    gain = (1 - VELTRACK) + VELTRACK * (velocity / 127) ** 2
    return level_db(x, rate) + 20 * math.log10(gain)


def main():
    if len(sys.argv) != 5:
        raise SystemExit(__doc__)
    corpus, calibrated, table, out = sys.argv[1:]
    _, regions = fit_voicing.parse_sfz(corpus)
    rows = list(csv.reader(open(table)))
    if rows[0] != ["key", "partial", "pp_db", "mf_db", "ff_db", "decay_scale"]:
        raise SystemExit("unexpected calibration CSV header")
    shift = {}
    for key in sorted({int(r[0]) for r in rows[1:]}):
        ref = {v: reference_db(regions, key, v) for v in KNOTS}
        cal = {}
        for v in KNOTS:
            rate, x = acoustic.read_wav(Path(calibrated) / f"note_{key:03d}_vel_{v:03d}.wav")
            cal[v] = level_db(x, rate)
        shift[key] = {v: (ref[v] - ref[68]) - (cal[v] - cal[68]) for v in (28, 112)}
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
