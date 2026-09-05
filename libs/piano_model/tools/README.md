# Offline acoustic benchmark

This measures the current library against native Salamander recordings without
an audio device, GUI, worker threads, resampling, or a runtime dependency. Python
3.11+ and NumPy are required for analysis; RIFF decoding uses stdlib parsing and
NumPy integer conversion (PCM16, PCM24, float32, including matching extensible
formats). Rust tests reuse only the existing dependency-free FFT and regression.

## Calibrated physical piano

`Piano::new` and the physical presets now use the measured modal calibration:
30 native pitch anchors, three velocity knots (28, 68, 112), and corrections
for all 240 possible string partials. Pitch and velocity interpolate gain in
dB; decay corrections interpolate in log space. The table changes string
excitation and decay while retaining the hammer, soundboard, pedals and room.
`Piano::new_uncalibrated` preserves the raw instrument for comparisons.

The fit improves bass sustained body, early partial balance, register loudness
and selected treble brightness measurements against Salamander Grand Piano V3
(Alexander Holm, CC BY 3.0). Six active tests in `acoustic_reference.rs` protect
those improvements, C3 attack and C4 velocity dynamics. `sound.rs` additionally
checks C7's early second-partial balance against the native recording while
keeping the old FluidR3 bounds as an explicit raw-model regression. These
targeted checks do not imply that every note matches the recorded piano.

Run the complete offline verification with:

```sh
cargo test --offline --release -p makepad-piano-model --all-targets
python3 -m unittest discover -s libs/piano_model/tools -p 'test_*.py'
```

From the repository root:

```sh
cargo build --offline --release --example render_acoustic -p makepad-piano-model
mkdir -p libs/piano_model/tools/runs
./target/release/examples/render_acoustic --out libs/piano_model/tools/runs/baseline --rate 48000 --notes 21,24,30,33,36,45,48,60,69,72,84,96 --velocities 28,68,112 --seconds 4
python3 libs/piano_model/tools/acoustic.py --baseline libs/piano_model/tools/runs/baseline --out libs/piano_model/tools/runs/baseline-report.json
```

The renderer defaults to `--stock` (`Piano::new`, including stock calibration).
`--raw` uses `Piano::new_with_params(rate, &DesignParams::default())`, equivalent
to `Piano::new_uncalibrated`. Custom raw
designs use, for example, `--raw --design rad_hp1=90,rad_hp2=40`. A design override
requires raw mode. All modes retain constructor output effects unless `--dry`
is given, with no preset, pedal, or note-off. Each pair starts a **fresh
instrument**, receives NoteOn at sample zero, and writes stereo float32
`note_021_vel_028.wav`-style files without additional clipping by the WAV writer.
`render.json` records the mode, parameters, timing, block size, `dry` flag and
actual effects selection after successful completion. Raw does not mean effects
bypassed.

`--voicing name=value,...` starts from `Voicing::default()` and applies the
overrides with `set_voicing` on every fresh instrument before NoteOn. It works
with stock, raw (including design overrides), custom calibration, and `--dry`.
The fields are `body_tap`, `knock`, `roughness`, `phantoms`, `attack_noise`,
`attack_body`, and `sympathetic`. Values must be finite within 0..2.5 inclusive,
except `attack_body`, which is limited to 0..1. Unknown fields, malformed values,
and values outside these bounds fail before creating the output directory.
Supply `--voicing` once; repeated fields within its list use the last value.
Omitted fields retain their defaults (1 for every field except `attack_body`,
which is 0); omitting the option preserves the default sound. Every `render.json`
includes all seven effective values in `voicing`, including defaults, with the
actual `f32` values widened to JSON numbers so their precision is retained.

```sh
./target/release/examples/render_acoustic --out libs/piano_model/tools/runs/fit-no-knock --calibration libs/piano_model/tools/runs/fit.csv --dry --voicing knock=0
```

Use `--dry` for fitting: it independently sets reverb mix and early reflection
level to zero and disables soft clipping (which also bypasses the limiter),
before processing any samples. The full modelled soundboard radiation remains.
Without `--dry`, constructor effects remain unchanged (reverb mix 0.3, early
reflection level 0.7, limiter/soft clipping on). Preserve existing default-effect
baseline WAVs for stock A/B; render fitting baselines and candidates into new
directories with `--dry`.

`--calibration FILE.csv` builds every fresh instrument with
`Piano::new_with_calibration(rate, &notes)` using an explicit construction-time
table, such as local fitter output. It replaces the stock table; default stock
mode or explicit `--stock` is allowed, but `--raw` and `--design` conflict.
The CSV must start with exactly:

```csv
key,partial,pp_db,mf_db,ff_db,decay_scale
```

Supply at least one MIDI key (21..108), with keys in strictly increasing groups
of exactly 240 rows (`CALIBRATION_PARTIALS`). Each group must contain partials
1..240 exactly once (in any order), covering every mode of the lowest register.
Gain arrays at each velocity and the decay array each have 240 entries; no
current mode uses a repeated last entry or a taper. Legacy 64-row groups are
rejected before output creation and must be regenerated from the full raw baseline.
The pp/mf/ff gain columns correspond to velocities 28/68/112 and must be finite
within -36..24 dB; decay scales must be finite within 0.1..4. Missing, empty,
malformed, duplicate or out-of-range tables fail before creating the output
directory. Parsing and instrument construction happen outside audio processing.
`render.json` uses mode `calibration` and embeds both the supplied path and the
exact CSV text in `calibration.path` / `calibration.csv` with JSON escaping;
provenance survives changes to the source file. Other modes record
`calibration: null`. No production calibration or voicing is changed.

```sh
./target/release/examples/render_acoustic --out libs/piano_model/tools/runs/raw-dry --raw --dry
./target/release/examples/render_acoustic --out libs/piano_model/tools/runs/fit-dry --calibration libs/piano_model/tools/runs/fit.csv --dry
```

Output directories must not exist; JSON/TSV output paths also refuse overwrite.
Keep a baseline directory, rebuild the example after runtime changes, and render
to a new candidate directory with the same CLI selection. Compare with:

```sh
python3 libs/piano_model/tools/acoustic.py --baseline libs/piano_model/tools/runs/baseline --candidate libs/piano_model/tools/runs/candidate --out libs/piano_model/tools/runs/comparison.json
```

The JSON contains per-pair measurements, SHA256s, signed model-reference deltas,
provisional deviations, candidate-baseline deltas, and changes in absolute
reference error (negative means closer). It never writes to model directories.
Include MIDI 60 at every velocity in model renders for C4 normalization, even
when comparing a subset. Its source hashes remain in subset reports. Successful
analysis exits zero even when a model differs: this is a diagnostic, not an
acceptance command. Missing corpus, model WAVs, completed manifests, unsupported
encodings, silence, or insufficient post-onset audio fail explicitly.

## Reference and fixture

The default corpus root is
`local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit`; override with
`--reference-root`. **Alexander Holm**, Yamaha C5, **CC BY 3.0**, as specified in
its README: <http://creativecommons.org/licenses/by/3.0/>. The archive is V3
although its README heading and SFZ comment still say V2.

Only native attack regions whose pitch center matches the MIDI note are used.
The actual SFZ selects **28 → layer 2 (27–34), 68 → layer 9 (65–72), and
112 → layer 14 (105–112)**. C4 omits `pitch_keycenter`, using SFZ's default 60;
the filename pitch is checked too. No transposed keys or guessed layer numbers.
The parser deliberately supports this corpus's simple numeric-key SFZ layout,
not the entire SFZ language. It does not apply SFZ gain/velocity tracking,
envelopes, loops, or release/noise regions to the source PCM.

`tests/data/salamander_v3.tsv` contains 36 reference-only rows. It records native
filenames, full WAV SHA256s, velocity bounds/layers, onset sample indices, Hz,
unwindowed RMS, and metrics. Metadata includes author/license, SFZ/README hashes,
and the analysis specification. No reference PCM is checked in. Reproduce into
a fresh file and compare byte-for-byte:

```sh
python3 libs/piano_model/tools/acoustic.py --fixture-out libs/piano_model/tools/runs/reference-regenerated.tsv
cmp libs/piano_model/tests/data/salamander_v3.tsv libs/piano_model/tools/runs/reference-regenerated.tsv
```

Fixture generation rejects model arguments, and automatically includes C4
anchors for subset selections. The checked-in fixture uses the full default set.

## Units and provisional tolerances

L/R powers are computed independently, then averaged; an anti-phase signal does
not cancel. Every signal aligns to the first nonoverlapping 1 ms RMS frame above
−40 dB relative to its largest such frame in the first 0.5 s. Windows below are
relative to that frame's start. Spectra use periodic Hann windows, padding to
the next power of two, and one-sided window-energy normalization. Frequency
bands include their lower edge and exclude their upper edge.

| Metric | Definition | Provisional absolute difference |
| --- | --- | --- |
| Early / late mid, high shares | 500–2000 and 2000–8000 Hz power / 20–20000 Hz power; 50–100 ms and 1–2 s; 10 log10, dB | 6 dB |
| Fundamental / cluster | P1 / max(P2…P6), 10 log10, dB; 50–300 ms and 1–2 s; broad `(n ± 0.4) × equal-tempered f0` bands | 6 dB |
| Register RMS | Stereo RMS 0–2 s / same-set C4 RMS at the same velocity; 20 log10, dB | 6 dB |
| Onset energy | Unwindowed sum of squares in first 5 ms / first 50 ms, fraction | 0.15 |
| Low, mid, high decay | Negative least-squares slope of 10 log10 band power, dB/s; 100 ms windows starting 0.1–0.9 s at 50 ms hops; bands 20–500, 500–2000, 2000–8000 Hz | 8 dB/s |

The JSON additionally reports raw RMS and RMS dB **relative to reference C4 at
the same velocity**. That absolute capture-gain-dependent comparison is not
gated; register RMS removes the model/reference C4 gain difference. Positive
decay means falling energy, negative means growth. Ratio floors are −150 dB;
decay power floors are relative to each signal's 0–2 s mean square.

These tolerances were selected before measuring the current model: 6 dB allows
a factor-four power-ratio error, 0.15 allows a 15-percentage-point onset error,
and 8 dB/s allows 7.2 dB divergence across 0.9 s. They are explicit diagnostic
budgets, not psychophysical acceptance limits or a fit to current errors.

The measurements characterize one piano/microphone setup. Weak bands can be
dominated by room noise (especially 20–500 Hz on high notes); no noise subtraction
or confidence filter is applied. Broad partial bands are not a fitted
inharmonicity estimate, and can include leakage/noise. Late treble can approach
the recording noise floor. Unless `--dry` is given, the renderer keeps its
artificial room and output processing. Native source recording gain and SFZ
layer velocity tracking are not a common loudness calibration. Do not interpret
every deviation as a voicing instruction. No final acceptance is claimed.

## Validation and baseline errors

```sh
python3 -m unittest discover -s libs/piano_model/tools -p test_acoustic.py -v
cargo test --offline --release -p makepad-piano-model --test acoustic_reference --test reference
cargo test --offline --release -p makepad-piano-model --test acoustic_reference stock_matches_native_acoustic_reference -- --ignored --nocapture
# Historical FluidR3 comparisons remain available, without any threshold edits:
cargo test --offline --release -p makepad-piano-model --test reference -- --ignored
```

At runtime source revision `4546edb0c734d15195dfb34c858461397e67ba38`, the release
build, six Python tests and three normal Rust acoustic checks passed. The
explicit acoustic comparison **failed 182 of 396 metric checks**. Rust and
Python identified the identical 182 deviations; printed values agreed within
`5e-7`. Six fresh raw renders with reversed note/velocity ordering were
byte-identical to stock. A raw design-override render also completed. These
checks verify the benchmark, not the instrument's acoustic acceptance.

| Metric | Outside budget / 36 | Worst pair (MIDI / velocity) | Model − reference |
| --- | ---: | --- | ---: |
| Early mid share | 9 | 96 / 68 | −16.713428 dB |
| Early high share | 17 | 30 / 28 | +24.956064 dB |
| Late mid share | 15 | 21 / 68 | −19.051525 dB |
| Late high share | 24 | 36 / 112 | −27.952017 dB |
| P1/cluster 50–300 ms | 20 | 72 / 112 | −23.330955 dB |
| P1/cluster 1–2 s | 28 | 48 / 28 | +36.253043 dB |
| Register RMS | 14 | 21 / 28 | +12.687532 dB |
| Onset 5/50 energy | 1 | 96 / 28 | +0.156294 |
| Low-band decay | 15 | 96 / 28 | +38.531435 dB/s |
| Mid-band decay | 15 | 96 / 68 | +17.456822 dB/s |
| High-band decay | 24 | 30 / 28 | +20.032005 dB/s |

For the diagnosed bass, A0 at velocity 112 has late mid-band share −16.849 dB
relative to reference and register RMS +8.155 dB; C1 has late P1/cluster
+18.164 dB. At C5/112 the early P1/cluster is −7.512524 dB versus reference
+15.818431 dB, and high-band decay is 12.978 dB/s slower. Brightness errors vary
by note/window/velocity: C6/112 early high-band share is actually 7.376 dB low.
The complete baseline error list is in local `runs/baseline-final-report.json` and
`runs/acoustic-rust.log` (ignored outputs, reproducible with the commands above).
Those results describe the raw baseline. The broad all-metrics diagnostic
remains ignored; the six targeted calibrated gates and native C7 check above
are active in the normal release suite.
