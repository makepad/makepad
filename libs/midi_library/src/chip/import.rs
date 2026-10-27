//! Any tracker, chiptune or game-music file as a Score: the format found
//! from its bytes (and name), read, played, and written as the MIDI
//! library's [`Arrangement`] and Score, with the samples or patches it
//! plays. Stage's File > Open and drag-and-drop come here for these
//! formats, as they go to the MIDI importer for MIDI.

use crate::chip::sid;
use crate::chip::tracker::{self, arrange::SampleAsset};
use crate::arrangement::{self, Arrangement};

/// The extensions this importer reads.
pub const EXTENSIONS: [&str; 11] = ["mod", "s3m", "xm", "it", "sid", "nsf", "nsfe", "vgm", "vgz", "spc", "gbs"];

/// Whether a file of this name is one of ours.
pub fn is_supported(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    // Amiga files are often named `mod.title`.
    EXTENSIONS.contains(&ext) || lower.rsplit('/').next().is_some_and(|f| f.starts_with("mod."))
}

/// Who and what a file is, when an index knows better than the file.
#[derive(Clone, Debug, Default)]
pub struct About {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    /// Header lines first in the Score: the credit, the licence, the source.
    pub notes: Vec<String>,
    /// `Score{rights: …}`: (license, credits, source, terms).
    pub rights: Option<(String, String, String, String)>,
    /// A SID's subtune (1-based; 0 is its start song) and how long to
    /// play it (seconds; 0 is the index's length or three minutes).
    pub song: u16,
    pub seconds: f64,
}

pub struct Imported {
    pub arrangement: Arrangement,
    /// The Score.
    pub source: String,
    /// The samples its samplers play (registered for renders by
    /// [`register`]; a Stage document takes them into its library).
    pub samples: Vec<SampleAsset>,
    /// "Amiga MOD", "C64 SID" ...
    pub format: String,
    /// The second of the original the Score's beat 0 is.
    pub offset: f64,
}

fn stem(name: &str) -> String {
    let file = name.rsplit('/').next().unwrap_or(name);
    let file = file.strip_prefix("mod.").unwrap_or(file);
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_string()
}

/// Import `bytes` (a file called `name`).
pub fn import(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    if sid::is_sid(bytes) {
        return import_sid(bytes, name, about);
    }
    if super::nsf::is_nsf(bytes) {
        return import_nsf(bytes, name, about);
    }
    if super::nsf::is_nsfe(bytes) {
        // As the NSF it describes, the track's own name as the title.
        let (nsf, labels) = super::nsf::from_nsfe(bytes)?;
        let h = super::nsf::header(&nsf)?;
        let song = if about.song == 0 { h.start } else { about.song as u8 };
        let mut about = about.clone();
        if about.title.is_empty() {
            if let Some(label) = labels.get(song as usize - 1).filter(|l| !l.is_empty()) {
                about.title = format!("{}: {label}", h.name);
            }
        }
        return import_nsf(&nsf, name, &about);
    }
    if super::spc::is_spc(bytes) {
        return import_spc(bytes, name, about);
    }
    if super::gbs::is_gbs(bytes) {
        return import_gbs(bytes, name, about);
    }
    if super::vgm::is_vgm(bytes) || (bytes.starts_with(&[0x1F, 0x8B]) && name.to_ascii_lowercase().ends_with(".vgz")) {
        return import_vgm(bytes, name, about);
    }
    let m = tracker::read(bytes, name)?;
    let played = tracker::replay::play(&m);
    if played.notes.is_empty() {
        return Err(format!("{name}: the module plays no notes"));
    }
    let title = if !about.title.is_empty() { about.title.clone() } else if !m.title.trim().is_empty() { m.title.trim().to_string() } else { stem(name) };
    let converted = tracker::arrange::convert(&m, &played, &tracker::arrange::hash(bytes), &title, &about.artist, about.notes.clone());
    let mut arrangement = converted.arrangement;
    arrangement.year = about.year;
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples: converted.samples, offset: 0.0 })
}

/// Make an import's samples playable in this process (offline renders,
/// covers, tests), and keep them in the sample store so a saved Score
/// finds them again ([`stored`]).
pub fn register(imported: &Imported) {
    tracker::arrange::register(&imported.samples);
    for s in &imported.samples {
        let _ = store(&s.asset, &s.sample);
    }
}

/// Where imported samples are kept: the samples of the songs the person
/// opened, by their library names (`trk-…`, `snes-…`, `nes-…`, `gb-…`).
pub fn store_dir() -> std::path::PathBuf {
    let mut dir = makepad_model_install::weights_root();
    dir.extend(["datasets", "samples"]);
    dir
}

fn store_path(asset: &makepad_score_live::AssetRef) -> std::path::PathBuf {
    let safe: String = format!("{}@{}", asset.id, asset.rev).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '@' { c } else { '_' }).collect();
    store_dir().join(format!("{safe}.smp"))
}

/// Keep a sample: its rate, loop and frames (f32, a channel or two).
pub fn store(asset: &makepad_score_live::AssetRef, s: &makepad_score_player::Sample) -> Result<(), String> {
    let path = store_path(asset);
    if path.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(store_dir()).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(24 + s.len() * 8);
    out.extend_from_slice(b"MPSM");
    out.extend_from_slice(&s.rate.to_le_bytes());
    let (a, b) = s.sustain.unwrap_or((0, 0));
    out.extend_from_slice(&a.to_le_bytes());
    out.extend_from_slice(&b.to_le_bytes());
    out.push(if s.right.is_some() { 2 } else { 1 });
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    for v in s.left.iter().chain(s.right.iter().flat_map(|r| r.iter())) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

/// A kept sample, when this machine has it.
pub fn stored(asset: &makepad_score_live::AssetRef) -> Option<std::sync::Arc<makepad_score_player::Sample>> {
    let b = std::fs::read(store_path(asset)).ok()?;
    if b.len() < 21 || &b[..4] != b"MPSM" {
        return None;
    }
    let f32_at = |at: usize| f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let u32_at = |at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let rate = f32_at(4);
    let (a, e) = (u32_at(8), u32_at(12));
    let channels = b[16] as usize;
    let n = u32_at(17) as usize;
    if b.len() < 21 + n * channels * 4 {
        return None;
    }
    let ch = |k: usize| -> std::sync::Arc<[f32]> { (0..n).map(|i| f32_at(21 + (k * n + i) * 4)).collect::<Vec<_>>().into() };
    let mut sample = makepad_score_player::Sample::new(ch(0), (channels == 2).then(|| ch(1)), rate);
    if e > a {
        sample = sample.with_loop(a, e);
    }
    Some(std::sync::Arc::new(sample))
}

/// The file as its own player plays it (dry, no Score): the original a
/// cover is held against.
pub fn original(bytes: &[u8], name: &str, about: &About, rate: f32) -> Result<(Vec<f32>, Vec<f32>), String> {
    if sid::is_sid(bytes) {
        let tune = sid::read(bytes)?;
        let song = if about.song == 0 { tune.header.start_song } else { about.song };
        let seconds = if about.seconds > 0.0 { about.seconds } else { SID_SECONDS };
        let trace = sid::machine::run(&tune, song, seconds);
        return Ok(sid::render::render(&trace, tune.header.mos8580, rate));
    }
    if super::nsf::is_nsf(bytes) {
        let h = super::nsf::header(bytes)?;
        let song = if about.song == 0 { h.start } else { about.song as u8 };
        let (_, run) = super::nsf::run(bytes, song, if about.seconds > 0.0 { about.seconds } else { NSF_SECONDS })?;
        let (voices, _) = super::nsf::voices(&run);
        return Ok(super::voices::render(&voices, run.seconds, rate, &|_| None));
    }
    if super::spc::is_spc(bytes) {
        let tag = super::spc::tag(bytes);
        let seconds = if about.seconds > 0.0 { about.seconds } else if tag.seconds > 5.0 { tag.seconds.min(600.0) } else { 150.0 };
        let run = super::spc::run(bytes, seconds)?;
        let (voices, samples, used) = super::spc::voices(&run, 125.0, "x");
        let by_id: std::collections::HashMap<String, std::sync::Arc<makepad_score_player::Sample>> = samples.into_iter().map(|s| (s.asset.id.clone(), s.sample)).collect();
        return Ok(super::voices::render(&voices, run.seconds, rate, &|t| used.get(&(t as u8)).and_then(|(id, root)| by_id.get(id).map(|s| (s.clone(), if root.is_finite() { *root } else { 60.0 })))));
    }
    if super::gbs::is_gbs(bytes) {
        let h = super::gbs::header(bytes)?;
        let song = if about.song == 0 { h.first } else { about.song as u8 };
        let (_, run) = super::gbs::run(bytes, song, if about.seconds > 0.0 { about.seconds } else { NSF_SECONDS })?;
        let (voices, samples) = super::gbs::voices(&run, 60.0, "x");
        return Ok(super::voices::render(&voices, run.seconds, rate, &|t| samples.get(t as usize).map(|s| (s.sample.clone(), 60.0))));
    }
    if super::vgm::is_vgm(bytes) || bytes.starts_with(&[0x1F, 0x8B]) {
        let data = super::vgm::unpack(bytes)?;
        let log = super::vgm::log(&data, if about.seconds > 0.0 { about.seconds } else { VGM_SECONDS })?;
        let mut voices = super::vgm::voices(&log);
        let (dac, samples) = super::vgm::dac(&log, "x");
        voices.extend(dac);
        return Ok(super::voices::render(&voices, log.seconds, rate, &|t| samples.get(t as usize).map(|s| (s.sample.clone(), 60.0))));
    }
    let m = tracker::read(bytes, name)?;
    let played = tracker::replay::play(&m);
    let a = tracker::render::render(&m, &played, rate);
    Ok((a.left, a.right))
}

/// What the song library knows about a tune (its credits, its library's
/// terms) for its Score's header and rights.
pub fn about_entry(entry: &crate::index::Entry) -> About {
    let mut notes = Vec::new();
    let credit = match entry.archive.as_str() {
        "hvsc" => format!("From the High Voltage SID Collection #85 (hvsc.c64.org), {}.", entry.path),
        "web" => format!("Downloaded from {}.", entry.tid),
        _ => String::new(),
    };
    if !entry.tid.is_empty() && entry.archive == "hvsc" {
        notes.push(format!("Released: {}.", entry.tid));
    }
    if !credit.is_empty() {
        notes.push(credit);
    }
    let (license, terms) = match entry.archive.as_str() {
        "web" if entry.path.starts_with("zophar/") => ("the game publisher's copyright".to_string(), super::zophar::NOTICE.to_string()),
        "hvsc" => (crate::chip::hvsc::LICENSE.to_string(), "copyrighted; for private listening and learning, never published".to_string()),
        _ => ("the composer's".to_string(), "copyrighted unless the source says otherwise; for private listening and learning".to_string()),
    };
    // A SID's subtune is the path's `#n` (the start song otherwise).
    let song = entry.path.rsplit_once('#').and_then(|(_, n)| n.parse().ok()).unwrap_or(0);
    About {
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        year: entry.year,
        notes,
        rights: Some((license, entry.artist.clone(), entry.path.clone(), terms)),
        song,
        seconds: if (entry.format == "sid" || entry.path.starts_with("zophar/")) && entry.features.seconds > 0.0 { entry.features.seconds.min(300.0) } else { 0.0 },
    }
}

/// A library tune imported, its samples made playable in this process.
pub fn import_entry(entry: &crate::index::Entry) -> Result<Imported, String> {
    let bytes = crate::index::bytes(entry)?;
    let name = if entry.path.starts_with("zophar/") { super::zophar::member(&entry.path) } else { entry.path.rsplit_once('#').map_or(entry.path.as_str(), |(p, _)| p) };
    let imported = import(&bytes, name, &about_entry(entry))?;
    register(&imported);
    Ok(imported)
}

/// How long an NES tune without a length plays.
pub const NSF_SECONDS: f64 = 150.0;

fn import_nsf(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    use super::nsf;
    let h = nsf::header(bytes)?;
    let song = if about.song == 0 { h.start } else { about.song as u8 };
    let seconds = if about.seconds > 0.0 { about.seconds } else { NSF_SECONDS };
    let (_, run) = nsf::run(bytes, song, seconds)?;
    let (mut voices, starts) = nsf::voices(&run);
    let (dpcm, samples) = nsf::dpcm(&run, &starts, &tracker::arrange::hash(bytes));
    voices.extend(dpcm);
    let ids: Vec<String> = samples.iter().map(|s| s.asset.id.clone()).collect();
    let unknown = |s: &str| s.is_empty() || s == "<?>";
    let mut header = about.notes.clone();
    if !unknown(&h.copyright) {
        header.push(format!("(c) {}.", h.copyright));
    }
    if h.songs > 1 {
        header.push(format!("Track {song} of {}.", h.songs));
    }
    if h.chips != 0 {
        header.push("The tune also uses a cartridge's extra sound chip, which this import leaves out.".into());
    }
    let meta = super::voices::Meta {
        title: if !about.title.is_empty() { about.title.clone() } else if !unknown(&h.name) { h.name.clone() } else { stem(name) },
        artist: if !about.artist.is_empty() { about.artist.clone() } else if !unknown(&h.artist) { h.artist.clone() } else { String::new() },
        year: about.year.or_else(|| sid::year(&h.copyright)),
        genre: "NES".into(),
        header,
        rate: run.rate,
    };
    let (mut arrangement, offset) = super::voices::convert(&voices, &meta, &|v, t| nsf::patch(v, t, &ids))?;
    // The two pulses a little apart, as a stereo NES mixes them.
    for p in arrangement.parts.iter_mut() {
        p.pan = if p.name.starts_with("pulse1") { -0.25 } else if p.name.starts_with("pulse2") { 0.25 } else { 0.0 };
    }
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples, offset })
}

fn import_gbs(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    use super::gbs;
    let h = gbs::header(bytes)?;
    let song = if about.song == 0 { h.first } else { about.song as u8 };
    let (_, run) = gbs::run(bytes, song, if about.seconds > 0.0 { about.seconds } else { NSF_SECONDS })?;
    let rate = 60.0;
    let (voices, samples) = gbs::voices(&run, rate, &tracker::arrange::hash(bytes));
    let ids: Vec<String> = samples.iter().map(|s| s.asset.id.clone()).collect();
    let mut header = about.notes.clone();
    if !h.copyright.is_empty() {
        header.push(format!("(c) {}.", h.copyright));
    }
    if h.songs > 1 {
        header.push(format!("Track {song} of {}.", h.songs));
    }
    let meta = super::voices::Meta {
        title: if !about.title.is_empty() { about.title.clone() } else if !h.title.is_empty() { h.title.clone() } else { stem(name) },
        artist: if !about.artist.is_empty() { about.artist.clone() } else { h.author.clone() },
        year: about.year.or_else(|| sid::year(&h.copyright)),
        genre: "Game Boy".into(),
        header,
        rate,
    };
    let (mut arrangement, offset) = super::voices::convert(&voices, &meta, &|v, t| gbs::patch(v, t, &ids))?;
    for p in arrangement.parts.iter_mut() {
        p.pan = if p.name.starts_with("pulse1") { -0.2 } else if p.name.starts_with("pulse2") { 0.2 } else { 0.0 };
    }
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples, offset })
}

fn import_spc(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    use super::spc;
    let tag = spc::tag(bytes);
    let seconds = if about.seconds > 0.0 { about.seconds } else if tag.seconds > 5.0 { tag.seconds.min(600.0) } else { 150.0 };
    let run = spc::run(bytes, seconds)?;
    let rate = 125.0;
    let (voices, samples, used) = spc::voices(&run, rate, &tracker::arrange::hash(bytes));
    let mut header = about.notes.clone();
    if !tag.game.is_empty() {
        header.push(format!("From {} (SNES).", tag.game));
    }
    if !tag.dumper.is_empty() {
        header.push(format!("Dumped by {}.", tag.dumper));
    }
    if !tag.comment.is_empty() {
        header.push(tag.comment.clone());
    }
    header.push("Each part is a DSP voice playing one of the cartridge's own samples (its loop kept) on a sampler; its echo is left out.".into());
    let meta = super::voices::Meta {
        title: if !about.title.is_empty() { about.title.clone() } else if !tag.song.is_empty() { tag.song.clone() } else { stem(name) },
        artist: if !about.artist.is_empty() { about.artist.clone() } else { tag.artist.clone() },
        year: about.year.or(tag.year),
        genre: "SNES".into(),
        header,
        rate,
    };
    let (mut arrangement, offset) = super::voices::convert(&voices, &meta, &|_, t| spc::patch(t, &used))?;
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples, offset })
}

/// The longest a VGM log is read.
pub const VGM_SECONDS: f64 = 300.0;

fn import_vgm(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    use super::vgm;
    let data = vgm::unpack(bytes)?;
    let tag = vgm::gd3(&data);
    let log = vgm::log(&data, if about.seconds > 0.0 { about.seconds } else { VGM_SECONDS })?;
    let mut voices = vgm::voices(&log);
    let (dac, samples) = vgm::dac(&log, &tracker::arrange::hash(bytes));
    voices.extend(dac);
    let dac_ids: Vec<String> = samples.iter().map(|s| s.asset.id.clone()).collect();
    let mut header = about.notes.clone();
    if !tag.game.is_empty() {
        header.push(format!("From {}{}.", tag.game, if tag.system.is_empty() { String::new() } else { format!(" ({})", tag.system) }));
    }
    if !tag.ripper.is_empty() {
        header.push(format!("Logged by {}.", tag.ripper));
    }
    let chips: Vec<&str> = [(log.sn_clock, "SN76489"), (log.opn_clock, "YM2612"), (log.opm_clock, "YM2151"), (log.ay_clock, "AY-3-8910")].iter().filter(|c| c.0 > 0.0).map(|c| c.1).collect();
    header.push(format!("Chips: {}; others in the log are left out.", chips.join(", ")));
    let meta = super::voices::Meta {
        title: if !about.title.is_empty() { about.title.clone() } else if !tag.track.is_empty() { tag.track.clone() } else { stem(name) },
        artist: if !about.artist.is_empty() { about.artist.clone() } else { tag.author.clone() },
        year: about.year.or_else(|| tag.date.get(..4).and_then(|y| y.parse().ok())),
        genre: if tag.system.is_empty() { "VGM".into() } else { tag.system.clone() },
        header,
        rate: log.rate,
    };
    let (mut arrangement, offset) = super::voices::convert(&voices, &meta, &|v, t| vgm::patch(v, t, &dac_ids))?;
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples, offset })
}

/// The default length of a SID tune without one in the index.
pub const SID_SECONDS: f64 = 180.0;

fn import_sid(bytes: &[u8], name: &str, about: &About) -> Result<Imported, String> {
    let tune = sid::read(bytes)?;
    let h = &tune.header;
    let song = if about.song == 0 { h.start_song } else { about.song };
    let seconds = if about.seconds > 0.0 { about.seconds } else { SID_SECONDS };
    let trace = sid::machine::run(&tune, song, seconds);
    let title = if !about.title.is_empty() { about.title.clone() } else if !h.name.is_empty() && h.name != "<?>" { h.name.clone() } else { stem(name) };
    let artist = if !about.artist.is_empty() { about.artist.clone() } else if h.author != "<?>" { h.author.clone() } else { String::new() };
    let mut header = about.notes.clone();
    if !h.released.is_empty() {
        header.push(format!("Released: {}.", h.released));
    }
    if h.songs > 1 {
        header.push(format!("Subtune {song} of {}.", h.songs));
    }
    let meta = sid::convert::Meta { title, artist, year: about.year.or_else(|| sid::year(&h.released)), mos8580: h.mos8580, header };
    let (mut arrangement, offset) = sid::convert::convert(&trace, &meta)?;
    arrangement.rights = about.rights.clone();
    let source = arrangement::to_score(&arrangement);
    Ok(Imported { format: arrangement.genre.clone(), arrangement, source, samples: Vec::new(), offset })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::tracker::{Cell, Fx, Instrument, Loop, Sample, Vol};

    /// A two-channel MOD written out: a looped square, three notes with
    /// a slide, an arpeggio and a volume.
    fn mod_file() -> Vec<u8> {
        let mut b = vec![0u8; 1084];
        b[..4].copy_from_slice(b"test");
        // Sample 1: 64 bytes (32 words), volume 64, looped whole.
        let s = 20;
        b[s..s + 6].copy_from_slice(b"square");
        b[s + 22..s + 24].copy_from_slice(&32u16.to_be_bytes());
        b[s + 25] = 64;
        b[s + 28..s + 30].copy_from_slice(&32u16.to_be_bytes());
        b[950] = 1;
        b[1080..1084].copy_from_slice(b"M.K.");
        let mut pat = vec![0u8; 64 * 4 * 4];
        let cell = |row: usize, ch: usize, period: u16, fx: u8, param: u8, pat: &mut Vec<u8>| {
            let at = (row * 4 + ch) * 4;
            pat[at] = (period >> 8) as u8;
            pat[at + 1] = period as u8;
            pat[at + 2] = 0x10 | fx;
            pat[at + 3] = param;
        };
        cell(0, 0, 428, 1, 4, &mut pat);
        cell(4, 1, 428, 0, 0x47, &mut pat);
        cell(8, 0, 214, 12, 32, &mut pat);
        b.extend(pat);
        b.extend((0..64).map(|i| if i < 32 { 100u8 } else { 156u8 }));
        b
    }

    #[test]
    fn samples_are_kept_and_read_back() {
        let dir = std::env::temp_dir().join(format!("chip-store-{}", std::process::id()));
        std::env::set_var("MAKEPAD_ASSET_AI_CACHE", &dir);
        let asset = makepad_score_live::AssetRef { id: "trk-test-s1".into(), rev: "1".into() };
        let s = makepad_score_player::Sample::new(vec![0.0, 0.5, -0.5, 0.25].into(), None, 8363.0).with_loop(1, 4);
        store(&asset, &s).unwrap();
        let back = stored(&asset).unwrap();
        assert_eq!((back.rate, back.sustain, back.left.to_vec()), (8363.0, Some((1, 4)), vec![0.0, 0.5, -0.5, 0.25]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_module_imports_as_a_score_that_loads_with_its_samples_and_effects() {
        let bytes = mod_file();
        assert!(is_supported("song.mod") && is_supported("mod.song") && is_supported("tune.SID") && !is_supported("a.mid"));
        let imported = import(&bytes, "test.mod", &About::default()).unwrap();
        let a = &imported.arrangement;
        assert_eq!(a.genre, "Amiga MOD");
        assert_eq!(imported.samples.len(), 1);
        assert_eq!(imported.samples[0].sample.sustain, Some((0, 64)));
        let notes: Vec<_> = a.parts.iter().flat_map(|p| p.notes.iter()).collect();
        // The arpeggio's row and the rest of its note are two notes.
        assert_eq!(notes.len(), 4);
        // The slide is a bend, the arpeggio an arp, the volume the velocity.
        assert!(notes.iter().any(|n| n.fx.is_some_and(|f| f.bend_n >= 2 && f.bend()[f.bend_n as usize - 1].1 > 0.5)));
        assert!(notes.iter().any(|n| n.fx.is_some_and(|f| f.arp() == [0.0, 4.0, 7.0])));
        assert!(notes.iter().any(|n| n.key == 72.0 && (n.vel - 0.5).abs() < 0.01));
        // The Score loads, its sampler names the sample, its notes keep
        // their options.
        let doc = makepad_score_live::model::load(&imported.source).unwrap_or_else(|d| panic!("{d:?}\n{}", imported.source)).doc;
        assert_eq!(doc.samples().len(), 1);
        assert!(doc.loops.iter().map(|l| l.note_opts.len()).sum::<usize>() >= 2);
        // And it plays: the samples registered, the render is not silent.
        tracker::arrange::register(&imported.samples);
        let r = makepad_score_live::render(&imported.source, &makepad_score_live::RenderOpts { seconds: Some(2.0), ..Default::default() }, &makepad_score_live::NoSamples).unwrap();
        assert!(r.left.iter().any(|x| x.abs() > 0.01));
        let _ = (Cell::EMPTY, Fx::None, Vol::None, Loop { from: 0, to: 1, pingpong: false }, Sample::silent(String::new()), Instrument::of_sample(String::new(), 0));
    }

    /// A PSID whose play routine alternates E5 and A4 every 16 frames,
    /// the gate open for 12.
    fn sid_file() -> Vec<u8> {
        let mut h = vec![0u8; 0x7C];
        h[..4].copy_from_slice(b"PSID");
        h[4..6].copy_from_slice(&2u16.to_be_bytes());
        h[6..8].copy_from_slice(&0x7Cu16.to_be_bytes());
        h[0x0A..0x0C].copy_from_slice(&0x1000u16.to_be_bytes());
        h[0x0C..0x0E].copy_from_slice(&0x1020u16.to_be_bytes());
        h[0x0E..0x10].copy_from_slice(&1u16.to_be_bytes());
        h[0x10..0x12].copy_from_slice(&1u16.to_be_bytes());
        h[0x16..0x1B].copy_from_slice(b"Tones");
        h[0x36..0x3A].copy_from_slice(b"Test");
        h[0x56..0x5A].copy_from_slice(b"1986");
        let mut code = vec![0u8; 0x60];
        code[..21].copy_from_slice(&[0xA9, 0x0F, 0x8D, 0x18, 0xD4, 0xA9, 0x09, 0x8D, 0x05, 0xD4, 0xA9, 0xF0, 0x8D, 0x06, 0xD4, 0xA9, 0x08, 0x8D, 0x03, 0xD4, 0x60]);
        let play = [
            0xE6, 0x20, 0xA5, 0x20, 0x29, 0x0F, 0xD0, 0x1A, 0xA5, 0x21, 0x49, 0x01, 0x85, 0x21, 0xAA, 0xBD, 0x50, 0x10, 0x8D, 0x00, 0xD4, 0xBD, 0x52, 0x10, 0x8D, 0x01, 0xD4, 0xA9, 0x41, 0x8D, 0x04, 0xD4, 0x60, 0xEA, 0xC9, 0x0C,
            0xD0, 0x05, 0xA9, 0x40, 0x8D, 0x04, 0xD4, 0x60,
        ];
        code[0x20..0x20 + play.len()].copy_from_slice(&play);
        code[0x50..0x54].copy_from_slice(&[0x45, 0xDA, 0x1D, 0x2B]);
        let mut file = h;
        file.extend([0x00, 0x10]);
        file.extend(code);
        file
    }

    #[test]
    fn a_sid_tune_runs_and_its_voice_becomes_notes_on_a_sid_patch() {
        let bytes = sid_file();
        let imported = import(&bytes, "tones.sid", &About { seconds: 4.0, ..Default::default() }).unwrap();
        let a = &imported.arrangement;
        assert_eq!((a.title.as_str(), a.artist.as_str(), a.year, a.genre.as_str()), ("Tones", "Test", Some(1986), "C64 SID"));
        let notes: Vec<_> = a.parts.iter().flat_map(|p| p.notes.iter()).collect();
        // 4 s at 50 frames: twelve notes, E5 and A4 by turns.
        assert!((11..=13).contains(&notes.len()), "{}", notes.len());
        assert!(notes.iter().all(|n| n.key == 69.0 || n.key == 76.0));
        // Held 12 of every 16 frames.
        let gap = notes[1].beat - notes[0].beat;
        assert!((notes[0].dur / gap - 0.75).abs() < 0.05);
        let node = match &a.parts[0].voice {
            crate::gm::Voice::Node(node) => node.clone(),
            v => panic!("{v:?}"),
        };
        assert!(node.contains("osc: @pulse width: 0.5") && node.contains("sustain: 1"), "{node}");
        let doc = makepad_score_live::model::load(&imported.source).unwrap_or_else(|d| panic!("{d:?}")).doc;
        assert!(!doc.loops.is_empty());
    }
}
