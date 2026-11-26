//! The audio shader guide for the AI composer (a `shader_guide` tool):
//! the language in one page, the prelude, the rules that keep a shader
//! realtime, and idioms that sound good.

pub const GUIDE: &str = r#"# Audio shaders

An audio shader is an instrument, effect or control node written in Splash and
compiled to native code that runs on the audio thread. Everything runs per
sample at 48 kHz, so write it like a GPU shader: small, branch-light, no
allocation. `check` compiles one and auditions it with the song playing
at 120 bpm (peak, RMS, DC, NaN, clipping, CPU, how voices end) before you
put it in a score.

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

Params can say how they show and move: `param(1200, 20, 16000, {unit: hz
curve: log label: "Cutoff" group: filter})`; `param(0, {choices: [saw
square tri]})` is an index 0..n-1 shown by name; `step: 0.5` snaps;
`smooth: 0.05` is how many seconds a change takes to settle (default one
64-sample block; stepped params and choices jump). Units: hz s ms db st
ct beats percent (the value is a 0..1 fraction, shown as 0..100%).
Curves: linear, log (min > 0), exp (fine near min, min may be 0).

An instrument defines `voice()`. An effect defines `effect(l, r)` or
`effect_block(l, r)`. A control node defines `block()` with a named output.
Edits with identical state layouts swap buffers in constant time. Layout changes copy matching variables during takeover.

Instrument inputs: `freq` (Hz), `note` (MIDI, fractional), `gate` (1 held,
0 released), `velocity` (0..1), `trigger` (true on the first sample of a
note), `sample_rate`. Constants `PI`, `TAU`. Expression (a keyboard or a
per-note controller): `bend` (semitones; `note` and `freq` already include it),
`pressure` (0..1), `slide` (0..1), `release_velocity` (0..1, set when the
key is let go), `cc(n)` (any controller 0..127 as 0..1; `mod_wheel` is
`cc(1)`). Each changes on its exact sample; all rest at 0. A shader's own
name wins over these.

Declare `mono()` at the top level to play one voice by default. The score
can override it. Legato keeps the sounding envelope. When the node glides,
`note` and `freq` follow the slide on each sample automatically. The pedal
holds `gate` until pedal-up. On per-note channels, `bend` includes the
master channel's bend.

A voice must fade to silence after release (use adsr(), or scale by an envelope that falls when
gate == 0.0); call stop() when it is done.

Musical time: `tempo` (bpm), `beat` (the song's beat, exact on its
sample), `bar` (bars, fractional: `fract(bar)` is the place in the
bar), `beats_per_bar`, `playing`. While stopped `beat` holds. A
beat-locked LFO: `sin(TAU * beat / division)`.

Identity: `voice_index` (the voice's slot; 0 in effects) and `instance`
(0..1, different for each node, the same in every render). `rand()` is the
same in every render of a score and different in every node and note; a
node's `seed: 3` picks another stream, the score's `Clock{seed: n}`
changes them all.

## Audio ports

Declare extra ports at the top level with a literal channel count of 1
(float) or 2 (vec2). Inputs and outputs are numbered separately in
declaration order, starting after the main stereo pair.

    let key = input(2)
    let env = output(1)
    fn effect(l,r) {
        env = max(abs(key.x), abs(key.y))
        vec2(l,r) * (1.0 - clamp(env,0.0,1.0))
    }

Read an input in `voice()`, `effect()` or a helper called from either.
It supplies the current base-rate frame, including inside an explicit
`oversample` lambda. Inputs are read-only and unavailable in `init()`
and `block()`. An unconnected input reads silence.

An output holds its last value. It starts at zero and carries across a
hot swap by name and type like a `var`. Read it back or assign it in
`block()`, `voice()`, `effect()` or their helpers. Assignment in
`init()` is an error. After each frame the held value adds into the
output, even on frames without an assignment. Each input channel read
costs one sample load. Each output channel costs one additive `Out` per
frame. Hosts supply discard buffers for unconnected outputs.
A score wires ShaderFx audio ports by name, e.g. `key: kick` or `Port{node: split name: @low}`.

A shader with `block()` and named outputs alone is a control node. It has
zero input channels. Each output holds its value until the next block.
For example, `let out = output(1) fn block() { out = sin(TAU * beat) }`
provides a beat-synced modulation source. Effects can also publish held outputs.

A score can wire a named output, loop, bus or main audio output into a
Shader, ShaderFx or ShaderControl param. At each absolute multiple of 64 frames the
host sets the target to the sum of each source's mean of left and right.
The normal param ramp follows. This can add up to four run calls per
256-frame pass on the wired node. This graph cost is separate from the
program cost shown by `check`.
Native effect params take the same block-start targets, clamped in their own units.
Engine Filter coefficients follow the absolute 32-frame grid. Mixer and gain
targets follow the 64-frame grid with 256-frame ramps. Delay takes the new
tempo on the same whole frame as the shader clock, however the host slices audio.
A native Lowpass steps its cutoff. The filter ShaderFx ramps it for smooth sweeps.
Native effect automation samples its ramp on the same grid. Duck's key response,
Reverb's room changes and DjFilter's centre crossing are identical across host buffer sizes.

Wire an instrument on its definition, e.g. `Shader{code: pad_code key: kick
cutoff: Port{node: lfo name: @out}}`. All voices read the same input. The
pad can multiply its filtered sample by `1.0 - clamp(abs(key),0.0,1.0)`
to duck on a mono key. Read its output from the loop that plays it:
`Port{node: pad_loop name: @env}` sums sounding voices and scope copies.
Instrument inputs and params follow graph order without latency compensation.
A late source produces a plan note naming the edge and the delay in frames.
Each extra cut reruns each sounding voice. With three epiano voices and
three extra calls per voice in 256 frames, the measured fixed cost adds
3 × 3 × 894 / 256 = 31.43 ops/frame. An unaligned pass can add four calls.

An output written only by `block()` or its helpers has control kind.
All other ports have audio kind. Instrument audio outputs sum across
voices. An instrument cannot declare a control output. Ports are not
supported yet in a shader at a raised rate (`rate(k)`, or a score's
`oversample:` on its node) or in a fused chain; run the high-rate part
in `oversample(...)` instead.

## Latency

An instrument or effect can declare `latency(n)` at the top level. Use the
maximum look-ahead it ever needs, in whole frames. The count is constant
and may use `let` constants, table lengths such as `len(t)`, and arithmetic.
If the look-ahead varies, delay the signal internally to keep the output
delay at the declared count. The default is zero frames. A score adds its
master limiter's 128 frames of look-ahead to its shaders' latency.
A shader effect behind a look-ahead receives song time delayed to match its input audio.
`check` reports the shader's latency in frames.

With `rate(k)`, the compiler adds the converter delay to `latency(n)`.
Declare the body's own look-ahead in whole base frames. At the raised rate,
n base frames occupy k·n samples. For example, an effect with `rate(4)`
and `latency(5)` reports 43 base frames.

## Block effects

    let BLOCK = 1024
    let HOP = 256
    fn effect_block(l, r) { ... }         // edit both f32 frame arrays in place

Every HOP samples the compiler copies the last BLOCK stereo samples from its
input rings, runs the body, and overlap-adds its frames into output rings.
BLOCK frames of latency are automatic and add to any declared latency().
Both constants must be positive powers of two with HOP <= BLOCK. HOP == BLOCK
processes disjoint frames. block() and param smoothing run as usual. The body
reads params at the last input sample of each hop. In check, the peak cost
over 64 frames shows the work concentrated at a hop.

Apply windows in the body. For HOP = BLOCK/4, fill a shared table once with
`let win = [0.0; BLOCK] fn init() { fill_hann(win) }`. Multiply each channel
by `win[i]` before the transform and by `win[i] * (2.0/3.0)` after the inverse.
The four overlapping squared periodic Hann windows sum to 3/2.

Choose one stereo entry, effect() or effect_block(). Put a block effect after
a voice to avoid allocating rings per voice. Keep the block effect
at base rate. Put rate(), oversample() and side ports in a separate shader.
The rings, work arrays and user state share the per-instance state limit.

## Types and rules

- Numbers are f32. Literals adapt: `i * 2` is an integer if i is one.
  Integer state: `var k = int(0)`. Loop counters are integers.
- vec2 is stereo: `vec2(l, r)`, `v.x`, `v.y`, `+ - * /` lane-wise.
- Arrays have fixed sizes: `[0.0; N]`, `[1.0, 2.0, 3.0]`. Indexing WRAPS:
  `buf[w - d]` reads d samples behind w in a ring buffer. `len(a)`.
- `fft(re, im)` and `ifft(re, im)` transform two same-length, power-of-two f32 arrays in place.
  Their bits are frozen by radix-2 decimation in time, separately rounded operations, and a final 1/N scale for `ifft`.
  Each costs (N/2) log2(N) butterflies plus permutation (and inverse scaling). Precompute large spectra in init() using local arrays or Shared tables.
- A complex bin is `vec2(real, imaginary)`. Use `cmul`, `cconj`, `cabs`, `carg`, `cpolar` and `cexp` with phase in radians.
- For a power-of-two length n, `hann(i, n)`, `hamming(i, n)` and `blackman(i, n)` return periodic windows for i in 0..n.
  Precompute a Shared Hann table with `let win = [0.0; 1024] fn init() { fill_hann(win) }`.
- Structs and arrays pass to helpers by reference: a helper can update
  them (`fn step(o) { o.phase = fract(o.phase + 0.01) }`).
- Control flow: `if/elif/else` (also as a value), `for i in 0..N`,
  `while`, `loop`, `break`, `continue`, `return`, `match x { 0 => a, _ => b }`.
  Loops stop after 1024 iterations; constant-range loops up to 16
  (64 for one-line bodies) are unrolled.
- Outside parameter info, no strings, objects, recursion or dynamic sizes. `%` on floats is
  a - b * trunc(a / b).
- Math: sin cos tan tanh exp exp2 log log2 log10 pow sqrt abs floor ceil
  round trunc fract sign min max clamp mix step smoothstep midi_to_hz
  db_to_gain. `rand()` is seeded noise in -1..1. `read(arr, pos)` and
  `read_cubic(arr, pos)` interpolate between elements.
- Keep phases wrapped (fract): sin() is exact to 1e-7 near 0 and less
  so for huge arguments.

## Recorded sound: sample tables

`let src = sample(2)` declares a stereo table the score fills from an audio
asset (`ShaderFx{code: ... src: asset(id, rev)}`). Use `sample(1)` for mono.
`src[i]` is one frame (vec2 for stereo). Its length, `len(src)`, is a
compile-time constant, so `[0.0; len(src)]` works. `read(src, pos)` and
`read_cubic(src, pos)` interpolate between frames. Positions are frames at
the engine rate. Before the data arrive the table is one frame of silence.
`check` binds its test recording to every table, so a convolver's level
warnings describe how that recording behaves as the impulse response.

The library effect `convolver` takes a mono or stereo impulse response.
Each input channel uses its corresponding response channel. A mono response
filters both independently. By default, init() scales the wet response to
unit energy summed across both channels. For the recorded amplitude, set
`normalize` to off, `wet` to 1, `dry` to 0 and `gain` to 0 dB. A unit
impulse then plays the response from frame zero.

To use the prelude routines directly, follow `convolver.splash` for the
table and state declarations. In `init()`, `conv_prepare_head(ir, taps)`
orders the direct taps for pairs of adjacent outputs. For each active level,
`conv_prepare(ir, h, scratch, b)` transforms the partitions using two
scratch arrays of 2*b floats. Row k of h starts at response frame (k+1)*b.
Each of its b+1 bins holds both channels' real and imaginary parts.
For the 64-frame level, `conv_near_roots(roots, scratch)` fills the
twiddle table for its 128-point transform using the same scratch arrays.
`conv_energy_gain(ir)` returns the reciprocal norm in f64. The library
saves its square root in Shared and applies it twice to the wet output,
so a tiny response can use a gain larger than an f32 can represent.
In `block()`, call `conv_spread(s, history, h)` for each level larger
than 64. Each call sums the next stripe of past input spectra.
In `effect(l,r)`, add `conv_head(ir, taps, ring, ahead, frame, x)` and
one `conv_frame(s, history, h, frame, x, roots, b)` per active level,
where x is vec2(l,r). The one-element stereo array `ahead` carries the
next frame's contribution from past inputs. Advance the integer frame
counter once per input frame.

## Sharing code: modules

`use name` imports a module (`name.fn(x)`). With `use name.*` you can call
its functions by their unqualified names. Your own names win. In a score,
define one with `name := ShaderModule{code: "..."}` inside `Score{…}`.
Every shader that uses it updates when it changes.

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

Declare `rate(k)` once at the top level to run the whole shader at a
constant factor of 1, 2, 4 or 8. The default is 1. For example:

```
rate(4)
fn effect(l,r) { vec2(soft_clip(2*l), soft_clip(2*r)) }
```

An effect that reads either input upsamples both. It runs its entry k times
per base frame and downsamples the output. With the default filters, round
trips at 2x, 4x and 8x take 34, 38 and 40 base frames. An instrument
generates sound directly at the raised rate. Each voice has its own down
filter with a delay of 17, 19 or 20 base frames respectively.
A score node's `oversample` field overrides the shader's `rate` factor and conversion modes.

`rate(k, mode)` selects both directions for an effect or the down direction
for an instrument. An effect can also use `rate(k, up, down)`. Choose
`filter` (the default half-band cascade) for alias rejection and exact phase.
`linear` interpolates from the previous input to the current one on the way
up and averages the k outputs on the way down. Use it for smooth controls.
`hold` repeats the input and keeps the last output, which suits a filter run
at a higher rate for stability. For an input impulse, the reported latency
is the nearest whole frame to the true delay, where the impulse peaks.
Half-frame ties round down, and a cheap conversion leaves a residual of
at most half a frame on that path.

`block()` runs once per 64 base frames and sees the same raised
`sample_rate` and `oversampling` as the entry. Set filter coefficients
there. Top-level initializers and `init()` also see `oversampling` equal
to k. At a base rate of 48 kHz, `[0.0; 4800 * oversampling]` gives a
0.1-second delay line at every factor. Glide and param smoothing still
advance in base frames. A raised shader can contain explicit regions.
Fusion requires a base-rate shader.

Use `oversample(state, input, fn(u) { .. })` to run part of a shader at
2x, 4x or 8x. Choose `Oversample2`, `Oversample4` or `Oversample8` for
the state and write the function literal at the call. The input and result
have the same type, f32 or vec2. As with `rate`, append one mode after the
function to set both directions, or append the up mode followed by the down mode.

```
latency(HALFBAND_LATENCY_4)
var os = Oversample4{}
fn effect(l,r) {
    oversample(os, vec2(l,r), fn(u) { clamp(2.0 * u, -1.0, 1.0) })
}
```

State used in the body advances once per sub-sample. Inside the body
`sample_rate` is the high rate and `trigger` fires on the first sub-sample
of a note. Params, beat, note and freq keep their base-frame values.
Inside nested regions, `oversampling` is the product of the factors and
`sample_rate` is the base rate times that product. Both return to their
enclosing values when the inner call returns.
Use it in buffer sizes such as `[0.0; 4 * oversampling]`. Coefficients set
in `block()` outside an explicit region use the base rate. Set coefficients where the
filter runs.

Give each nested call its own state. Its factor is relative to the enclosing
region, including a whole-shader or node rate. A 2x shaper inside a 4x
drive uses the 4x-to-8x stage. A helper containing the shaper works the
same way. The largest absolute rate is 8x.

Each boundary keeps its own up and down modes. For filtered stages, the
nest pads each direction once at the highest rate. Signal that skips the
inner call is not padded: it leads the nest and can fall between base
frames. With both directions
filtered, 2x inside 2x takes 38 base frames, and 2x inside 4x or 4x inside
2x takes 40. The compiler checks the nest as one region. A whole-shader
rate already contributes its own conversion latency: a filtered 4x effect
with a nested 2x identity needs only `latency(2)` beyond its automatic 38.
For mixed modes, add their phase offsets in base frames and round once,
with half-frame ties going to the earlier frame.

An explicit region converts up only when the body reads its parameter.
A read in either arm of a branch counts. When both directions use the
filter, declare `HALFBAND_LATENCY_2`, `HALFBAND_LATENCY_4` or `HALFBAND_LATENCY_8`.
For a single filter direction, start with `HALFBAND_DOWN_LATENCY_2`,
`HALFBAND_DOWN_LATENCY_4` or `HALFBAND_DOWN_LATENCY_8`. For `filter, hold`
subtract one frame at every factor. For `linear, filter` add one frame at
4x and 8x, as in `latency(HALFBAND_DOWN_LATENCY_4 + 1)`, when the body
reads its input. All other single-filter cases use the constant directly.
Two cheap conversions or a single cheap down conversion need no latency
declaration. Sound generated inside
a region is ahead of its processed input by the up latency. Regions in series
add their latencies. The compiler only catches a declaration below the largest
single region.

At 48 kHz, running a hard clip at 4x reduces total alias power by about
14 dB for a 5 kHz sine driven to twice the clip level. Its far harmonics
fold at the high rate too. A softer curve can gain far more. An 11 kHz
sine at 0.75 peak through `x - x*x*x/3` produces a 33 kHz third harmonic.
At 4x the down filter suppresses it, reducing the 15 kHz alias by 115 dB.

Half-band conversion takes one mono frame at a time. Keep separate state
for each direction and channel. Process the raised samples in time order
between the two calls. At 2x and 4x they fit in one vector. At 8x, supply
`var pairs = [vec2(0.0); 4]` so both calls can use the same eight samples.

| State per direction | Up call | Down call | Up / down / round trip (base frames) | Counted ops up / down |
| --- | --- | --- | ---: | ---: |
| `Halfband2`, 1x to 2x | `let v = halfband_up2(u,x)` | `halfband_down2(d,v)` | 17 / 17 / 34 | 241 / 243 |
| `HalfbandStage4`, 2x to 4x stage | `let v = halfband_stage4_up(u,x)` | `halfband_stage4_down(d,v,0)` | 2.25 / 2.25 / 4.5 | 87 / 89 |
| `HalfbandStage8`, 4x to 8x stage | `let v = halfband_stage8_up(u,x)` | `halfband_stage8_down(d,v,0)` | 0.875 / 0.875 / 1.75 | 74 / 76 |
| `Halfband4`, 1x to 4x | `let v = halfband_up4(u,x)` | `halfband_down4(d,v)` | 19 / 19 / 38 | 439 / 425 |
| `Halfband8`, 1x to 8x | `halfband_up8(u,x,pairs)` | `halfband_down8(d,pairs)` | 20 / 20 / 40 | 763 / 745 |

Each count is one call above a pass-through effect in `check`. The 4x up
measurement sums adjacent samples in pairs. The 8x up measurement sums
four stereo pairs, and the 8x down measurement fills four input pairs.
Cascades include their top-rate padding.

Use `var f = [Svf{}; n]` for an order `2*n` cascade. For an odd order add
`var o = OnePole{}`. Keep a separate filter state for each signal path.
Call the setters in `block()` and the sample functions in `voice()` or
`effect(l,r)`. Costs below are approximate counted ops per call above a
pass-through effect, averaged over 256 frames. Cascade pairs of costs are
for one and three sections. Player pairs are for a mono and a stereo table
during playback at speed 0.8. The loop figures cover a 256-frame interval. Cubic reads at its splice use wrapped taps.

| Call | Result | Ops per call |
| --- | --- | ---: |
| `butter_set(f, cutoff)` | Set even-order maximally flat sections | 147 / 260 |
| `butter_lp(f, x)` | Even-order low-pass | 30 / 86 |
| `butter_hp(f, x)` | Even-order high-pass | 38 / 110 |
| `butter_set_odd(f, o, cutoff)` | Set the sections and real pole | 153 / 265 |
| `butter_lp_odd(f, o, x)` | Odd-order low-pass | 41 / 97 |
| `butter_hp_odd(f, o, x)` | Odd-order high-pass | 50 / 122 |
| `butter_real_set(o, cutoff)` | Set the single pole for order one | 77 |
| `butter_real_lp(o, x)` | First-order low-pass. Subtract from x for high-pass | 11 |
| `split4_set(s, freq)` | Set the crossover frequency of a Split4 | 107 |
| `split4(s, x)` | Low and high as a vec2, each −6 dB at the split | 94 |
| `eq_bell_set(e, freq, q, db)` | Set an Eq for a bell centred on freq | 125 |
| `eq_low_shelf_set(e, freq, q, db)` | Set an Eq for low-frequency gain | 129 |
| `eq_high_shelf_set(e, freq, q, db)` | Set an Eq for high-frequency gain | 129 |
| `eq(e, x)` | Apply the current bell or shelf | 40 |
| `svf_multi(f, x)` | One Svf step returns low, normalised band and high as a vec3 | 41 including a sum to stereo |
| `pink(p)` | Pink state shapes white noise to −3 dB/octave at 44.1–96 kHz | 77 |
| `brown(b)` | Brown state integrates white noise with a leak for −6 dB/octave | 34 |
| `lfo_beat(division, shape)` | One cycle per division beats. Shapes 0–4 are sine, triangle, saw up, square and sample-and-hold | 73 / 20 / 18 / 18 / 39 by shape |
| `player_start(p, at)` | Seek a Player to an absolute frame | 1 |
| `play(p, t, speed)` | Cubic one-shot at signed speed. Silence past either end | 47 / 75 |
| `play_loop(p, t, speed, from, to)` | Cubic loop over `[from, to)`, including interpolation across the splice | 111 / 140 |

For three flat-summing bands, split the high output of the lower Split4
with a second Split4. Run the low output of the lower split through a
separate `svf_ap` set to the upper frequency and Q 0.707106781. This gives
both paths the upper split's phase. The stereo `channel_strip` example
adds a bell, two shelves, a tempo-synced high-band gate and pink noise.
Its audition counts 2,320 ops per stereo frame.

## Idioms that sound good

- Band-limit every oscillator (saw_blep, pulse_blep, tri_blep); naive
  `2 * p - 1` aliases into whistles above ~1 kHz.
- Filter envelopes: a decaying env times an amount added to the cutoff,
  keytracked (`cutoff + env * amt * freq / 261.0`), set in block().
- Velocity to timbre, not only volume: scale FM index, cutoff, brightness.
- Detune that moves: re-draw each unison voice's offset slowly with
  rand(); fixed detune beats mechanically.
- Use `rate(4)` for nonlinear shaping such as drive, fold or hard clip.
- Plucks: a delay line of one period with a fractional (allpass) part,
  a lowpass in the loop and a noise burst; loss per period from T60.
- Stereo: two decorrelated sources or a few ms of delay on one side
  sound wider than panning one signal.
- Leave headroom: peaks around 0.5; add dc_block() after asymmetric
  shaping.

## Cost (counted ops per sample, one voice)

`check` counts the ops a voice runs per sample (the same on every
machine): a polyBLEP saw + SVF is ~350, 4-op FM ~580, a 4x oversampled
shaper ~1900. Aim for well under 2000 per voice.
`peak_ops_per_block` is the busiest 64-frame call on one voice, effect or control node, in ops per frame including its fixed call cost.
A static estimate above 4,000,000 worst-case ops per frame gives a source warning.
The shader still compiles and plays. Reduce loop sizes or precompute spectra in
`init()`. For a transform once per hop, use `effect_block()`. An FFT warning
gives the size that fits the threshold when one fits.
An audio shader’s `init()` or constant evaluation runs until it finishes. A newer edit
cancels a slow one. Synth and Stage show “Still initialising…” while it runs. On a page
without worker threads, an unfinished init or constant stops after 200,000,000 loop steps
and reports a shader problem.
At 48 kHz on the slowest backend (WebAssembly), `check` warns above 3,879 mean ops/frame (1/64 of realtime per instance) or 31,038 ops/frame in a single 64-frame block (1/16 of the 128-frame worklet callback).

## Testing a shader

Pass the shader source, test source and host bindings to `selftest`. Write the
predicate for the shader's kind in the test source. For example:

    fn check_voice(frame, out) -> bool { abs(out.x) <= 1.0 && abs(out.y) <= 1.0 }
    fn check_effect(frame, in, out) -> bool { out.x == in.x && out.y == in.y }
    fn check_control(frame, out) -> bool { out.x >= -1.0 && out.x <= 1.0 }

The host plays the same three-second audition as `check` at 48 kHz and 120 bpm.
Notes 48, 52 and 55 enter at frames 0, 12000 and 24000 and release at 48000.
A mono instrument plays them legato. Effects receive a stereo saw chord with
clicks. Missing sample bindings receive the built-in recording.

The predicate runs in the interpreter on the recorded frames, starting at
zero. `out` is the stereo mix and `in` is the effect's stereo input. The
control predicate runs on each named output, with a mono value in both lanes. The
test can use shader helpers, imports, sample tables and its own persistent
state. The report names the rendering backend and the predicate, with the
first frame that failed. Run this audition off the audio thread.

## Seeing inside: probes

`probe(name, x)` reports any value while the shader plays, without
changing the sound: `score_listen` answers each node's probes as
[min max last] over what it heard, `check` lists their ranges, and a
playing score's probes can be read live. A probe costs three ops a
sample where it runs at base rate. Inside `rate(k)` or an `oversample`
region, only the last sub-sample of each base frame contributes to the
reported last value, minimum and maximum. Name probes for what they show (`probe(cutoff_hz,
fc)`); a probe in code that can never run is not listed.
"#;
