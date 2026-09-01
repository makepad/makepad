//! The music library: a folder of scores, listed so a piece can be opened
//! without the command line.
//!
//! The folder is a preference, not a constant. It defaults to the development
//! corpus when that happens to be present in the checkout being run from, but
//! nothing here depends on it: point the library at any directory of `.mid`,
//! `.musicxml` or native scores and it lists what is there. A missing or empty
//! folder is a normal state with a sentence explaining it, not an error.
//!
//! Entries carry what is cheap to know: composer and title (the file's own
//! sequence-name meta event when it has one, the file name otherwise) and the
//! playing time. The scan reads MIDI *metadata* only — header, track lengths
//! and tempo changes — so listing a folder costs a few milliseconds and never
//! runs the importer, which stays where it belongs, behind opening a file.

use std::path::{Path, PathBuf};

/// Extensions the app can actually open (see `document::ScoreLoader`).
const PLAYABLE: [&str; 6] = ["mid", "midi", "musicxml", "mxl", "xml", "mpscore"];

/// Where an entry's bytes come from.
///
/// The library is a folder of files, but the application also carries a few
/// public-domain pieces inside the binary so a fresh install has real music to
/// play before anyone has pointed it at a folder. Those have no path, so the
/// browser cannot open them by one — the bytes travel with the entry instead.
#[derive(Clone, Debug, PartialEq)]
pub enum EntrySource {
    /// A file on disk, at `LibraryEntry::path`.
    File,
    /// A piece compiled into the binary: its bytes and the extension that says
    /// how to read them.
    Bundled {
        bytes: &'static [u8],
        extension: &'static str,
        credit: &'static str,
        /// See [`BundledPiece::attribution`].
        attribution: Option<&'static str>,
    },
}

/// One playable file in the library folder.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryEntry {
    pub source: EntrySource,
    pub path: PathBuf,
    /// From the file name's leading token, when it has one.
    pub composer: String,
    /// The file's own sequence name when it carries one, else the file name.
    pub title: String,
    /// The title the file name alone gives, kept as the tiebreak when two
    /// files claim the same sequence name.
    pub from_name: String,
    /// Playing time, when the file says enough to work it out.
    pub seconds: Option<f64>,
}

impl LibraryEntry {
    /// The single line the browser lists this entry as.
    pub fn line(&self) -> String {
        let mut line = String::new();
        if !self.composer.is_empty() {
            line.push_str(&self.composer);
            line.push_str(" · ");
        }
        line.push_str(&self.title);
        if let Some(seconds) = self.seconds {
            line.push_str("   ");
            line.push_str(&format_duration(seconds));
        }
        line
    }
}

/// `m:ss`, the way a track length is written everywhere else.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// A folder of scores and what a scan of it found.
#[derive(Clone, Debug, Default)]
pub struct MusicLibrary {
    dir: Option<PathBuf>,
    /// The pieces that ship inside the binary. They head every listing, so a
    /// fresh install is never an empty shelf.
    bundled: Vec<LibraryEntry>,
    entries: Vec<LibraryEntry>,
    /// Why the folder produced nothing, when that is worth saying.
    problem: Option<String>,
    scanned: bool,
}

impl MusicLibrary {
    /// Point the library at the configured folder, falling back to the
    /// development corpus while it exists. Reads nothing yet: the folder is
    /// scanned the first time the browser is actually opened.
    pub fn new(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf).or_else(default_library_dir),
            bundled: Vec::new(),
            entries: Vec::new(),
            problem: None,
            scanned: false,
        }
    }

    /// Install the pieces the application carries. They are listed first and
    /// survive every rescan, so pointing the library at a folder adds to the
    /// shelf rather than replacing it.
    pub fn set_bundled(&mut self, pieces: &[BundledPiece]) {
        self.bundled = pieces.iter().map(BundledPiece::entry).collect();
        // The shelf is visible from the first frame now that it lives in the
        // sidebar, so it is filled here rather than on first open.
        self.rescan();
    }

    /// Scan on first use. Never fails: an unreadable folder is an empty
    /// library that says why.
    pub fn ensure_scanned(&mut self) {
        if !self.scanned {
            self.rescan();
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    pub fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
        self.rescan();
    }

    /// The folder as the browser writes it, for the path field.
    pub fn dir_text(&self) -> String {
        self.dir
            .as_deref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default()
    }

    pub fn rescan(&mut self) {
        self.entries.clear();
        self.entries.extend(self.bundled.iter().cloned());
        self.problem = None;
        self.scanned = true;
        let Some(dir) = self.dir.clone() else {
            self.problem = Some(
                "No music folder set yet. Choose a folder of MIDI or MusicXML scores to browse."
                    .to_string(),
            );
            return;
        };
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(error) => {
                self.problem = Some(format!("{} · {error}", dir.display()));
                return;
            }
        };
        let mut paths: Vec<PathBuf> = read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_playable(path))
            .collect();
        paths.sort();
        self.entries.extend(paths.into_iter().map(describe));
        resolve_duplicate_titles(&mut self.entries);
        if self.entries.len() == self.bundled.len() {
            self.problem = Some(format!(
                "No scores in {} — the browser lists .mid, .musicxml and .{} files.",
                dir.display(),
                crate::document::NATIVE_EXTENSION
            ));
        }
    }

    /// The sentence to show when there is nothing to list.
    pub fn empty_state(&self) -> Option<&str> {
        self.entries.is_empty().then(|| {
            self.problem
                .as_deref()
                .unwrap_or("No scores found in this folder.")
        })
    }

    /// A one-line summary of what was found, for the browser's header.
    pub fn summary(&self) -> String {
        match (&self.dir, self.entries.len()) {
            (None, _) => "No folder chosen".to_string(),
            (Some(_), 0) => "Nothing to play here".to_string(),
            (Some(_), count) => {
                let total: f64 = self.entries.iter().filter_map(|entry| entry.seconds).sum();
                format!(
                    "{count} piece{} · {} of music",
                    if count == 1 { "" } else { "s" },
                    format_duration(total)
                )
            }
        }
    }

    /// The index of an already-open file, so the browser can point at it.
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| same_file(&entry.path, path))
    }
}

/// Whether two paths name the same file. The library scans absolute paths
/// while a document may have been opened by a relative one (from the command
/// line, or a recents entry), so a plain `==` misses the match that puts the
/// "now playing" mark on the right row.
pub fn same_file(a: &Path, b: &Path) -> bool {
    // A bundled entry has no path at all, and two of those are not the same
    // piece just because neither came from disk.
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    match (std::path::absolute(a), std::path::absolute(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn is_playable(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| PLAYABLE.contains(&value.as_str()))
}

/// A folder named deliberately, and nothing else.
///
/// This used to hunt for a development corpus in the enclosing checkout, which
/// meant the shelf silently filled with whatever happened to be lying around
/// beside the binary. The shipped pieces are the shelf; anything else is a
/// folder somebody chose.
pub fn default_library_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("MAKEPAD_SCORE_LIBRARY").map(PathBuf::from)?;
    dir.is_dir().then_some(dir)
}

/// How long a title may be before the row starts to run off its own width.
const TITLE_LIMIT: usize = 72;

fn describe(path: PathBuf) -> LibraryEntry {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let (composer, from_name) = split_file_stem(&stem);
    let meta = std::fs::read(&path)
        .ok()
        .map(|bytes| midi_metadata(&bytes))
        .unwrap_or_default();
    let title = meta
        .name
        .filter(|name| is_informative(name))
        .map(|name| shorten(&drop_leading_composer(&name, &composer)))
        .unwrap_or_else(|| from_name.clone());
    LibraryEntry {
        source: EntrySource::File,
        path,
        composer,
        title,
        from_name,
        seconds: meta.seconds,
    }
}

/// A piece the application carries, as the browser lists it.
///
/// Every one of these is public domain at the source — score and MIDI
/// rendering both — which is what lets them live inside a permissively
/// licensed binary with no attribution or share-alike obligation attached.
pub struct BundledPiece {
    pub composer: &'static str,
    pub title: &'static str,
    pub credit: &'static str,
    /// Who played it and under what licence, when the piece is somebody's
    /// PERFORMANCE rather than an engraving. Carried with the piece so the
    /// credit cannot drift away from the thing it credits: it is shown on the
    /// status line when the piece opens, and listed in About.
    pub attribution: Option<&'static str>,
    pub extension: &'static str,
    pub bytes: &'static [u8],
}

impl BundledPiece {
    /// The entry the browser lists, with the playing time read from the MIDI
    /// the same way a scanned file's is.
    pub fn entry(&self) -> LibraryEntry {
        let meta = midi_metadata(self.bytes);
        LibraryEntry {
            source: EntrySource::Bundled {
                bytes: self.bytes,
                extension: self.extension,
                credit: self.credit,
                attribution: self.attribution,
            },
            path: PathBuf::new(),
            composer: self.composer.to_string(),
            title: self.title.to_string(),
            from_name: self.title.to_string(),
            seconds: meta.seconds,
        }
    }
}

/// A sequence name is often the *collection* rather than the piece: two
/// Debussy files both call themselves "Suite bergamasque". A title that lands
/// on more than one file says nothing, so those fall back to the file names,
/// which are unique by construction.
fn resolve_duplicate_titles(entries: &mut [LibraryEntry]) {
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for entry in entries.iter() {
        *seen.entry(entry.title.clone()).or_insert(0) += 1;
    }
    for entry in entries.iter_mut() {
        if seen.get(&entry.title).copied().unwrap_or(0) > 1 {
            entry.title = entry.from_name.clone();
        }
    }
}

/// "Chopin Nocturne Opus 27 Nr. 2" under a Chopin heading is one Chopin too
/// many.
fn drop_leading_composer(title: &str, composer: &str) -> String {
    if composer.is_empty() {
        return title.to_string();
    }
    let lower_title = title.to_ascii_lowercase();
    let lower_composer = composer.to_ascii_lowercase();
    match lower_title.strip_prefix(&lower_composer) {
        Some(rest) if rest.starts_with(' ') || rest.starts_with(':') || rest.starts_with(',') => {
            let trimmed = title[composer.len()..].trim_start_matches([' ', ':', ',']);
            if trimmed.is_empty() {
                title.to_string()
            } else {
                trimmed.to_string()
            }
        }
        _ => title.to_string(),
    }
}

fn shorten(title: &str) -> String {
    if title.chars().count() <= TITLE_LIMIT {
        return title.to_string();
    }
    let mut out: String = title.chars().take(TITLE_LIMIT - 1).collect();
    while out.ends_with([' ', ',', '-']) {
        out.pop();
    }
    out.push('…');
    out
}

/// `beethoven-moonlight-1` -> (`Beethoven`, `Moonlight 1`). A file that is not
/// named that way keeps its whole name as the title and gets no composer,
/// which is exactly right for someone else's folder.
fn split_file_stem(stem: &str) -> (String, String) {
    match stem.split_once('-') {
        Some((composer, rest)) if !composer.is_empty() && !rest.is_empty() => {
            (title_case(composer), title_case(&rest.replace(['-', '_'], " ")))
        }
        _ => (String::new(), title_case(&stem.replace(['-', '_'], " "))),
    }
}

fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, word) in text.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
            None => {}
        }
    }
    out
}

/// A sequence name worth showing instead of the file name. "Piano", "Track 1"
/// and the like say nothing the list does not already show.
fn is_informative(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.len() < 4 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "piano" | "untitled" | "midi" | "score" | "music" | "grand piano" | "acoustic grand piano"
    ) && !lower.starts_with("track ")
}

/// What a MIDI file says about itself without importing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiMetadata {
    /// The first sequence/track-name meta event.
    pub name: Option<String>,
    /// Playing time in seconds, following tempo changes.
    pub seconds: Option<f64>,
}

/// Read the header, walk every track's delta times and tempo changes, and
/// report the name and the length. Deliberately tolerant: a file this cannot
/// parse simply has no metadata, and opening it still goes through the real
/// importer.
pub fn midi_metadata(bytes: &[u8]) -> MidiMetadata {
    let mut meta = MidiMetadata::default();
    if bytes.len() < 14 || &bytes[..4] != b"MThd" {
        return meta;
    }
    let header_len = be_u32(&bytes[4..8]) as usize;
    let division = be_u16(&bytes[12..14]);
    let mut cursor = 8 + header_len;

    // (tick, microseconds per quarter) changes, gathered across all tracks
    // because a type-1 file keeps them in the conductor track.
    let mut tempo_changes: Vec<(u64, u32)> = Vec::new();
    let mut end_tick = 0_u64;

    while cursor + 8 <= bytes.len() {
        let chunk = &bytes[cursor..cursor + 4];
        let length = be_u32(&bytes[cursor + 4..cursor + 8]) as usize;
        let start = cursor + 8;
        let end = start.saturating_add(length).min(bytes.len());
        cursor = start.saturating_add(length);
        if chunk != b"MTrk" {
            continue;
        }
        let track = &bytes[start..end];
        let mut at = 0_usize;
        let mut tick = 0_u64;
        let mut running_status = 0_u8;
        while at < track.len() {
            let Some((delta, used)) = read_varint(&track[at..]) else { break };
            at += used;
            tick += u64::from(delta);
            let Some(&status) = track.get(at) else { break };
            let status = if status < 0x80 {
                running_status
            } else {
                at += 1;
                running_status = status;
                status
            };
            match status {
                0xff => {
                    let Some(&kind) = track.get(at) else { break };
                    at += 1;
                    let Some((length, used)) = read_varint(&track[at..]) else { break };
                    at += used;
                    let length = length as usize;
                    let payload_end = at.saturating_add(length).min(track.len());
                    let payload = &track[at..payload_end];
                    match kind {
                        0x03 if meta.name.is_none() => {
                            let name = decode_text(payload);
                            if !name.trim().is_empty() {
                                meta.name = Some(name.trim().to_string());
                            }
                        }
                        0x51 if payload.len() == 3 => {
                            let micros = (u32::from(payload[0]) << 16)
                                | (u32::from(payload[1]) << 8)
                                | u32::from(payload[2]);
                            if micros > 0 {
                                tempo_changes.push((tick, micros));
                            }
                        }
                        _ => {}
                    }
                    at = at.saturating_add(length);
                }
                0xf0 | 0xf7 => {
                    let Some((length, used)) = read_varint(&track[at..]) else { break };
                    at += used + length as usize;
                }
                _ => {
                    let data = match status & 0xf0 {
                        0xc0 | 0xd0 => 1,
                        0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => 2,
                        // An unknown status byte means the stream is no longer
                        // being read where events start; stop this track.
                        _ => break,
                    };
                    at += data;
                }
            }
        }
        end_tick = end_tick.max(tick);
    }

    meta.seconds = ticks_to_seconds(end_tick, division, &mut tempo_changes);
    meta
}

/// Convert a tick count to seconds, honouring every tempo change on the way.
fn ticks_to_seconds(end_tick: u64, division: u16, changes: &mut Vec<(u64, u32)>) -> Option<f64> {
    if end_tick == 0 {
        return None;
    }
    if division & 0x8000 != 0 {
        // SMPTE division: frames per second and ticks per frame, so the tick
        // rate is fixed and no tempo map applies.
        let frames = f64::from(-((division >> 8) as i8));
        let per_frame = f64::from(division & 0xff);
        let rate = frames * per_frame;
        return (rate > 0.0).then(|| end_tick as f64 / rate);
    }
    let per_quarter = f64::from(division & 0x7fff);
    if per_quarter <= 0.0 {
        return None;
    }
    changes.sort_by_key(|(tick, _)| *tick);
    let mut seconds = 0.0_f64;
    let mut tick = 0_u64;
    // 120 BPM until the file says otherwise, as the format specifies.
    let mut micros = 500_000_u32;
    for (change_tick, change_micros) in changes.iter().copied() {
        let change_tick = change_tick.min(end_tick);
        if change_tick > tick {
            seconds += (change_tick - tick) as f64 / per_quarter * f64::from(micros) / 1.0e6;
            tick = change_tick;
        }
        micros = change_micros;
    }
    seconds += (end_tick - tick) as f64 / per_quarter * f64::from(micros) / 1.0e6;
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

/// MIDI text is bytes with no declared encoding. Take UTF-8 when it is valid
/// (modern files) and Latin-1 otherwise, which is what the older collections
/// were written in — the difference is `Für Elise` versus `F?r Elise`.
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

fn read_varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0_u32;
    for (index, byte) in bytes.iter().take(4).enumerate() {
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

fn be_u16(bytes: &[u8]) -> u16 {
    u16::from(bytes[0]) << 8 | u16::from(bytes[1])
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) << 24
        | u32::from(bytes[1]) << 16
        | u32::from(bytes[2]) << 8
        | u32::from(bytes[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal type-0 file: a name, a tempo, one note, an end-of-track.
    fn one_track_midi(ticks_per_quarter: u16, micros_per_quarter: u32, quarters: u32) -> Vec<u8> {
        let mut track = Vec::new();
        // delta 0, meta 0x03 "Test Piece"
        track.extend_from_slice(&[0x00, 0xff, 0x03, 0x0a]);
        track.extend_from_slice(b"Test Piece");
        // delta 0, meta 0x51 tempo
        track.extend_from_slice(&[0x00, 0xff, 0x51, 0x03]);
        track.push((micros_per_quarter >> 16) as u8);
        track.push((micros_per_quarter >> 8) as u8);
        track.push(micros_per_quarter as u8);
        // delta 0, note on
        track.extend_from_slice(&[0x00, 0x90, 60, 100]);
        // note off after `quarters` quarters, written as a varint
        let mut delta = Vec::new();
        let mut value = u32::from(ticks_per_quarter) * quarters;
        let mut stack = vec![(value & 0x7f) as u8];
        value >>= 7;
        while value > 0 {
            stack.push(((value & 0x7f) as u8) | 0x80);
            value >>= 7;
        }
        stack.reverse();
        delta.extend_from_slice(&stack);
        track.extend_from_slice(&delta);
        track.extend_from_slice(&[0x80, 60, 0]);
        track.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);

        let mut file = Vec::new();
        file.extend_from_slice(b"MThd");
        file.extend_from_slice(&6_u32.to_be_bytes());
        file.extend_from_slice(&0_u16.to_be_bytes());
        file.extend_from_slice(&1_u16.to_be_bytes());
        file.extend_from_slice(&ticks_per_quarter.to_be_bytes());
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(track.len() as u32).to_be_bytes());
        file.extend_from_slice(&track);
        file
    }

    #[test]
    fn the_scan_reads_the_name_and_the_playing_time() {
        // 8 quarters at 120 BPM is 4 seconds.
        let bytes = one_track_midi(480, 500_000, 8);
        let meta = midi_metadata(&bytes);
        assert_eq!(meta.name.as_deref(), Some("Test Piece"));
        let seconds = meta.seconds.expect("a length");
        assert!((seconds - 4.0).abs() < 1.0e-6, "{seconds}");
        assert_eq!(format_duration(seconds), "0:04");

        // Half the tempo, twice the time.
        let slow = midi_metadata(&one_track_midi(480, 1_000_000, 8));
        assert!((slow.seconds.unwrap() - 8.0).abs() < 1.0e-6);
        assert_eq!(format_duration(slow.seconds.unwrap()), "0:08");
    }

    #[test]
    fn a_file_the_scanner_cannot_read_simply_has_no_metadata() {
        assert_eq!(midi_metadata(&[]), MidiMetadata::default());
        assert_eq!(midi_metadata(b"not a midi file at all"), MidiMetadata::default());
        // A truncated header must not panic or invent a length.
        let bytes = one_track_midi(480, 500_000, 4);
        for cut in [8, 14, 20, 24] {
            let _ = midi_metadata(&bytes[..cut.min(bytes.len())]);
        }
    }

    #[test]
    fn latin_1_text_becomes_the_letters_it_meant() {
        // "Für Elise" as the older collections wrote it.
        assert_eq!(decode_text(b"F\xfcr Elise"), "Für Elise");
        assert_eq!(decode_text("Für Elise".as_bytes()), "Für Elise");
    }

    #[test]
    fn file_names_become_a_composer_and_a_title() {
        assert_eq!(
            split_file_stem("beethoven-moonlight-1"),
            ("Beethoven".to_string(), "Moonlight 1".to_string())
        );
        assert_eq!(
            split_file_stem("chopin-fantaisie-impromptu"),
            ("Chopin".to_string(), "Fantaisie Impromptu".to_string())
        );
        // Someone else's folder, named however they like.
        assert_eq!(
            split_file_stem("My Own Piece"),
            (String::new(), "My Own Piece".to_string())
        );
    }

    #[test]
    fn an_uninformative_sequence_name_loses_to_the_file_name() {
        assert!(!is_informative("Piano"));
        assert!(!is_informative("Track 1"));
        assert!(!is_informative("  "));
        assert!(is_informative("Sonate D960, 1. Satz"));
    }

    /// A sequence name that names the *collection* — two Debussy files both
    /// calling themselves "Suite bergamasque" — tells the reader nothing about
    /// which piece a row is.
    #[test]
    fn a_title_two_files_share_falls_back_to_the_file_names() {
        let entry = |name: &str, title: &str| LibraryEntry {
            source: EntrySource::File,
            path: PathBuf::from(format!("/music/{name}.mid")),
            composer: "Debussy".to_string(),
            title: title.to_string(),
            from_name: name.to_string(),
            seconds: None,
        };
        let mut entries = vec![
            entry("Clair De Lune", "Suite bergamasque"),
            entry("Preludes", "Suite bergamasque"),
            entry("Estampes", "Estampes"),
        ];
        resolve_duplicate_titles(&mut entries);
        assert_eq!(entries[0].title, "Clair De Lune");
        assert_eq!(entries[1].title, "Preludes");
        // A title only one file claims is left exactly as the file gave it.
        assert_eq!(entries[2].title, "Estampes");
    }

    #[test]
    fn a_title_does_not_repeat_the_composer_or_run_off_the_row() {
        assert_eq!(
            drop_leading_composer("Chopin Nocturne Opus 27 Nr. 2", "Chopin"),
            "Nocturne Opus 27 Nr. 2"
        );
        assert_eq!(drop_leading_composer("Chopinesque", "Chopin"), "Chopinesque");
        assert_eq!(drop_leading_composer("Nocturne", ""), "Nocturne");
        let long = "a".repeat(200);
        let short = shorten(&long);
        assert_eq!(short.chars().count(), TITLE_LIMIT);
        assert!(short.ends_with('…'));
        assert_eq!(shorten("Für Elise"), "Für Elise");
    }

    /// A file opened by a relative path is the same file the library scanned
    /// by an absolute one; the "now playing" mark depends on knowing that.
    #[test]
    fn a_relative_path_still_matches_the_scanned_entry() {
        let cwd = std::env::current_dir().expect("a working directory");
        assert!(same_file(Path::new("Cargo.toml"), &cwd.join("Cargo.toml")));
        assert!(!same_file(Path::new("Cargo.toml"), &cwd.join("Cargo.lock")));
    }

    /// A missing folder is a normal state with a sentence, never an error the
    /// user has to decode.
    #[test]
    fn a_missing_folder_degrades_to_a_clear_empty_state() {
        let mut library = MusicLibrary::new(Some(Path::new("/nonexistent/score/folder")));
        library.ensure_scanned();
        assert!(library.entries().is_empty());
        let empty = library.empty_state().expect("an explanation");
        assert!(empty.contains("/nonexistent/score/folder"), "{empty}");
        assert_eq!(library.summary(), "Nothing to play here");
    }

    #[test]
    fn a_folder_of_scores_lists_them_sorted_with_a_line_each() {
        let dir = std::env::temp_dir().join(format!(
            "makepad-score-library-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        std::fs::write(dir.join("brahms-intermezzo.mid"), one_track_midi(480, 500_000, 8))
            .expect("a file");
        std::fs::write(dir.join("aaa-first.mid"), one_track_midi(480, 500_000, 4)).expect("a file");
        // Not a score: must not be listed.
        std::fs::write(dir.join("notes.txt"), b"hello").expect("a file");

        let mut library = MusicLibrary::new(Some(&dir));
        assert!(
            library.entries().is_empty(),
            "the folder is not read until it is browsed"
        );
        library.ensure_scanned();
        assert_eq!(library.entries().len(), 2);
        assert!(library.empty_state().is_none());
        assert_eq!(library.entries()[0].composer, "Aaa");
        assert_eq!(library.entries()[1].composer, "Brahms");
        // Both fixtures call themselves "Test Piece", so a name that says
        // nothing loses to the file name, which is unique by construction.
        assert_eq!(library.entries()[0].title, "First");
        assert_eq!(library.entries()[1].title, "Intermezzo");
        let line = library.entries()[1].line();
        assert!(line.starts_with("Brahms · Intermezzo"), "{line}");
        assert!(line.ends_with("0:04"), "{line}");
        assert_eq!(library.summary(), "2 pieces · 0:06 of music");
        assert_eq!(library.index_of(&dir.join("aaa-first.mid")), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
