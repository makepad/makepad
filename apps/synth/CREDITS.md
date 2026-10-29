# Credits

Makepad Synth's code is MIT OR Apache-2.0. It commits and ships no third-party data: everything below that is not
Makepad's own is fetched on the person's request (Resources, the Song library), after its notice and licence are
shown, into the weights folder (`~/.makepad/weights/`, or `MAKEPAD_ASSET_AI_CACHE`), outside every repository.
Each install is pinned by size and SHA-256 and writes a provenance record beside the file (`makepad-model-install`).
See also `libs/midi_library/CREDITS.md` (the song library's data and code) and `libs/score_live/CREDITS.md` (the
score engine's example scores and sound palette).

## Downloaded on request (Resources)

- **FluidR3 GM**, Frank Wen's General MIDI SoundFont (release 3, 2008), MIT licence (the full text is shown in
  Resources). What a score's `@soundfont` instruments and imported songs play. Fetched from Debian's
  `fluid-soundfont` source package; never shipped.
- **Lakh MIDI Dataset v0.1**, clean subset and match scores, Colin Raffel, CC BY 4.0. The song library's MIDI files.
  The songs the files transcribe are their owners' copyrighted works: study them, play them, make covers for
  yourself; publishing or selling a recording of a cover needs a licence for the song from its rights holder. Every
  score imported from one keeps its credit.
- **Million Song Dataset** track names and release years (Bertin-Mahieux, Ellis, Whitman and Lamere, ISMIR 2011),
  freely available for research. Part of the song library's default download.
- **Research extras, a separate opt-in** (never part of the song library's default download; the library works
  without them): the **tagtraum MSD genre annotations CD2** (Hendrik Schreiber, ISMIR 2015), research only and
  strictly non-commercial, and the **Last.fm tags of the Million Song Dataset**, for research.
- **High Voltage SID Collection** (release #85, hvsc.c64.org): the tunes are copyrighted by their composers;
  private enjoyment only (the collection's disclaimer is shown before install). Each tune keeps its composer,
  title and release.

- **Surge XT patch library** (opt-in, two sets: the current content of upstream commit dab78c6, patch format
  revision 31, and the 1.3.4 release's content, revision 25), github.com/surge-synthesizer/surge, licensed
  GPL-3.0-or-later; about 360 third-party patches declare CC0 in their own metadata and are CC0. What Synth's
  `Tandem{patch: …}` instrument plays. Only the data folders are unpacked (patches, wavetables, FX and modulator
  presets, tuning library, factory impulse responses); never the skins, the "Waldorf" wavetables (no grant on
  record) or the third-party impulse responses. Synth's engine that plays them is our own MIT code, written from a
  behavioural description without the upstream source. Credits: the factory library by Claes Johanson (Vember Audio)
  and the Surge Synth Team (EvilDragon, baconpaul, Fred Cormier, Arty, Macc, Taron, thynctank); third-party patch
  folders, each credited to its author: A.Liv, Aleksey Zhehanov, Altenberg, Argitoth, Black-Sided Sun, Bluelight,
  Cybersoda, Damon Armani, Dan Maurer, Databroth, Emu ("The Emu"), Exquis MPE (Intuitive Instruments), Giana
  Brotherz (Sola), Inigo Kennedy, Jacky Ligon, John Valentine, Kinsey Dulcet, Kuniklo, Kyurumi, Landosonic,
  LinnStrument MPE (Roger Linn, thoughtForm), Lopyt, Luna, Malfunction, Nick Moritz, Noisegeek, Psiome Send Sound,
  Rare Earth (Leonard Bowman), Rozzer, Slowboat, Stefan Singer, TNMG (The Nerdy Music Guy), Vincent Zauhar, Vospi,
  Xenofish, Zoozither (Polyslax), qb; wavetable folders: A.Liv, Art of Sound, Damon Armani, Emu, Emu VSCO, Layzer,
  Ocean Swift Synthesis, Philippe Favre, Qonundrai, TNMG, Venus Theory.

## Downloaded on the person's own pick (Song library, chat tools)

- **Zophar's Domain** (zophar.net): game music rips (NSF, SPC, GBS, VGM), one game at a time; the music is its
  publisher's copyright, kept with its credits for listening and learning, never published.
- **The Mod Archive** (modarchive.org): MOD, S3M, XM and IT modules, one at a time; the composers keep their
  copyright unless a module carries a public-domain or Creative Commons licence.

## Code in the synth engine (`libs/audio_synth`, MIT)

- The twist oscillator's macro engines and the granular texture effect are Rust ports of Emilie Gillet's MIT code
  (github.com/pichenettes/eurorack: the STM32 module code and stmlib; the texture effect's variant modes from
  github.com/mqtthiqs/parasites, MIT), copyright Emilie Gillet, with each upstream file's MIT notice kept
  at the top of the Rust file ported from it.
- The small-effects host's 75 effects are Rust ports of Chris Johnson's Airwindows plugins
  (github.com/airwindows/airwindows), MIT, "Copyright (c) 2018 Chris Johnson", the notice kept in each file.

## In the repository

- The classical example scores (`libs/score_live/examples/classical`) are transcriptions of Bernd Krueger's piano
  performances from piano-midi.de, licensed CC BY-SA 3.0 DE; the adaptations carry the same licence (see
  `libs/score_live/examples/classical/LICENSE`).
- `resources/icon.svg`, `resources/icon_1024.png`, `resources/synth-icon.png`: the Synth app icon (a saw wave over
  four level bars), original, made for this project: hand-written SVG, no traced or third-party artwork. The PNGs
  are `rsvg-convert -w 1024 -h 1024 resources/icon.svg`, copied to both names.
- `resources/icons/*.svg` (play, pause, stop, save, more, note, text, timeline): original, hand-written for Synth.
