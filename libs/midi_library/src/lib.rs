//! The MIDI song library: songs as MIDI, an opt-in download, searchable in
//! Stage, opened as Scores (recognisable covers through the Score engine),
//! read as text by the music AI to learn how real arrangements are built.
//!
//! - [`dataset`]: what is downloaded (the Lakh MIDI Dataset's clean subset
//!   and the Million Song Dataset metadata it is matched to), pinned by URL,
//!   size and SHA-256 and fetched by the same installer as the GM SoundFont
//!   and the fonts, when the person opts in. The songs are their owners'
//!   works; every cover carries its credit ([`dataset::NOTICE`]).
//! - [`tar`], [`md5`], [`meta`]: reading the archive and the metadata.
//! - [`analyse`] and [`gm`]: what a MIDI file holds: tempo, key, metre,
//!   parts and their roles, length, and a quality score (a full arrangement
//!   or a sketch).
//! - [`index`]: the searchable catalogue (artist, title, year and decade,
//!   genre and tags, tempo, key, roles, quality): "80s synthpop".
//! - [`import`] and [`form`]: a MIDI file as a readable Score (a loop per
//!   part on the best patch for its role, the tempo map, the form as
//!   sections found from repetition, a mix and master chain).
//! - [`arrangement`]: what a song is between its sources and its Score.
//! - [`roll`]: the text transcription (a dense piano roll an AI reads and
//!   writes), from an arrangement or a Score and back.
//! - [`arranger`]: the second stage: the music model rewrites the
//!   mechanical import as an idiomatic Score, checked against the song.
//! - [`cover`]: a cover rendered through the engine at a loudness target.
//! - [`tools`]: the AI tools (midi_lookup, midi_transcribe, score_text),
//!   for Stage's control API and as an MCP server (`midi_library mcp`).
//! - [`refs`]: short excerpts of imported covers kept as reference examples
//!   the composers are shown when a request's style matches one.

pub mod analyse;
pub mod arrangement;
pub mod chip;
pub mod arranger;
pub mod cover;
pub mod dataset;
pub mod form;
pub mod gm;
pub mod import;
pub mod index;
pub mod md5;
pub mod meta;
pub mod refs;
pub mod roll;
pub mod sound;
pub mod tar;
pub mod tools;
