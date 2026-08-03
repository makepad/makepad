//! Makepad Arcade stock asset library — an AI-queryable index over the CC0
//! model packs fetched by `apps/arcade/download_assets.sh`.
//!
//! The point of this crate is that an agent (or a kid talking to one) finds a
//! model by *describing* it, never by knowing its filename. Two consumers:
//!
//! - the cloud agent, via [`agent::tool_descriptor`] + [`agent::execute`] —
//!   it SEARCHES, it never receives the catalogue (hundreds of entries do not
//!   fit in a prompt, and the library grows with every pack);
//! - the local librarian model, via [`AssetIndex::best`] — a single best
//!   match plus a confidence signal, so the app can decide whether to act
//!   locally or escalate.
//!
//! Ids are stable, human-readable and guessable (`kenney/racing/vehicle-truck-yellow`)
//! because the AI writes them into game code. They are keyed on where the file
//! lives, not on its category, so retuning the category tree never breaks a
//! saved game.

pub mod agent;
pub mod aliases;
pub mod audio_aliases;
mod glb;

use std::path::{Path, PathBuf};

pub use aliases::CATEGORIES;
pub use audio_aliases::AUDIO_CATEGORIES;

/// What kind of thing an entry is. Music is distinguished from Sound so a game
/// never fires a 30-second track as a hit sound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssetKind {
    Model,
    Sound,
    Music,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AssetKind::Model => "model",
            AssetKind::Sound => "sound",
            AssetKind::Music => "music",
        }
    }
}

/// Where an asset came from. The architecture stays multi-source (KayKit is
/// already a second source, others may follow); only Kenney and KayKit ship
/// today.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Kenney,
    KayKit,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Kenney => "kenney",
            Source::KayKit => "kaykit",
        }
    }
    pub fn credit(self) -> &'static str {
        match self {
            Source::Kenney => "Kenney (kenney.nl) — CC0",
            Source::KayKit => "KayKit / Kay Lousberg (kaylousberg.com) — CC0",
        }
    }
    pub fn url(self) -> &'static str {
        match self {
            Source::Kenney => "https://kenney.nl/assets",
            Source::KayKit => "https://kaylousberg.itch.io/",
        }
    }
    fn from_dir(name: &str) -> Option<Source> {
        match name {
            "kenney" => Some(Source::Kenney),
            "kaykit" | "characters" => Some(Source::KayKit),
            _ => None,
        }
    }
}

/// One model in the library.
#[derive(Clone, Debug)]
pub struct AssetEntry {
    /// Stable public id, e.g. `kenney/racing/vehicle-truck-yellow`. This is
    /// what an agent writes into game code — never a filesystem path.
    pub id: String,
    /// Human display name from the curated table, else a prettified stem.
    pub name: String,
    pub kind: AssetKind,
    pub path: PathBuf,
    pub source: Source,
    pub pack: String,
    pub license: &'static str,
    pub credit: &'static str,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    /// Has a skin/skeleton — safe to drive with animation clips. Models only.
    pub rigged: bool,
    pub animated: bool,
    /// Approximate bounding size in model units, when the GLB exposed it.
    pub size: Option<[f32; 3]>,
    /// Container format, e.g. "glb" or "ogg".
    pub format: String,
    /// Playback length in seconds when known. Audio only, and only when the
    /// container exposed it cheaply.
    pub duration: Option<f32>,
    /// Whether this is meant to loop (engines, ambience) rather than fire once.
    pub loops: bool,
    /// False when the repo has no decoder for this format — see
    /// [`AssetIndex::undecodable`].
    pub decodable: bool,
    pub bytes: u64,
}

impl AssetEntry {
    /// Longest bounding dimension, for "big"/"small" filters. `None` when the
    /// GLB did not expose bounds.
    pub fn max_extent(&self) -> Option<f32> {
        self.size.map(|s| s[0].max(s[1]).max(s[2]))
    }
}

/// A ranked search hit.
#[derive(Clone, Debug)]
pub struct Hit<'a> {
    pub entry: &'a AssetEntry,
    pub score: u32,
    /// Which query terms actually matched — useful for explaining a choice.
    pub matched: Vec<String>,
}

/// Search filters. All optional; `None` means "don't care".
#[derive(Clone, Default, Debug)]
pub struct Filters {
    pub rigged_only: bool,
    pub category: Option<String>,
    pub source: Option<Source>,
    pub max_extent: Option<f32>,
    pub kind: Option<AssetKind>,
}

/// The library. Built by walking the downloaded model directories.
#[derive(Clone, Debug, Default)]
pub struct AssetIndex {
    entries: Vec<AssetEntry>,
    /// Directories that were expected but absent — reported, never fatal.
    missing: Vec<PathBuf>,
}

impl AssetIndex {
    /// Walk `root` (typically `apps/arcade/resources`) and index every `.glb`
    /// under a recognised source directory. An absent or empty root yields an
    /// empty index plus a `missing` note — never an error, because the models
    /// are gitignored and a fresh checkout legitimately has none.
    pub fn build(root: &Path) -> AssetIndex {
        let mut index = AssetIndex::default();
        if !root.is_dir() {
            index.missing.push(root.to_path_buf());
            return index;
        }
        let mut source_dirs: Vec<(Source, PathBuf)> = Vec::new();
        // resources/models/<source>/<pack>/*.glb
        let models = root.join("models");
        if models.is_dir() {
            let mut subs = read_dir_sorted(&models);
            subs.retain(|p| p.is_dir());
            for dir in subs {
                let name = file_name(&dir);
                if let Some(src) = Source::from_dir(&name) {
                    source_dirs.push((src, dir));
                }
            }
        } else {
            index.missing.push(models);
        }
        // resources/characters/*.glb — the original KayKit drop location.
        let chars = root.join("characters");
        if chars.is_dir() {
            source_dirs.push((Source::KayKit, chars));
        }
        // resources/audio/<source>/<pack>/*.ogg
        let audio = root.join("audio");
        if audio.is_dir() {
            let mut subs = read_dir_sorted(&audio);
            subs.retain(|p| p.is_dir());
            for dir in subs {
                if let Some(src) = Source::from_dir(&file_name(&dir)) {
                    source_dirs.push((src, dir));
                }
            }
        } else {
            index.missing.push(audio);
        }

        for (source, dir) in source_dirs {
            // A source dir either holds packs (subdirs) or files directly.
            let children = read_dir_sorted(&dir);
            let packs: Vec<(String, PathBuf)> = if children.iter().any(|p| p.is_dir()) {
                children
                    .into_iter()
                    .filter(|p| p.is_dir())
                    .map(|p| (file_name(&p), p))
                    .collect()
            } else {
                vec![(file_name(&dir), dir.clone())]
            };
            for (pack, pack_dir) in packs {
                for file in read_dir_sorted(&pack_dir) {
                    match file.extension().and_then(|e| e.to_str()) {
                        Some("glb") => index.entries.push(build_entry(source, &pack, &file)),
                        Some("ogg") | Some("wav") => {
                            index.entries.push(build_audio_entry(source, &pack, &file))
                        }
                        _ => {}
                    }
                }
            }
        }
        index.entries.sort_by(|a, b| a.id.cmp(&b.id));
        index
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }
    /// Directories that were expected but not present — the caller can turn
    /// this into a "run download_assets.sh" hint.
    pub fn missing(&self) -> &[PathBuf] {
        &self.missing
    }

    /// Entries the repo currently cannot decode. Kenney ships audio as Ogg
    /// Vorbis only and this tree has no vorbis decoder, so these are indexed
    /// and searchable but not yet playable — callers should surface that
    /// rather than silently playing nothing.
    pub fn undecodable(&self) -> Vec<&AssetEntry> {
        self.entries.iter().filter(|e| !e.decodable).collect()
    }

    pub fn count_of(&self, kind: AssetKind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    /// Resolve an exact id. Generated code calls this; a miss is a hallucinated
    /// id and should fail loudly — see [`AssetIndex::resolve_or_explain`].
    pub fn resolve(&self, id: &str) -> Option<&AssetEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Resolve, or produce an error naming near-misses, so a hallucinated id
    /// fails fast with something actionable instead of rendering nothing.
    pub fn resolve_or_explain(&self, id: &str) -> Result<&AssetEntry, String> {
        if let Some(e) = self.resolve(id) {
            return Ok(e);
        }
        // Re-use the ranking to suggest alternatives: the tail of an id is the
        // most descriptive part, so search on that.
        let probe = id.rsplit('/').next().unwrap_or(id).replace(['-', '_'], " ");
        let near: Vec<&str> = self
            .find(&probe)
            .into_iter()
            .take(3)
            .map(|h| h.entry.id.as_str())
            .collect();
        if near.is_empty() {
            Err(format!(
                "unknown model id '{id}' and no near match in a library of {} models \
                 — call find_model to search",
                self.entries.len()
            ))
        } else {
            Err(format!(
                "unknown model id '{id}' — did you mean: {}",
                near.join(", ")
            ))
        }
    }

    /// Rank the library against a natural-language query. Multi-word phrases
    /// work: every term scores independently and the total decides, so
    /// "fast red car" beats a plain "car" without requiring all terms to hit.
    pub fn find(&self, query: &str) -> Vec<Hit<'_>> {
        self.find_filtered(query, &Filters::default())
    }

    pub fn find_filtered(&self, query: &str, filters: &Filters) -> Vec<Hit<'_>> {
        let terms = tokenize(query);
        let mut hits: Vec<Hit> = Vec::new();
        for entry in &self.entries {
            if filters.rigged_only && !entry.rigged {
                continue;
            }
            if let Some(cat) = &filters.category {
                if !entry.categories.iter().any(|c| c.starts_with(cat.as_str())) {
                    continue;
                }
            }
            if let Some(src) = filters.source {
                if entry.source != src {
                    continue;
                }
            }
            if let Some(max) = filters.max_extent {
                if entry.max_extent().map(|e| e > max).unwrap_or(false) {
                    continue;
                }
            }
            if let Some(kind) = filters.kind {
                if entry.kind != kind {
                    continue;
                }
            }
            let (mut score, matched) = score_entry(entry, &terms);
            // A lone substring brush (score 2-3) is noise, not a match: without
            // this floor, asking for a "digger" — which the library does not
            // contain — returned an unrelated model instead of nothing, and a
            // confidently wrong answer is worse than an empty one.
            if score >= MIN_HIT_SCORE {
                score += whole_query_bonus(entry, query);
                hits.push(Hit { entry, score, matched });
            }
        }
        // Deterministic: score desc, then id asc — never HashMap order.
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.entry.id.cmp(&b.entry.id)));
        hits
    }

    /// Single best match plus a confidence in 0.0..=1.0, for the local
    /// librarian tier: high confidence means "just do it", low means the app
    /// should escalate to the cloud agent rather than guess.
    pub fn best(&self, query: &str) -> Option<(&AssetEntry, f32)> {
        let hits = self.find(query);
        let top = hits.first()?;
        let runner_up = hits.get(1).map(|h| h.score).unwrap_or(0);
        // Confidence blends absolute strength with the margin over the next
        // candidate: a strong hit that is also clearly ahead is trustworthy.
        let strength = (top.score as f32 / 12.0).min(1.0);
        let margin = if top.score == 0 {
            0.0
        } else {
            (top.score - runner_up) as f32 / top.score as f32
        };
        Some((top.entry, (0.6 * strength + 0.4 * margin).clamp(0.0, 1.0)))
    }

    /// Category names present in the library, with counts. Sorted for
    /// determinism.
    pub fn category_counts(&self) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for e in &self.entries {
            for c in &e.categories {
                match out.iter_mut().find(|(name, _)| name == c) {
                    Some((_, n)) => *n += 1,
                    None => out.push((c.clone(), 1)),
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

// ------------------------------------------------------------------ scoring

/// Minimum score for an entry to count as a hit at all. Tuned so a
/// word-inside-a-phrase match (4) or a category match (3) qualifies but a bare
/// substring brush (2) does not.
const MIN_HIT_SCORE: u32 = 3;

/// Split a query into lowercase terms, dropping stop words that carry no
/// selection power ("a", "the", "some") but keeping short meaningful ones.
fn tokenize(q: &str) -> Vec<String> {
    // Function words must be dropped, not merely down-weighted: the curated
    // aliases are phrases ("something to shoot at"), so a stray preposition in
    // the query would otherwise score a full word-in-phrase match and drag in
    // unrelated entries — "a digger like at the roadworks" matched a flying
    // enemy purely on the word "at".
    const STOP: &[&str] = &[
        "a", "an", "the", "some", "i", "want", "need", "please", "me", "my", "for", "of", "with",
        "to", "in", "on", "is", "it", "that", "give", "add", "put", "make", "get", "can", "you",
        "and", "like", "at", "from", "by", "into", "when", "where", "then", "this", "these",
        "those", "there", "here", "be", "are", "was", "were", "do", "does", "did", "has", "have",
        "had", "will", "would", "should", "could", "about", "as", "so", "if", "or", "but", "not",
        "im", "its", "your", "our", "their", "his", "her", "please", "just", "really",
    ];
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty() && !STOP.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Score one entry against the query terms.
///
/// Weights encode the intent: an exact keyword hit is worth far more than a
/// substring brush, and a phrase alias ("something to drive") that matches a
/// multi-word query is worth the most, because those are the queries people
/// actually type.
fn score_entry(entry: &AssetEntry, terms: &[String]) -> (u32, Vec<String>) {
    let mut score = 0u32;
    let mut matched: Vec<String> = Vec::new();
    for term in terms {
        let mut best = 0u32;
        // The term itself plus its curated expansions.
        let mut probes: Vec<String> = vec![term.clone()];
        for e in aliases::expand(term) {
            probes.push(e.to_string());
        }
        for probe in &probes {
            let exact_bonus = if probe == term { 2 } else { 0 };
            for kw in &entry.keywords {
                if kw == probe {
                    best = best.max(6 + exact_bonus);
                } else if kw.split_whitespace().any(|w| w == probe) {
                    // A word inside a phrase alias ("drive" in "something to drive").
                    best = best.max(4 + exact_bonus);
                } else if kw.len() > 3 && probe.len() > 3 && kw.contains(probe.as_str()) {
                    best = best.max(2 + exact_bonus);
                }
            }
            for cat in &entry.categories {
                if cat.split('/').any(|c| c == probe) {
                    best = best.max(3 + exact_bonus);
                }
            }
        }
        if best > 0 {
            score += best;
            matched.push(term.clone());
        }
    }
    // Reward covering more of the query: two matched terms out of two is a
    // better answer than two out of five.
    if matched.len() > 1 {
        score += (matched.len() as u32 - 1) * 2;
    }
    (score, matched)
}

/// Bonuses that depend on the query as a whole rather than term by term.
///
/// Two cases the per-term pass gets wrong on its own: "something to drive"
/// scored the same for a truck (whose alias is exactly that) and for a road
/// (aliased "something to drive on"); and "coin" scored a coin *sound* as
/// highly as the coin *model*, because both merely list it as a keyword.
fn whole_query_bonus(entry: &AssetEntry, query: &str) -> u32 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return 0;
    }
    let mut bonus = 0;
    // The query IS one of this entry's aliases, verbatim.
    if entry.keywords.iter().any(|k| *k == q) {
        bonus += 8;
    }
    // The query names the thing: its display name or the last path segment of
    // its id (`.../vehicle-truck-red` -> "vehicle truck red").
    if entry.name.to_lowercase() == q {
        bonus += 6;
    }
    if let Some(stem) = entry.id.rsplit('/').next() {
        let stem = stem.replace(['-', '_'], " ").to_lowercase();
        if stem == q {
            bonus += 6;
        } else if stem.split_whitespace().any(|w| w == q) {
            // "coin" naming platformer/coin, not merely listed as a keyword.
            bonus += 4;
        }
    }
    bonus
}

// ------------------------------------------------------------ entry building

fn build_entry(source: Source, pack: &str, file: &Path) -> AssetEntry {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
    let key = format!("{pack}/{stem}");
    let curated = aliases::lookup(&key);

    // Filename tokens are the floor; curation is the value on top.
    let mut keywords: Vec<String> = Vec::new();
    for tok in stem.split(['-', '_']) {
        push_unique(&mut keywords, tok.to_lowercase());
    }
    push_unique(&mut keywords, pack.to_lowercase());
    if let Some(c) = curated {
        for a in c.aliases {
            push_unique(&mut keywords, a.to_string());
        }
    }

    let categories: Vec<String> = curated
        .map(|c| c.categories.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let name = curated
        .map(|c| c.name.to_string())
        .unwrap_or_else(|| prettify(&stem));

    let probe = glb::probe(file);
    let bytes = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    AssetEntry {
        id: format!("{}/{}/{}", source.as_str(), pack, stem),
        name,
        kind: AssetKind::Model,
        path: file.to_path_buf(),
        source,
        pack: pack.to_string(),
        license: "CC0-1.0",
        credit: source.credit(),
        categories,
        keywords,
        rigged: probe.rigged,
        animated: probe.animated,
        size: probe.size,
        format: "glb".to_string(),
        duration: None,
        loops: false,
        decodable: true,
        bytes,
    }
}

/// Build a sound/music entry. Curation is keyed on the *family* (the filename
/// with Kenney's variant number stripped), so `footstep_wood_003` inherits the
/// `footstep_wood` row.
fn build_audio_entry(source: Source, pack: &str, file: &Path) -> AssetEntry {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("sound").to_string();
    let family = audio_aliases::family_of(&stem);
    let key = format!("{pack}/{family}");
    let curated = audio_aliases::lookup(&key);
    let format = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut keywords: Vec<String> = Vec::new();
    // Filename tokens are the floor. camelCase is split too, because Kenney's
    // audio uses it heavily ("spaceEngineLarge" -> space engine large).
    for tok in split_ident(&stem) {
        push_unique(&mut keywords, tok);
    }
    push_unique(&mut keywords, pack.replace('-', " "));
    if let Some(c) = curated {
        for a in c.aliases {
            push_unique(&mut keywords, a.to_string());
        }
    }

    let kind = curated.map(|c| c.kind).unwrap_or(AssetKind::Sound);
    let categories: Vec<String> = curated
        .map(|c| c.categories.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let name = curated
        .map(|c| c.name.to_string())
        .unwrap_or_else(|| prettify(&family));

    // Looping material: engines, ambience and force fields are held, not fired.
    let loops = keywords.iter().any(|k| k == "loop")
        || categories.iter().any(|c| c == "sound/engine");

    let bytes = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);

    AssetEntry {
        id: format!("{}/{}/{}", source.as_str(), pack, stem),
        name,
        kind,
        path: file.to_path_buf(),
        source,
        pack: pack.to_string(),
        license: "CC0-1.0",
        credit: source.credit(),
        categories,
        keywords,
        rigged: false,
        animated: false,
        size: None,
        duration: None,
        loops,
        // No vorbis decoder exists in this repo, so ogg is indexed and
        // searchable but not yet playable — see AssetIndex::undecodable.
        decodable: format != "ogg",
        format,
        bytes,
    }
}

/// Split an identifier into lowercase words on separators AND camelCase
/// boundaries: "spaceEngineLarge" -> ["space", "engine", "large"].
fn split_ident(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if ch.is_ascii_uppercase() && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur.push(ch.to_ascii_lowercase());
        } else {
            cur.push(ch.to_ascii_lowercase());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()));
    out
}

fn prettify(stem: &str) -> String {
    stem.split(['-', '_'])
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !s.is_empty() && !v.contains(&s) {
        v.push(s);
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

/// Directory listing in sorted order — the index must not inherit filesystem
/// enumeration order, or ids and rankings would shift between machines.
fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}
