//! The High Voltage SID Collection: every known C64 tune (release #85,
//! 61,157 SIDs), an opt-in download from the collection's own site,
//! pinned by size and SHA-256, its disclaimer shown before install and a
//! tune's credits (title, author, release, STIL notes) kept with it.
//!
//! Installing it unpacks the archive once with our own 7z reader into a
//! pack of the tunes (each LZ4-compressed) and a catalogue the song
//! library merges (`library.tsv`): composer, title, year and publisher,
//! the game, the subtunes and their lengths (Songlengths.md5), the STIL's
//! notes, and what a short run of each tune plays (tempo, key, voices).

use crate::analyse::Features;
use crate::gm::Role;
use crate::index::Entry;
use makepad_model_install::{self as models, Installable};
use std::collections::HashMap;
use std::path::PathBuf;

pub const LICENSE: &str = "HVSC: the tunes are copyrighted; private enjoyment only";

pub const LICENSE_TEXT: &str = "The High Voltage SID Collection (https://hvsc.c64.org), release #85 (June 2026). From its DOCUMENTS/Disclaimer.txt: \"All of the tunes available in the High Voltage SID Collection (HVSC) are copyrighted.\" \"Use of the SIDs in this collection for any purpose other than private enjoyment is subject to relevant copyright law.\" \"Unauthorised commercial exploitation of any tune in this collection is illegal.\" The collection's creators \"do not have (and neither do they claim) the legal authority to grant licences\". Each tune keeps its composer, title and release; covers made from them are for listening and learning, not for publishing.";

pub const ARCHIVE: Installable = Installable::dataset(
    "HVSC #85",
    LICENSE,
    LICENSE_TEXT,
    "hvsc.c64.org (the collection's download, release #85): HVSC_85-all-of-them.7z",
    84_827_547,
    "f770339229446df1b8e037938fb70f3f4c953cb4a6eeef7ce357f05ca2c73225",
    "https://hvsc.brona.dk/HVSC/HVSC_85-all-of-them.7z",
    "datasets/hvsc/HVSC_85-all-of-them.7z",
);

/// The collection's folder (the archive, the pack and the catalogue).
pub fn dir() -> PathBuf {
    let mut dir = models::weights_root();
    dir.extend(["datasets", "hvsc"]);
    dir
}

pub fn pack_path() -> PathBuf {
    dir().join("tunes.pack")
}

pub fn offsets_path() -> PathBuf {
    dir().join("tunes.tsv")
}

pub fn catalogue_path() -> PathBuf {
    dir().join("library.tsv")
}

pub fn installed() -> bool {
    models::installed(&ARCHIVE)
}

/// Whether the archive is unpacked into the pack and catalogue.
pub fn unpacked() -> bool {
    pack_path().is_file() && offsets_path().is_file() && catalogue_path().is_file()
}

/// Songlengths.md5: each tune's subtune lengths (seconds), by path.
pub fn songlengths(text: &str) -> HashMap<String, Vec<f64>> {
    let mut out = HashMap::new();
    let mut path: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(p) = line.strip_prefix("; ") {
            path = Some(p.trim().to_string());
        } else if let (Some(p), Some((_, lengths))) = (path.take(), line.split_once('=')) {
            let secs = lengths
                .split_whitespace()
                .filter_map(|t| {
                    let t = t.split('(').next()?;
                    let (m, s) = t.split_once(':')?;
                    Some(m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?)
                })
                .collect();
            out.insert(p, secs);
        }
    }
    out
}

/// What the STIL says about a tune: its notes, and each subtune's title.
#[derive(Clone, Debug, Default)]
pub struct Stil {
    pub comment: String,
    /// (subtune, "TITLE (ARTIST)") lines; subtune 0 is the whole file.
    pub titles: Vec<(u16, String)>,
}

pub fn stil(text: &str) -> HashMap<String, Stil> {
    let mut out: HashMap<String, Stil> = HashMap::new();
    let mut path: Option<String> = None;
    let mut sub = 0u16;
    let mut field: Option<String> = None;
    for raw in text.lines() {
        if raw.starts_with('#') {
            continue;
        }
        if raw.starts_with('/') && raw.trim_end().ends_with(".sid") {
            path = Some(raw.trim().to_string());
            sub = 0;
            field = None;
            continue;
        }
        let Some(p) = path.clone() else { continue };
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(n) = line.strip_prefix("(#").and_then(|r| r.strip_suffix(')')) {
            sub = n.parse().unwrap_or(0);
            continue;
        }
        let e = out.entry(p).or_default();
        let (key, value) = match line.split_once(':') {
            Some((k, v)) if k.chars().all(|c| c.is_ascii_uppercase()) && !k.is_empty() => (Some(k.to_string()), v.trim().to_string()),
            _ => (None, line.to_string()),
        };
        if key.is_some() {
            field = key.clone();
        }
        match field.as_deref() {
            Some("COMMENT") => {
                if e.comment.len() < 400 {
                    if !e.comment.is_empty() {
                        e.comment.push(' ');
                    }
                    e.comment.push_str(&value);
                }
            }
            Some("TITLE") | Some("NAME") if key.is_some() => e.titles.push((sub, value)),
            Some("ARTIST") if key.is_some() => {
                if let Some(last) = e.titles.iter_mut().rev().find(|t| t.0 == sub) {
                    last.1 = format!("{} ({value})", last.1);
                }
            }
            _ => {}
        }
    }
    out
}

/// A tune in the pack.
#[derive(Clone, Debug)]
struct Packed {
    offset: u64,
    len: usize,
    raw: usize,
}

fn offsets() -> Result<HashMap<String, Packed>, String> {
    let text = std::fs::read_to_string(offsets_path()).map_err(|e| format!("the SID collection is not unpacked on this machine ({e}): midi_library chip hvsc"))?;
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let path = f.next()?.to_string();
            Some((path, Packed { offset: f.next()?.parse().ok()?, len: f.next()?.parse().ok()?, raw: f.next()?.parse().ok()? }))
        })
        .collect())
}

/// A tune's file, by its path in the collection.
pub fn bytes(path: &str) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = path.split('#').next().unwrap_or(path);
    let all = offsets()?;
    let p = all.get(path).ok_or_else(|| format!("{path} is not in the SID collection"))?;
    let mut f = std::fs::File::open(pack_path()).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(p.offset)).map_err(|e| e.to_string())?;
    let mut packed = vec![0u8; p.len];
    f.read_exact(&mut packed).map_err(|e| e.to_string())?;
    let mut out = vec![0u8; p.raw];
    let n = makepad_lz4::decompress_safe(&packed, &mut out).map_err(|e| format!("{path}: {e:?}"))?;
    out.truncate(n);
    Ok(out)
}

/// What a short run of a tune plays, as the catalogue's features.
fn features(bytes: &[u8], song: u16, seconds: f64) -> Option<Features> {
    let tune = super::sid::read(bytes).ok()?;
    let run = seconds.clamp(5.0, 45.0);
    let trace = super::sid::machine::run(&tune, song, run);
    let meta = super::sid::convert::Meta { title: String::new(), artist: String::new(), year: None, mos8580: tune.header.mos8580, header: Vec::new() };
    let (a, _) = super::sid::convert::convert(&trace, &meta).ok()?;
    let mut roles: Vec<Role> = a.parts.iter().map(|p| p.role).collect();
    roles.sort();
    roles.dedup();
    let notes: usize = a.parts.iter().map(|p| p.notes.len()).sum();
    let voices: std::collections::HashSet<String> = a.parts.iter().map(|p| p.name.split('_').next().unwrap_or("").to_string()).collect();
    // A full tune: all voices busy, a bass, drums and a lead, and long.
    let quality = (voices.len() as u32 * 15 + roles.len() as u32 * 8 + (notes as u32 * 60 / run as u32).min(25) + if seconds >= 60.0 { 10 } else { 0 }).min(100) as u8;
    Some(Features {
        bpm: a.bpm,
        key: a.key.clone(),
        sig: "4/4".into(),
        parts: a.parts.len(),
        programs: Vec::new(),
        roles,
        seconds,
        notes,
        melody: a.parts.iter().any(|p| p.melody),
        quality,
    })
}

/// The publisher in a release field ("1986 Ocean" -> "Ocean").
fn publisher(released: &str) -> String {
    released.trim_start_matches(|c: char| c.is_ascii_digit() || c == '?' || c == '-').trim().to_string()
}

/// Unpack the installed archive into the pack and catalogue (`threads`
/// tunes read at once; `progress(done, total)`).
pub fn unpack(threads: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<usize, String> {
    let archive = std::fs::read(models::dest_path(&ARCHIVE)).map_err(|e| format!("the SID collection is not installed ({e})"))?;
    let a = super::archive::sevenz::Archive::open(&archive)?;
    let mut docs: HashMap<String, Vec<u8>> = HashMap::new();
    // Each tune LZ4-packed as it is unpacked (the pack's bytes).
    let mut tunes: Vec<(String, Vec<u8>, usize)> = Vec::new();
    let mut failed: Option<String> = None;
    a.for_each(|e, b| {
        if let Some(path) = e.name.strip_prefix("C64Music") {
            if path.ends_with(".sid") {
                match makepad_lz4::compress_default(b) {
                    Ok(packed) => tunes.push((path.to_string(), packed, b.len())),
                    Err(err) => failed = Some(format!("{path}: {err:?}")),
                }
            } else if path.ends_with("Songlengths.md5") || path.ends_with("STIL.txt") {
                docs.insert(path.rsplit('/').next().unwrap_or("").to_string(), b.to_vec());
            }
        }
    })?;
    if let Some(e) = failed {
        return Err(e);
    }
    drop(a);
    drop(archive);
    let lengths = songlengths(&String::from_utf8_lossy(docs.get("Songlengths.md5").map_or(&[][..], |v| v.as_slice())));
    let notes = stil(&String::from_utf8_lossy(docs.get("STIL.txt").map_or(&[][..], |v| v.as_slice())));
    let total = tunes.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let chunk = total.div_ceil(threads.max(1)).max(1);
    let mut rows: Vec<Vec<Entry>> = Vec::with_capacity(total);
    std::thread::scope(|s| {
        let handles: Vec<_> = tunes
            .chunks(chunk)
            .map(|part| {
                let (lengths, notes, done) = (&lengths, &notes, &done);
                s.spawn(move || {
                    part.iter()
                        .map(|(path, packed, raw)| {
                            let mut bytes = vec![0u8; *raw];
                            let out = match makepad_lz4::decompress_safe(packed, &mut bytes) {
                                Ok(_) => entries(path, &bytes, lengths.get(path), notes.get(path)),
                                Err(_) => Vec::new(),
                            };
                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            if n % 1000 == 0 || n == total {
                                progress(n, total);
                            }
                            out
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            rows.extend(h.join().unwrap_or_default());
        }
    });
    // The pack: each tune LZ4-compressed, one after another.
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut pack: Vec<u8> = Vec::new();
    let mut offsets = String::new();
    for (path, packed, raw) in &tunes {
        offsets.push_str(&format!("{path}\t{}\t{}\t{raw}\n", pack.len(), packed.len()));
        pack.extend_from_slice(packed);
    }
    let entries: Vec<Entry> = rows.into_iter().flatten().collect();
    let count = entries.len();
    std::fs::write(pack_path(), pack).map_err(|e| e.to_string())?;
    std::fs::write(offsets_path(), offsets).map_err(|e| e.to_string())?;
    crate::index::write(&entries, &catalogue_path())?;
    Ok(count)
}

/// The most subtunes of one file catalogued.
const SUBTUNES: usize = 12;

/// A tune's catalogue rows: its start song, and each other subtune that
/// plays at least 45 seconds (a game's levels; not its jingles), as
/// `path#n`.
fn entries(path: &str, bytes: &[u8], lengths: Option<&Vec<f64>>, stil: Option<&Stil>) -> Vec<Entry> {
    let Ok(tune) = super::sid::read(bytes) else { return Vec::new() };
    let h = &tune.header;
    let mut out: Vec<Entry> = entry(path, bytes, lengths, stil, h.start_song).into_iter().collect();
    for song in (1..=h.songs).filter(|&n| n != h.start_song) {
        if out.len() > SUBTUNES {
            break;
        }
        if lengths.and_then(|l| l.get(song as usize - 1)).is_some_and(|&secs| secs >= 45.0) {
            out.extend(entry(path, bytes, lengths, stil, song));
        }
    }
    out
}

/// A tune's catalogue row for one subtune.
fn entry(path: &str, bytes: &[u8], lengths: Option<&Vec<f64>>, stil: Option<&Stil>, song: u16) -> Option<Entry> {
    let tune = super::sid::read(bytes).ok()?;
    let h = &tune.header;
    let unknown = |s: &str| s.is_empty() || s == "<?>";
    let start = song == h.start_song;
    let md5 = if start { crate::md5::hex(bytes) } else { crate::md5::hex(&[bytes, &song.to_le_bytes()[..]].concat()) };
    let seconds = lengths.and_then(|l| l.get(song as usize - 1)).copied().unwrap_or(crate::chip::import::SID_SECONDS);
    let features = features(bytes, song, seconds).unwrap_or(Features { bpm: 120.0, key: String::new(), sig: "4/4".into(), parts: 0, programs: Vec::new(), roles: Vec::new(), seconds, notes: 0, melody: false, quality: 0 });
    let category = path.trim_start_matches('/').split('/').next().unwrap_or("").to_lowercase();
    let mut tags: Vec<(String, f32)> = vec![("c64".into(), 100.0), ("sid".into(), 100.0), ("chiptune".into(), 100.0)];
    match category.as_str() {
        "games" => tags.push(("game".into(), 100.0)),
        "demos" => tags.push(("demo".into(), 100.0)),
        _ => {}
    }
    let publisher = publisher(&h.released);
    for w in publisher.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
        tags.push((crate::meta::norm(w), 50.0));
    }
    if h.songs > 1 {
        tags.push((format!("subtunes{}", h.songs), 10.0));
    }
    if h.mos8580 {
        tags.push(("8580".into(), 30.0));
    }
    let stil_title = stil.and_then(|s| s.titles.iter().find(|t| t.0 == song).or_else(|| s.titles.iter().find(|t| t.0 == 0))).map(|t| t.1.clone());
    if let Some(t) = &stil_title {
        // A cover of another song names it.
        for w in t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4) {
            tags.push((crate::meta::norm(w), 20.0));
        }
    }
    // Game music: a game's tune in the musicians' folders has a publisher.
    let game = if category == "games" || (category == "musicians" && !publisher.is_empty()) { h.name.clone() } else { String::new() };
    let mut genres = vec!["chiptune".to_string()];
    if category == "demos" {
        genres.push("demoscene".into());
    }
    Some(Entry {
        id: md5[..8].to_string(),
        artist: if unknown(&h.author) { "Unknown".into() } else { h.author.clone() },
        title: {
            let name = if unknown(&h.name) { path.rsplit('/').next().unwrap_or(path).trim_end_matches(".sid").replace('_', " ") } else { h.name.clone() };
            if start {
                name
            } else {
                // A subtune by the STIL's title for it, else its number.
                match stil.and_then(|s| s.titles.iter().find(|t| t.0 == song)) {
                    Some(t) => format!("{name}: {}", t.1),
                    None => format!("{name} (tune {song})"),
                }
            }
        },
        path: if start { path.to_string() } else { format!("{path}#{song}") },
        md5,
        // The STIL's notes and the release, for the credit.
        tid: [h.released.clone(), stil.map_or(String::new(), |s| s.comment.chars().take(200).collect())].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" | "),
        matched: 0.0,
        year: super::sid::year(&h.released),
        genres,
        tags,
        features,
        format: "sid".into(),
        platform: "c64".into(),
        game,
        archive: "hvsc".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songlengths_and_stil_read() {
        let l = songlengths("[Database]\r\n; /MUSICIANS/H/Hubbard_Rob/Commando.sid\r\nabc=3:15 0:05.500 1:02(G)\r\n");
        assert_eq!(l["/MUSICIANS/H/Hubbard_Rob/Commando.sid"], vec![195.0, 5.5, 62.0]);
        let s = stil("#####\n/MUSICIANS/H/Hubbard_Rob/Commando.sid\n  TITLE: Commando (arcade)\n ARTIST: Tamayo Kawamoto\nCOMMENT: The famous one.\n         Second line.\n(#2)\n  TITLE: High score\n\n/DEMOS/X.sid\nCOMMENT: x\n");
        let c = &s["/MUSICIANS/H/Hubbard_Rob/Commando.sid"];
        assert_eq!(c.titles, vec![(0, "Commando (arcade) (Tamayo Kawamoto)".to_string()), (2, "High score".to_string())]);
        assert_eq!(c.comment, "The famous one. Second line.");
        assert_eq!(publisher("1985 Elite"), "Elite");
    }
}
