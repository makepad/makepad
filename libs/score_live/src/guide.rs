//! The composer guide: what an AI reads before it writes a score (the
//! `score_guide` tool, the SYNTH page's composer and the video maker).

use crate::theory;
use makepad_score_player::presets;

/// The music AI's living production guide (genre rules, arrangement, mix
/// targets, mistakes, and rules learned from the user's listening),
/// served after the Score guide's vocabulary.
pub const MUSIC_AGENTS: &str = include_str!("guide/MUSIC-AGENTS.md");

pub fn guide() -> String {
    let mut out = String::from(GUIDE);
    out.push_str("\n## Vocabulary\n");
    let sounds = crate::palette::sounds::sounds();
    out.push_str("- Synth presets (`preset: @name`; `instrument_search` describes them):");
    for p in &sounds.patches {
        out.push_str(&format!(" {} ({}),", p.name, p.family));
    }
    out.pop();
    out.push('\n');
    out.push_str("- Morph presets (`Morph{preset: @name}`):");
    for (name, about, _) in makepad_audio_synth::morph::PRESETS {
        out.push_str(&format!(" {name} ({about}),"));
    }
    out.pop();
    out.push('\n');
    out.push_str("- Drum kits (`Drums{kit: @name}`):");
    for k in &sounds.kits {
        out.push_str(&format!(" {} ({}),", k.name, k.about.split(':').next().unwrap_or("")));
    }
    out.pop();
    out.push('\n');
    out.push_str(&format!("- Drums (`hit(@name)`, `note: @name`): {}\n", presets::DRUM_SLOTS.join(", ")));
    out.push_str(&format!("- Chords (`chord(root, @kind, inversion)`): {}\n", theory::CHORD_KINDS.join(", ")));
    out.push_str(&format!("- Scales (`scale(root, @kind, octaves)`): {}\n", theory::SCALE_KINDS.join(", ")));
    out.push_str("\n## Templates (`score_new {template}`)\n");
    for (name, source) in crate::templates::TEMPLATES {
        let first = source.lines().next().unwrap_or("").trim_start_matches("//").trim();
        out.push_str(&format!("- `{name}`: {first}\n"));
    }
    out.push('\n');
    out.push_str(MUSIC_AGENTS);
    out
}

const GUIDE: &str = r#"# Writing a score

A score is one Splash document. It is a graph of nodes (instruments, loops,
FX, sections) and the code of its loops. It plays live in the app that
hosts it (Makepad Synth, Stage's SYNTH page), renders to audio, and runs
inside games. Every change you
make plays at once: a changed loop switches at its next cycle, a changed
knob now. A source that does not load changes nothing; the problems come
back with their lines.

## The document

```
use mod.score.*
let clock = Clock{bpm: 124 sig: [4 4] seed: 7 swing: 0.1 params: {intensity: 0.5}}
let kit = Drums{kit: @analog_hybrid tune: {kick: -1} gain: {hat: 0.7}}
let bass = Synth{preset: @acid cutoff: 700 res: 6}
let kick = Loop{play: kit hits: "x...x...x...x..." amp: 0.9}
let line = Loop{play: bass beats: 4 amp: 0.6
    code: fn(i) {
        for s in 0..16 { play(choose([@a1 @c2 @e2]), {cutoff: rand(300, 1500) dur: 0.2}) sleep(0.25) }
    }}
let pump = Duck{in: [line] key: kick depth: 6 release: 0.2}
let verb = Reverb{in: [pump] room: 0.5 mix: 0.2}
let a = Section{bars: 8 loops: [kick]}
let b = Section{bars: 16 loops: [kick line]}
Score{label: "Name" clock kit bass kick line pump verb a b arrange: [a b]}
```

- Every node is `let name = Type{…}` and is listed in the final `Score{…}`.
  Names are lower_snake_case. List sources before the FX that read them.
- `Clock`: bpm, sig [beats unit], seed (all randomness), swing (0..0.75 of
  a sixteenth on off sixteenths), params {name: value} a game or
  `score_set` changes live.
- Instruments. `Synth`: preset, or osc (@sine @tri @saw @square @pulse
  @softsaw) / noise (@white @pink @brown), filter (@lp @hp @bp @ladder),
  cutoff (Hz), res (Q 0.5..20), attack decay sustain release (s, sustain
  level 0..1), drive (0..20), detune (cents), vib (Hz), vib_depth
  (semitones), fm (ratio) with index, pluck (damping 0..1), width (pulse),
  pwm (Hz) with pwm_depth (0..0.45: pulse-width modulation), delay (s: a
  layer that starts later, e.g. a noise click then a tone), unison (1..6)
  with spread (cents), gain, mono. Fields on a preset
  override it. `layers: [{osc: @saw cutoff: 900} {osc: @sine octave: -1}]`
  stacks up to six layers. `Drums`: kit, tune {drum: semitones}, gain
  {drum: x}, recipes {drum: "recipe text"}, samples {drum: asset(id, rev)}.
  `Sampler`: src asset(id, rev), root (note), attack, release.
  `Ironfish`: the rich analog-style synth with its own chorus, delay and
  reverb (16 voices; best for leads, pads and basses). program ("init"
  "glass" "acid" "sub" "formant" "chip" "supersaw" "pad", or 0-7), and
  each 0..1 over its range: cutoff resonance env (filter envelope)
  attack decay sustain release sub noise glide crush delay chorus reverb
  output. A note holds for its dur (a quarter beat without one).
  `Morph`: the wavetable synth (32 voices): three oscillators a b c,
  each @wavetable (a_table: @basic @pwm @harmonic @formant @fm @sync
  @organ @bell @vocal @digital @noisy @growl, or @user with a_frames:
  [[partial amplitudes] ...]; a_pos morphs the frames), @granular,
  @spectral (partials reshaped: a_shift a_stretch a_tilt a_odd a_formant)
  or @sample (a_src: asset(id, rev)); two warps each (a_warp1: @sync
  @bend @pwm @mirror @fm @fold @drive ... with a_warp1_amt), unison
  (a_unison a_detune a_blend a_stack), a sub and a noise oscillator, two
  filters (f1_type f1_cutoff f1_res ...), env1-4 (env1 the amp), lfo1-8
  (shapes incl. @drawn @lorenz @rossler and 2D paths @circle @eight with
  a y output lfo1y; lfo1_sync: @n8 ...), macros m1-m8, an arp (arp_on
  arp_pattern arp_rate) and a rack `fx: [@delay @reverb]`. Modulation is
  rows: `mods: [[@lfo1 @a_pos 0.4] [@env2 @f1_cutoff 0.5] [@lfo2 @pitch
  0.02 @mod]]` (amount in the knob's travel; an aux source scales it).
  `preset: @name` starts from a built-in; morph_params lists every
  parameter with its range and the presets. A note holds for its dur.
  `Tandem`: the hybrid synth that plays the hybrid patch library (an
  opt-in download): `Tandem{patch: "Leads/Name"}`, and any parameter set
  over the patch by storage name in raw units (`a_filter1_cutoff: 12`,
  `a_env1_release: -1`). tandem_patches finds patches; tandem_params
  explains the parameters (pitches and cutoffs are notes vs A440, times
  log2 seconds, levels amplitudes with gain v^3).
  `Shader`: an instrument written as an audio shader: `lib: "acid"` (the
  library: supersaw analog_poly acid pluck epiano modal_bell techno_kick)
  or `code: "…"` (its own, see the audio shader guide), mono (one voice,
  legato: slides), and any of its params by name (`cutoff: 600`).
  `Voice`: sung voices, played with `sing` (see Vocals): profile (@soprano
  @alto @tenor @baritone @bass, or a library singer asset(id, rev)),
  choir (singers, 1..8), parts (harmony parts a choir splits into),
  vibrato (semitones) vibrato_rate (Hz) vibrato_delay (s) portamento (s)
  scoop (semitones) breath (0..1) drift jitter (cents).
- `ShaderFx{in: [loops or fx] lib: "echo" wet: 0.3}`: an effect written as
  an audio shader (library: drive phaser flanger echo gated_verb), params by name.
- `Loop`: play (an instrument), beats (the cycle's length; without it the
  cycle is as long as the code sleeps), amp, pan, mute, solo, dur (the
  default note length in beats), sync (@bar, @now or another loop: where
  a loop added live starts), hits: "x..." with note (a drum or a note) and
  step (beats per character: x hit, X accent, o ghost, 1-9 velocity, . rest),
  code: fn(i) {…} run once per cycle (i is the cycle number), when: fn(i)
  {…} (false rests the cycle), follow: {param: @intensity from: 0.3 to: 0.7}
  (the loop fades in as the param rises), notes: [[beat note dur vel] …]
  (a note list for imported or written-out songs: beats from the cycle's
  start, the note a number, @c4 or a GM drum name, dur and vel optional;
  `[beat @program 40]` changes the General MIDI program, `[beat @pedal 1]`
  moves a piano's sustain pedal (0 up, 1 down); a note may end with its
  options after a comma, `[0 @a4 2 0.8, {bend: [[0 0] [0.5 2]] vib: [6
  0.3 0.5] arp: [0 4 7] arp_step: 0.0625 env: [[0 1] [2 0.2]] cutoff_env:
  [[0 400] [1 3000]] pan: -0.5 start: 0.25}]`
  (bend: semitones over beats; vib: rate Hz, depth semitones, delay beats;
  arp: offsets stepped every arp_step beats; env: level over beats; start:
  where a sample starts, 0..1): tracker and chip music; a list of such lists,
  `notes: [right_hand left_hand pedal]`, plays them merged; streamed as the
  clock reaches it, so thousands of notes are fine; the cycle is `beats`
  or the list rounded up to a bar). `Stinger` is a loop that plays once
  when triggered (quant: @bar, @now or beats).
- `Instrument{kind: @piano | @drums | @soundfont}`: Makepad's native
  instruments. @piano is the modelled grand (preset), @drums the modelled
  acoustic kit (hit @kick @snare @rim @clap @hat @pedalhat @ohat @tom1
  @tom2 @tom3 (high to low) @floortom @crash @ride @bell, or GM numbers),
  @soundfont a General MIDI program (program: a number or a name:
  @strings @brass @choir @horn @flute @timpani @harp @pizzicato
  @french_horn ...; bank 128 holds the GM drum kits: program 0 standard,
  8 room, 16 power, 24 electronic, 25 analog drum machine, 32 jazz, 40
  brush, 48 orchestra; bank 8 the variations; a note list's program
  changes pick others). `instrument_search` finds the program or kit
  that fits a description, with its code and measured character. Without src it plays the GM SoundFont (FluidR3
  GM, installed from the app's Resources: a score that needs it and does
  not find it says so); src: asset(id, rev) plays a library SF2. gain.
- MIDI: `Instrument{kind: @midi channel: 2 port: "IAC Bus 1"}` plays
  outboard gear or another app: its loops' notes go out as MIDI (program
  changes, the pedal as CC 64; no port: every output). `Loop{play: keys
  midi_in: true}` (or a channel) lets a MIDI keyboard play that loop's
  instrument live; `held()` is the keys held (lowest first: `sleep(arp(
  held(), @up, 0.25))` arpeggiates them) and `cc(74)` a controller's value
  0..1 in loop code; `Clock{params: {cutoff: 0} midi: {cutoff: 74}}` lets a
  controller move a param (and the FX settings that follow it) at once.
  MIDI clock in and out is the host app's (in Stage its utility panel,
  where the score's tempo is the show's while BPM follows SYNTH).
- Audio in: `let mic = Instrument{kind: @input gain: 1}` and
  `Loop{play: mic}` pass the default microphone / line in through that
  loop (its amp, mute, sends and the FX that read it:
  `Delay{in: [voice] …}`), at the graph's sample rate.
- OSC: `Clock{osc_in: 9000 osc_out: "127.0.0.1:57120"}`. Loop code sends
  `osc_send("/filter", [0.5 "on"])` on its beat (or to `"host:port"` as a
  third argument) and reads `osc("/fader1")` (the first number that
  address last brought); `/param/<name> value` (or `/<name>`) moves a
  Clock param at once. A render sends nothing.
- Automation: an FX setting may follow a Clock param,
  `Lowpass{in: [pads] cutoff: {param: @build from: 200 to: 8000 ramp: 4}}`
  (0 is from, 1 is to, reached over `ramp` beats), and a Section sets params
  as it starts, `Section{bars: 8 set: {build: 1}}`: builds, drops, filter
  sweeps. Loops read the same params (`param(@build)`, `follow:`).
- Aux sends: a Loop's `sends: {verb: 0.3 echo: 0.15}` feeds FX that much
  and keeps its own path to the master (set the return's mix to 1).
- FX read `in: [loops or FX]` and are summed into the master unless another
  FX reads them: Lowpass/Highpass/Bandpass (cutoff res mix), Delay (time in
  beats, feedback, mix, spread 0..1 ping-pong), Reverb (room 0..1, mix,
  damp Hz), Drive (amount, mix, tone), Crush (bits, down, mix), Chorus
  (rate, depth ms, mix), Comp (threshold dB, ratio, makeup dB), Duck
  (key: the loop that pushes it down, depth dB, release s), Bus (gain,
  pan), Limiter (ceiling dBFS, gain). Mixing: `Eq{in: [pad] b1: [@highpass 80]
  b2: [@peak, 900, -3, 1.4] b4: [@highshelf 9000 2]}`: up to four bands, each
  [@shape Hz dB Q] (@peak @lowshelf @highshelf @highpass @lowpass @off;
  commas when a number is negative, else `900 -3` subtracts); single
  settings are b1_freq b1_gain b1_q (and b1_type). `Width{in: [..] width:
  1.4 mid: 0 side: 0 bass_mono: 150}`: 0 is mono, 1 as is, 2 twice as
  wide; mid and side in dB; below bass_mono Hz the side goes (mono lows).
  `Comp{in: [..] key: kick threshold: -24 ratio: 4 attack: 5 release: 120
  knee: 6 makeup: 0}`: attack and release in ms, knee in dB; with `key:`
  it listens to another loop or FX (a sidechain). The master has a limiter at
  `Score{ceiling: -0.8}` (sample peak: for -1 dBTP set about -1.5).
- `Section{bars loops: [..] next: @other}` and `arrange: [a b c]` in
  Score: a loop plays only in the sections that list it; the last section
  repeats, and so does one reached by a goto that is not in the
  arrangement. `repeat: true` (or `next: @self`) plays a section again
  until a goto; `next: @name` may name any section. `bpm: 140` sets the
  tempo from the section's first beat (`ramp: 4` reaches it over four
  beats). `tempo: [[0 72] [3.5 64] [4 70]]` is a tempo map inside the
  section (beats from its start, each point held to the next): a
  performer's rubato, fermatas included (4-400 bpm). Without an
  arrangement every loop plays.
- Tempo: `Clock{bpm params: {mood: 0} tempo: {param: @mood from: 64 to: 104
  ramp: 4}}` moves the tempo with a param (0 is from, 1 is to) from the
  next beat; games also set it directly. Tempo changes are exact: notes,
  beat events and an edit's beat marks follow them.

## Loop code

- `play(note, {amp pan dur cutoff res attack decay sustain release drive
  detune})`: a note (@c4, @fs2, @eb3, a MIDI number) or a list of notes (a
  chord), at the loop's current beat. `hit(@kick, {amp pan tune})` a drum.
  `sample(note, {amp dur})` the loop's sampler. `hits("x..x", @hat, 0.25)`
  a pattern from here. `pedal(1)` / `pedal(0)` a piano's sustain pedal
  (between is half-pedal).
- Custom envelopes: `play(@a2, {env: [[0 0] [1 1] [3 0.2] [4 0]]})` shapes
  the note's level through breakpoints `[beats level]` (up to 8; the note
  lasts to the last one unless `dur` says; the patch's own envelope opens
  at once and holds), `cutoff_env: [[0 200] [2 6000]]` sweeps its filter
  in Hz, `curve: 2` bends the segments (slow then fast; 0.5 the other way).
  Synths, drums and samplers.
- `fx(@reverb, {room: 0.9 mix: 0.5}, fn() { play(60) })` plays the body's
  notes through an effect of their own, then where the loop goes; fx()
  nests (`fx(@reverb, fn() { fx(@echo, {time: 0.5}, fn() { … }) })`: echo,
  then reverb). The settings are the effect's (@lowpass @highpass @bandpass
  @delay/@echo @reverb @drive @crush @chorus @comp @bus @limiter) and apply
  from that beat. Write fx() in the loop's own code with the effect as an
  atom: each one becomes a node of the graph.
- `cue(@drop)` marks a moment; another loop's `wait(@drop)` moves on to the
  next time it comes (now, if it cues on this very beat) and returns the
  beats waited. A loop without `beats:` that waits then plays follows the
  cueing loop in lockstep. The cue is expected where the cueing loop gave it
  in its last cycle, repeating every cycle.
- A sampler's notes (`sample` or `play`) take `rate: 0.5` (half speed,
  an octave down; `-1` is backwards), `start: 0.25 finish: 0.5` (a region
  as fractions), `slice: 3 slices: 8` (one of 8 equal slices), `onset: 2`
  (from the third hit found in the sample to the next), `looped: true`
  (loops the region until `dur` ends; 4 beats if no dur), `fit: 4` (the
  region lasts 4 beats by speed: pitch follows) or `stretch: 4` (lasts 4
  beats at its own pitch). `sample(nil, {onset: i % 4})` chops a break.
- A note's pitch can move under it: `play(@a3, {bend: [[0 0] [1 -12]]})`
  (semitones over beats: a slide or a fall), `vib: [5.5 0.25 0.5]` (rate
  Hz, depth semitones, starting half a beat in), `arp: [0 3 7] arp_step:
  0.0625` (a chip arpeggio, one step a 64th). Synths and samplers; a
  sampler's own loop (from its file) holds a note as long as it lasts.
- `let h = play(@a2, {dur: 4})` returns the note's handle; later
  `glide(h, {note: @e3 amp: 0.4 cutoff: 3000}, 1)` slides that playing note
  there over 1 beat (`by: 12` is semitones from where it is; what is not
  named stays). Synths and shader instruments glide; a shader moves by
  `note` only.
- `sing([@e4 @g4 @a4 _ @g4], "a-ma-zing _ grace", {dur: 1 durs: [...] vel})`
  on a Voice: one syllable per note (see Vocals). It does not move time
  and returns its length: `sleep(sing(...))`.
- Lists: `rev rot(l, n) mirror reflect stretch(l, n) rep(l, n) first(l, n)
  last(l, n) drop(l, n) sort pick(l, n) line(from, to, n) bits(v, width)`.
  Harmony: `deg(3, @c4, @dorian)` a scale degree, `snap(note, @c4, @minor)`
  the nearest scale note. Phrases (none move time; each returns its length
  for `sleep(...)`): `phrase(notes, durs)`, `arp(chord, @updown, 0.25, 2)`,
  `strum(chord, 0.04)`. Shared: `put(@chord, c)` in one loop, `get(@chord)`
  in another (the latest put at or before that beat). `log(x)` shows in
  score_status. A Loop's `swing:` overrides the Clock's; `humanize: {time:
  0.01 vel: 0.1}` nudges time (beats) and velocity, the same each time.
- `bar()` the bar playing from the piece's start (0), `section()` the section's
  name (nil without sections), `section_bar()` the bar within it.
- `floor ceil round abs min max clamp(x, lo, hi)` for arithmetic. A note
  id is not a number: `note(@c4) + 12`, never `@c4 + 12` (a loop that does
  it reports an error).
  Chord kinds by string or id: `"7"` or `@dom7`, `@maj7`, `@m7`.
- `sleep(beats)` moves the loop's time on. Time is exact: notes land on
  the sample, nothing drifts.
- `tick(ring)` the ring's next element (a counter that persists across
  cycles), `look(ring)` the current one, `at(ring, i)` wraps.
  `choose(list)`, `shuffle(list)`, `rand(lo, hi)`, `irand(lo, hi)`,
  `one_in(n)`, `chance(p)`: seeded per (score seed, loop, cycle), so the
  same seed plays the same piece and a cycle plays the same whenever it
  is evaluated.
- `chord(@a2, @m7, inversion)`, `scale(@d3, @dorian, octaves)`, `note(@c4)`
  (a number), `euclid(hits, steps, rotate)` (a list of true/false),
  `steps("x.o.")` (velocities or nil), `param(@intensity)`, `beat()` (the
  loop's time in this cycle), `cycle()`.
- Splash: `let`, `for x in 0..8 {}`, `for x in [..] {}`, `if a {} else {}`,
  arrays `[a b c]`, `a[i]`, `%`.

## Vocals

```
use mod.score.*
let clock = Clock{bpm: 84 sig: [3 4]}
let lead = Voice{profile: @alto vibrato: 0.3 vibrato_delay: 0.25}
let choir = Voice{profile: @soprano choir: 6 parts: 3 breath: 0.2}
let melody = Loop{play: lead beats: 12
    code: fn(i) {
        sleep(sing([@d4 @g4 @b4 @g4 @b4 @a4], "a-ma-zing _ grace how", {durs: [1 2 0.5 0.5 2 1]}))
        sleep(sing([@g4 @e4 @d4], "sweet the sound", {durs: [2 1 2]}) + 1)
    }}
let pad = Loop{play: choir beats: 12
    code: fn(i) {
        sing([[@g4 @d4 @b3]], "ooh", {dur: 6 vel: 0.5})
        sleep(6)
        sing([[@e4 @c4 @g3]], "ah", {dur: 3 vel: 0.5})
        sleep(3)
        sing([[@d4 @b3 @g3]], "mm", {dur: 3 vel: 0.5})
        sleep(3)
    }}
Score{clock lead choir melody pad}
```

- `sing(notes, "lyrics", {dur durs vel})` sings one syllable per note.
  Notes are like `play`'s (@e4, a MIDI number); a list inside the list
  (`[[@e4 @c4]]`) is a chord, sung in harmony parts by a choir with
  `parts:` (two chords side by side read as an index: `[[@e4 @c4] [@d4 @b3]]`
  does not parse, use `let` names or one `sing` per chord); `_` or nil in
  the notes is a rest.
- Lyrics: words split into syllables by how they are said; write `hel-lo`
  to mark a break. `_` in the lyrics holds the previous syllable over the
  next note (a melisma: the vowel goes on, the pitch slurs). `/lʌv/` is
  IPA, `/hə.loʊ/` with the syllables split by dots. More notes than
  syllables hold the last one; extra syllables are dropped (a warning).
- The vowel lands on the beat and the consonants come just before it, as
  a singer does: write the rhythm of the vowels. Notes that touch are
  sung legato (a glide, no new attack); leave a rest to breathe.
- vel is loudness and tension: 0.4 soft and breathy, 0.9 bright and full.
- Ranges: soprano C4-C6, alto F3-F5, tenor C3-C5, baritone G2-G4, bass
  D2-D4. A library singer (a Kokoro voice analysed into a profile) sings
  in about the range its speaking voice sits in, up to an octave above.
- Writing: mostly steps and small leaps (a fifth at most), a step back
  after a leap. One syllable per note in fast passages, melismas on long
  stressed vowels. Stressed syllables on strong beats. Open vowels ("ah",
  "oh", "day", "night") on long high notes, not "ee" or "oo" at the top
  of a soprano. Keep pads and keys out of the vocal's octave and duck
  them a little under it.
- Choirs: `choir: 4-8` with `parts: 1` is a unison ensemble; long "ooh",
  "ah" and "mm" chords with `parts: 3` are a vocal pad. vibrato 0.2-0.4
  (semitones) and vibrato_delay 0.2-0.5 s keep short notes straight.

## Piano

```
use mod.score.*
let clock = Clock{bpm: 72 sig: [3 4]}
let piano = Instrument{kind: @piano}
/// The melody, on top and a level louder than the accompaniment.
let right_hand = [
    // bar 1 mp
    [0 @e5 1.95 0.5] [2 @d5 1 0.44]
    // bar 2 p
    [3 @c5 2.95 0.42]
]
/// Bass on the beat, the chord after it, softer.
let left_hand = [
    [0 @a2 2.9 0.4] [1 @e3 0.95 0.3] [1.02 @c4 0.93 0.28] [2 @e3 0.95 0.27] [2.02 @c4 0.93 0.26]
    [3 @f2 2.9 0.38] [4 @c3 0.95 0.28] [4.02 @a3 0.93 0.26] [5 @c3 0.95 0.26] [5.02 @a3 0.93 0.24]
]
/// Legato pedal: up as the harmony changes, down again just after it.
let pedal = [[0.1 @pedal 1] [2.95 @pedal 0] [3.1 @pedal 1] [5.9 @pedal 0]]
let phrase = Loop{play: piano amp: 1 notes: [right_hand left_hand pedal]}
let a = Section{bars: 2 loops: [phrase] tempo: [[0 72] [3 70] [4.5 64] [5 56]]}
Score{label: "Piano phrase" arrange: [a] clock piano phrase a}
```

- One `Instrument{kind: @piano}` (the modelled grand) and one loop for the
  whole piano: a loop is one instrument with one pedal, so two hands in two
  loops are two pianos. `notes: [right_hand left_hand pedal]` merges named
  lists (let them above: one per hand or voice) into one loop, and `amp: 1`
  keeps each vel as written. In code, `play` the hands in one loop and
  `pedal(1)` / `pedal(0)` (0.3-0.6 is half-pedal) move the damper pedal at
  the loop's time. The keyboard is A0 (21) to C8 (108).
- `examples/classical` holds eight real performances written this way
  (Bach, Beethoven, Chopin, Debussy, Liszt, Schumann; Bernd Krueger,
  piano-midi.de, licensed CC BY-SA 3.0 DE: anything made from them keeps
  that credit and licence). Read one before writing piano music: how the
  bars are voiced, pedalled and timed there is what sounds like a pianist.
- Voicing. The melody sits on top a dynamic level (0.08-0.15 vel) above
  the accompaniment; the bass a little above the inner notes. Below C3 use
  octaves, fifths and tenths, never close thirds (mud). Spread the left
  hand (bass, then the chord an octave or more up), keep the right hand's
  chord notes under the melody. Stagger a chord's notes by 0.01-0.03 beat
  (the melody note first or last) and a rolled chord by 0.05-0.1 per note.
- Pedalling. Legato pedal: lift where the harmony changes and press again
  just after the new bass note (0.05-0.2 beat later), so the new chord is
  caught and the old one cleared. Never hold one pedal across a change of
  harmony, except for a deliberate wash (impressionist, or a low pedal
  point). Lift before rests and at the ends of phrases; no pedal for dry
  staccato. The notes' own durs still matter: they say which keys are held
  when the pedal comes up.
- Dynamics. vel is the key's speed and the model's tone follows it (brighter
  as it rises): pp 0.2-0.3, p 0.3-0.44, mp 0.44-0.57, mf 0.57-0.69,
  f 0.69-0.82, ff above. Shape every phrase: rise toward its high point,
  fall after it, the last note softer. Repeated notes and chords are never
  identical (vary by 0.02-0.05). A crescendo is a vel ramp over bars.
- Rubato is time, not notes: write the notes on the beat grid and bend
  the clock with a Section's `tempo: [[beat bpm] ...]` (beats from the
  section's start; each point holds until the next). Slow into cadences
  (10-25% over the last beat or two), move on a little through the middle
  of a phrase, take time before a climax, and write a fermata as one point
  at a very low bpm (8-20) for that beat, then the tempo again. A Section's
  `bpm` + `ramp` is a written accelerando or ritardando. Tempo marks
  (quarter notes): Adagio 50-70, Andante 72-90, Moderato 90-110, Allegro
  120-150.
- Durations. Legato: each note lasts to the next (a hair of overlap,
  0.02-0.05 beat, is how pianists join notes). Staccato: a third to a half
  of the note's value. A final chord is held long, with the pedal, and let
  go (pedal up) to end.

## Composing so it sounds finished

Form. Pick a genre template first (bpm, bars per section, order). Sections
are multiples of 4 bars, usually 8 or 16. Change something every 4 bars
and something substantial every 8 (a layer in or out, a new drum pattern,
another register). Never start with everything: bring in one or two
layers per 4-8 bars (drums or pad, then bass, then chords, the hook in the
second section). Take layers out before a drop and bring them all back on
its downbeat. Mark the last bar of a section (a fill, a kick drop, a
filter opening). The peak comes late; the outro thins out. End on the
tonic, never mid-bar.

Harmony and melody. Mood picks the mode: bright major/lydian, sad or epic
aeolian, tense harmonic minor/phrygian, dreamy maj7/add9/sus. Chords change
every bar or two (house, pop), every four (ambient). Keep common tones
between chords; pads open voicings with the root low; nothing stacked in
thirds below C3. Bass plays roots on strong beats, E1-E3. A melody is a
1-2 bar motif repeated and developed, chord tones on strong beats, rests
to breathe, the highest note two thirds in. Keep a hook identical across
repeats; vary everything around it.

Rhythm. Swing 0 for techno, 0.1-0.2 for house, lo-fi and hip-hop. Humanise
what is not a machine: velocities with rand(), an accent on beat 1, ghost
snares at 0.2-0.35; keep the kick straight. Density ladder: kick and snare,
then 8th hats, then 16ths with open hats on the offbeat, then euclidean
percussion (3/8, 5/16), then fills.

Mix. Each role owns a band: kick and bass alone below 120 Hz (Highpass the
pads, keys and leads at 150-250 Hz), hats and percussion high and panned,
pads wide, lead centred. Levels relative to the kick: bass -3..-6 dB, snare
-2..-4, hats -12..-18, pads -12..-18, chords -8..-12, lead -4..-8. Then
always mix and master it (next section): a score without it sounds barren.

Genre defaults (bpm, swing, drums; the full production rules per
subgenre, and the rules learned from listening, are in "Music agents"
below, libs/score_live/src/guide/MUSIC-AGENTS.md). Techno 124-132, 0, four-on-the-floor,
offbeat open hat, 16th hats. House 120-126, 0.15, four-on-the-floor, clap
on 2 and 4, organ or piano stabs. Trance 136-140, supersaw after a long
breakdown. Drum and bass 170-176, two-step kick and snare, reese bass.
Lo-fi 70-90, 0.2, lazy kick, EP chords with 9ths, lofi kit. Synthwave
84-110, gated snare, octave eighths bass, arps. Ambient 60-80, no drums,
pads with long attacks, a bell motif, a big hall. Cinematic 70-100, low
ostinato, toms, swells. Chiptune 120-160, chip kit, pulse lead, arpeggios.

Fitting a video (`score_render` then an AudioClip in the edit). Resolve the
length to whole bars; nudge the bpm by up to 3 so cut points fall on beats;
put accents or fills on the cuts; keep the music 6-10 dB lower under
voice-over (a Bus with a lower gain in that section); the peak where the
edit peaks; end resolved on the last frame or fade over the last bars.

Games. Write a loop of 8-16 bars and 2-3 intensity layers (`follow:` on a
Clock param), stingers for events, sections for states (explore, danger,
combat) with `next:` for how they chain. The game sets params, triggers
stingers and changes sections; everything lands on the beat or the bar.

## Mixing and mastering

Every score gets a mix and a master; write them before you render. The dry
sum of the loops sounds thin and barren however good the notes are.

- Returns, fed by sends (`sends: {room: 0.2 plate: 0.3}` on a Loop; each
  return has `mix: 1` and no `in:`): a short room for the drums (room
  0.3-0.45), a plate or hall for vocals, leads, pads and effects (0.55-0.9),
  and a tempo-synced delay throw (time 0.75 or 1 beat, feedback 0.35-0.45,
  spread 1). High-pass every return at 200-300 Hz (one `Highpass` reading
  them all). Sends: drums 0.1-0.3 to the room, vocals and leads 0.2-0.3 to the
  plate and 0.1-0.2 to the throw, pads and choirs 0.3-0.45 to the hall,
  risers, reverse swells and crashes 0.4-0.5.
- Buses. Drums: a glue Comp over the kit (threshold -16..-18, ratio 3,
  attack 10-30, release 100-200, makeup 2), a little Drive for tape (amount
  0.1-0.3, mix 0.3-0.4), and parallel compression: a Comp return fed by
  sends from the kick, snare and claps (threshold -34, ratio 10-12,
  makeup 10). Vocals: Highpass 100-140, then Comp (threshold -20..-22, ratio
  3-3.5, makeup 3-4). Harmony and choir: their own compressed, wider bus.
  Bass: a Comp, mono, ducked under the kick. Orchestra: strings, brass and
  percussion each on a gently compressed bus (ratio 2-2.5).
- Carving. Highpass everything but the kick and the bass (pads and keys
  150-250 Hz, leads 180-200, vocals 100-140); scoop the pads' mids and soften
  their top (`Eq{in: [pads] b1: [@peak, 500, -3, 1] b2: [@highshelf, 9000,
  -2]}`); low-pass the lo-fi and dark buses.
- Movement. Duck pads, keys and bass under the kick where the style pumps
  (Duck depth 3-6, release 0.15-0.3). Width on pads and leads (Chorus mix
  0.2-0.5, or `Width`), the bass mono. Automate: a Lowpass on the music bus
  whose cutoff follows a param (`cutoff: {param: @sweep from: 450 to: 18000
  ramp: 16}`) that the sections set, closing into a breakdown and opening
  into the drop; a Bus on the hall return whose gain follows `swell`, raised
  on transitions; delay throws on phrase ends (`fx(@delay, {time: 0.75
  feedback: 0.5 mix: 0.45}, fn() { … })` around the last note).
- Master. Everything nothing else reads goes into a glue Comp (threshold
  -12..-16, ratio 1.6-2, attack 20, release 150, knee 6), then `Eq` (a
  high-pass at 28 Hz, +1 dB low shelf, -1.5 dB at 350 Hz, +1.5 dB air), then
  `Width{width: 1.1 bass_mono: 140}`, a fast peak Comp (threshold -10, ratio
  5, attack 1, release 80), a `Limiter` whose gain sets the loudness, and a
  Lowpass at 16 kHz after it (the limiter's clipped edges overshoot between
  samples). Loudness: about -10 LUFS integrated for club and pop, -10.5 to
  -11 for songs, scores and games, never above -1 dBTP; lower the limiter's
  ceiling if the true peak is over.

```
let room = Reverb{room: 0.3 mix: 1 damp: 7000}
let plate = Reverb{room: 0.6 mix: 1 damp: 6500}
let throw = Delay{time: 0.75 feedback: 0.4 mix: 1 spread: 1}
let returns = Highpass{in: [room plate throw] cutoff: 280}
let drum_bus = Comp{in: [kick hats] threshold: -16 ratio: 3 attack: 15 release: 150 makeup: 2}
let smash = Comp{threshold: -34 ratio: 12 makeup: 10}
let pump = Duck{in: [bass] key: kick depth: 6 release: 0.2}
let pad_hp = Highpass{in: [pads] cutoff: 240}
let pad_eq = Eq{in: [pad_hp] b1: [@peak, 500, -3, 1] b2: [@highshelf, 9000, -2]}
let sweep = Lowpass{in: [pad_eq] cutoff: {param: @sweep from: 450 to: 18000 ramp: 16}}
let glue = Comp{in: [returns drum_bus smash pump sweep] threshold: -12 ratio: 1.8 attack: 20 release: 150 knee: 6 makeup: 1}
let master_eq = Eq{in: [glue] b1: [@highpass 28] b2: [@lowshelf 90 1] b3: [@peak, 350, -1.5, 0.9] b4: [@highshelf 10000 1.5]}
let master_width = Width{in: [master_eq] width: 1.1 bass_mono: 140}
let master_peak = Comp{in: [master_width] threshold: -10 ratio: 5 attack: 1 release: 80 knee: 4 makeup: 0}
let master_limit = Limiter{in: [master_peak] ceiling: -1.5 gain: 1.8}
let master_lp = Lowpass{in: [master_limit] cutoff: 16000 res: 0.5}
```
(with `sends: {room: 0.12 smash: 0.3}` on the kick, `sends: {plate: 0.35}`
on the pads, and sections that `set: {sweep: 0}` and `set: {sweep: 1}`).
`score_listen`'s master RMS reads about 2 dB below the integrated loudness:
aim it near -12 dBFS for -10 LUFS, peaks at the limiter's ceiling.

## Checking your work

You cannot hear. After every change call `score_listen` (4-8 bars from
where you changed) and read it: per-loop levels against the table above,
the band balance verdict, the onset grid (is the groove what you meant?),
the notes each loop played, the warnings (a silent loop, clipping). Call
`score_check` when a loop may raise an error. Fix, listen again. Before
`score_render`: a section change at least every 16 bars and a variation
every 4, transitions on the boundaries, a resolved ending, a mix and a
master chain, nothing clips.
"#;

#[cfg(test)]
mod tests {
    #[test]
    fn the_guide_names_the_vocabulary_and_every_template() {
        let guide = super::guide();
        assert!(guide.contains("analog_hybrid") && guide.contains("minor_pentatonic") && guide.contains("`vj_sync`") && guide.contains("## Mixing and mastering") && guide.contains("## Learned from listening"));
        // Every example document in the guide is itself a score that
        // loads, and plays.
        let mut examples = 0;
        for block in guide.split("```\n").skip(1).step_by(2) {
            let block = &block[..block.find("```").unwrap_or(block.len())];
            if block.starts_with("use mod.score") {
                crate::model::ScoreDoc::load(block).unwrap_or_else(|d| panic!("{d:?}\n{block}"));
                let opts = crate::render::RenderOpts { seconds: Some(3.0), ..Default::default() };
                let out = crate::render::render(block, &opts, &crate::NoSamples).unwrap_or_else(|d| panic!("{d:?}"));
                assert!(out.left.iter().any(|v| v.abs() > 1e-3), "an example is silent:\n{block}");
                examples += 1;
            }
        }
        assert!(examples >= 2, "{examples} examples");
    }
}
