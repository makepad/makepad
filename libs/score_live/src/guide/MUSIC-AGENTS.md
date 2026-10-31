# Music agents: the living production guide

The music AI reads this when it composes a Score; the evaluator scores
songs against it. It extends the Score guide (syntax, mix chain) with
production rules. Where they differ, the order of authority is: Learned
from listening, then the subgenre rules, then the general rules.

## Learned from listening

Dated one-line rules from the user's verdicts on generated songs: DON'T
rules from bad verdicts, DO rules from good ones. They override everything
below when they conflict.

- (none yet)

How to update this file:
1. After each listening evaluation, distil each verdict into one generalisable line: `YYYY-MM-DD don't:` (bad) or `do:` (good), the rule with a number where possible, then a short why.
   - `2026-10-02 don't: detune a supersaw lead past 20 c; over 4 bars it sounded cheap and out of tune.`
   - `2026-10-02 do: cut to 1 bar of silence before the last drop; the drop felt twice as big.`
2. Never paste the user's raw words ("this is horrible"); write the cause behind the reaction, as a rule that applies to the next song.
3. Merge a new rule with an older one on the same point; delete rules a later verdict contradicts.
4. If a learned rule replaces a generic rule below, edit the generic rule too and drop the learned line.
5. Keep this section under 30 lines and the whole file under 5k words.

## Units and conventions

- Bars are 4/4 unless said. A phrase is 8 bars; sections are 8, 16 or 32 bars. Drops land on bar 1 of a 16- or 32-bar block, counted from the song's first bar (bar 17, 33, 49, 65...).
- Levels are dB relative to the kick's peak (kick = 0) unless marked dBFS or LUFS.
- "Energy" is 1-10: count of active layer roles plus density (16th hats, bass on, lead on, full width).
- Pitches are scientific: C4 = middle C = MIDI 60; E1 = 41 Hz, A1 = 55 Hz, F1 = 44 Hz.

## Subgenres

| Genre | BPM | Key feel | Drum grid | Bass | Signature |
|---|---|---|---|---|---|
| House (deep/tech) | 120-126 (124) | minor 7ths, dorian | 4/4 kick, clap 2+4, offbeat open hat, swing 0.1-0.2 | short offbeat or syncopated, sub or fm_bass, E1-E2 | stabs, organ, piano, chord loop of 1-2 bars |
| Techno | 125-135 (130) | one-chord minor/phrygian, drones | 4/4 kick, swing 0, 16th hats, ride on offbeats | rumble (kick tail reverb, low-passed) or rolling 16ths | hypnotic 1-bar loop mutated by filters; 64-bar arcs |
| Progressive / big room | 126-130 (128) | minor, i-VI-III-VII | 4/4; big room drop = kick only + lead stab | rolling offbeat (prog), almost none in big room drop | long build, festival lead, drop on silence |
| Future bass | 140-160 half-time (150) | major 7, add9, 6-2-4-5 | half-time: kick 1, snare 3 (beat 3 of a 4/4 bar at full tempo), trap hats in 16th/32nd rolls | boom bass or chord-bass, supersaw chords are the drop | pumping wide maj7/9 supersaw chords, vocal-chop lead, pitch-bent chords |
| Drum & bass | 170-176 (174) | minor, aeolian/phrygian | two-step: kick on 1 and the & of 3 (16ths 1 and 11 counting from 1), snare 2+4, shuffled 16th hats | reese or sub + mid-bass, D1-A1 | rolling breaks, 32-bar drops, fast amen-style fills |
| Dubstep / riddim | 140 (half-time 70) | minor, phrygian b2, tritone | kick 1 (+ 2.5 optional), snare 3 only | sub sine + growls/wobbles (LFO 1/4-1/16) | call-and-response bass phrases, heavy silence gaps |
| Trance (uplifting/prog) | 132-140 (138) | aeolian, i-VI-III-VII, vi-IV-I-V | 4/4, offbeat open hat, 16th closed hats | rolling offbeat: 8ths on the &, or 16ths "x.xx" off the kick | supersaw lead, gated pads, 32-64 bar breakdown with a held chord |

### House
- Kick 0, clap/snare -2..-4 on 2 and 4, closed hat 16ths -14..-18 with velocity 0.5/0.8 alternation, open hat on every & at -10..-12, shaker 16ths -18.
- Bass: notes start on the 16th after the kick or on the &; 1/16-1/8 note length; never on the kick's downbeat. Root plus fifth/octave moves.
- Chords: m7/m9 stabs (stab, epiano, organ) on the 2nd or 4th 16th of a beat; 1-3 hits per bar; same rhythm every bar, notes change every 1-2 bars.
- Arrangement (128 bars): intro 16 (drums only) / groove 16 (+bass) / main 32 (+chords, hook) / breakdown 16 (no kick, chords + vocal) / main 32 / outro 16 (drums only, DJ-friendly).

### Techno
- Kick: long, tuned F#1-A1, tail 250-400 ms; rumble = kick through Reverb (room 0.6, mix 1) then Lowpass 200 Hz, ducked by the kick 8-12 dB.
- No chord changes needed; one 1-bar or 2-bar motif in one key, transformed by cutoff, resonance, delay feedback, pattern rotation (`rot`, `euclid`) every 8-16 bars.
- Percussion: euclid(3,8), euclid(5,16), euclid(7,16) on rim, conga, zap, panned +-0.3..0.6.
- Arrangement: blocks of 32 bars; a "breakdown" is 8-16 bars of no kick with the rumble or pad; total 6-8 min equivalent.
- Swing 0; humanize velocity only, never time on the kick.

### Progressive house / big room
- Build 16 bars, then a 1-beat to 1-bar gap (silence or only a vocal/impact), then the drop on bar 1.
- Big room drop: kick 0 dB, every beat; lead is a short, loud, octave-stacked stab (saw_lead + square_lead, unison 5-7) on a 1-2 bar rhythm; bass mostly absent, the kick carries the low end.
- Progressive: rolling offbeat bass, plucks with long delay (3/16 or 1/4 dotted, feedback 0.35), chord progression changes every 2 bars, filter opens over 32 bars.

### Future bass
- Drop chords: supersaw (unison 6, spread 20-35 cents), voiced over 2-3 octaves: root in octave 2-3 (bass), 7th/9th/11th up top (C5-C6). Lowpass at 10-14 kHz.
- Pump: Duck depth 10-18 dB, release 0.2-0.35 s (a quarter beat at 150 is 0.4 s), so chords breathe audibly on every kick.
- Chord rhythm: 8th/16th stutters and syncopation (3+3+2 sixteenths), pitch glides into chords (`glide` by -12..-24 semitones over 0.25-0.5 beat, or rise into them).
- Progressions: IV-V-iii-vi, vi-IV-I-V, ii7-V7-Imaj7-vi7 in major; F, C, G, D major are common.
- Lead: vocal chop or saw/square lead with 1/16 rhythms, octave jumps.

### Drum & bass
- Two-step drums: kick on 1 and the & of 3 (hits "x.........x.....": 16ths 1 and 11), snare on 2 and 4 (16ths 5 and 13), ghost snares 0.2-0.35 vel on 16ths 8 and 15. Hats 16ths with swing 0.05-0.1, or a chopped break layered under at -6 dB.
- Bass: sub sine below 100 Hz kept separate from the mid-bass (reese, fm_bass) high-passed at 120-150 Hz; Duck the mid more than the sub.
- Snare is the loudest element after the kick: -1..-3, centered, body 180-220 Hz, crack 4-6 kHz.
- Arrangement: intro 32 (DJ, drums only) / build 16 / drop 1 32 / break 16-32 / build 8-16 / drop 2 32 (variation: new bass phrase) / outro 32.

### Dubstep / riddim
- Half-time: kick 1 (and optionally 2.5, 3.75), snare 3 at full force, hats 8ths or 16ths quiet (-18).
- Bass: sub sine 0 to -3 dB, always mono, playing the root; mid-bass (fm_bass, saw with ladder filter, Drive 6-12) carries rhythm with wobble rate 1/4, 1/8, 1/8T, 1/16 changing every 1-2 bars.
- Drop phrasing: 4-bar call-and-response (2 bars bass phrase A, 2 bars answer B), with 1/8-1/4 beat silences before the snare for impact. Riddim: one repetitive 1-bar bass motif with small variations every 2 bars.
- Keys: E, F, F# minor (sub roots E1-F#1 hit hard); phrygian b2 and tritone for menace.

### Trance
- Rolling bass: 16ths "x.xx" or offbeat 8ths, root notes, Duck 4-8 dB under the kick, sub-heavy, A1-E2.
- Lead: supersaw (unison 6, spread 25-40 cents) layered with a pluck an octave up; Delay 3/16 feedback 0.3, Reverb hall mix 0.25.
- Gated pad: pad hit in 16ths via `hits` or amp chop (on 70%, off 30%).
- Breakdown 32-64 bars: kick out, pad + piano/pluck states the full melody, then a 16-bar build with snare roll, riser and white-noise sweep; drop = melody on lead with full drums.
- Progressions: i-VI-III-VII, i-iv-VI-V, vi-IV-I-V; often a key with 4-6 flats or A minor; occasional modulation up a whole step for the last drop.

## Genre and era palettes

For a batch the director picks a different row per song. Tempo, signature sounds (engine names), harmony habit, arrangement trope.

**80s**
| Sub-genre | BPM | Sounds | Harmony | Trope |
|---|---|---|---|---|
| Synthpop | 110-130 | `@square_lead` hook, `@stab` chords, `@fm_bass` eighths, analog_80s/`studio` kit | major, I-V-vi-IV, bright 9ths | catchy 2-bar hook, chorus lift |
| Italo disco | 115-125 | octave-bouncing `@fm_bass` 16ths, `@arp`, `@glass_pad`, `@bell` lead | minor, i-VI-VII, vocoder-like `formant` | four-on-the-floor, long arpeggio intro |
| New wave | 130-160 | `@pluck` or GM clean guitar, `@organ`, `@picked_bass` | modal, I-bVII-IV | straight 8th drive, short verses |
| Hi-NRG | 125-135 | offbeat octave `@fm_bass`, `@stab` brass hits, analog_hybrid claps | minor, i-iv-v | relentless, key change for the last chorus |
| Power ballad | 64-80 | `@epiano` or GM piano, `@strings` swell, `@saw_lead` solo, `studio` kit with big gated snare | major, I-vi-IV-V, suspended 4ths | half-time verses, huge last chorus up a step |
| Electro-funk | 100-115 | analog_80s syncopated kick and cowbell, `@clav`, `@boom_bass` slides | minor 7ths, one-chord vamps | breaks with drop-outs, call and response |
| Darkwave | 100-130 | `@reese` or `@saw_lead` cold, `@glass_pad`, analog_80s dry, `@chip` blips | minor/phrygian, i-bII | sparse, gloomy, unresolved endings |
| Minimal wave | 90-120 | one `@square_lead`, one `@acid`-style bass, lo-fi `chip`/analog_80s kit | 2 chords or a drone | repetition, few layers, cheap-charm |
| Sophisti-pop | 90-110 | `@epiano`, GM fretless bass, GM sax or flute, soft `studio` kit | maj7/min9, ii-V moves | smooth, jazzy turnarounds |
| Freestyle | 110-120 | analog_80s syncopated, `@stab` orchestra hits, `@square_lead`, `@fm_bass` | minor, i-VI-III-VII | Latin-syncopated hats, breakdown with percussion |

**Other eras, one line each:** 70s disco 110-125 (`@strings` stabs, octave bass, offbeat open hat, I-vi-ii-V); 70s funk 95-110 (`@clav`, `@picked_bass` slap, swung 16ths, one-chord vamp); 90s house 120-128 (`@organ` stab bass, analog_hybrid, piano chords); 90s eurodance 130-140 (`@supersaw` hook, offbeat `@sub`, minor i-VI-III-VII); 90s trip-hop 80-95 (`lofi` kit swung, `@reese` sub, dark minor 9ths); 90s jungle 160-175 (chopped `studio` breaks, `@sub`, `@pad`); 2010s future bass 140-160 (`@supersaw` chords, pitched `formant` chops); lo-fi hip hop 70-90 (`lofi` kit swung, `@epiano` 9ths, vinyl noise).

**Real songs.** `midi_lookup` finds songs (decade, genre, tag, artist; chip and game music by machine and format too: "c64 sid hubbard", "amiga mod", "nes"); `midi_transcribe` reads one as text (chords, sixteenth grid, drum rows; a chip tune's notes carry `:arp=`, `:vib`, `:bend=` marks); `score_text` reads yours alike. For era briefs read 8-16 bars of two; for chiptune, take the arpeggios, pulse widths and fast vibrato from a SID or NES reference (note options `arp`, `vib`, `bend`, Synth `pwm`). Never copy melodies.
**Covers.** Do: brief each part's sound from the record's era, then pick the fitting GM program (the file's) or GM kit (standard, room, power, electronic, analog, jazz, brush, orchestra); check stems against it. Don't: default synth roles to presets, or name brands.

## Choosing sounds

- Ask `instrument_search` before picking a sound: describe the part ("warm 70s fingered bass", "bright 80s gated rock kit", "breathy pan flute"), optionally `kind: drums|melodic` and `role`. It answers with the Score code (`Instrument{kind: @soundfont ...}`), tags and measured words (bright/dark, thin/full, short/long, punchy; kits by kick, snare and hats) and a `gain` that sets the part at its family's usual level.
- Acoustic and band parts (piano, guitars, real bass, strings, brass, sax, organ, any cover of a recorded song) belong on the GM SoundFont: it beats the recipe presets there. Kits: standard (pop/rock), room (live, indie), power (80s arena, gated), electronic (early-80s pads), analog drum machine (electro, hip hop, boom-bap, 80s pop), jazz, brush (ballads, bossa), orchestra (film). Bank 8 holds variations: detuned EPs, 12-string, funk guitar, synth bass 3/4, orchestral pad.
- Synth presets, Ironfish and shader instruments win for electronic parts: acid lines, supersaw leads, sub basses, modern pads and electronic drums for house, techno, trance, DnB.
- One kit per song. Use a second kit only for an extra percussion layer, panned.
- Keep velocity alive. A GM program follows the format's velocity curve, so velocity 0.5 is 12 dB below 1.0. Write accents and ghosts; never put every note at one velocity.

## Arrangement

- Energy curve (1-10) of a standard 128-160 bar EDM song: intro 3 -> build 5->8 -> drop 9-10 -> breakdown 2-4 -> build 6->9 -> drop 2 at 10 (with a variation) -> outro 3.
- Templates (bars):

| Template | Sections |
|---|---|
| Festival (big room, dubstep, future bass) | intro 16 / build 16 / drop 32 / break 16 / build 16 / drop 32 / outro 8-16 |
| Club (house, techno) | intro 32 / A 32 / B 32 / break 16-32 / A' 32 / outro 32 |
| Trance | intro 32 / A 32 / break 32-64 / build 16 / drop 32 / outro 32 |
| DnB | intro 32 / build 16 / drop 32 / break 32 / build 16 / drop 32 / outro 32 |
| Radio/short (2-3 min) | intro 8 / verse 16 / build 8 / drop 16 / break 8 / build 8 / drop 16 / outro 8 |

- Drops land exactly on a 16-bar boundary and on beat 1; the downbeat carries kick + crash + bass + sub impact together.
- Change a layer every 8 bars; a fill or transition in the last bar (or last 2 beats) of every 8; a substantial change every 16.
- Drop 2 is never a copy: change the bass phrase, add a counter-melody, switch the lead octave, or alternate the second half of the drop.
- Intros and outros for DJ genres (house, techno, trance, DnB): kick present, no tonal bass for the first 16 bars, no melody conflict.
- The breakdown removes kick and sub; it keeps at most 3 elements (pad/chords, melody, FX). Its last 2-4 bars may lowpass everything to 300-600 Hz.
- Phrase lengths: melodies in 2-, 4- or 8-bar phrases; 4-bar answer to a 4-bar question; odd lengths only as a deliberate pickup.
- Length targets: festival edits 3-4 min; club extended 5-7 min; at 128 bpm 16 bars = 30 s, at 174 bpm 32 bars = 44 s.

## Drums

| Element | Choice / tuning | Pattern | Level |
|---|---|---|---|
| Kick | analog_hybrid house/techno; analog_80s or a punchy layered kick for future bass/trap; short tight kick for DnB/dubstep. Tune its fundamental to the key root or fifth, F1-A#1 (44-58 Hz) | 4/4, half-time 1(+3), or two-step | 0 |
| Clap / snare | clap + snare layered, offset the clap 5-15 ms late for width | 2+4; half-time on 3 | -2..-4 |
| Closed hat | bright, short (20-60 ms) | 16ths with accents on the & ; trap rolls 1/32, 1/24 in the last beat of a phrase | -12..-18 |
| Open hat | 150-300 ms, choked by the closed hat | every & (house/techno/trance) | -9..-12 |
| Ride / shaker | ride on 8ths in techno/trance peaks; shaker 16ths | | -16..-20 |
| Percussion | rim, conga, cowbell, zap; euclid rhythms | panned +-0.3..0.7 | -14..-20 |

- Kick tuning: one kick per song, the same pitch throughout; pitch envelope from 150-250 Hz down to the fundamental in 30-80 ms; length 150-250 ms (house), 300-500 ms (techno, boom-kick styles, unless the kick is the bass).
- Layering: at most 3 layers (click/top 2-5 kHz, body 80-150 Hz, sub 40-60 Hz) with aligned starts; high-pass the top layer at 300 Hz so only one layer owns the sub.
- Swing: 0 techno, trance, big room; 0.1-0.2 house; 0.15-0.3 lo-fi/garage hats; DnB 0.05-0.1 on hats only. Kick and snare stay on the grid.
- Velocity: hats alternate 0.6/0.9 (or random 0.5-0.9); ghost snares 0.2-0.35; never all 1.0.
- Fills: last bar of 8: snare/tom 16ths over the last beat or two, or a 1-beat kick drop. Last bar of 16: bigger (crash on the next downbeat, reverse cymbal 1 bar into it). Remove the kick for the last 1-2 beats before every drop.
- Snare roll build (16 bars): bars 1-4 quarters, 5-8 eighths, 9-12 16ths, 13-15 16ths/32nds, last bar 32nds or silence; velocity ramps 0.4 -> 1.0; pitch rises 0 -> +12 semitones; Highpass on the roll opens from 200 Hz to 1-2 kHz.
- Crash on beat 1 of every drop and every 16-bar section start; -10..-12, high-passed at 400 Hz.

## Bass

- Types: sub (sine, `sub`), boom bass (`boom_bass`, long, pitch glide), reese (`reese`, two saws detuned +-10..20 cents, lowpass 600-1200 Hz), acid (`acid`, ladder, resonance 5-10, cutoff envelope), FM/growl (`fm_bass`, index 2-8 moved per note), pluck bass (`picked_bass`), offbeat/rolling (short saw or square + sub).
- Range: fundamentals E1-E2 (41-82 Hz) for subs; nothing tonal below 30 Hz; Highpass master at 25-30 Hz.
- Kick/bass relationship: they never sound at the same moment with full energy. Either (a) bass off the kick (offbeat, rolling, syncopated) or (b) a Duck: depth 6-10 dB for basses, release 0.1-0.25 s (about 1/8 beat at 128 bpm is 0.12 s), or (c) the kick IS the bass (boom bass / big room).
- Tune bass to the kick: the kick fundamental equals the song's root or fifth within 1 semitone.
- Mono: everything below 120-150 Hz is mono (`Width{bass_mono: 140}` on the master and Width 0 or no unison spread on sub layers). Stereo detune only on the mid-bass above 150 Hz.
- Split complex basses: sub sine layer (lowpass 100 Hz) + mid layer (highpass 100-150 Hz) so the sub stays clean while the mid moves.
- Bass note length: 1/16-1/8 for house/trance offbeats; legato with glide 30-80 ms for reese and boom bass; leave 1/16 gaps before the next kick when not ducked.
- Bass follows chord roots on strong beats; passing notes and octave jumps on weak 16ths.

## Harmony

- Keys: minor dominates (about 70%). Use A, F, G, C#/D, E minor for subs that fit F1-A1 kicks. Major for future bass and uplifting pop crossovers.
- Modes: aeolian (default), dorian (house, soulful), phrygian (techno, dubstep, dark), harmonic minor (trance tension, V chord major), lydian (dreamy future bass).
- Progressions (minor): i-VI-III-VII (Am-F-C-G), i-VII-VI-VII, i-iv-VI-V, VI-VII-i-i, i-v-VI-iv. Major: I-V-vi-IV, vi-IV-I-V, IV-V-iii-vi, ii-V-I-vi with 7ths.
- Chord rhythm: change every 1 bar (future bass, pop), 2 bars (house, trance, prog), 4 bars or never (techno, riddim).
- Loop length: 4 chords over 4 or 8 bars, repeated; a variation on the 4th repeat (substitute the last chord, e.g. VII -> V major).
- Voicings: root in the bass only; pads/chords from C3 up; open voicings (root-5-10, drop-2); no thirds below C3; top notes move by step (voice leading: keep common tones, move other voices 1-2 semitones).
- Color: house m7/m9/m11; future bass maj7/add9/sus2; trance plain triads + sus4 -> 3 resolutions; techno power fifths or single notes.
- Tension and release: build with dominant (V or bVII), sus4, or a pedal bass under changing chords; resolve to i on the drop downbeat. The breakdown may end on the IV or V for 1-2 bars then go to silence.
- Never change key in the middle of a phrase; a key lift of +1 or +2 semitones only at a final drop or chorus.

## Melody and hooks

- Range: hook within an octave to a 10th; leads C4-C6; avoid sitting in the vocal octave when a vocal is present.
- Length: 1-2 bar motif; full hook 4 bars (motif, motif', motif, resolution) or 8 bars as question (4) + answer (4).
- Rhythm: syncopated 16ths with 3-3-2 (dotted 8ths) groupings; a rest of at least 1/8 every bar; start on an offbeat or pickup for drive.
- Notes: chord tones on beats 1 and 3, passing notes between; at most 5-7 distinct pitches in a hook; leaps of a 4th-octave followed by a step back.
- Repetition rule: the motif is repeated exactly at least twice in 4 bars; variation every 4th bar (change the last 2-4 notes, the octave, or the rhythm of the tail). 50-75% repetition is the target; less is unmemorable, more without variation is monotonous.
- Call and response: lead 2 bars, answer 2 bars by another instrument (pluck vs. lead, or bass vs. lead in dubstep), or same instrument an octave apart.
- Climax note: the highest pitch once per 8 bars, in bar 5-7.
- The hook first appears by bar 17-33 (preview it filtered or on a pluck in the intro/breakdown, full in the drop).

## Sound design recipes (engine primitives)

| Sound | Recipe |
|---|---|
| Supersaw | `preset: @supersaw` or Synth osc @saw unison 6, spread 20-40 cents, attack 0-10 ms, release 0.2-0.5 s, lowpass 8-14 kHz; layer `{osc: @saw octave: -1}` for body; Chorus mix 0.2; Width 1.3 |
| Pluck | `preset: @pluck` or saw/square, filter envelope decay 0.1-0.25 s from cutoff 4-6 kHz to 400 Hz, sustain 0, release 0.1; Delay 3/16 or 3/8 beats feedback 0.3, mix 0.25 |
| Pad | `@pad`/`@warm_pad`/`@glass_pad`; attack 0.3-1.5 s, release 1-3 s, unison 3-4, lowpass 2-6 kHz; Highpass 200 Hz; send 0.3-0.45 to a hall |
| Sub | `@sub` or sine, attack 2-5 ms, sustain 0.9, mono, no FX except Duck; optional 5-10% drive for audibility on small speakers |
| Reese | `@reese`: 2 saws detuned +-12..18 cents, lowpass 700-1200 Hz res 1.5-3, + sine sub an octave down; move cutoff with `cutoff_env` or a param; Drive 2-5 |
| Growl / wobble | `@fm_bass` with index moved per note 1->6, or saw through ladder filter res 6-12 with cutoff stepping in 1/8-1/16 per `play`; Drive 8-15 then Eq cut at 300 Hz |
| Acid | `@acid`: ladder res 6-9, cutoff 300-1500 per note via `rand` or a ring, accents (+amp, +cutoff) on 2-4 of 16 steps, glide on tied notes |
| FM bell | `@bell` / `Shader lib: "modal_bell"` or Synth osc @sine fm ratio 3.5 or 7, index 3-6 decaying, decay 1-2 s; high octave C5-C7; Delay 1/4 + Reverb 0.4 |
| Stab | `@stab` or `@organ`: chord, decay 0.15-0.3 s, Reverb short, played on offbeat 16ths |
| Riser | noise @white through Bandpass or Highpass whose cutoff follows a param 300 Hz -> 10 kHz over 8-16 bars; plus a saw with `glide` +12..+24 semitones; amp `env` ramp up; send 0.4-0.5 to reverb |
| Noise sweep / downlifter | white noise `cutoff_env` 10 kHz -> 300 Hz over 2-4 beats, after a drop or into a breakdown |
| Impact | on the drop's beat 1: kick + `@sub` note at the root with pitch `glide` from +12 down over 1 beat + crash + long reverb tail (room 0.9, mix 0.5) |
| Reverse swell | crash/pad with `env` 0 -> 1 over the last 1-2 bars into the downbeat, then cut at the downbeat |
| Gated snare (80s) | snare and toms send ~0.5 to `ShaderFx{lib: "gated_verb" hold: 0.2 release: 0.04 room: 0.85}` (a bright room cut by a gate on each hit); keep the dry snare loud |
| Vocal-like chop | `formant` Ironfish program or Voice with short syllables "ah"/"oh" at 1/16, pitched in the scale |

## Builds and transitions

- Length: 8 bars (house, techno, DnB), 16 bars (big room, trance, future bass, dubstep).
- Stack, in order of entry: snare roll (from bar 1), riser noise (bar 1 or 9), pitch rise on lead/chords (last 4-8 bars), Highpass on the music bus 20 Hz -> 300-800 Hz (whole build), reverb send rising 0.1 -> 0.4, kick removed or switched to 1/8 or 1/16 in the last 2-4 bars.
- Automation via a Clock param: `Section{set: {build: 0}}` at the build start, the Lowpass/Highpass cutoff and riser follow `build` with `ramp` = build length in beats; the drop section sets `build: 0` and `sweep: 1`.
- The drop-in trick: 1/2 to 1 bar (2 beats minimum) of silence or only a vocal/impact tail right before the drop; or a reverse swell that cuts dead at the downbeat. Use once or twice per song, not before every section.
- Fake drop: build -> 1 bar of a different, smaller groove -> real drop. Use at most once.
- Downlifter + crash out of every drop into the breakdown.
- Transitions every 8 bars inside a drop: 1-beat fill, open hat swell, or delay throw on the lead's last note.

## Mixing targets

| Element | Level vs kick | Slot (Hz) | Highpass | Stereo |
|---|---|---|---|---|
| Kick | 0 | 45-100 body, 2-5k click | none (30 Hz master) | mono |
| Sub / bass | -2..-6 | 30-120 | none (sub), 100-150 (mid-bass) | mono below 140 |
| Snare / clap | -2..-4 | 180-250 body, 2-6k | 120-180 | center, clap layer may be wide |
| Hats / ride | -12..-18 | 6-14k | 300-500 | width 1.2-1.5, pan +-0.2..0.4 |
| Percussion | -14..-20 | 300-6k | 200-300 | pan +-0.3..0.7 |
| Chords / supersaw | -6..-10 | 300-6k | 150-250 | wide 1.3-1.8 |
| Pads | -12..-18 | 200-4k | 200-300 | wide 1.5-2 |
| Lead | -4..-8 | 1-6k | 180-250 | center or 1.1-1.3 |
| Vocal | -3..-6 | 1-5k, air 10k | 100-140 | center |
| FX / risers | -8..-14 at peak | full | 200-400 | wide |

- EQ moves: cut 200-400 Hz by 2-4 dB on pads/chords (mud); cut 2.5-4 kHz 1-3 dB on supersaws when harsh; dip the bass 1-2 dB at the kick's fundamental, or the kick at the bass's main note; boost the lead 2-3 dB at 2-4 kHz only if it is masked.
- Sidechain: Duck on bass 6-10 dB, chords/pads 4-8 dB (future bass 10-18), reverbs 3-6 dB; release about 1/8 beat for bass, 1/4 beat for chords/pads (pumping), always keyed from the kick.
- Reverb: drums room 0.3-0.45 send 0.1-0.2; leads/plucks plate 0.6 send 0.2-0.3; pads hall 0.8 send 0.3-0.45; never on the kick or sub; highpass returns at 200-300 Hz; duck returns under the kick.
- Delay: 3/16 or 1/4-dotted (0.75 beat) for plucks and leads, 1/8 for vocals, feedback 0.25-0.45, spread 1; delay throws on the last note of phrases.
- Headroom before mastering: the sum peaks at -6 to -3 dBFS; the kick alone around -12 to -16 dBFS peak.
- Loudness (after the master chain): about -10 LUFS integrated is the default; drops may reach -7 to -8 LUFS short-term for big room, dubstep, DnB; never above -1 dBTP. Integrated loudness of the drop should exceed the breakdown by 4-8 LU (contrast is loudness).

## Common amateur mistakes (and how to detect them)

| Mistake | Detection (source or `score_listen`) | Fix |
|---|---|---|
| Everything plays from bar 1 | a section in the first 16 bars lists more than 4 loops; energy at bar 1 >= drop energy | add layers 1-2 per 8 bars |
| No contrast between drop and breakdown | breakdown RMS within 3 dB of the drop; kick present in the breakdown | remove kick and sub, lowpass, 4-8 LU quieter |
| Drop not on a 16-bar boundary | cumulative bars before a drop section not a multiple of 16 (8 at most for short forms) | pad or trim the build |
| Drop 2 identical to drop 1 | same loops, same code, same notes in both drop sections | change bass phrase, lead octave, counter-melody |
| Kick and bass collide | bass notes on kick positions with no Duck on the bass; low band verdict "muddy/boomy" | offbeat the bass or Duck 6-10 dB |
| Stereo low end | sub or bass loop with unison spread/Width > 0 and no `bass_mono` on master | Width bass_mono 120-150, sub mono |
| Muddy mids | no Highpass on pads/chords/leads; 200-500 Hz band hot in the balance verdict | highpass 150-250, cut 300 Hz 2-4 dB |
| Kick in the wrong key | kick tuning not root/fifth of the song key within 1 semitone | retune via `tune: {kick: n}` |
| Hook too long/aimless | motif longer than 2 bars without repetition; more than 7 pitches; range over a 10th | 1-2 bar motif repeated with variation |
| No repetition or too much | hook bar-to-bar identity below 50% or 100% for 16+ bars | repeat x2, vary every 4th bar |
| Static loop | no layer change in 16 bars; no fill on any 8-bar boundary | layer change every 8, fill in bar 8 |
| Machine-gun velocities | all hits same velocity in hats/snares | accents, ghosts 0.2-0.35, rand 0.5-0.9 |
| Swing on the kick | kick off grid in onset grid | swing only hats/percussion |
| Weak build | build without at least 3 of: roll, riser, filter sweep, pitch rise, kick drop; no param ramp | add the stack |
| Drop without impact | no crash/sub impact on the drop downbeat; no silence or cut before it | add impact + 1/2-1 bar gap |
| Thin chords | triads only in a future bass/trance drop; unison 1 | 7ths/9ths, supersaw unison 6 |
| Low thirds | chord notes stacked in thirds below C3 | move voices up, root/fifth only below C3 |
| Over-reverb | reverb on kick/sub; total send > 0.5 on drums; tails mask the next kick | reverb off lows, highpass returns, duck returns |
| Too loud / clipped | true peak > -1 dBTP; clipping warning; integrated > -7 LUFS | lower limiter gain, raise headroom |
| Too quiet / unmastered | integrated below -14 LUFS; no master chain | add the master chain |
| Wrong tempo for genre | bpm outside the subgenre table range | set bpm in range |
| Ending not resolved | last bar not on the tonic, ends mid-phrase, no outro | 8-16 bar outro ending on i/I |
| Elements fight in the same band | two loops at similar level whose main notes share an octave (e.g. lead and chords both C4-C5 at -6) | move one an octave, carve with Eq, lower one 4-6 dB |
| Pads/hats too loud | pad or hat loop within 6 dB of the kick | follow the level table |

## Evaluator scoring checklist

Score 0-2 each (max 20): genre tempo and drum grid; arrangement bar counts and drop placement; energy curve and contrast; kick/bass relationship and mono low end; harmony (progression, voicing, no low thirds); hook (motif length, repetition with variation, range); build quality (3+ techniques, drop-in gap); sound design fit to genre; mix levels and bands per the table; loudness and peaks. A Learned-from-listening rule that is broken costs 2 points.
