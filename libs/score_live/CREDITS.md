# Credits

- `examples/classical/*.score.splash`: transcriptions of Bernd Krueger's piano performances from piano-midi.de
  (http://www.piano-midi.de), licensed CC BY-SA 3.0 DE (https://creativecommons.org/licenses/by-sa/3.0/de/); the
  adaptations carry the same licence (see `examples/classical/LICENSE`). The compositions are in the public domain;
  the crate's code stays MIT OR Apache-2.0.
- Everything else here (the example scores in `examples/` and `examples/music/`, the sound palette in
  `src/palette/`, the GM program and kit tables) is original to Makepad and under the crate's licence. The GM
  tables list the General MIDI program and drum-key names, which are part of the General MIDI specification.
- `src/palette/catalogue.tsv` holds numbers measured from renders of the General MIDI SoundFont the app downloads on
  request (FluidR3 GM by Frank Wen, MIT licence); the SoundFont itself is never committed or shipped.
