# Credits and third-party data

This crate commits no third-party data. Everything below is fetched on the
person's own request, after its notice and licence are shown, into the
weights folder (`~/.makepad/weights/`, or `MAKEPAD_ASSET_AI_CACHE`), outside
every repository. Each install writes a provenance record beside the file
(`makepad-model-install`).

## Code

- `src/chip/archive/ppmd.rs`: a Rust translation of PPMd var.H by Dmitry
  Shkarin as laid out in the LZMA SDK's Ppmd7 by Igor Pavlov, both public
  domain.

## Data a plain `midi_library install --accept` downloads (`src/dataset.rs`)

- Lakh MIDI Dataset v0.1, clean subset (`clean_midi.tar.gz`) and its match
  scores (`match_scores.json`), Colin Raffel, from colinraffel.com/projects/lmd.
  CC BY 4.0. The songs the MIDI files transcribe are their owners'
  copyrighted works: for study, practice and covers for yourself; publishing
  or selling a recording needs a licence from the rights holder.
- Million Song Dataset track names (`unique_tracks.txt`) and release years
  (`tracks_per_year.txt`), from millionsongdataset.com (Bertin-Mahieux,
  Ellis, Whitman and Lamere, ISMIR 2011). Freely available for research.

## Opt-in extras, research terms only (`install --accept --genres --tags`)

Never part of a plain install; the index works without them.

- tagtraum MSD genre annotations CD2 (`msd_tagtraum_cd2.cls.zip`), from
  tagtraum.com (Hendrik Schreiber, ISMIR 2015). Research only, strictly
  non-commercial.
- The Last.fm tags of the Million Song Dataset (`lastfm_tags.db`), from
  millionsongdataset.com/lastfm, courtesy of Last.fm, for research.

## Chip music, download-only (`src/chip/`)

Copyrighted game music; kept with its credits for listening and learning,
never bundled, redistributed or published.

- High Voltage SID Collection release #85 (`chip install-hvsc --accept`),
  hvsc.c64.org: the tunes are copyrighted by their composers; private
  enjoyment only (the collection's Disclaimer is shown before install).
- Zophar's Domain (zophar.net): NSF, SPC, GBS and VGM rips, one game at a
  time on the person's own pick; the music is its publisher's.
- The Mod Archive (modarchive.org): MOD, S3M, XM and IT modules, one at a
  time on the person's own pick; the composers keep their copyright unless a
  module carries a public-domain or Creative Commons licence.

## SoundFont

- FluidR3 GM, Frank Wen (release 3, 2008), MIT. Not part of this crate:
  the score engine (`makepad-score-live`, `gm_font`) names it and the
  installer fetches it on request from Debian's `fluid-soundfont` source
  package. Covers render with it when no other voice set is chosen.
