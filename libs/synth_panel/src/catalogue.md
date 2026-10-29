# Synthesiser widgets

Every synth panel has the same skeleton, in this order, everything folded but
the tree:

```
flow: Down width: Fill spacing: 10
ScoreTree{}       // the voices: mute, solo, level, pan, a tiny roll, a meter;
                  // under each, its instrument with the switch; a click on a
                  // voice or its instrument unfolds the tree right there:
                  // the instrument switch (heard on rest, one click) and
                  // that instrument's SynthTweaks
SynthTrack{}      // a voice's "variations", unfolded under it by the tree:
                  // where it plays in the song, its part, melody variants
                  // (procedural and AI), audition, keep, apply, merge bar by
                  // bar
SynthTweaks{node: "acid" PanelModule{title: "Acid bass" …}}   // one per
                  // instrument: its detailed controls, unfolded by the tree
                  // under the voice that plays it
```

The tree draws SynthTrack and the SynthTweaks in place under the voice;
where they are listed they take no room.

Only the `SynthTweaks` are written per score; never put level, pan, mute,
solo or the instrument switch in them (the tree has them). `SynthTrack{asks:
"4 variations|more syncopated|darker"}` sets the AI variant buttons.

The widgets name score nodes (`node: "mono"`) and read and write their
fields (`mono.cutoff`); a field the node does not write yet is added the
first time it is moved. Every drag is heard at once while the hand is down
and written once at the release.

- `SynthPicker{node: "lead"}`: the instrument switch as a widget (the tree
  has one on every instrument row; use this only outside the skeleton).
- The envelope is the panel widgets' `PanelEnvelope{attack: "pad.attack"
  decay: "pad.decay" sustain: "pad.sustain" release: "pad.release"}`.
- `SynthFilter{node: "bass"}`: the filter's response; drag sideways for the
  cutoff and up/down for the resonance (`cutoff:`, `res:` paths to rebind).
- `SynthWave{node: "lead"}`: the oscillator's cycle (osc, width, fm); a
  click steps through the waves.
- `SynthScope{node: "bass"}`: what the node sounds like now and its level
  (an instrument through the loops that play it, a loop, an effect).
- Morph (the wavetable synth node) has its own pictures, each running the
  synth's own code:
  - `SynthTable{node: "lead" osc: "a"}`: oscillator a's frame as it plays
    (table, position, both warps, spectral reshaping); drag sideways for
    `a_pos`, a click steps `a_table`.
  - `SynthLfo{node: "lead" lfo: 1}`: LFO 1's shape over a cycle (drawn
    points show as dots); a click steps `lfo1_shape`, drag up/down `lfo1_var`.
  - `SynthMods{node: "lead"}`: the mod rows (source > destination, amount
    bar); drag a row sideways for its amount.
  A Morph's knobs bind its parameters by key (`lead.a_pos`, `lead.f1_cutoff`,
  `lead.lfo1_rate`, `lead.m1`); the score's `morph_params` lists them by
  module (osc_a, filter1, env2, lfo1, macros, arp, fx_*). A good Morph tweak
  module: SynthTable per active oscillator with its pos, warp amounts and
  unison knobs; the filter with cutoff/res; env1; the LFOs that are routed
  (SynthLfo + rate); SynthMods; the macros by their names.
- `SynthKeys{node: "lead" octave: 3 octaves: 2}`: a keyboard that plays the
  instrument as it is now.

- `SynthGen{node: "lead" gen: "arp" choices: "root a2 chord m7 pattern updown rate 4"}`:
  a code generator as a control: it rewrites the loop's part from a few
  choices, heard as they change, written with its Write button (one undo
  step). Generators (`gen`) and their choices:
  - `arp`: root (a note), chord (major minor m7 maj7 sus4 …), pattern (up
    down updown downup random chord), rate (steps a beat: 1 2 3 4 6 8),
    octaves 1-4, gate 0.1-1, swing 0-0.5, repeats (ratchets) 1-4, curve
    (velocity: flat rise fall accent swell random), beats, seed.
  - `chords`: root, scale (major minor dorian … blues), degrees ("1 5 6 4"),
    size (triad seventh ninth), voicing (close open drop2 spread), inversion
    0-3, rhythm (whole half stabs pulse), bars per chord, curve.
  - `euclid` (a drum loop): hits, steps, rotate, rate, repeats, curve.
  - `transpose`: semitones, octaves (every written note of the part).
  - `scale`: root, scale (every written note moved into it).
  - `density` (a hit pattern): amount 0-1 (thinner below 0.5, ghost notes
    above), seed. `velocity`: curve, amount, seed. `humanize`: time, vel.
    `swing`: amount.
  Give a melodic loop an arp or chords generator and a drum loop euclid,
  density and velocity, beside the instrument's own controls: the panel
  should make parts, not only turn knobs.

A click on any of them tells you (the score's agent) which node the person
is looking at: the selection your next change is about.

## What sounds live

Fields an editor moves live (no reload): a Synth's numbers (cutoff, res,
attack, decay, sustain, release, drive, detune, vib, vib_depth, fm, index,
pluck, width, gain, …), a Shader instrument's params, a Loop's amp, pan,
mute and solo, an effect's settings. Choices (osc, filter), counts
(unison) and instrument switches are heard when written (at the release).

## An instrument's tweaks

Make them look and work like a hardware synth's section for THIS sound: its
pictures first, then a row of knobs, a macro for the gesture a player wants
("Brightness" = cutoff + drive; "Space" = its reverb send and the delay's
feedback), its effects, a keyboard to try it. Ranges around the sound's own
values, wide enough to be useful, never so wide that most of the travel is
noise. For a cover, prefer General MIDI programs and kits (the tree's switch
lists them).

```
SynthTweaks{node: "mono" PanelModule{title: "Bass" tint: "#ff8a3d"
    View{flow: Right width: Fill height: Fit spacing: 8
        SynthFilter{node: "mono" width: Fill}
        PanelEnvelope{attack: "mono.attack" decay: "mono.decay" sustain: "mono.sustain" release: "mono.release" width: Fill}
    }
    PanelRow{
        PanelKnob{path: "mono.cutoff" min: 200 max: 6000}
        PanelKnob{path: "mono.res" min: 0.7 max: 12}
        PanelKnob{path: "mono.drive" min: 0 max: 6}
        PanelMacro{label: "Growl" targets: "mono.cutoff 300 2400, mono.drive 0.5 5"}
    }
    SynthKeys{node: "mono" octave: 1}
    PanelMood{}
}}
```
