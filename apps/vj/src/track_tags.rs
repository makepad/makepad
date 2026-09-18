//! What a library row can say about a track without analysing it.
//!
//! Artist, album, genre and year are not measurements. They are already in the
//! file, written there by whoever tagged it, and a listing that leaves the
//! ARTIST column blank until some background pass has decoded the audio is
//! answering a question nobody asked. This module is the cheap answer: open
//! the file, read its head, hand back the strings.
//!
//! Everything here is shaped for a listing of hundreds of records being filled
//! on the UI thread:
//!
//! - **the read is bounded** (see [`HEAD_BYTES`]) — never the whole file, and
//!   never an allocation sized straight from a header field;
//! - **the answer is total** — a file with no tags is an empty [`TrackTags`],
//!   which is a real answer worth caching, and distinct from the `None` that
//!   means the file would not open at all;
//! - **nothing is guessed** — an empty field means the file did not say. A
//!   cell shows blank rather than a plausible-looking invention, because an
//!   operator who sees a year in a column will believe it.
//!
//! The parsing itself belongs to `makepad_audio_decode`: its `read_tags`
//! already sniffs the container and reads ID3v2 text frames and Vorbis
//! comments into one shape. What is added here is the three things a library
//! row needs that a tag reader does not owe it — a bounded head read that
//! grows only when the file's own headers say it must, the container's
//! nominal bitrate, and the small normalisations (numeric genres, dates that
//! are not years) that turn a tag value into a cell.

use makepad_audio_decode::mp3::FrameHeader;
use makepad_audio_decode::{AudioFormat, Tags};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

// ---------------------------------------------------------------------------
// how much of a file this is willing to touch
// ---------------------------------------------------------------------------

/// First read, in bytes: 256 KiB.
///
/// The number is chosen against what actually sits at the head of a library
/// file rather than against a round figure. An ID3v2 tag with no cover art is
/// under 4 KiB; the common case is a tag carrying one embedded JPEG, which
/// taggers and phone rips write at anything from 20 KiB (a thumbnail) to
/// ~200 KiB (a 600x600 front cover). FLAC puts STREAMINFO, the comment block,
/// the seektable and any PICTURE block at the head too, in the same size band.
/// 256 KiB therefore answers the overwhelming majority of files in ONE read —
/// which is the property that matters, because a second read is a second seek
/// and the listing is doing this per row.
///
/// It is deliberately not larger. 256 KiB per track over a 600-record library
/// is 150 MB of sequential head reads; a megabyte-sized default would be four
/// times that for files that gain nothing from it.
const HEAD_BYTES: usize = 256 * 1024;

/// Ceiling on the head after top-ups, in bytes: 8 MiB.
///
/// A tag that declares more than this is a picture dump — a lossless scan of a
/// gatefold sleeve — and the text frames we want are either already inside the
/// 256 KiB we hold or are not worth eight megabytes of disk to reach. Growing
/// stops here and whatever was read is parsed as-is.
const MAX_HEAD_BYTES: usize = 8 * 1024 * 1024;

/// Times the head may be re-asked to grow. One covers the usual case (a tag
/// whose declared length overruns the first read); the rest cover a FLAC whose
/// metadata chain has more than one oversized block. A file that has not
/// converged by then is one whose headers disagree with themselves.
const MAX_TOP_UPS: usize = 4;

/// How far past the container's tag the MPEG frame-sync scan will look, in
/// bytes. A stream whose first frame is more than 64 KiB past its own tag is
/// not a stream we are reading the bitrate of; this is the same window the
/// decode crate's own format probe uses.
const SYNC_SCAN_BYTES: usize = 1 << 16;

/// Longest duration `bitrate_from_size` will divide by. Anything beyond a day
/// is not a track, it is a bad number arriving from somewhere upstream.
const MAX_DURATION_SECS: f64 = 24.0 * 60.0 * 60.0;

/// Highest rate `bitrate_from_size` will believe. Eight channels of 32-bit
/// 192 kHz PCM is under 50 Mbit/s, so nothing real lands above this; a value
/// that does came from a duration that is wrong rather than merely short.
const MAX_KBPS: f64 = 100_000.0;

/// Parenthesised genre codes read from the front of one value. Real files
/// carry one, occasionally two; the bound is here so a line of `(((((` costs
/// four iterations rather than a scan.
const MAX_GENRE_REFS: usize = 4;

/// Metadata blocks walked at the head of a FLAC. A real file has under ten;
/// the bound is what stops a head of zeroes, whose blocks are all four bytes
/// long and none of them last, from being walked a megabyte at a time.
const MAX_FLAC_BLOCKS: usize = 1024;

// ---------------------------------------------------------------------------
// the row
// ---------------------------------------------------------------------------

/// What a library row can say about a track without analysing it.
/// Every field is already display-ready: empty string means "the file
/// does not say", which a cell shows as blank rather than as a guess.
// Not `Eq`: the three tag hints below are floats and a key, and a tag
// that says 128.0 twice is the same tag either way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    /// Four digits when the file gives a full date; whatever it gave
    /// otherwise. Not parsed into a number — some files say "1998-03".
    pub year: String,
    /// Nominal container bitrate in kbit/s, when the container states
    /// one. `None` for formats that do not (and for variable-rate files
    /// whose header does not carry a nominal figure).
    pub bitrate_kbps: Option<u32>,
    /// What the file SAYS its tempo is. A claim, not a measurement: the
    /// tab never shows it in the tempo column, because that column is a
    /// report of what this machine measured and a tag is somebody else's
    /// word. It is worth having as a hint for the detector, which has to
    /// choose an octave and can be told which one somebody expected.
    pub tag_bpm: Option<f64>,
    /// What the file says its key is, read from the notations tags are
    /// written in. Same standing as the tempo: a hint, never a report.
    pub tag_key: Option<crate::track_key::KeyEstimate>,
    /// A replay-gain figure in decibels, when the file carries one. This
    /// one IS usable directly: it is a level match somebody already
    /// measured, and it is a better first answer than a broadband average
    /// of the samples.
    pub tag_gain_db: Option<f64>,
}

impl TrackTags {
    /// True when the file said nothing at all worth showing.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.artist.is_empty()
            && self.album.is_empty()
            && self.genre.is_empty()
            && self.year.is_empty()
            && self.bitrate_kbps.is_none()
    }

    /// `key value` per line, one line per field, values on one line each.
    ///
    /// Tags are read from the file itself, which for a store track means the
    /// one moment its bytes are on this machine. That moment does not come
    /// again — the second session finds the analysis cached and never
    /// fetches the record — so what was read has to be written down or the
    /// column goes blank on the next launch.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (key, value) in [
            ("title", &self.title),
            ("artist", &self.artist),
            ("album", &self.album),
            ("genre", &self.genre),
            ("year", &self.year),
        ] {
            // A newline inside a tag would forge a second field; the reader
            // is line-based and the writer has to keep it that way.
            let value = value.replace(['\n', '\r'], " ");
            if !value.trim().is_empty() {
                out.push_str(&format!("{key} {}\n", value.trim()));
            }
        }
        if let Some(kbps) = self.bitrate_kbps {
            out.push_str(&format!("bitrate {kbps}\n"));
        }
        // The three hints ride along for the same reason everything else
        // does: a store track's bytes are here once, and a claim nobody
        // wrote down is a claim nobody can use next session.
        if let Some(bpm) = self.tag_bpm {
            out.push_str(&format!("tag_bpm {bpm}\n"));
        }
        if let Some(key) = self.tag_key {
            out.push_str(&format!("tag_key {}\n", key.camelot()));
        }
        if let Some(db) = self.tag_gain_db {
            out.push_str(&format!("tag_gain {db}\n"));
        }
        out
    }

    pub fn from_text(text: &str) -> TrackTags {
        let mut out = TrackTags::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim().to_string();
            match key {
                "title" => out.title = value,
                "artist" => out.artist = value,
                "album" => out.album = value,
                "genre" => out.genre = value,
                "year" => out.year = value,
                "bitrate" => out.bitrate_kbps = value.parse().ok(),
                "tag_bpm" => {
                    out.tag_bpm = value.parse().ok().filter(|bpm: &f64| {
                        bpm.is_finite() && (40.0..=300.0).contains(bpm)
                    })
                }
                "tag_key" => out.tag_key = crate::track_key::parse_key(&value),
                "tag_gain" => {
                    out.tag_gain_db = value
                        .parse()
                        .ok()
                        .filter(|db: &f64| db.is_finite() && db.abs() <= 40.0)
                }
                _ => {}
            }
        }
        out
    }
}

/// Where one track's tags are kept, beside its analysis sidecar and keyed the
/// same way, so clearing the cache clears both.
pub fn sidecar_path(dir: &std::path::Path, key: &str) -> std::path::PathBuf {
    dir.join(format!("{key}.tags"))
}

pub fn save_sidecar(dir: &std::path::Path, key: &str, tags: &TrackTags) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = sidecar_path(dir, key);
    let temporary = path.with_extension("tags.tmp");
    if std::fs::write(&temporary, tags.to_text()).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

/// An empty file is a real answer — "this record carries no tags" — and is
/// worth keeping, so a missing FILE is the only `None`.
pub fn load_sidecar(dir: &std::path::Path, key: &str) -> Option<TrackTags> {
    let body = std::fs::read_to_string(sidecar_path(dir, key)).ok()?;
    Some(TrackTags::from_text(&body))
}

/// Read one file's metadata.
///
/// Returns `None` only when the file cannot be opened at all; a file with
/// no tags returns an empty `TrackTags`, which is a different answer and
/// is worth caching as one.
pub fn read_for_path(path: &Path) -> Option<TrackTags> {
    Some(read_from_bytes(&read_head(path)?))
}

/// Read metadata from bytes already in hand.
pub fn read_from_bytes(bytes: &[u8]) -> TrackTags {
    // A head that will not parse is a file that said nothing, not a file that
    // failed. The distinction matters one level up: a blank row that is known
    // to be blank never gets re-read, and a listing that retried every
    // untagged file on every scroll would spend the whole show on disk.
    let tags = makepad_audio_decode::read_tags(bytes).unwrap_or_default();
    TrackTags {
        title: tags.title.clone().unwrap_or_default(),
        artist: artist_of(&tags),
        album: tags.album.clone().unwrap_or_default(),
        genre: genre_of(&tags),
        year: year_of(&tags),
        bitrate_kbps: nominal_bitrate(bytes),
        tag_bpm: tag_bpm(&tags),
        tag_key: tag_key(&tags),
        tag_gain_db: tag_gain_db(&tags),
    }
}

/// A tempo a file claims, if it claims a plausible one.
///
/// Files say "128", "128.00", and occasionally "128,5" with a decimal
/// comma. Anything outside the band a record can be at is a tag somebody
/// typed wrong, and a wrong hint is worse than none.
fn tag_bpm(tags: &makepad_audio_decode::Tags) -> Option<f64> {
    let raw = find_tag(tags, &["TBPM", "BPM", "TEMPO"])?;
    let bpm: f64 = raw.trim().replace(',', ".").parse().ok()?;
    (bpm.is_finite() && (40.0..=300.0).contains(&bpm)).then_some(bpm)
}

/// A key a file claims, in whichever of the three notations it used.
fn tag_key(tags: &makepad_audio_decode::Tags) -> Option<crate::track_key::KeyEstimate> {
    let raw = find_tag(tags, &["TKEY", "INITIALKEY", "INITIAL KEY", "KEY"])?;
    crate::track_key::parse_key(&raw)
}

/// A replay-gain figure, in decibels. Files write it as "-7.23 dB".
fn tag_gain_db(tags: &makepad_audio_decode::Tags) -> Option<f64> {
    let raw = find_tag(
        tags,
        &["REPLAYGAIN_TRACK_GAIN", "REPLAYGAIN_ALBUM_GAIN", "R128_TRACK_GAIN"],
    )?;
    let cleaned = raw.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ' ');
    let db: f64 = cleaned.trim().parse().ok()?;
    // R128 tags are in Q7.8 fixed point relative to -23 LUFS, not in
    // decibels; a value that large is one of those, and reading it as
    // decibels would ask for a gain of several thousand.
    let db = match raw.to_ascii_uppercase().starts_with("R128") || db.abs() > 60.0 {
        true => db / 256.0,
        false => db,
    };
    (db.is_finite() && db.abs() <= 40.0).then_some(db)
}

/// The first of these keys the file carried, by upper-cased name.
fn find_tag<'a>(
    tags: &'a makepad_audio_decode::Tags,
    names: &[&str],
) -> Option<&'a str> {
    for name in names {
        if let Some((_, value)) = tags.all.iter().find(|(key, _)| key == name) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Bitrate from a file's size and a known duration, for containers whose
/// header states no nominal rate. Returns None for a nonsense duration.
///
/// This counts the tags and the cover art as if they were audio, so it reads a
/// few kbit/s high on a heavily tagged file. That is the right trade for the
/// column it fills: the operator is comparing a 320 against a 128, not
/// auditing an encoder.
pub fn bitrate_from_size(bytes: u64, duration_secs: f64) -> Option<u32> {
    if !duration_secs.is_finite() || duration_secs <= 0.0 || duration_secs > MAX_DURATION_SECS {
        return None;
    }
    let kbps = bytes as f64 * 8.0 / duration_secs / 1000.0;
    if !kbps.is_finite() || kbps < 1.0 || kbps > MAX_KBPS {
        return None;
    }
    Some(kbps.round() as u32)
}

// ---------------------------------------------------------------------------
// fields
// ---------------------------------------------------------------------------

/// The performer, falling back to the band and album-artist frames.
///
/// This fallback is most of why the column was blank. A rip that carries only
/// TPE2 (the band) or a Vorbis comment spelled `ALBUM ARTIST` with a space in
/// it has an artist — it just does not have one under the four names the tag
/// reader routes into its `artist` slot. Showing the band beats showing
/// nothing, and neither is a guess: both are what the file says.
fn artist_of(tags: &Tags) -> String {
    if let Some(artist) = tags.artist.as_deref() {
        return artist.to_string();
    }
    for key in ["TPE2", "TP2", "ALBUM ARTIST", "PERFORMER"] {
        if let Some(value) = tags.get(key).filter(|value| !value.is_empty()) {
            return value.to_string();
        }
    }
    String::new()
}

/// The release year, from the release date and then the frames that carry a
/// date under another name.
fn year_of(tags: &Tags) -> String {
    let raw = tags
        .date
        .as_deref()
        .or_else(|| tags.get("TDRL"))
        .or_else(|| tags.get("TDOR"))
        .or_else(|| tags.get("ORIGINALDATE"))
        .unwrap_or("");
    normalize_year(raw)
}

/// A date reduced to a year, or left alone.
///
/// Files say "1998", "1998-03-21", "1998/03", "March 1998" and worse. Leading
/// digits that could be a year become the year; anything else is passed
/// through trimmed, on the rule that showing the operator exactly what the
/// file claims is better than showing them a year the file never gave. Two
/// digits stay two digits: "98" is 1998 or 2098 and the file did not say.
fn normalize_year(raw: &str) -> String {
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    if bytes.len() >= 4 && bytes[..4].iter().all(|b| b.is_ascii_digit()) {
        let head = &raw[..4];
        if let Ok(year) = head.parse::<u32>() {
            if (1000..=2999).contains(&year) {
                return head.to_string();
            }
        }
    }
    raw.to_string()
}

/// The genre, with ID3v1 numbers resolved to names.
fn genre_of(tags: &Tags) -> String {
    resolve_genre(tags.genre.as_deref().unwrap_or(""))
}

/// Turn a genre value into something a cell can show.
///
/// The decode crate hands genre through verbatim and does not claim otherwise
/// — it has no number table, by design, because a decoder has no business
/// holding one. So the table lives here. It matters more than it looks: an
/// ID3v2 file written by an ID3v1-era tagger stores "(17)" rather than "Rock",
/// and a library that shows "(17)" in the GENRE column is showing the operator
/// a fact they cannot sort by.
///
/// A refinement typed after the codes wins over the codes: "(17)Southern
/// Rock" means the tagger picked a number and then said what they actually
/// meant.
fn resolve_genre(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    // "((" is how ID3v2 escapes a free-form name that really does start with
    // a parenthesis.
    if let Some(rest) = raw.strip_prefix("((") {
        return format!("({rest}");
    }
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        return genre_name(raw).unwrap_or(raw).to_string();
    }
    let mut rest = raw;
    let mut named: Vec<&str> = Vec::new();
    for _ in 0..MAX_GENRE_REFS {
        let Some(body) = rest.strip_prefix('(') else { break };
        let Some(close) = body.find(')') else { break };
        let code = body[..close].trim();
        rest = body[close + 1..].trim();
        match genre_name(code) {
            Some(name) => named.push(name),
            // A code with no name is not a genre, but it is what the file
            // said, so it survives rather than being silently dropped.
            None if !code.is_empty() => named.push(code),
            None => {}
        }
    }
    if !rest.is_empty() {
        return rest.to_string();
    }
    if named.is_empty() {
        return raw.to_string();
    }
    named.join(", ")
}

fn genre_name(code: &str) -> Option<&'static str> {
    match code {
        "RX" | "rx" => return Some("Remix"),
        "CR" | "cr" => return Some("Cover"),
        _ => {}
    }
    ID3V1_GENRES.get(code.parse::<usize>().ok()?).copied()
}

/// The numeric genre list ID3v1 froze at eighty entries, plus the player
/// extensions that everyone's tagger shipped afterwards. Index is the code.
///
/// Entry 133 is given here under the name it is catalogued by today rather
/// than the slur the original list used.
const ID3V1_GENRES: [&str; 192] = [
    "Blues", "Classic Rock", "Country", "Dance", "Disco", "Funk", "Grunge",
    "Hip-Hop", "Jazz", "Metal", "New Age", "Oldies", "Other", "Pop", "R&B",
    "Rap", "Reggae", "Rock", "Techno", "Industrial", "Alternative", "Ska",
    "Death Metal", "Pranks", "Soundtrack", "Euro-Techno", "Ambient",
    "Trip-Hop", "Vocal", "Jazz+Funk", "Fusion", "Trance", "Classical",
    "Instrumental", "Acid", "House", "Game", "Sound Clip", "Gospel", "Noise",
    "Alternative Rock", "Bass", "Soul", "Punk", "Space", "Meditative",
    "Instrumental Pop", "Instrumental Rock", "Ethnic", "Gothic", "Darkwave",
    "Techno-Industrial", "Electronic", "Pop-Folk", "Eurodance", "Dream",
    "Southern Rock", "Comedy", "Cult", "Gangsta", "Top 40", "Christian Rap",
    "Pop/Funk", "Jungle", "Native American", "Cabaret", "New Wave",
    "Psychadelic", "Rave", "Showtunes", "Trailer", "Lo-Fi", "Tribal",
    "Acid Punk", "Acid Jazz", "Polka", "Retro", "Musical", "Rock & Roll",
    "Hard Rock", "Folk", "Folk-Rock", "National Folk", "Swing", "Fast Fusion",
    "Bebob", "Latin", "Revival", "Celtic", "Bluegrass", "Avantgarde",
    "Gothic Rock", "Progressive Rock", "Psychedelic Rock", "Symphonic Rock",
    "Slow Rock", "Big Band", "Chorus", "Easy Listening", "Acoustic", "Humour",
    "Speech", "Chanson", "Opera", "Chamber Music", "Sonata", "Symphony",
    "Booty Bass", "Primus", "Porn Groove", "Satire", "Slow Jam", "Club",
    "Tango", "Samba", "Folklore", "Ballad", "Power Ballad", "Rhythmic Soul",
    "Freestyle", "Duet", "Punk Rock", "Drum Solo", "A Cappella", "Euro-House",
    "Dance Hall", "Goa", "Drum & Bass", "Club-House", "Hardcore", "Terror",
    "Indie", "BritPop", "Afro-Punk", "Polsk Punk", "Beat",
    "Christian Gangsta Rap", "Heavy Metal", "Black Metal", "Crossover",
    "Contemporary Christian", "Christian Rock", "Merengue", "Salsa",
    "Thrash Metal", "Anime", "JPop", "Synthpop", "Abstract", "Art Rock",
    "Baroque", "Bhangra", "Big Beat", "Breakbeat", "Chillout", "Downtempo",
    "Dub", "EBM", "Eclectic", "Electro", "Electroclash", "Emo",
    "Experimental", "Garage", "Global", "IDM", "Illbient", "Industro-Goth",
    "Jam Band", "Krautrock", "Leftfield", "Lounge", "Math Rock",
    "New Romantic", "Nu-Breakz", "Post-Punk", "Post-Rock", "Psytrance",
    "Shoegaze", "Space Rock", "Trop Rock", "World Music", "Neoclassical",
    "Audiobook", "Audio Theatre", "Neue Deutsche Welle", "Podcast",
    "Indie Rock", "G-Funk", "Dubstep", "Garage Rock", "Psybient",
];

// ---------------------------------------------------------------------------
// bitrate
// ---------------------------------------------------------------------------

/// The rate the container states, in kbit/s.
///
/// MP3 is the only one of the three formats read here whose container carries
/// a rate at all: it is in every frame header. FLAC and Ogg Vorbis state a
/// sample rate and a channel count and nothing about the compressed size, so
/// they answer `None` and the caller reaches for [`bitrate_from_size`] with
/// the duration it already has.
///
/// The header word is handed to the decode crate's own `FrameHeader::parse`
/// rather than being taken apart here, so this agrees with the decoder about
/// what a valid frame is — including the free-format and reserved encodings
/// it refuses, which have no derivable rate to report.
fn nominal_bitrate(bytes: &[u8]) -> Option<u32> {
    if makepad_audio_decode::sniff(bytes) != Some(AudioFormat::Mp3) {
        return None;
    }
    let start = declared_id3v2_len(bytes).min(bytes.len());
    let end = start.saturating_add(SYNC_SCAN_BYTES).min(bytes.len());
    let window = bytes.get(start..end)?;
    let (at, header) = first_frame(window)?;
    if declares_variable_rate(window.get(at..)?, &header) {
        // The stream says outright that its rate moves. This frame's own
        // figure is the size of THIS frame and describes nothing else, so
        // reporting it would put a number in the column that is true of one
        // twenty-sixth of a second of the track.
        return None;
    }
    Some(header.bitrate_kbps)
}

/// First confirmed MPEG frame in `window`, and where it starts.
///
/// The confirmation rule is the decode crate's: a header only counts when a
/// second, compatible header sits exactly one frame later, or when the frame
/// runs to the end of what we hold and there is nothing left to confirm
/// against. Without it, any three bytes of audio that happen to contain a
/// sync pattern would report a bitrate, and MP3 payload is full of them.
fn first_frame(window: &[u8]) -> Option<(usize, FrameHeader)> {
    let mut at = 0usize;
    while at + 4 <= window.len() {
        at += window[at..].iter().position(|&b| b == 0xff)?;
        let word = window.get(at..at + 4)?;
        let word = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        if let Some(header) = FrameHeader::parse(word) {
            let next = at.saturating_add(header.frame_bytes);
            let confirmed = match window.get(next..next + 4) {
                Some(second) => {
                    let second =
                        u32::from_be_bytes([second[0], second[1], second[2], second[3]]);
                    FrameHeader::parse(second).is_some_and(|s| header.compatible_with(&s))
                }
                None => next <= window.len(),
            };
            if confirmed {
                return Some((at, header));
            }
        }
        at += 1;
    }
    None
}

/// True when the stream's first frame carries a header declaring that the
/// bitrate varies.
///
/// The two spellings of that header differ by exactly this: one is written by
/// an encoder that varied the rate, the other by an encoder that did not and
/// wanted the frame count anyway. So the second is a positive statement that
/// the frame header's figure holds for the whole file, not an absence of one.
fn declares_variable_rate(frame: &[u8], header: &FrameHeader) -> bool {
    let at = 4 + usize::from(header.protected) * 2 + header.side_info_bytes();
    if let Some(tag) = frame.get(at..at.saturating_add(4)) {
        if tag == b"Xing" {
            return true;
        }
        if tag == b"Info" {
            return false;
        }
    }
    frame.get(36..40) == Some(&b"VBRI"[..])
}

// ---------------------------------------------------------------------------
// the bounded head read
// ---------------------------------------------------------------------------

/// Enough of `path` for a tag read, and no more.
///
/// One open, one buffer, and a top-up only when the bytes already in hand
/// declare that the metadata runs past them. `None` means the file would not
/// give up a single byte — it is gone, or it is a directory, or the volume
/// said no. An empty file is `Some(empty)`: it opened, and it said nothing.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    // The length is a hint, used only to avoid reserving 256 KiB for a 3 KiB
    // file. A stale or lying one costs nothing, because every fill stops at
    // the real end of the stream regardless of what the hint said.
    let cap = file
        .metadata()
        .ok()
        .map(|meta| meta.len().min(MAX_HEAD_BYTES as u64) as usize)
        .unwrap_or(MAX_HEAD_BYTES);
    let mut head: Vec<u8> = Vec::new();
    let mut at_end = match fill_to(&mut file, &mut head, HEAD_BYTES.min(cap)) {
        Ok(at_end) => at_end,
        // A read that failed after handing back some bytes still leaves a head
        // worth parsing; one that failed with nothing is not a file.
        Err(_) if !head.is_empty() => true,
        Err(_) => return None,
    };
    for _ in 0..MAX_TOP_UPS {
        if at_end {
            break;
        }
        let want = declared_head_len(&head).min(MAX_HEAD_BYTES).min(cap);
        if want <= head.len() {
            break;
        }
        match fill_to(&mut file, &mut head, want) {
            Ok(more) => at_end = more,
            Err(_) => break,
        }
    }
    Some(head)
}

/// Grow `buf` to `want` bytes from `file`. `Ok(true)` means the stream ended
/// inside this read, so asking for more would only cost another syscall.
fn fill_to(file: &mut File, buf: &mut Vec<u8>, want: usize) -> std::io::Result<bool> {
    let start = buf.len();
    if want <= start {
        return Ok(false);
    }
    buf.resize(want, 0);
    let mut at = start;
    while at < want {
        match file.read(&mut buf[at..]) {
            Ok(0) => {
                buf.truncate(at);
                return Ok(true);
            }
            Ok(read) => at += read,
            Err(ref err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => {
                buf.truncate(at);
                return Err(err);
            }
        }
    }
    Ok(false)
}

/// How long the head would have to be for the metadata in it to be complete,
/// according to the metadata itself.
///
/// This asks the headers, never the caller. An ID3v2 tag declares its own
/// total length in its first ten bytes; a FLAC metadata chain declares each
/// block's length in that block's first four. Either can legitimately overrun
/// the first read, and both do so for the same reason: someone embedded a
/// cover.
fn declared_head_len(bytes: &[u8]) -> usize {
    let after_tag = declared_id3v2_len(bytes);
    if bytes.get(after_tag..after_tag.saturating_add(4)) == Some(&b"fLaC"[..]) {
        return flac_metadata_end(bytes, after_tag.saturating_add(4));
    }
    // Either an MPEG stream, an Ogg stream, or a tag whose end is not yet in
    // hand. All three want the tag plus room for the first frame.
    after_tag.saturating_add(SYNC_SCAN_BYTES)
}

/// Total length of an ID3v2 tag at the head of `bytes`, as the tag DECLARES
/// it — not clamped to what has been read, because finding out whether we have
/// read enough is the entire point of asking.
///
/// The decode crate has this arithmetic too, and keeps it private; a caller
/// that wanted the clamped answer could get it from the tags themselves. Nine
/// lines of syncsafe decoding is the price of not editing that crate to widen
/// its surface for one reader.
fn declared_id3v2_len(bytes: &[u8]) -> usize {
    if bytes.len() < 10 || &bytes[0..3] != b"ID3" || bytes[3] == 0xff || bytes[4] == 0xff {
        return 0;
    }
    // Syncsafe: seven bits per byte, so the size can never contain a false
    // frame sync. A byte with its top bit set is not this field.
    let mut size = 0usize;
    for &b in &bytes[6..10] {
        if b & 0x80 != 0 {
            return 0;
        }
        size = (size << 7) | (b & 0x7f) as usize;
    }
    let footer = usize::from(bytes[5] & 0x10 != 0) * 10;
    10 + size + footer
}

/// Where the FLAC metadata chain starting at `at` ends, as far as the bytes in
/// hand can say.
///
/// A block whose declared body runs past the buffer stops the walk at that
/// body's end — which is exactly the number the caller needs to top up to, and
/// the next walk gets further. Nothing here is parsed: only skipped by length.
fn flac_metadata_end(bytes: &[u8], mut at: usize) -> usize {
    for _ in 0..MAX_FLAC_BLOCKS {
        let Some(block) = bytes.get(at..at.saturating_add(4)) else {
            return at.saturating_add(4);
        };
        let last = block[0] & 0x80 != 0;
        let len = ((block[1] as usize) << 16) | ((block[2] as usize) << 8) | block[3] as usize;
        at = at.saturating_add(4).saturating_add(len);
        if last {
            break;
        }
    }
    at
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// What a file CLAIMS about itself: read, bounded, and never confused
    /// with a measurement.
    #[test]
    fn a_files_own_claims_are_read_and_bounded() {
        let tags = |pairs: &[(&str, &str)]| {
            let mut tags = makepad_audio_decode::Tags::default();
            for (key, value) in pairs {
                tags.push(key, value);
            }
            tags
        };
        assert_eq!(tag_bpm(&tags(&[("TBPM", "128")])), Some(128.0));
        assert_eq!(tag_bpm(&tags(&[("BPM", "128.50")])), Some(128.5));
        // A decimal comma is how half the world writes it.
        assert_eq!(tag_bpm(&tags(&[("TBPM", "128,5")])), Some(128.5));
        // And a tempo no record has is a typo, not a hint.
        assert_eq!(tag_bpm(&tags(&[("TBPM", "0")])), None);
        assert_eq!(tag_bpm(&tags(&[("TBPM", "9999")])), None);
        assert_eq!(tag_bpm(&tags(&[("TBPM", "fast")])), None);
        assert_eq!(tag_bpm(&tags(&[])), None);

        assert_eq!(
            tag_key(&tags(&[("TKEY", "Am")])).map(|k| k.camelot()),
            Some("8A".to_string())
        );
        assert_eq!(
            tag_key(&tags(&[("INITIALKEY", "8A")])).map(|k| k.camelot()),
            Some("8A".to_string())
        );
        assert!(tag_key(&tags(&[("TKEY", "H7")])).is_none());

        // Replay gain, with and without its unit.
        assert_eq!(tag_gain_db(&tags(&[("REPLAYGAIN_TRACK_GAIN", "-7.23 dB")])), Some(-7.23));
        assert_eq!(tag_gain_db(&tags(&[("REPLAYGAIN_TRACK_GAIN", "+2.5")])), Some(2.5));
        // The R128 tag is fixed point against -23 LUFS, not decibels: read
        // as decibels it would ask for a gain of several thousand.
        let r128 = tag_gain_db(&tags(&[("R128_TRACK_GAIN", "-1280")])).expect("a figure");
        assert!((r128 + 5.0).abs() < 1e-9, "{r128}");
        assert!(tag_gain_db(&tags(&[("REPLAYGAIN_TRACK_GAIN", "loud")])).is_none());
    }

    /// A claim written down comes back the same, and one that cannot mean
    /// anything does not come back at all.
    #[test]
    fn the_claims_survive_the_sidecar() {
        let mine = TrackTags {
            title: "Bike".into(),
            tag_bpm: Some(128.5),
            tag_key: crate::track_key::parse_key("Am"),
            tag_gain_db: Some(-7.25),
            ..Default::default()
        };
        let back = TrackTags::from_text(&mine.to_text());
        assert_eq!(back.tag_bpm, Some(128.5));
        assert_eq!(back.tag_key.map(|k| k.camelot()), Some("8A".to_string()));
        assert_eq!(back.tag_gain_db, Some(-7.25));
        // And a hand-edited file that says something impossible says nothing.
        let broken = TrackTags::from_text("tag_bpm 9999
tag_gain 500
tag_key H
");
        assert_eq!(broken.tag_bpm, None);
        assert_eq!(broken.tag_gain_db, None);
        assert!(broken.tag_key.is_none());
    }

    use std::path::PathBuf;

    /// MPEG-1 Layer III, 44.1 kHz, 128 kbit/s, joint stereo, no CRC — the
    /// header word the decode crate's own tests use.
    const MPEG1_128: u32 = 0xfffb_9064;
    /// 144 * 128000 / 44100, unpadded.
    const MPEG1_128_BYTES: usize = 417;

    /// An ID3v2.3 tag. `declared` overrides the size field so a test can make
    /// the tag claim more than it holds; text is Latin-1, so keep it ASCII.
    fn id3v2_tag(frames: &[(&str, &str)], declared: Option<usize>) -> Vec<u8> {
        let mut body: Vec<u8> = Vec::new();
        for (id, text) in frames {
            let mut payload = vec![0u8];
            payload.extend_from_slice(text.as_bytes());
            body.extend_from_slice(id.as_bytes());
            body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(&payload);
        }
        let size = declared.unwrap_or(body.len());
        let mut out = vec![0u8; 10];
        out[0..3].copy_from_slice(b"ID3");
        out[3] = 3;
        out[6] = ((size >> 21) & 0x7f) as u8;
        out[7] = ((size >> 14) & 0x7f) as u8;
        out[8] = ((size >> 7) & 0x7f) as u8;
        out[9] = (size & 0x7f) as u8;
        out.extend_from_slice(&body);
        out
    }

    /// One ID3v2.3 frame with a body of `len` zero bytes, under an id the tag
    /// reader skips by length. Stands in for an embedded cover.
    fn filler_frame(id: &str, len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id.as_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.resize(out.len() + len, 0);
        out
    }

    /// `count` back-to-back frames, zero-filled after each header.
    fn mpeg_frames(count: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..count {
            out.extend_from_slice(&MPEG1_128.to_be_bytes());
            out.resize(out.len() + MPEG1_128_BYTES - 4, 0);
        }
        out
    }

    fn tagged_mp3(frames: &[(&str, &str)]) -> Vec<u8> {
        let mut file = id3v2_tag(frames, None);
        file.extend_from_slice(&mpeg_frames(3));
        file
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("vj-track-tags-{label}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn a_tag_and_a_frame_header_fill_every_column() {
        let file = tagged_mp3(&[
            ("TIT2", "Windowlicker"),
            ("TPE1", "Aphex Twin"),
            ("TALB", "Windowlicker"),
            ("TYER", "1999"),
            ("TCON", "Electronic"),
        ]);
        let tags = read_from_bytes(&file);
        assert_eq!(tags.title, "Windowlicker");
        assert_eq!(tags.artist, "Aphex Twin");
        assert_eq!(tags.album, "Windowlicker");
        assert_eq!(tags.year, "1999");
        assert_eq!(tags.genre, "Electronic");
        assert_eq!(tags.bitrate_kbps, Some(128));
        assert!(!tags.is_empty());
    }

    #[test]
    fn a_tag_that_declares_more_than_it_holds_invents_nothing() {
        let mut file = id3v2_tag(&[("TPE1", "Aphex Twin")], Some(4_000_000));
        file.extend_from_slice(&mpeg_frames(3));
        let tags = read_from_bytes(&file);
        // The tag reader clamps to what is there, so the one frame that IS in
        // the buffer still lands; nothing beyond it is imagined.
        assert_eq!(tags.artist, "Aphex Twin");
        assert_eq!(tags.title, "");
        assert_eq!(tags.album, "");
        assert_eq!(tags.year, "");
        // The declared length swallows the audio, so there is no frame header
        // to be found and no rate to report — which is a blank cell, not a
        // wrong one.
        assert_eq!(tags.bitrate_kbps, None);
    }

    #[test]
    fn a_file_with_no_tags_is_empty_rather_than_a_failure() {
        let tags = read_from_bytes(b"this is not audio and never was, at any length");
        assert_eq!(tags, TrackTags::default());
        assert!(tags.is_empty());
    }

    #[test]
    fn an_untagged_stream_still_has_a_bitrate_to_show() {
        let tags = read_from_bytes(&mpeg_frames(3));
        assert_eq!(tags.artist, "");
        assert_eq!(tags.bitrate_kbps, Some(128));
        // A row with a rate in it is not a row that said nothing.
        assert!(!tags.is_empty());
    }

    #[test]
    fn a_year_comes_off_the_front_of_a_date_and_is_never_invented() {
        assert_eq!(normalize_year("1998"), "1998");
        assert_eq!(normalize_year("1998-03-21"), "1998");
        assert_eq!(normalize_year("1998/03"), "1998");
        assert_eq!(normalize_year("  2024-01-01T00:00:00Z  "), "2024");
        // Two digits are 1998 or 2098 and the file did not say which.
        assert_eq!(normalize_year("98"), "98");
        assert_eq!(normalize_year("  banana  "), "banana");
        assert_eq!(normalize_year("March 1998"), "March 1998");
        // Four digits that are not a year stay as written.
        assert_eq!(normalize_year("0000"), "0000");
        assert_eq!(normalize_year("9999"), "9999");
        assert_eq!(normalize_year(""), "");
        assert_eq!(normalize_year("\u{e9}t\u{e9} 1998"), "\u{e9}t\u{e9} 1998");
    }

    #[test]
    fn a_date_frame_under_another_name_still_reaches_the_year() {
        let tags = read_from_bytes(&tagged_mp3(&[("TDRL", "2007-06")]));
        assert_eq!(tags.year, "2007");
    }

    #[test]
    fn a_numeric_genre_resolves_to_the_name_it_stands_for() {
        assert_eq!(resolve_genre("17"), "Rock");
        assert_eq!(resolve_genre("(17)"), "Rock");
        assert_eq!(resolve_genre(" (17) "), "Rock");
        assert_eq!(resolve_genre("(0)"), "Blues");
        assert_eq!(resolve_genre("(191)"), "Psybient");
        assert_eq!(resolve_genre("(4)(13)"), "Disco, Pop");
        // A typed refinement beats the number it was filed under.
        assert_eq!(resolve_genre("(17)Southern Rock"), "Southern Rock");
        assert_eq!(resolve_genre("(RX)"), "Remix");
        assert_eq!(resolve_genre("(CR)"), "Cover");
        // Out of range, unopened, unclosed and escaped: all pass through.
        assert_eq!(resolve_genre("(255)"), "255");
        assert_eq!(resolve_genre("999"), "999");
        assert_eq!(resolve_genre("Drum & Bass"), "Drum & Bass");
        assert_eq!(resolve_genre("(unclosed"), "(unclosed");
        assert_eq!(resolve_genre("((Parens)"), "(Parens)");
        assert_eq!(resolve_genre("(((("), "(((");
        assert_eq!(resolve_genre("   "), "");
    }

    #[test]
    fn a_numeric_genre_in_a_real_tag_reaches_the_column() {
        let tags = read_from_bytes(&tagged_mp3(&[("TCON", "(17)")]));
        assert_eq!(tags.genre, "Rock");
    }

    #[test]
    fn the_band_frame_stands_in_when_the_lead_performer_is_absent() {
        let tags = read_from_bytes(&tagged_mp3(&[("TIT2", "Bike"), ("TPE2", "Pink Floyd")]));
        assert_eq!(tags.artist, "Pink Floyd");
        // The lead performer still wins when the file carries both.
        let tags = read_from_bytes(&tagged_mp3(&[("TPE1", "Syd Barrett"), ("TPE2", "Pink Floyd")]));
        assert_eq!(tags.artist, "Syd Barrett");
    }

    #[test]
    fn a_stream_that_declares_a_moving_rate_reports_no_nominal_one() {
        // The tag sits at 4 + 32 bytes of side info into the first frame.
        let mut varying = mpeg_frames(3);
        varying[36..40].copy_from_slice(b"Xing");
        assert_eq!(read_from_bytes(&varying).bitrate_kbps, None);

        let mut fixed = mpeg_frames(3);
        fixed[36..40].copy_from_slice(b"Info");
        assert_eq!(read_from_bytes(&fixed).bitrate_kbps, Some(128));

        let mut vbri = mpeg_frames(3);
        vbri[36..40].copy_from_slice(b"VBRI");
        assert_eq!(read_from_bytes(&vbri).bitrate_kbps, None);
    }

    #[test]
    fn a_lone_sync_word_inside_noise_is_not_a_bitrate() {
        let mut noise = vec![0x11u8; 4096];
        noise[100..104].copy_from_slice(&MPEG1_128.to_be_bytes());
        assert_eq!(nominal_bitrate(&noise), None);
    }

    #[test]
    fn a_bitrate_from_size_needs_a_duration_that_could_be_a_track() {
        // 320 kbit/s for exactly one minute.
        assert_eq!(bitrate_from_size(2_400_000, 60.0), Some(320));
        // One second of CD-rate stereo PCM, to catch a factor-of-eight slip.
        assert_eq!(bitrate_from_size(176_400, 1.0), Some(1411));
        assert_eq!(bitrate_from_size(0, 60.0), None);
        assert_eq!(bitrate_from_size(5_000_000, 0.0), None);
        assert_eq!(bitrate_from_size(5_000_000, -1.0), None);
        assert_eq!(bitrate_from_size(5_000_000, f64::NAN), None);
        assert_eq!(bitrate_from_size(5_000_000, f64::INFINITY), None);
        assert_eq!(bitrate_from_size(5_000_000, f64::NEG_INFINITY), None);
        // A day and a half is not a track, and a millisecond is not a duration
        // that yields a believable rate.
        assert_eq!(bitrate_from_size(5_000_000, 130_000.0), None);
        assert_eq!(bitrate_from_size(5_000_000, 0.001), None);
        assert_eq!(bitrate_from_size(u64::MAX, 60.0), None);
    }

    #[test]
    fn a_missing_path_is_none_and_a_real_one_is_the_fields() {
        let dir = scratch_dir("paths");
        let missing = dir.join("no-such-track.mp3");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(read_for_path(&missing), None);

        let path = dir.join("tagged.mp3");
        let file = tagged_mp3(&[("TIT2", "Bike"), ("TPE1", "Pink Floyd"), ("TYER", "1967-08")]);
        std::fs::write(&path, &file).expect("write the test track");
        let tags = read_for_path(&path).expect("a file that exists reads");
        assert_eq!(tags.title, "Bike");
        assert_eq!(tags.artist, "Pink Floyd");
        assert_eq!(tags.year, "1967");
        assert_eq!(tags.bitrate_kbps, Some(128));

        // An empty file opened fine and said nothing: that is an answer.
        let empty = dir.join("empty.mp3");
        std::fs::write(&empty, []).expect("write an empty file");
        let tags = read_for_path(&empty).expect("an empty file still opens");
        assert!(tags.is_empty());

        // A directory is not a file, however it fails on this platform.
        assert_eq!(read_for_path(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tag_longer_than_the_first_read_is_topped_up_from_disk() {
        let dir = scratch_dir("topup");
        let path = dir.join("big-cover.mp3");
        // A cover that lands the text frames past the 256 KiB first read.
        let cover = filler_frame("APIC", HEAD_BYTES + 8_192);
        let mut body = cover;
        body.extend_from_slice(&{
            let tag = id3v2_tag(&[("TPE1", "Aphex Twin"), ("TIT2", "Xtal")], None);
            tag[10..].to_vec()
        });
        let mut file = id3v2_tag(&[], Some(body.len()));
        file.extend_from_slice(&body);
        file.extend_from_slice(&mpeg_frames(3));
        assert!(file.len() > HEAD_BYTES, "the test file must overrun one read");
        std::fs::write(&path, &file).expect("write the test track");

        let tags = read_for_path(&path).expect("a file that exists reads");
        assert_eq!(tags.artist, "Aphex Twin");
        assert_eq!(tags.title, "Xtal");
        assert_eq!(tags.bitrate_kbps, Some(128));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flac_metadata_chain_is_walked_by_length_only() {
        // Two blocks: a 34-byte STREAMINFO, then a last block of 300 bytes.
        let mut file = b"fLaC".to_vec();
        file.extend_from_slice(&[0x00, 0x00, 0x00, 0x22]);
        file.resize(file.len() + 34, 0);
        file.extend_from_slice(&[0x84, 0x00, 0x01, 0x2c]);
        assert_eq!(flac_metadata_end(&file, 4), 4 + 4 + 34 + 4 + 300);
        assert_eq!(declared_head_len(&file), 4 + 4 + 34 + 4 + 300);
        // A chain that never sets the last-block bit stops at the buffer.
        let unterminated = vec![0u8; 64];
        assert!(flac_metadata_end(&unterminated, 0) >= 64);
    }

    #[test]
    fn nothing_in_a_malformed_head_reaches_a_panic() {
        let mut heads: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![0x00],
            vec![0xff],
            vec![0xff; 4096],
            vec![0x00; 4096],
            b"ID3".to_vec(),
            b"ID3\x03\x00\x00\x00\x00".to_vec(),
            b"fLaC".to_vec(),
            b"OggS".to_vec(),
            b"OggS\x00\x02".to_vec(),
        ];
        // A tag header with every size byte set, a truncated frame header, and
        // a frame header with nothing behind it.
        heads.push(b"ID3\x03\x00\x10\x7f\x7f\x7f\x7f".to_vec());
        heads.push(MPEG1_128.to_be_bytes()[..3].to_vec());
        heads.push(MPEG1_128.to_be_bytes().to_vec());
        let mut cut = id3v2_tag(&[("TPE1", "Aphex Twin")], None);
        cut.extend_from_slice(&MPEG1_128.to_be_bytes()[..2]);
        heads.push(cut);
        let mut half_frame = mpeg_frames(2);
        half_frame.truncate(MPEG1_128_BYTES + 2);
        heads.push(half_frame);

        for head in &heads {
            let tags = read_from_bytes(head);
            // Whatever came back, it is a value and not a crash; the fields
            // are either empty or something the bytes actually contained.
            assert!(tags.year.len() <= 512);
            let _ = tags.is_empty();
            let _ = declared_head_len(head);
            let _ = declared_id3v2_len(head);
            let _ = nominal_bitrate(head);
        }
    }

    #[test]
    fn a_corrupted_head_is_still_only_ever_an_answer() {
        // A deterministic sweep, because the bytes that reach this module come
        // off whatever disk the operator plugged in, and the interesting
        // damage is not the empty buffer — it is a tag that is ALMOST right.
        // Seeded arithmetic rather than a crate: the point is repeatability,
        // and a failing seed prints itself.
        let good = tagged_mp3(&[("TIT2", "Bike"), ("TPE1", "Pink Floyd"), ("TCON", "(17)")]);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2_000 {
            let mut damaged = good.clone();
            let cuts = 1 + (next() % 6) as usize;
            for _ in 0..cuts {
                let at = (next() as usize) % damaged.len();
                damaged[at] = (next() % 256) as u8;
            }
            // A truncation as well, since a half-written file is the most
            // common damage a library actually contains.
            let keep = (next() as usize) % (damaged.len() + 1);
            damaged.truncate(keep);
            let tags = read_from_bytes(&damaged);
            let _ = tags.is_empty();
            let _ = declared_head_len(&damaged);
            assert!(tags.bitrate_kbps.is_none_or(|kbps| (8..=320).contains(&kbps)));
        }
    }

    #[test]
    fn a_sidecar_carries_the_tags_to_the_next_session() {
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-tags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tags = TrackTags {
            title: "Bike".into(),
            artist: "A Band".into(),
            album: "An Album".into(),
            genre: "Southern Rock".into(),
            year: "1998".into(),
            bitrate_kbps: Some(320),
            // The hints do not ride the sidecar: they come off the file
            // itself, and a claim is only worth reading where it is made.
            tag_bpm: None,
            tag_key: None,
            tag_gain_db: None,
        };
        assert!(load_sidecar(&dir, "abc").is_none(), "nothing written yet");
        save_sidecar(&dir, "abc", &tags);
        assert_eq!(load_sidecar(&dir, "abc"), Some(tags));
        // A record that genuinely carries no tags is an ANSWER, and writing
        // it down is what stops the next session opening the file again.
        save_sidecar(&dir, "bare", &TrackTags::default());
        assert_eq!(load_sidecar(&dir, "bare"), Some(TrackTags::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tag_with_a_newline_in_it_cannot_forge_a_second_field() {
        // The format is line-based, so an artist containing a newline would
        // otherwise write a line the reader takes as another field.
        let tags = TrackTags {
            artist: "A Band\nalbum Forged".into(),
            ..TrackTags::default()
        };
        let back = TrackTags::from_text(&tags.to_text());
        assert_eq!(back.album, "", "a newline must not become another field");
        assert_eq!(back.artist, "A Band album Forged");
    }

    #[test]
    fn a_mangled_sidecar_line_costs_one_field_and_not_the_rest() {
        let back = TrackTags::from_text("artist A Band\nnonsense\nbitrate huh\nyear 1998\n");
        assert_eq!(back.artist, "A Band");
        assert_eq!(back.year, "1998");
        assert_eq!(back.bitrate_kbps, None);
        assert_eq!(TrackTags::from_text(""), TrackTags::default());
    }

    #[test]
    fn a_declared_id3_length_is_what_the_tag_claims_not_what_is_there() {
        let tag = id3v2_tag(&[("TIT2", "Bike")], Some(1_000_000));
        assert_eq!(declared_id3v2_len(&tag), 10 + 1_000_000);
        // A non-syncsafe size byte is not a header this understands.
        let mut broken = tag.clone();
        broken[9] = 0x80;
        assert_eq!(declared_id3v2_len(&broken), 0);
        assert_eq!(declared_id3v2_len(b"no tag here at all"), 0);
        assert_eq!(declared_id3v2_len(&[]), 0);
        // The footer flag adds its ten bytes.
        let mut footered = id3v2_tag(&[], Some(100));
        footered[5] = 0x10;
        assert_eq!(declared_id3v2_len(&footered), 120);
    }
}
