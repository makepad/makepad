//! The catalogue: one row per MIDI file of the clean subset, with its
//! artist and title (the archive names them), the MSD track it matches
//! best (its year, its tagtraum genre, its Last.fm tags) and what the file
//! holds ([`crate::analyse::Features`]). Written once as `index.tsv` beside
//! the downloads; searched by decade, genre, tag, artist and title words.

use crate::analyse::{self, Features};
use crate::dataset;
use crate::gm::Role;
use crate::meta::{self, norm};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The first 8 hex digits of the file's MD5: stable across rebuilds.
    pub id: String,
    pub artist: String,
    pub title: String,
    /// The file's path in the archive.
    pub path: String,
    pub md5: String,
    /// The MSD track it matches best, and how well (0..1; 0 when it was
    /// found by artist and title instead).
    pub tid: String,
    pub matched: f32,
    pub year: Option<u16>,
    /// The tagtraum genres, or one guessed from the tags and instruments
    /// (`~electronic`) when the MSD has none.
    pub genres: Vec<String>,
    /// The strongest Last.fm tags (normalised) with their weights.
    pub tags: Vec<(String, f32)>,
    pub features: Features,
    /// The file's format: `mid`, or a tracker or chip format (`mod`,
    /// `s3m`, `xm`, `it`, `sid` ...).
    pub format: String,
    /// The machine it was made for (`c64`, `amiga`, `pc`, `nes` ...);
    /// empty for MIDI.
    pub platform: String,
    /// The game it is from, when it is game music.
    pub game: String,
    /// The library it is in: `lmd` (this crate's MIDI set) or a chip
    /// collection (`hvsc` ...), which [`bytes`] reads it from.
    pub archive: String,
}

impl Entry {
    pub fn decade(&self) -> Option<u16> {
        self.year.map(|y| y / 10 * 10)
    }

    /// "Artist - Title (1984)".
    pub fn label(&self) -> String {
        match self.year {
            Some(year) => format!("{} - {} ({year})", self.artist, self.title),
            None => format!("{} - {}", self.artist, self.title),
        }
    }

    /// A file name: lowercase words joined by `_`.
    pub fn slug(&self) -> String {
        let mut out = String::new();
        for c in format!("{} {}", self.artist, self.title).chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
            } else if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
        out.trim_end_matches('_').chars().take(60).collect()
    }
}

pub fn index_path() -> PathBuf {
    dataset::dir().join("index.tsv")
}

/// Artist and title from `clean_midi/Artist/Title.mid` (a second version
/// of a song is `Title.1.mid`).
pub fn names(path: &str) -> Option<(String, String)> {
    let mut parts = path.rsplit('/');
    let file = parts.next()?;
    let artist = parts.next()?.trim().to_string();
    let stem = file.strip_suffix(".mid").or_else(|| file.strip_suffix(".MID")).or_else(|| file.strip_suffix(".midi"))?;
    let stem = match stem.rsplit_once('.') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => stem,
    };
    Some((artist, stem.trim().to_string()))
}

/// A genre from the tags (the strongest genre-like tag) or the instruments,
/// marked with `~` as a guess.
fn guess_genre(tags: &[(String, f32)], f: &Features) -> String {
    const GENRES: [&str; 24] = [
        "synthpop", "newwave", "electronic", "dance", "disco", "house", "techno", "hiphop", "rap", "rnb", "soul", "funk", "jazz", "blues", "country", "folk", "metal", "punk", "reggae", "classical", "rock", "pop", "latin", "world",
    ];
    if let Some((tag, _)) = tags.iter().take(12).find(|(t, _)| GENRES.iter().any(|g| t == g || t.ends_with(g))) {
        return format!("~{tag}");
    }
    let synth = f.programs.iter().filter(|p| matches!(p, 38 | 39 | 50 | 51 | 62 | 63 | 80..=103)).count();
    let distorted = f.programs.iter().any(|p| matches!(p, 29 | 30));
    let jazz = f.programs.iter().any(|p| matches!(p, 26 | 64..=67)) && f.programs.contains(&32);
    let g = if distorted {
        "rock"
    } else if synth >= 2 {
        "electronic"
    } else if jazz {
        "jazz"
    } else if f.roles.len() == 1 && f.roles[0] == Role::Keys {
        "piano"
    } else {
        "pop"
    };
    format!("~{g}")
}

/// Build the catalogue from the installed files. `progress(done, total)`
/// as the songs are read; `threads` readers.
pub fn build(threads: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Vec<Entry>, String> {
    let missing = dataset::missing();
    if !missing.is_empty() {
        return Err(format!("not installed: {} (midi_library install)", missing.iter().map(|m| m.short).collect::<Vec<_>>().join(", ")));
    }
    let gz = std::fs::read(dataset::path(&dataset::CLEAN_MIDI)).map_err(|e| e.to_string())?;
    let tar = crate::tar::gunzip(&gz).map_err(|e| format!("clean_midi.tar.gz: {e}"))?;
    drop(gz);
    let files: Vec<crate::tar::Entry> = crate::tar::entries(&tar).into_iter().filter(|e| names(&e.path).is_some()).collect();
    let matches = meta::matches(&std::fs::read(dataset::path(&dataset::MATCH_SCORES)).map_err(|e| e.to_string())?)?;
    // Read every file: its md5 and features.
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = files.len();
    let chunk = total.div_ceil(threads.max(1)).max(1);
    let mut read: Vec<Option<(String, Features)>> = Vec::with_capacity(total);
    std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                let tar = &tar;
                let done = &done;
                s.spawn(move || {
                    part.iter()
                        .map(|f| {
                            let bytes = &tar[f.offset..f.offset + f.len];
                            let out = analyse::read(bytes).ok().filter(|song| !song.parts.is_empty()).map(|song| (crate::md5::hex(bytes), analyse::features(&song)));
                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            if n % 500 == 0 || n == total {
                                progress(n, total);
                            }
                            out
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            read.extend(h.join().unwrap_or_default());
        }
    });
    // The MSD tracks a file may be: LMD's matches (audio matching often
    // finds a cover version) and the tracks of the same artist and title.
    let tracks = meta::tracks(&std::fs::read_to_string(dataset::path(&dataset::TRACKS)).map_err(|e| e.to_string())?);
    let candidates: Vec<Vec<(String, f32)>> = files
        .iter()
        .zip(&read)
        .map(|(f, r)| {
            let Some((md5, _)) = r else { return Vec::new() };
            let mut c: Vec<(String, f32)> = matches.get(md5).cloned().unwrap_or_default();
            if let Some((artist, title)) = names(&f.path) {
                for tid in tracks.by_name.get(&(norm(&artist), norm(&title))).into_iter().flatten() {
                    if !c.iter().any(|x| &x.0 == tid) {
                        c.push((tid.clone(), 0.0));
                    }
                }
            }
            c
        })
        .collect();
    let wanted: HashSet<String> = candidates.iter().flatten().map(|t| t.0.clone()).collect();
    let year_text = std::fs::read_to_string(dataset::path(&dataset::YEARS)).map_err(|e| e.to_string())?;
    let years = meta::years(&year_text);
    let first_years = meta::first_years(&year_text);
    drop(year_text);
    // Research-only extras: used when the person installed them, otherwise
    // the index goes without (guessed genres, no tags).
    let genres = if dataset::research_installed(&dataset::GENRES) { meta::genres_zip(&dataset::path(&dataset::GENRES))? } else { Default::default() };
    let tags = if dataset::research_installed(&dataset::TAGS) { meta::tags(&dataset::path(&dataset::TAGS), &wanted)? } else { Default::default() };
    let mut out = Vec::new();
    for ((file, read), candidates) in files.iter().zip(read).zip(candidates) {
        let Some((md5, features)) = read else { continue };
        let Some((artist, title)) = names(&file.path) else { continue };
        let artist_key = norm(&artist);
        // The track by this artist (the original, not a cover), with the
        // most to say about it; else LMD's best match.
        let by_artist = |t: &String| tracks.artist_of.get(t).is_some_and(|a| *a == artist_key);
        let info = |t: &String| tags.contains_key(t) as u8 * 2 + genres.contains_key(t) as u8;
        let (tid, matched) = candidates
            .iter()
            .filter(|(t, _)| by_artist(t))
            .max_by(|a, b| (info(&a.0), a.1).partial_cmp(&(info(&b.0), b.1)).unwrap_or(std::cmp::Ordering::Equal))
            .or_else(|| candidates.first())
            .cloned()
            .unwrap_or_default();
        // The song's first release: the earliest year of the same artist
        // and title (a compilation is later), else the track's.
        let year = first_years.get(&(artist_key.clone(), norm(&title))).copied().or_else(|| years.get(&tid).copied());
        let song_tags: Vec<(String, f32)> = tags.get(&tid).map(|t| t.iter().take(16).cloned().collect()).unwrap_or_default();
        let song_genres = match genres.get(&tid) {
            Some(g) => g.iter().map(|g| norm(g)).collect(),
            None => vec![guess_genre(&song_tags, &features)],
        };
        out.push(Entry { id: md5[..8].to_string(), artist, title, path: file.path.clone(), md5, tid, matched, year, genres: song_genres, tags: song_tags, features, format: "mid".into(), platform: String::new(), game: String::new(), archive: "lmd".into() });
    }
    out.sort_by(|a, b| (a.artist.to_lowercase(), a.title.to_lowercase()).cmp(&(b.artist.to_lowercase(), b.title.to_lowercase())));
    Ok(out)
}

const COLUMNS: [&str; 24] = [
    "id", "artist", "title", "year", "genres", "tags", "bpm", "key", "sig", "parts", "programs", "roles", "seconds", "notes", "melody", "quality", "tid", "matched", "md5", "path", "format", "platform", "game", "archive",
];

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

pub fn to_tsv(entries: &[Entry]) -> String {
    let mut out = COLUMNS.join("\t");
    out.push('\n');
    for e in entries {
        let f = &e.features;
        let row = [
            e.id.clone(),
            clean(&e.artist),
            clean(&e.title),
            e.year.map_or(String::new(), |y| y.to_string()),
            e.genres.join(","),
            e.tags.iter().map(|(t, v)| format!("{t}:{v:.0}")).collect::<Vec<_>>().join(","),
            format!("{}", f.bpm),
            f.key.clone(),
            f.sig.clone(),
            f.parts.to_string(),
            f.programs.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(","),
            f.roles.iter().map(|r| r.name()).collect::<Vec<_>>().join(","),
            format!("{}", f.seconds),
            f.notes.to_string(),
            (f.melody as u8).to_string(),
            f.quality.to_string(),
            e.tid.clone(),
            format!("{:.3}", e.matched),
            e.md5.clone(),
            clean(&e.path),
            e.format.clone(),
            e.platform.clone(),
            clean(&e.game),
            e.archive.clone(),
        ];
        out.push_str(&row.join("\t"));
        out.push('\n');
    }
    out
}

pub fn from_tsv(text: &str) -> Vec<Entry> {
    let mut lines = text.lines();
    let Some(header) = lines.next() else { return Vec::new() };
    let col: HashMap<&str, usize> = header.split('\t').enumerate().map(|(i, c)| (c, i)).collect();
    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let get = |name: &str| col.get(name).and_then(|i| fields.get(*i)).copied().unwrap_or("");
            let list = |name: &str| get(name).split(',').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
            Some(Entry {
                id: get("id").to_string(),
                artist: get("artist").to_string(),
                title: get("title").to_string(),
                path: get("path").to_string(),
                md5: get("md5").to_string(),
                tid: get("tid").to_string(),
                matched: get("matched").parse().unwrap_or(0.0),
                year: get("year").parse().ok(),
                genres: list("genres"),
                tags: list("tags").into_iter().filter_map(|t| {
                    let (tag, v) = t.rsplit_once(':')?;
                    Some((tag.to_string(), v.parse().unwrap_or(0.0)))
                }).collect(),
                features: Features {
                    bpm: get("bpm").parse().unwrap_or(120.0),
                    key: get("key").to_string(),
                    sig: get("sig").to_string(),
                    parts: get("parts").parse().unwrap_or(0),
                    programs: list("programs").iter().filter_map(|p| p.parse().ok()).collect(),
                    roles: list("roles").iter().filter_map(|r| Role::parse(r)).collect(),
                    seconds: get("seconds").parse().unwrap_or(0.0),
                    notes: get("notes").parse().unwrap_or(0),
                    melody: get("melody") == "1",
                    quality: get("quality").parse().unwrap_or(0),
                },
                // Catalogues from before these columns are MIDI.
                format: Some(get("format")).filter(|f| !f.is_empty()).unwrap_or("mid").to_string(),
                platform: get("platform").to_string(),
                game: get("game").to_string(),
                archive: Some(get("archive")).filter(|a| !a.is_empty()).unwrap_or("lmd").to_string(),
            })
        })
        .filter(|e| !e.id.is_empty())
        .collect()
}

pub fn write(entries: &[Entry], path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, to_tsv(entries)).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Vec<Entry>, String> {
    let own = std::fs::read_to_string(path).map_err(|e| format!("{}: {e} (build it: midi_library index)", path.display()));
    // The other installed libraries' catalogues (`library.tsv` beside
    // their downloads: the chip collections) join it.
    let mut extra = Vec::new();
    if path == index_path() {
        for lib in library_paths() {
            if let Ok(text) = std::fs::read_to_string(&lib) {
                extra.extend(from_tsv(&text));
            }
        }
    }
    match own {
        Ok(text) => {
            let mut all = from_tsv(&text);
            all.extend(extra);
            Ok(all)
        }
        Err(_) if !extra.is_empty() => Ok(extra),
        Err(e) => Err(e),
    }
}

/// The catalogues of the other libraries installed beside this one.
pub fn library_paths() -> Vec<PathBuf> {
    let Some(datasets) = dataset::dir().parent().map(Path::to_path_buf) else { return Vec::new() };
    let mut out: Vec<PathBuf> = std::fs::read_dir(&datasets).map(|dir| dir.filter_map(|e| e.ok()).map(|e| e.path().join("library.tsv")).filter(|p| p.is_file()).collect()).unwrap_or_default();
    out.sort();
    out
}

/// A song's file, from the library it is in.
pub fn bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    match entry.archive.as_str() {
        "" | "lmd" => midi(&[entry.path.as_str()])?.pop().flatten().ok_or_else(|| format!("{} is not in the installed MIDI archive", entry.path)),
        "hvsc" => crate::chip::hvsc::bytes(&entry.path),
        "web" => crate::chip::web::bytes(entry),
        other => Err(format!("no library `{other}` on this machine")),
    }
}

/// The MIDI files at `paths` in the installed archive (one decompression
/// for all of them), in order; none for a path not in it.
pub fn midi(paths: &[&str]) -> Result<Vec<Option<Vec<u8>>>, String> {
    let gz = std::fs::read(dataset::path(&dataset::CLEAN_MIDI)).map_err(|e| format!("the MIDI archive is not installed (midi_library install): {e}"))?;
    let tar = crate::tar::gunzip(&gz).map_err(|e| format!("clean_midi.tar.gz: {e}"))?;
    let files = crate::tar::entries(&tar);
    Ok(paths.iter().map(|p| files.iter().find(|f| f.path == *p).map(|f| tar[f.offset..f.offset + f.len].to_vec())).collect())
}

/// A decade as a query writes it: `80s`, `1980s`, `'80s`, `eighties`.
fn decade_of(word: &str) -> Option<u16> {
    const WORDS: [(&str, u16); 7] = [("fifties", 1950), ("sixties", 1960), ("seventies", 1970), ("eighties", 1980), ("nineties", 1990), ("noughties", 2000), ("2000s", 2000)];
    if let Some((_, d)) = WORDS.iter().find(|(w, _)| *w == word) {
        return Some(*d);
    }
    let digits = word.trim_start_matches('\'').strip_suffix('s')?;
    let n: u16 = digits.parse().ok()?;
    match digits.len() {
        2 if n % 10 == 0 => Some(if n >= 30 { 1900 + n } else { 2000 + n }),
        4 if n % 10 == 0 && (1900..2030).contains(&n) => Some(n),
        _ => None,
    }
}

/// Words a query says that name nothing ("C64 SID by Rob Hubbard").
const STOPWORDS: [&str; 10] = ["by", "the", "a", "an", "of", "from", "in", "on", "for", "and"];

/// What a query asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub decade: Option<u16>,
    pub years: Option<(u16, u16)>,
    pub artist: Option<String>,
    /// Words each matching a tag, genre, artist or title (all must).
    pub words: Vec<String>,
    pub min_quality: u8,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut q = Query::default();
        for raw in text.split_whitespace() {
            let lower = raw.to_lowercase();
            if let Some(artist) = lower.strip_prefix("artist:") {
                q.artist = Some(norm(artist));
            } else if let Some(min) = lower.strip_prefix("quality:") {
                q.min_quality = min.trim_start_matches(">=").parse().unwrap_or(0);
            } else if let Some(d) = decade_of(&lower) {
                q.decade = Some(d);
            } else if let Some((a, b)) = lower.split_once('-').and_then(|(a, b)| Some((a.parse::<u16>().ok()?, b.parse::<u16>().ok()?))).filter(|(a, b)| *a > 1900 && b >= a) {
                q.years = Some((a, b));
            } else if let Ok(year) = lower.parse::<u16>() {
                if year > 1900 {
                    q.years = Some((year, year));
                }
            } else if !norm(&lower).is_empty() && !STOPWORDS.contains(&norm(&lower).as_str()) {
                q.words.push(norm(&lower));
            }
        }
        q
    }
}

/// How well one word matches an entry, or none.
fn word_score(e: &Entry, artist: &str, title: &str, word: &str) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut take = |v: f32| best = Some(best.map_or(v, |b: f32| b.max(v)));
    if let Some((_, val)) = e.tags.iter().find(|(t, _)| t == word) {
        take(1.0 + val / 100.0);
    } else if word.len() >= 4 {
        if let Some((_, val)) = e.tags.iter().find(|(t, _)| t.contains(word)) {
            take(0.5 + val / 200.0);
        }
    }
    if e.genres.iter().any(|g| g.trim_start_matches('~') == word) {
        take(1.0);
    }
    if artist.contains(word) {
        take(1.5);
    }
    // The format, the machine and the game ("amiga mod", "c64", "snes rpg").
    if e.format == word || e.platform == word {
        take(1.2);
    }
    if !e.game.is_empty() && norm(&e.game).contains(word) {
        take(1.2);
    }
    if title.contains(word) {
        take(1.2);
    }
    best
}

/// The entries matching `query`, best first (one per song: the best
/// version of a title), at most `limit`.
pub fn search<'a>(entries: &'a [Entry], query: &str, limit: usize) -> Vec<(&'a Entry, f32)> {
    let q = Query::parse(query);
    let mut hits: Vec<(&Entry, f32)> = Vec::new();
    for e in entries {
        if e.features.quality < q.min_quality {
            continue;
        }
        let artist = norm(&e.artist);
        let title = norm(&e.title);
        if q.artist.as_ref().is_some_and(|a| !artist.contains(a.as_str())) {
            continue;
        }
        let mut score = 0.0f32;
        if let Some(decade) = q.decade {
            let tag = format!("{}s", decade % 100);
            let long = format!("{decade}s");
            if e.decade() == Some(decade) {
                score += 1.0;
            } else if e.year.is_none() && e.tags.iter().take(12).any(|(t, _)| *t == tag || *t == long) {
                score += 0.6;
            } else {
                continue;
            }
        }
        if let Some((a, b)) = q.years {
            if !e.year.is_some_and(|y| (a..=b).contains(&y)) {
                continue;
            }
        }
        // Words, two at a time where the pair is one tag ("new wave"). A
        // song missing some words still counts, below every song that has
        // them all ("80s synthpop, Jan Hammer-like" finds the synthpop).
        let mut missed = 0;
        let mut found = 0;
        let mut i = 0;
        while i < q.words.len() {
            let pair = q.words.get(i + 1).map(|next| format!("{}{next}", q.words[i]));
            if let Some(s) = pair.as_deref().and_then(|p| word_score(e, &artist, &title, p)) {
                // Two words as one ("rob hubbard", "new wave") count more
                // than the two matched apart.
                score += s * 2.5;
                found += 1;
                i += 2;
                continue;
            }
            match word_score(e, &artist, &title, &q.words[i]) {
                Some(s) => {
                    score += s;
                    found += 1;
                }
                None => missed += 1,
            }
            i += 1;
        }
        if found == 0 && !q.words.is_empty() {
            continue;
        }
        score -= missed as f32 * 10.0;
        score += e.features.quality as f32 / 100.0 + e.matched * 0.5;
        hits.push((e, score));
    }
    hits.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut seen = HashSet::new();
    hits.retain(|(e, _)| seen.insert((norm(&e.artist), norm(&e.title))));
    hits.truncate(limit);
    hits
}

/// The id or the "artist - title" a person names.
pub fn find<'a>(entries: &'a [Entry], name: &str) -> Option<&'a Entry> {
    if let Some(e) = entries.iter().find(|e| e.id == name) {
        return Some(e);
    }
    let (artist, title) = name.split_once(" - ")?;
    let (artist, title) = (norm(artist), norm(title));
    entries.iter().filter(|e| norm(&e.artist) == artist && norm(&e.title) == title).max_by_key(|e| e.features.quality)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(artist: &str, title: &str, year: Option<u16>, tags: &[&str], quality: u8) -> Entry {
        Entry {
            id: format!("{:08x}", artist.len() * 1000 + title.len()),
            artist: artist.into(),
            title: title.into(),
            path: format!("clean_midi/{artist}/{title}.mid"),
            md5: "0".repeat(32),
            tid: "TR".into(),
            matched: 0.6,
            year,
            genres: vec!["pop".into()],
            tags: tags.iter().map(|t| (t.to_string(), 100.0)).collect(),
            features: Features { bpm: 120.0, key: "C major".into(), sig: "4/4".into(), parts: 6, programs: vec![0, 33], roles: vec![Role::Drums, Role::Bass], seconds: 200.0, notes: 2000, melody: true, quality },
            format: "mid".into(),
            platform: String::new(),
            game: String::new(),
            archive: "lmd".into(),
        }
    }

    #[test]
    fn names_queries_and_search() {
        assert_eq!(names("clean_midi/A-ha/Take On Me.1.mid"), Some(("A-ha".into(), "Take On Me".into())));
        assert_eq!(names("clean_midi/Band/Hit.mid"), Some(("Band".into(), "Hit".into())));
        let q = Query::parse("80s synthpop artist:a-ha quality:60");
        assert_eq!((q.decade, q.artist.as_deref(), q.words.clone(), q.min_quality), (Some(1980), Some("aha"), vec!["synthpop".to_string()], 60));
        assert_eq!(Query::parse("eighties").decade, Some(1980));
        assert_eq!(Query::parse("1980s").decade, Some(1980));
        assert_eq!(Query::parse("1983-1986").years, Some((1983, 1986)));
        let all = vec![
            entry("A-ha", "Take On Me", Some(1985), &["synthpop", "80s", "newwave"], 80),
            entry("A-ha", "Take On Me", Some(1985), &["synthpop"], 40),
            entry("Depeche Mode", "Enjoy the Silence", None, &["synthpop", "80s"], 70),
            entry("Nirvana", "Lithium", Some(1991), &["grunge", "90s"], 90),
            entry("Blondie", "Call Me", Some(1979), &["newwave"], 60),
        ];
        let hits: Vec<&str> = search(&all, "80s synthpop", 10).iter().map(|(e, _)| e.artist.as_str()).collect();
        assert_eq!(hits, vec!["A-ha", "Depeche Mode"]);
        assert_eq!(search(&all, "80s synthpop", 10)[0].0.features.quality, 80);
        assert_eq!(search(&all, "new wave", 10).len(), 2);
        assert_eq!(search(&all, "lithium", 10)[0].0.artist, "Nirvana");
        assert!(search(&all, "70s grunge", 10).is_empty());
        // Words that match nothing rank a song lower, not out.
        let partial: Vec<&str> = search(&all, "80s synthpop hammerlike", 10).iter().map(|(e, _)| e.artist.as_str()).collect();
        assert_eq!(partial, vec!["A-ha", "Depeche Mode"]);
        assert!(search(&all, "zzz", 10).is_empty());
        assert_eq!(find(&all, "a-ha - take on me").map(|e| e.features.quality), Some(80));
        assert_eq!(all[0].slug(), "a_ha_take_on_me");
        let back = from_tsv(&to_tsv(&all));
        assert_eq!(back, all);
    }
}
