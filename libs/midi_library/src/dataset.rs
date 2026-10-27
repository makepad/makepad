//! What the song library downloads: each file pinned by URL, size and
//! SHA-256, fetched by Stage's resource installer
//! (`makepad_model_install`: a resumable `.part`, the hash checked
//! before the file is committed, a provenance record beside it) into the
//! weights folder (`~/.makepad/weights/datasets/lmd/`), outside every
//! checkout. Nothing here ships with Stage: each is an opt-in download
//! ([`ModelUse::Dataset`]) the person installs after its notice and
//! licence are shown, and every song keeps its credit.
//!
//! - The Lakh MIDI Dataset's clean subset (Colin Raffel, CC BY 4.0): 17k
//!   MIDI files named by artist and title. The songs they transcribe are
//!   their owners' copyrighted works ([`NOTICE`]).
//! - LMD's match scores: which Million Song Dataset tracks each file
//!   matches, and how well (the key to the metadata below).
//! - The Million Song Dataset's track names (to match a file LMD did not
//!   match with confidence by its artist and title) and release years.
//! - Opt-in only, never in a plain install ([`RESEARCH`]): the tagtraum genre
//!   annotations of the MSD (CD2: 15 genres, research only, non-commercial)
//!   and the Last.fm tags of the MSD ("synthpop", "new wave", "80s" ...,
//!   for research).

use makepad_model_install::{self as models, Installable, ModelUse};
use std::path::PathBuf;

/// Credited wherever the library's songs are used.
pub const CREDIT: &str = "Lakh MIDI Dataset v0.1 (Colin Raffel, \"Learning-Based Methods for Comparing Sequences, with Applications to Audio-to-MIDI Alignment and Matching\", PhD thesis, 2016), CC BY 4.0; matched to the Million Song Dataset (Bertin-Mahieux, Ellis, Whitman and Lamere, ISMIR 2011)";

/// What the person reads before installing, and every cover repeats.
pub const NOTICE: &str = "The songs in this library are MIDI transcriptions of copyrighted works; their copyrights stay with their owners. Use them to study, practise and make covers for yourself. Publishing or selling a recording of a cover needs a licence for the song (a mechanical licence) from its rights holder. Transcriptions: the Lakh MIDI Dataset (Colin Raffel), CC BY 4.0.";

const LMD_LICENSE: &str = "CC BY 4.0 (Lakh MIDI Dataset); the songs are their owners' copyrighted works";
const LMD_TEXT: &str = "The Lakh MIDI Dataset is distributed under a Creative Commons Attribution 4.0 International license (https://creativecommons.org/licenses/by/4.0/). Credit: Colin Raffel, \"Learning-Based Methods for Comparing Sequences, with Applications to Audio-to-MIDI Alignment and Matching\", PhD thesis, Columbia University, 2016. https://colinraffel.com/projects/lmd/\n\nThe songs in this library are MIDI transcriptions of copyrighted works; their copyrights stay with their owners. Use them to study, practise and make covers for yourself. Publishing or selling a recording of a cover needs a licence for the song (a mechanical licence) from its rights holder.";
const MSD_LICENSE: &str = "Million Song Dataset: freely available for research";
const MSD_TEXT: &str = "The Million Song Dataset (http://millionsongdataset.com) is a freely available collection of audio features and metadata for research. Credit: Thierry Bertin-Mahieux, Daniel P. W. Ellis, Brian Whitman and Paul Lamere, \"The Million Song Dataset\", ISMIR 2011. Song names and years only.";
const TAGTRAUM_LICENSE: &str = "tagtraum MSD genre annotations: research only, non-commercial";
const TAGTRAUM_TEXT: &str = "tagtraum industries' MSD genre ground truth (CD2), https://www.tagtraum.com/msd_genre_datasets.html: \"Research only, strictly non-commercial.\" Credit: Hendrik Schreiber, \"Improving Genre Annotations for the Million Song Dataset\", ISMIR 2015. Used to label songs' genres on your machine.";
const LASTFM_TEXT: &str = "The Last.fm dataset, the official song tags of the Million Song Dataset (http://millionsongdataset.com/lastfm/), courtesy of Last.fm, for research. Used to tag songs (synthpop, new wave, 80s) on your machine.";

pub const CLEAN_MIDI: Installable = Installable::dataset(
    "LMD clean MIDI",
    LMD_LICENSE,
    LMD_TEXT,
    "colinraffel.com/projects/lmd: clean_midi.tar.gz (LMD v0.1, the subset named by artist and title)",
    234_283_029,
    "de1bb64cbc0cf35545a05b5c3e786aa6890cfa144edffc4b827ff41bf8c33dc5",
    "http://hog.ee.columbia.edu/craffel/lmd/clean_midi.tar.gz",
    "datasets/lmd/clean_midi.tar.gz",
);

pub const MATCH_SCORES: Installable = Installable::dataset(
    "LMD match scores",
    LMD_LICENSE,
    LMD_TEXT,
    "colinraffel.com/projects/lmd: match_scores.json (MSD track id -> MIDI md5 -> confidence)",
    7_237_036,
    "267bc606dfa21f0ad0601a4a080972cd4ae8088fe4003b9bb2811b5be060a102",
    "http://hog.ee.columbia.edu/craffel/lmd/match_scores.json",
    "datasets/lmd/match_scores.json",
);

pub const YEARS: Installable = Installable::dataset(
    "MSD years",
    MSD_LICENSE,
    MSD_TEXT,
    "millionsongdataset.com additional files: tracks_per_year.txt",
    35_028_078,
    "ddd3b017cd5059b127e2bf960af8ac0d70d67ecf225e76711be2dc2dfc793bb0",
    "http://millionsongdataset.com/sites/default/files/AdditionalFiles/tracks_per_year.txt",
    "datasets/lmd/tracks_per_year.txt",
);

pub const TRACKS: Installable = Installable::dataset(
    "MSD track names",
    MSD_LICENSE,
    MSD_TEXT,
    "millionsongdataset.com additional files: unique_tracks.txt (track, song, artist, title)",
    84_046_293,
    "f9bb19e7f5d39b22c958a3653f0fac60411fb0e4f0405f5a547d613d81adaa3a",
    "http://millionsongdataset.com/sites/default/files/AdditionalFiles/unique_tracks.txt",
    "datasets/lmd/unique_tracks.txt",
);

pub const GENRES: Installable = Installable::dataset(
    "MSD genres (tagtraum CD2)",
    TAGTRAUM_LICENSE,
    TAGTRAUM_TEXT,
    "tagtraum.com: msd_tagtraum_cd2.cls.zip",
    2_473_570,
    "3a9f84938cacc105864b320f8c52caa7ba5a05e71c39fc40744ffdc323c8e5d1",
    "https://www.tagtraum.com/genres/msd_tagtraum_cd2.cls.zip",
    "datasets/lmd/msd_tagtraum_cd2.cls.zip",
);

pub const TAGS: Installable = Installable::dataset(
    "MSD Last.fm tags",
    MSD_LICENSE,
    LASTFM_TEXT,
    "millionsongdataset.com/lastfm: lastfm_tags.db (SQLite)",
    594_083_840,
    "2bf7d84d44612ab775afbc97d85e752fb2c9dfa75d357cc4fcd50b5d6c9fe6e5",
    "http://millionsongdataset.com/sites/default/files/lastfm/lastfm_tags.db",
    "datasets/lmd/lastfm_tags.db",
);

/// What a plain install downloads, smallest first: the songs, their match
/// scores, the MSD track names and years. Every file is open for any use the
/// library makes of it; nothing research-restricted is in here.
pub const FILES: [&Installable; 4] = [&MATCH_SCORES, &YEARS, &TRACKS, &CLEAN_MIDI];

/// Opt-in extras under research-only terms (tagtraum CD2 is strictly
/// non-commercial; the Last.fm tags are for research): never part of
/// [`FILES`], fetched only when the person asks for them by name
/// (`midi_library install --accept --genres --tags`) after reading their
/// notice. The index uses them when they are installed and goes without
/// when they are not.
pub const RESEARCH: [&Installable; 2] = [&GENRES, &TAGS];

const _: () = {
    let mut i = 0;
    while i < FILES.len() {
        assert!(matches!(FILES[i].use_, ModelUse::Dataset));
        i += 1;
    }
    let mut i = 0;
    while i < RESEARCH.len() {
        assert!(matches!(RESEARCH[i].use_, ModelUse::Dataset));
        i += 1;
    }
};

/// Where an installed file is.
pub fn path(file: &Installable) -> PathBuf {
    models::dest_path(file)
}

/// The library's own folder (the index and the reference excerpts are
/// written here, beside the downloads).
pub fn dir() -> PathBuf {
    let mut dir = models::weights_root();
    dir.extend(["datasets", "lmd"]);
    dir
}

/// The files of a plain install not on this machine yet.
pub fn missing() -> Vec<&'static Installable> {
    models::missing_for(&FILES)
}

/// Whether an opt-in research file is on this machine.
pub fn research_installed(file: &Installable) -> bool {
    models::installed(file)
}

/// Total bytes of the files given.
pub fn bytes(files: &[&Installable]) -> u64 {
    files.iter().map(|f| f.bytes).sum()
}
