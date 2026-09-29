//! The audio shader guide for the AI composer (a `shader_guide` tool):
//! the language in one page, the prelude, the rules that keep a shader
//! realtime, and idioms that sound good.

pub const GUIDE: &str = r#"# Audio shaders

An audio shader is an instrument or an effect written in Splash syntax and
compiled to native code that runs on the audio thread. Everything runs per
sample at 48 kHz, so write it like a GPU shader: small, branch-light, no
allocation. `check` compiles one and auditions it (peak, RMS, DC, NaN,
clipping, CPU, how voices end) before you put it in a score.

## Shape of a shader

    let cutoff = param(1200, 20, 16000)   // a knob: default, min, max (smoothed)
    let N = 4                             // a constant (sizes, loop counts)
    let TABLE = [0.0; 2048]               // a shared table, filled by fn init()
    var phase = 0.0                       // state: persists per voice / per effect
    var f = Svf{}                         // state struct (from the prelude)
    var buf = [0.0; 4096]                 // a delay line / ring buffer
    struct Op { phase: 0.0 env: 0.0 }     // your own struct (field: default)
    fn helper(x) { x * 0.5 }              // helpers are inlined; last expression returns
    fn block() { svf_set(f, cutoff, 0.8) }// optional: every 64 samples (coefficients)
    fn init() { ... }                     // optional: once, fills top-level `let` tables
    fn voice() { ... }                    // INSTRUMENT: one voice, returns f32 or vec2
    fn effect(l, r) { ... }               // EFFECT: stereo in, returns vec2

A shader has exactly one of voice() or effect(l, r).

Instrument inputs: `freq` (Hz), `note` (MIDI, fractional), `gate` (1 held,
0 released), `velocity` (0..1), `trigger` (true on the first sample of a
note), `sample_rate`. Constants `PI`, `TAU`. A voice must fade to silence
after release (use adsr(), or scale by an envelope that falls when
gate == 0.0); call stop() when it is done.

A param named `bpm` is set from the song tempo.

## Types and rules

- Numbers are f32. Literals adapt: `i * 2` is an integer if i is one.
  Integer state: `var k = int(0)`. Loop counters are integers.
- vec2 is stereo: `vec2(l, r)`, `v.x`, `v.y`, `+ - * /` lane-wise.
- Arrays have fixed sizes: `[0.0; N]`, `[1.0, 2.0, 3.0]`. Indexing WRAPS:
  `buf[w - d]` reads d samples behind w in a ring buffer. `len(a)`.
- Structs and arrays pass to helpers by reference: a helper can update
  them (`fn step(o) { o.phase = fract(o.phase + 0.01) }`).
- Control flow: `if/elif/else` (also as a value), `for i in 0..N`,
  `while`, `loop`, `break`, `continue`, `return`, `match x { 0 => a, _ => b }`.
  Loops stop after 1024 iterations; constant-range loops up to 16
  (64 for one-line bodies) are unrolled.
- No strings, objects, recursion or dynamic sizes. `%` on floats is
  a - b * trunc(a / b).
- Math: sin cos tan tanh exp exp2 log log2 log10 pow sqrt abs floor ceil
  round trunc fract sign min max clamp mix step smoothstep midi_to_hz
  db_to_gain. `rand()` is seeded noise in -1..1. `read(arr, pos)` and
  `read_cubic(arr, pos)` interpolate between elements.
- Keep phases wrapped (fract): sin() is exact to 1e-7 near 0 and less
  so for huge arguments.

## The prelude (always available)

Oscillators (p = phase 0..1, dt = freq / sample_rate):
  advance(p, dt) · saw_blep(p, dt) · pulse_blep(p, dt, width) ·
  tri_blep(Tri{}, p, dt) · varsaw(p, bright) · blep(t, dt)
Filters (keep the struct as state, set coefficients in block()):
  Svf: svf_set(f, cutoff, q) then svf_lp / svf_bp / svf_hp / svf_notch /
  svf_ap (f, x) · Ladder: ladder_set(f, cutoff, res 0..1), ladder(f, x) ·
  OnePole: onepole_set(f, cutoff), onepole_lp / onepole_hp ·
  DcBlock: dc_block(d, x) · Allpass1: allpass1(a, x, coef)
Envelopes: adsr(Adsr{}, a, d, s, r) (from trigger/gate) ·
  decay_env(Decay{}, secs) (percussive, from trigger)
Delays: tap(buf, w, d) · tap_cubic(buf, w, d) (fractional reads)
Shaping: soft_clip · hard_clip · fold (wavefolder) · asym_clip(x, bias)
Stereo: pan(x, p) · width(v, w) (mid/side)
Oversampling: Os4 with os4_up(o, x, k) / os4_push(o, y) / os4_out(o):
    for k in 0..4 { os4_push(o, shape(os4_up(o, x, k))) }
    let y = os4_out(o)

## Idioms that sound good

- Band-limit every oscillator (saw_blep, pulse_blep, tri_blep); naive
  `2 * p - 1` aliases into whistles above ~1 kHz.
- Filter envelopes: a decaying env times an amount added to the cutoff,
  keytracked (`cutoff + env * amt * freq / 261.0`), set in block().
- Velocity to timbre, not only volume: scale FM index, cutoff, brightness.
- Detune that moves: re-draw each unison voice's offset slowly with
  rand(); fixed detune beats mechanically.
- Nonlinear things (drive, fold, hard clip) at 4x with Os4.
- Plucks: a delay line of one period with a fractional (allpass) part,
  a lowpass in the loop and a noise burst; loss per period from T60.
- Stereo: two decorrelated sources or a few ms of delay on one side
  sound wider than panning one signal.
- Leave headroom: peaks around 0.5; add dc_block() after asymmetric
  shaping.

## Cost (native, one voice at 48 kHz)

A polyBLEP saw + SVF is ~9 ns per sample, 4-op FM ~65 ns, a 4x
oversampled shaper ~250 ns. 20833 ns per sample is one whole core;
aim for well under 1000 ns per voice.
"#;
