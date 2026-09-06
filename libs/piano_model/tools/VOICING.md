# Offline measured modal voicing

`fit_voicing.py` proposes numeric strike gains and empirical decay corrections
from **complete radiated stereo model renders** and native Salamander recordings.
It does not use nominal `radiativity()`, fit waveform phase, tune frequencies,
optimize unrelated model parameters, or establish perceptual acceptance. A fit is
an input to a subsequent rerender and independent benchmark/listening review.
Only Python 3.10+ and NumPy are required; no audio device, GPU, network, or package
installation is used.

## Inputs and invocation

Run from the checkout, choosing a new output directory for each iteration:

```sh
python3 libs/piano_model/tools/fit_voicing.py \
  --renders local/piano-renders/raw \
  --corpus local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit \
  --out local/piano-voicing/pass1
```

Required render files are `note_{key:03}_vel_{velocity:03}.wav`, stereo IEEE
float32 at 48 kHz, held for at least four seconds, and a `render.json` JSON object.
The fitter reads the final renderer output, including its radiation, unisons,
soundboard and any effects actually present. Keep renderer settings fixed between
iterations. The manifest hashes `render.json`; the default analytic fitter preserves
its contents without assuming a renderer-specific schema. The optional decay probe
below requires and validates the renderer's calibration schema. WAV headers and
duration are checked, but the fitter cannot prove the renderer held the key or
used a claimed calibration.

The default discovers all 30 native keycenters from
`SalamanderGrandPianoV3.sfz` (the original, non-retuned SFZ). `--notes 48,60,72`
selects a pilot. Every selected key requires velocities **28, 68, 112**. Additional
velocities listed by `--velocities` are also required at every selected key.
Every additional render with a valid filename at a selected key is included,
even when not explicitly requested; extras need not exist at every other key.
Unselected/non-native keys are outside the fit. Missing required renders,
native regions, or selected reference WAVs fail before writing outputs. There is
no neighboring-pitch or nearest-available-layer substitution.

To require all 16 representative layers:

```sh
python3 libs/piano_model/tools/fit_voicing.py \
  --renders local/piano-renders/all-layers \
  --corpus local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit \
  --out local/piano-voicing/all-layers-pass1 \
  --velocities 13,28,35,40,45,48,53,60,68,76,84,92,100,112,116,124
```

SFZ velocity boundaries select the native attack recording: **28 → layer 2,
68 → layer 9, 112 → layer 14**. The reader respects global/group region
inheritance, recognizes native note filenames, verifies their pitch keycenters,
and excludes release, pedal and resonance samples. It rejects tuning offsets,
ambiguous regions, and unsupported preprocessor directives. This is deliberately
a parser for this corpus, not a general SFZ synthesizer. Targets are the recorded
PCM24 layer amplitudes: SFZ `amp_veltrack`, envelopes and playback gain are not
applied. Two velocities selecting the same layer still contribute two separate
model observations against that native recording.

`--out` is mandatory. The four named output files may be replaced only in that
explicit directory, after all input analysis and serialization succeed. Files
are staged then individually atomically replaced; the set of four is not a
filesystem transaction. Unrelated output files are untouched. Output must be
outside the corpus and render directories and must not overwrite `--previous`.
Source WAVs are never modified. Use a new output directory to retain each pass.

## Measurement and confidence

1. Decode little-endian RIFF PCM24 references and float32 renders without folding
   channels. The core WAV reader also accepts PCM16/32 and PCM/float extensible
   headers, and rejects damaged or non-finite audio. Onset is the first 1 ms
   block whose stereo RMS exceeds −40 dB relative to the largest block RMS in
   the first 0.5 seconds. The block start defines time zero reproducibly.
2. Independently identify each recording's lines in an onset-relative
   0.1–1.2 second Hann FFT. Fit `f_n = n f0 sqrt(1 + B n²)` using a deterministic
   coarse search and iterative robust weighted regression of `(f_n/n)²` against
   `n²`. Search `f0` within ±45 cents of MIDI pitch and `B` in `[0, 0.01]`.
   A zero-B boundary represents unresolved stiffness; it is not negative B.
   Local peak prominence must exceed 15 dB and line power must exceed −60 dB
   relative to the strongest line. Noise estimation excludes the close-unison
   cluster. The model and reference never share fitted `f0` or `B`.
3. Retain measured centers, including the independently observed first-partial
   cluster. The fitted curve assigns partial numbers; it does not force observed
   peaks onto its frequencies. Report curve confidence, supported-line count,
   residual cents, and boundary hits. With fewer than three useful lines, `B`
   is unidentified and upper-mode confidence is reduced. A boundary hit or poor
   fit needs review; it is not evidence of precise physical parameters.
   Supplement weak/absent long-window lines (confidence at most 0.25) with an
   **onset-relative 0–0.15 second Hann FFT**, without changing `f0`, `B`, or their
   reported fit confidence/support. Match early local maxima to this recording's
   own predicted stiff-string frequencies. Search radius is
   `min(0.23 * nearest harmonic gap, 2.5 * max(2 / 0.15, 0.003 * predicted Hz))`.
   The local floor is the median between 1.15 search radii and 0.45 harmonic gaps
   from the predicted center; require at least four flank bins. Require peak
   prominence above 15 dB and peak power above −60 dB relative to the strongest
   early peak. Confidence fades over 15–30 dB prominence and −60 to −40 dB
   relative power, multiplied by Gaussian proximity with scale
   `min(0.23 * gap, max(2 / 0.15, 0.003 * predicted Hz))` and long-fit confidence.
   Disjoint search bands and unique peak assignment prevent one peak from
   representing adjacent modes. Early evidence requires long-fit confidence at
   least 0.25 and no constraint boundary hit. It replaces a weak long line only
   when stronger; already reliable long centers/confidence are retained.
4. Measure power in a **0–0.12 second Hann window (center 0.06 s)**, followed by
   **0.30 second Hann windows centered at 0.35, 0.65, 1.0, and 1.4 seconds**.
   Their onset-relative intervals are `[0, 0.12)`, `[0.20, 0.50)`, `[0.50, 0.80)`,
   `[0.85, 1.15)`, and `[1.25, 1.55)` seconds. Window lengths are explicit, and
   model/reference/probe use identical windows. Reports retain sample counts,
   onset-relative start samples, actual sample-center times and durations;
   the center of a sampled Hann is half a sample before the nominal center.
   At A0 the short window spans only about three cycles and can leave low modes
   unresolved; the later windows span about eight cycles. Zero padding
   interpolates bins but does not improve actual resolution.
   One-sided FFT-bin power is
   `mean_channels(|FFT(x Hann)|²) * one_sided_factor / (Nfft sum(Hann²))`.
   Summing bins therefore gives window-weighted stereo mean-square power,
   independent of padding. Anti-phase left/right audio retains its power.
5. Each fixed peak neighborhood has half-width
   `min(0.23 * nearest predicted harmonic gap, max(2.5 / window_duration, 0.008 * frequency))`
   in Hz. This includes close unison lines and their main lobes while excluding
   adjacent harmonics. Track the strongest bin inside this same neighborhood
   at each time, rather than jumping to unrelated transient peaks. Subtract the
   median local flank noise power times the band bin count. Confidence fades
   between 10–25 dB band SNR and −60 to −40 dB band/total power; it is also
   weighted by the retained long/early line confidence. Unresolved, absent and near-Nyquist
   partials have zero line confidence. No two-sided gain ratio or decay estimate
   is formed from zero-confidence bands. Width is fixed for each duration and
   partial; within that neighborhood the peak may move between windows.
6. Estimate positive loss in dB/s from the 0.35–1.4 second windows using a
   weighted median of pair slopes. Require three reliable windows spanning at
   least 0.7 seconds. Reject loss below 0.4 dB/s, rises exceeding 2 dB between
   windows, or residual excursions over 3 dB. Residual scatter, disagreement
   among slopes and noise confidence further reduce decay confidence. This
   protects against beat dips, rising components and late noise floors. It
   deliberately leaves many ambiguous decays unchanged.

Alongside line power, retain a **total-power upper bound** at every valid
independently predicted `f_n` neighborhood, even if no line was identified there.
Use the same harmonic-gap-limited width and flank floor as above, but keep the
entire band sum (including noise) and **add one further flank-floor power times
the band bin count**. This is a conservative measured energy ceiling for the
windowed band, not a recovered partial amplitude or a statistical coverage claim.
Broadband energy and leakage raise this ceiling, making a cut harder to justify.
Zero means the neighborhood could not be measured, not that its power is zero.

Using the ceiling requires reference location confidence of at least 0.25, no
pitch/stiffness constraint boundary hit, and a partial inside the range of
reliably identified **long-window** harmonics. An independently observed early
line outside that range supports only its own location, weighted by its line
and curve confidence; it does not extend ceilings through missing high modes
between or beyond the observations. The fundamental alone may be extrapolated below
that range if at least three of partials 2–8 are reliable: these low modes locate
the key and fundamental without relying on uncertain high-mode stiffness. The
ceiling's center always comes from the reference's own fitted curve, never the
model's frequencies. The measurement report retains `predicted_centers_hz`,
`power_upper_bound`, and the separate per-partial `location_confidence`; none of
these grants line or decay confidence to a missing component.

The early spectrum can recover short-lived high lines, but broad/noisy energy,
insufficiently prominent peaks and unresolved narrow gaps remain unsupported.
Neither windowed power nor a stiff-string approximation uniquely identifies a
physical pole in a coupled radiating piano. Reported losses are empirical band
losses, and all decay corrections need a new render to assess their effect.

## Shared level, gain and decay updates

One common reference dB offset is computed from onset-relative stereo RMS over
0.05–0.45 seconds of **C4 (MIDI 60), velocity 68**:

```text
reference_offset_db = 20 log10(model_anchor_RMS / reference_anchor_RMS)
target_band_db = reference_band_db + reference_offset_db
```

This anchor pair is required even for a pilot omitting C4, unless
`--reference-offset-db NUMBER` supplies an explicit fixed common offset. The
offset, raw anchor RMS values, and source hashes are recorded. There is no
per-note normalization or additional per-note gain. Relative note levels already
contribute through measured partial powers. Unmodeled broadband energy and
uncorrected modes mean total note RMS is not guaranteed to match after a fit.

For each partial, robustly average reliable log reference/model loss ratios over
all velocities. Confidence shrinks the proposed log ratio toward zero. A small
neighbor regularizer (at most 0.15) applies only to already supported,
low-confidence decay corrections; absent partials never inherit a neighbor's
correction. Bound the **iteration decay multiplier to `[0.25, 2]`**, multiply the
previous calibration's scale, then bound the **absolute scale to `[0.1, 4]`**.

Compute strike gain corrections from both early windows (0.06 and 0.35 seconds),
using power ratios in dB. Subtract the analytically predicted effect of the
*actually applied* decay change before fitting gain. With measured model loss
`L` dB/s, applied decay ratio `r`, time `t`, and finite-window bias `C` evaluated
with that observation's actual Hann length `N`:

```text
C(L, N) = 10 log10(sum(Hann_N² exp(-ln(10)/10 * L * centered_time_N)) / sum(Hann_N²))
predicted_decay_change_db = -L (r - 1) t + C(r L, N) - C(L, N)
strike_update_db = target_band_db - model_band_db - predicted_decay_change_db
```

This removes the first-order double count between strike amplitude and decay,
including finite-window averaging for an exponential. It does not assert that
the runtime band loss is a pole sigma or that scaling sigma produces this exact
change. If decay changes but that velocity has no reliable model loss estimate,
its gain observation is withheld because this compensation cannot be made.

Fit gain observations at all available velocities by weighted least squares
using exactly the runtime interpolation weights: piecewise linear **in dB** at
knots `[28, 68, 112]`, clamped outside. The diagonal zero-update prior is
`0.02 + 0.5 * max(0, 1 - weighted knot support)`. Weak evidence thus fades toward
zero residual correction. There is no cross-knot smoothing. An intermediate
velocity can algebraically constrain both neighboring knots, so positive updates
also require evidence in that knot's own required render: multiply a boost by
`min(1, knot_observation_confidence / 0.25)`. An absent soft partial cannot inherit
a boost from an intermediate or loud layer. This conservative projection may
increase the least-squares residual; its factors are recorded. Bound each **iteration gain update to
±12 dB**, add the previous gain, then bound **absolute gain to `[−36, +24] dB**.
No supported source band means no boost, even if the reference has a strong
unrelated line. The fit never invents energy to fill deep model zeros.

When reference line confidence is at most 0.2, its upper bound can instead
contribute a **cut-only observation**. Require model band confidence above 0.25
and the reference location gate above. The measured model power must exceed
`reference_upper_bound * 10^(reference_offset_db / 10)`; the correction must also
remain negative after the same actually applied decay compensation. This uses
the shared offset, with no special bass curve or per-note normalization. Use the
least restrictive eligible early-window ceiling and weight it by
`0.25 * model_band_confidence * reference_location_confidence`. Weak-reference
windows supply no two-sided observation. Upper bounds never enter decay fitting;
an absent reference fundamental therefore retains its previous decay scale
unless other velocities supply genuine decay measurements.

These negative-only observations enter the same velocity fit, separately from
identified-line observations, and supply **no boost evidence**. Every knot with
nonzero interpolation weight at a censored observation is projected to a
nonpositive update. Thus even strong positive observations at other velocities
cannot turn that observation into a boost, including after iteration/absolute
clamping. This conservative projection can suppress a supported boost at an
adjacent velocity. Reports separate the cut ceilings, weights and guarded knots;
their before/after residual is the one-sided violation
`max(0, applied_gain_update_db - cut_only_upper_db)`. A cut beyond the ceiling has
zero violation, not an incentive to boost back up. Bounds are not equality
measurements, and a residual can remain when confidence, competing observations
or the gain limits prevent a full correction.

For a subsequent pass, render using the previous calibration and pass its CSV:

```sh
python3 libs/piano_model/tools/fit_voicing.py \
  --renders local/piano-renders/pass1 \
  --corpus local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit \
  --previous local/piano-voicing/pass1/calibration.csv \
  --out local/piano-voicing/pass2
```

The previous CSV must have exactly 240 unique bounded finite rows for every fitted
key; legacy 64-row tables are rejected explicitly. By default the shared anchor
is **reused** from the sibling `metadata.json`,
whose generated CSV hash must match; it is not re-pinned to the changed render.
An explicit `--reference-offset-db` can supply the original fixed anchor when
metadata is unavailable. Ensure that this value and the rendered calibration
are correct. Missing evidence preserves the previous calibration: its new
residual correction is zero. A first raw pass starts at gain 0 and decay 1.

## Optional measured decay response

Use a matched baseline/probe pair to refine bands whose power beats or rises, so
the analytic positive-loss gate cannot estimate a correction:

```sh
python3 libs/piano_model/tools/fit_voicing.py \
  --renders local/piano-renders/pass1-baseline \
  --decay-probe local/piano-renders/pass1-probe --probe-decay-factor 0.7 \
  --corpus local/score-corpus/salamander/SalamanderGrandPianoV3_48khz24bit \
  --previous local/piano-voicing/pass1/calibration.csv \
  --out local/piano-voicing/pass2
```

Both options and `--previous` are required together. Render the baseline with
the previous CSV. Create the probe CSV with **identical keys and gains**, replacing
every decay scale by `clamp(previous_scale * factor, 0.1, 4)`, including rows
outside a pilot selection. The factor must be finite, positive and unequal to 1;
0.7 is a typical perturbation. Both manifests must use `mode: "calibration"`
and embed the CSV text. The fitter compares every embedded row with the expected
table (absolute tolerance `5.1e-7`, relative tolerance `2e-7`, allowing six-place
CSV and float32 precision). It uses the **actual embedded probe scale** in the
derivative denominator, including clamping and rounding.

Both render directories must have the same notes and velocities, with WAV
inventories matching their manifests. Schema, rate, held-note timing, block size,
dry/effects and all other renderer/voicing settings must match; only calibration
path and CSV text may differ. This mode requires held stereo 48 kHz float32
renders of at least four seconds. It reuses the previous global amplitude anchor,
never measures a new one from the baseline or probe. An explicit offset must
match the prior metadata when present; if that metadata is absent, supply the
original fixed offset explicitly. A present metadata file with a mismatched CSV
hash is rejected even with an explicit offset.

Analyze each probe independently with the same line identification, frequency,
band-power and confidence machinery. Validate the power/confidence array shapes,
per-window sample lengths and matching model/probe/reference times and durations.
For each partial, velocity and time window:

```text
D = (probe_power_db - model_power_db) / log(actual_probe_scale / previous_scale)
target_db = reference_power_db + fixed_offset_db - model_power_db
predicted_change_db = velocity_weights · gain_updates_db + D * log_decay_update
```

Jointly fit the three gain knot updates and one shared log-decay update across
the five windows and available velocities. Only positive-power tonal observations
with model, probe and reference confidence above 0.2 identify decay; empirical
positive-loss confidence is unused. Require at least three windows spanning 0.7 s
within a velocity, each with combined confidence (including robust residual
weight) above 0.05. Center derivatives **within each velocity** before testing
variation: their weighted centered RMS must reach 1 dB per unit log scale and
at least 5% of their uncentered RMS. Constant response, no response, an absolute
log perturbation below `1e-4`, or insufficient time support gives zero decay
update. Reference upper bounds and noise floors never identify decay.

Weighted least squares uses the existing gain zero prior and a log-decay zero
prior of 1, with twelve iterations of 3 dB Huber residual reweighting. Recheck
identifiability after reweighting. Bound the local decay factor to **`[0.5, 2]`**
and the absolute scale to `[0.1, 4]`, then refit gains using the measured effect
of that **applied** decay update. Gain fitting uses all five supported windows;
with an applied decay update, omit windows without a confident model/probe
derivative. Preserve the knot boost evidence guards, least restrictive eligible
early-window reference cut ceilings, cut-only knot projection, ±12 dB gain step
and `[−36, +24]` absolute gain bounds. Unsupported partials retain their previous
values. No neighboring decay regularization is used in this mode.

The four output filenames and CSV/Rust numeric formats are unchanged. Metadata
adds the probe factor, embedded metadata and hashes/sizes of its manifest and
used WAVs. Summary adds independent probe measurements, per-window derivatives
and confidence, time coverage and conditioning, requested/applied log-decay and
gain updates, and predicted residuals after both clamps and guards. Conditioning
and residuals describe this local two-render approximation, not acoustic success.
An unrepresentably large requested decay ratio is `null`; its log update remains
reported and the applied scale is bounded.
Rerender the candidate and check held-out velocities before accepting it; the
coupled response may change outside the measured perturbation.

## Outputs and review

- `calibration.csv`: sorted MIDI keys, 240 rows per key, partials 1–240; columns
  `key,partial,pp_db,mf_db,ff_db,decay_scale`.
- `calibration_data.rs`: `use super::CalibrationNote;` and
  `pub const DEFAULT_CALIBRATION: &[CalibrationNote] = &[...]`, with
  `gain_db: [[f32; 240]; 3]` in pp/mf/ff order and `decay_scale: [f32; 240]`.
  It is intended as a public child module of the module defining `CalibrationNote`;
  the constant stays public so external integration tests can import it.
  This lane does not install it into runtime. Both numeric outputs use six
  decimal places and are validated for shapes, finiteness and absolute limits.
- `metadata.json`: algorithm/settings (including separate identification intervals,
  nominal `windows_seconds` centers and `window_durations_seconds`), script SHA256,
  Python/NumPy versions,
  original shared anchor, prior CSV hash, render metadata, generated data hashes,
  attribution and SHA256/byte-size manifests for the SFZ, available README and
  every **used** native WAV/render (including a separate anchor when needed).
  Unused corpus WAVs are not hashed; no PCM or derived sample data is emitted.
- `summary.json`: per-input onsets, exact sample windows, independent pitch fits,
  long/early line confidence and early-line selection, band powers, tracked
  peaks, confidence and empirical losses; per-partial residual observations,
  unclamped/applied corrections and clamp flags; aggregate unsupported/clamp
  counts. Its status always requires closed-loop validation.

Outputs have no timestamps, random choices or embedded recordings. With identical
inputs, settings and numerical environment they are reproducible. Different
NumPy/FFT platforms may differ at floating-point precision. Keep manifests beside
the chosen data and preserve raw renders to make comparisons auditable.

Before integrating numeric data, rerender the held notes with it and compare
against the same independent reference frequencies and fixed amplitude anchor.
Review first-partial/unison-cluster energy, both early and sustained bands,
whole-note RMS, velocity continuity, decay confidence, boundary/clamp counts and
deep zeros. Include velocities between knots and notes between native samples
to check runtime interpolation. The runtime interpolates dB and log-decay in
MIDI pitch and tapers correction back to neutral over future partials 241–256;
this fitter emits all 240 entries. A lower fit residual on input data is not an
acceptance test, and a saturated first pass is not a reason for automatic unlimited passes.

## Reference attribution and verification

Reference recordings: **Salamander Grand Piano V3**, **Alexander Holm**,
**Creative Commons Attribution 3.0**:
<https://creativecommons.org/licenses/by/3.0/>. The seeded corpus README supplies
this attribution and license; its heading still says V2 and its changelog
describes V3. Preserve attribution and the source manifest with derived numeric
calibration. Production receives numeric modal gains/decay only, never PCM or
derived playback samples. No claim of author endorsement is made.

Synthetic tests require no real corpus and create/remove temporary fixtures
inside the checkout:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s libs/piano_model/tools -p test_fit_voicing.py -v
cargo check --release --offline -p makepad-piano-model
cargo test --release --offline -p makepad-piano-model
```

Tests cover WAV24/float/extensible decoding, anti-phase stereo and FFT padding
normalization, onset, independent known `f0/B`, known gain/decay recovery,
early-only A0 p64/p80/p100 attenuation, broadband attack rejection, early-peak
proximity/uniqueness, mixed-window analytic recovery, unison clusters,
missing/noisy partials, cut-only missing/weak fundamentals and
their one-sided residuals, location/shared-offset gates, censored velocity guards,
unreliable decay rejection, iteration
composition/clamps, velocity interpolation and use of extra layers, native SFZ
mapping, shared level/anchor reuse, missing-input failures, source preservation,
and deterministic 240-row CSV/Rust shape/order/finite values and legacy-table rejection.
Probe tests additionally cover non-monotone envelope recovery, robust residuals,
constant/unresponsive/unperturbed decay rejection, time support, bounded response
compensation, noise/censoring guards, full embedded CSV and renderer provenance,
fixed anchor reuse and deterministic probe outputs. The original response
fixtures retain their independent 0.15 s / 0.30 s first-window definition;
additional tests cover mixed-length recovery and mismatched window rejection.
