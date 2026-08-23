//! OBSERVED ORIGIN DIRECTORIES: a folder on disk that the store keeps
//! catalogued, live, without ever copying what is in it.
//!
//! This is the LIVECODING seam. Somebody — a person with an editor, or a
//! coding agent with a shell — drops `my_look.splash` into
//! `local/vjfx/`, and a second later it is a normal catalog asset with a
//! thumbnail, an alias and a search row. They save the file again and the
//! store publishes a NEW REVISION of that same asset, which is what a
//! running VJ slot watches for so the effect recompiles in place.
//!
//! Three properties make that honest rather than magic:
//!
//! - **No copy.** The published `Source` file is a [`crate::blobrefs`]
//!   reference: the store hashes the file where it lies and records path +
//!   size + digest. The bytes stay in the user's checkout, which is where
//!   the editor and `git` expect them. Only the derived thumbnail is the
//!   store's own.
//! - **Hash-immutable identity, mutable alias.** A revision is named by its
//!   content, so "editing an asset" is a category error. Editing a FILE
//!   publishes a new revision of the same asset and re-points the alias
//!   `vjfx/<file-stem>` at it. Anything holding that alias sees a
//!   republish event; anything holding a revision keeps exactly the bytes
//!   it asked for.
//! - **Nothing happens twice.** The observer compares the file's digest
//!   against the head revision's `Source` digest before publishing, so a
//!   touch, a `git checkout` that restores identical bytes, or an editor
//!   that writes-then-rewrites costs one hash and no catalog write.
//!
//! ## Ownership
//!
//! Reference admission is loopback-only by policy ([`crate::BlobRefPolicy`])
//! because it is precisely "read any file this process can read". So the
//! observer belongs to the process that HOSTS the store — asset-ui's
//! embedded server, or the VJ in fully-local mode — and is never started
//! against a store on another machine. An app merely attached to a LAN
//! store has no business handing it local paths.
//!
//! ## Write stability
//!
//! Editors write in bursts: truncate, write, rename, touch. The observer
//! debounces the burst and then requires the file's (size, mtime) to hold
//! still for a further window before it reads it, so a half-written
//! document is never published as a revision the catalog then carries
//! forever.

use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{AssetKind, FileRole, MediaType, ThumbnailMedia};
use makepad_filesystem_watcher::{FileSystemWatcher, WatchRoot};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// The namespace observed effect documents publish into — the same one the
/// VJ's bundled presets seed into, so an observed document and a seeded one
/// are the same kind of row in the same lane.
pub const VJFX_NAMESPACE: &str = "vjfx";

/// Marks an asset whose Source file is a path this store observes. The VJ
/// reads it as "this one is live-editable"; seeding reads its ABSENCE as
/// "this head is not mine to overwrite".
pub const OBSERVED_TAG: &str = "livecoded";

/// The tag that marks an effect as transition-suited (mirrors the VJ's
/// `effects::seed::TRANSITION_TAG`; the string is the wire contract).
pub const TRANSITION_TAG: &str = "transition";

/// Engines whose whole job is SHAPING the incoming program picture rather
/// than drawing an unrelated scene — which is exactly what makes a document
/// usable in the transition slot.
pub const TRANSITION_ENGINES: &[&str] = &["transition", "screen", "tiles"];

/// Bundled transition-suited docs whose ENGINE alone cannot say so (the
/// raymarch and videomesh art transitions). The VJ passes its full
/// authoritative stem list; a host without one (asset-ui observing the
/// same origin) still must not strip these of their `transition` tag on a
/// republish — that made them vanish from the VJ's transition lane.
pub const TRANSITION_STEM_FALLBACK: &[&str] = &["89_beat_lens", "242_trans_ball"];

/// Largest effect document the observer will publish. A `.splash` is a
/// text document; anything past this is not one, and refusing early keeps
/// a stray binary out of the catalog.
pub const MAX_DOC_BYTES: u64 = 1024 * 1024;

/// How deep an origin directory is walked. Origin dirs are meant to be
/// flat; a couple of levels covers `local/vjfx/experiments/`.
const MAX_DEPTH: usize = 4;

/// How long an origin directory's event burst is collected before anything
/// is looked at. Mirrors the studio backend's own file-watch batching.
pub const DEFAULT_DEBOUNCE_MS: u64 = 80;

/// How long a file's (size, mtime) must hold still after the burst before
/// it is read. An editor's truncate-then-write is two events milliseconds
/// apart; this is what stops the truncate from becoming a revision.
pub const DEFAULT_STABLE_MS: u64 = 150;

/// How often the loop wakes when nothing is happening.
const TICK_MS: u64 = 40;

#[derive(Clone, Debug)]
pub struct ObserveConfig {
    /// Directories to observe. Created if absent — an origin directory that
    /// does not exist yet is the normal first-run state, not an error.
    pub roots: Vec<PathBuf>,
    /// Namespace observed documents publish into.
    pub namespace: String,
    pub debounce_ms: u64,
    pub stable_ms: u64,
    /// File stems the embedder KNOWS are transition-suited even though
    /// their engine is not in [`TRANSITION_ENGINES`] (the VJ passes its
    /// bundled transition list). Empty is a fine default.
    pub transition_stems: Vec<String>,
    pub log: bool,
}

impl ObserveConfig {
    /// The effect-document shape: `*.splash` under `roots`, published as
    /// `vjfx/<stem>`.
    pub fn vjfx(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            namespace: VJFX_NAMESPACE.to_string(),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            stable_ms: DEFAULT_STABLE_MS,
            // The engine test covers most transition docs; the fallback
            // stems cover the art transitions it cannot. An embedder with
            // the authoritative list (the VJ) extends this.
            transition_stems: TRANSITION_STEM_FALLBACK.iter().map(|s| s.to_string()).collect(),
            log: true,
        }
    }
}

/// What one observed file's pass did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A new asset was minted for this file.
    Published { alias: String },
    /// The file changed: a new revision of the SAME asset was published and
    /// the alias re-pointed at it.
    Republished { alias: String },
    /// The head already carries exactly these bytes. No catalog write.
    Unchanged { alias: String },
    /// Not a document this observer publishes.
    Ignored,
    Failed { alias: String, error: String },
}

/// A file the observer is waiting on: when to look, and what it looked like
/// last time it looked.
#[derive(Clone, Copy, Debug)]
struct Pending {
    ready_at_ms: u64,
    stamp: Option<(u64, u128)>,
}

/// `.splash` under one of the roots, not hidden, within the depth bound.
fn is_observed_doc(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.eq_ignore_ascii_case("splash"))
        .unwrap_or(true)
    {
        return false;
    }
    !path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(true)
}

fn stamp_of(path: &Path) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified))
}

/// Every observed document under `root` right now. Bounded in depth, sorted
/// so two sweeps of the same tree do the same work in the same order.
pub fn sweep(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with('.') {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            // A symlinked tree is either walked twice or an escape from the
            // origin directory; neither is wanted.
            Ok(meta) if meta.is_symlink() => continue,
            Err(_) => continue,
            _ => {}
        }
        if path.is_dir() {
            walk(&path, depth + 1, out);
        } else if is_observed_doc(&path) {
            out.push(path);
        }
    }
}

/// The alias an observed document publishes under: its FILE STEM, so the
/// path an agent edits and the name the catalog shows are the same word.
/// This is deliberately the same rule the bundled presets seed under, which
/// is what makes editing a bundled preset's file republish that preset.
pub fn alias_for(namespace: &str, path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    let stem = stem.trim();
    if stem.is_empty() || stem.len() > 64 {
        return None;
    }
    if !stem
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
    {
        return None;
    }
    Some(format!("{namespace}/{stem}"))
}

/// `name:` from the document (first occurrence), else the file stem.
fn title_of(source: &str, fallback: &str) -> String {
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("name:") {
            let v = rest.trim().trim_matches(',').trim().trim_matches('"');
            if !v.is_empty() {
                return v.chars().take(120).collect();
            }
        }
    }
    fallback.to_string()
}

/// The document's leading comment block, stripped of `//`.
fn description_of(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("//") {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(rest.trim());
        } else if !t.is_empty() {
            break;
        }
        if out.len() > 300 {
            break;
        }
    }
    out
}

/// The value of a top-level `key:` in the document, as written.
///
/// Deliberately a scanner and not a parser: this module publishes a
/// document it does not evaluate (the VJ's own `EffectDoc::parse` is the
/// only authority on what a document MEANS), and it needs exactly two
/// facts out of it — what to call it and whether it belongs in the
/// transition lane. `key` must OPEN its line (the document's own style) or
/// follow a `{`/`,`, so neither `// engine: …` in the prose header nor an
/// `engine:` inside a shader body is ever mistaken for the declaration.
fn field_str(source: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    let bytes = source.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(&needle) {
        let at = from + rel;
        from = at + needle.len();
        let line_start = bytes[..at]
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let opens_line = bytes[line_start..at].iter().all(u8::is_ascii_whitespace);
        let prev = bytes[..at]
            .iter()
            .rev()
            .find(|b| !b.is_ascii_whitespace())
            .copied();
        if !opens_line && !matches!(prev, None | Some(b'{') | Some(b',')) {
            continue;
        }
        let rest = source[at + needle.len()..]
            .lines()
            .next()
            .unwrap_or_default()
            .trim();
        let value = rest
            .split(&[',', '}'][..])
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"');
        return Some(value.to_string());
    }
    None
}

/// Is this document transition-suited? Three ways to be, cheapest first:
/// it says so (`transition: true`), its engine is one of the
/// picture-shaping families, or the embedder named its stem.
pub fn declares_transition(source: &str, stem: &str, extra_stems: &[String]) -> bool {
    if field_str(source, "transition").is_some_and(|v| v.starts_with("true")) {
        return true;
    }
    if field_str(source, "engine")
        .is_some_and(|engine| TRANSITION_ENGINES.contains(&engine.as_str()))
    {
        return true;
    }
    extra_stems.iter().any(|s| s == stem)
}

/// Publish one observed document, or report that nothing needed doing.
///
/// Blocking (network + one file hash). Runs on the observer thread, never
/// on a UI thread.
pub fn publish_doc(client: &mut AssetClient, path: &Path, config: &ObserveConfig) -> Outcome {
    if !is_observed_doc(path) {
        return Outcome::Ignored;
    }
    let Some(alias_str) = alias_for(&config.namespace, path) else {
        return Outcome::Ignored;
    };
    let abs = match std::path::absolute(path) {
        Ok(p) => p,
        Err(error) => {
            return Outcome::Failed { alias: alias_str, error: format!("absolute path: {error}") }
        }
    };
    let meta = match std::fs::metadata(&abs) {
        Ok(m) => m,
        // Deleted between the event and this pass: not an error, just gone.
        Err(_) => return Outcome::Ignored,
    };
    if !meta.is_file() || meta.len() == 0 {
        return Outcome::Ignored;
    }
    if meta.len() > MAX_DOC_BYTES {
        return Outcome::Failed {
            alias: alias_str,
            error: format!("{} bytes is not an effect document", meta.len()),
        };
    }
    let source = match std::fs::read_to_string(&abs) {
        Ok(s) => s,
        Err(error) => {
            return Outcome::Failed { alias: alias_str, error: format!("read: {error}") }
        }
    };
    let digest = makepad_asset_data::BlobId::hash_of(source.as_bytes());
    let Ok(alias) = alias_str.parse::<makepad_asset_data::AssetAlias>() else {
        return Outcome::Ignored;
    };

    // Is the head already exactly these bytes? Asking costs two small
    // control-plane calls and saves a whole revision per editor touch.
    let mut reuse: Option<makepad_asset_data::AssetId> = None;
    let mut republish = false;
    match client.resolve_alias(&alias) {
        Ok(dto) => {
            reuse = Some(dto.asset_id);
            republish = true;
            match client.fetch_asset_manifest(&dto.head_revision) {
                Ok(manifest) => {
                    let head = manifest
                        .files
                        .iter()
                        .find(|f| f.role == FileRole::Source)
                        .map(|f| f.blob);
                    if head == Some(digest) {
                        return Outcome::Unchanged { alias: alias_str };
                    }
                }
                Err(error) => {
                    return Outcome::Failed {
                        alias: alias_str,
                        error: format!("head manifest: {error}"),
                    }
                }
            }
        }
        Err(ClientError::NotFound { .. }) => {}
        Err(error) => {
            return Outcome::Failed { alias: alias_str, error: format!("alias probe: {error}") }
        }
    }

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("effect");
    let title = title_of(&source, stem);
    let (jpeg, w, h) = placeholder_thumbnail(stem);
    let mut bundle = PublishBundle::new(
        &config.namespace,
        AssetKind::VjEffect,
        title,
        // THE NO-COPY PATH: the store hashes this file where it lies. The
        // catalog gains a row; the disk gains nothing.
        vec![PublishBundleFile::reference(
            FileRole::Source,
            MediaType::Text,
            abs.clone(),
            None,
        )],
        PublishThumbnail::plain(jpeg, ThumbnailMedia::Jpeg, w, h),
        PublishRights::generated_cc0(),
    );
    bundle.alias = Some(alias);
    bundle.asset_id = reuse;
    bundle.description = description_of(&source);
    let mut tags = vec!["vjeffect".to_string(), OBSERVED_TAG.to_string()];
    if declares_transition(&source, stem, &config.transition_stems) {
        tags.push(TRANSITION_TAG.to_string());
    }
    bundle.tags = tags;
    bundle.generator = "observed origin".to_string();
    bundle.provenance = format!("observed in place at {}", abs.display());
    match client.publish_bundle(&bundle) {
        Ok(_) if republish => Outcome::Republished { alias: alias_str },
        Ok(_) => Outcome::Published { alias: alias_str },
        Err(error) => Outcome::Failed { alias: alias_str, error: error.to_string() },
    }
}

/// Observe `config.roots` until `stop` flips.
///
/// Blocking; spawn it on a thread. Everything it does is bounded: one file
/// per pass, one hash per publish, and a debounce that collapses an
/// editor's burst into a single look.
pub fn run(client: &mut AssetClient, config: &ObserveConfig, stop: &AtomicBool) {
    let mut roots = Vec::new();
    for (index, root) in config.roots.iter().enumerate() {
        if let Err(error) = std::fs::create_dir_all(root) {
            if config.log {
                eprintln!("[observe] cannot create origin {}: {error}", root.display());
            }
            continue;
        }
        let path = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        roots.push(WatchRoot { mount: format!("origin{index}"), path });
    }
    if roots.is_empty() {
        if config.log {
            eprintln!("[observe] no usable origin directory; not watching");
        }
        return;
    }

    let (tx, rx) = mpsc::channel::<PathBuf>();
    let watcher = match FileSystemWatcher::start(roots.clone(), move |event| {
        let _ = tx.send(event.path);
    }) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            // No platform watcher (or too many watches): the initial sweep
            // still runs, and the loop still polls the roots — degraded,
            // never silent.
            if config.log {
                eprintln!("[observe] filesystem watcher unavailable: {error}");
            }
            None
        }
    };
    if config.log {
        eprintln!(
            "[observe] watching {} for *.splash → {}/<stem>",
            roots
                .iter()
                .map(|r| r.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            config.namespace
        );
    }

    let now = || crate::host::util::now_ms();
    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    // The initial sweep: a file dropped while the app was off is exactly as
    // interesting as one dropped while it is running.
    for root in &roots {
        for path in sweep(&root.path) {
            pending.insert(path, Pending { ready_at_ms: now(), stamp: None });
        }
    }
    // Poll fallback interval when the platform watcher could not start.
    let mut next_poll_ms = now();

    while !stop.load(Ordering::Acquire) {
        // Collect this tick's events (the debounce window restarts on each).
        while let Ok(path) = rx.recv_timeout(Duration::from_millis(TICK_MS)) {
            if is_observed_doc(&path) {
                pending.insert(
                    path,
                    Pending {
                        ready_at_ms: now().saturating_add(config.debounce_ms),
                        stamp: None,
                    },
                );
            } else {
                // A directory-level or otherwise unnameable event. This is
                // the NORMAL shape on Windows (the watcher reports the root
                // for every change) and on macOS's degraded/coalesced paths
                // — something under a root changed, we just were not told
                // what. Re-sweep the root it belongs to; the digest compare
                // makes unchanged files free. Dropping these left the
                // watcher silently dead on those platforms.
                for root in &roots {
                    if path.starts_with(&root.path) {
                        for doc in sweep(&root.path) {
                            pending.entry(doc).or_insert(Pending {
                                ready_at_ms: now().saturating_add(config.debounce_ms),
                                stamp: None,
                            });
                        }
                    }
                }
            }
            if stop.load(Ordering::Acquire) {
                return;
            }
        }
        if stop.load(Ordering::Acquire) {
            return;
        }
        if watcher.is_none() && now() >= next_poll_ms {
            next_poll_ms = now().saturating_add(1000);
            for root in &roots {
                for path in sweep(&root.path) {
                    pending.entry(path).or_insert(Pending { ready_at_ms: now(), stamp: None });
                }
            }
        }

        // One file per pass: the publish is a network round trip and a
        // hash, and a burst of fifty saves must not become fifty of them
        // between two stop checks.
        let ready = pending
            .iter()
            .filter(|(_, p)| p.ready_at_ms <= now())
            .map(|(path, p)| (path.clone(), *p))
            .min_by(|a, b| a.0.cmp(&b.0));
        let Some((path, state)) = ready else { continue };
        let stamp = stamp_of(&path);
        if stamp.is_none() {
            pending.remove(&path);
            continue;
        }
        if state.stamp != stamp {
            // Still moving: re-arm and look again after the stability
            // window. This is what a half-written document looks like.
            pending.insert(
                path,
                Pending {
                    ready_at_ms: now().saturating_add(config.stable_ms),
                    stamp,
                },
            );
            continue;
        }
        pending.remove(&path);
        let outcome = publish_doc(client, &path, config);
        if config.log {
            match &outcome {
                Outcome::Published { alias } => {
                    eprintln!("[observe] published {alias} ← {}", path.display())
                }
                Outcome::Republished { alias } => {
                    eprintln!("[observe] new revision of {alias} ← {}", path.display())
                }
                Outcome::Unchanged { .. } | Outcome::Ignored => {}
                Outcome::Failed { alias, error } => {
                    eprintln!("[observe] {alias} FAILED: {error}")
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// placeholder thumbnail
// ---------------------------------------------------------------------------

/// A modest but never-flat placeholder: a per-name colored weave, 256x256
/// JPEG. Every consumer of effect documents replaces it with a real
/// rendered picture keyed by revision — which is exactly why a new revision
/// regenerates the thumbnail for free.
fn placeholder_thumbnail(name: &str) -> (Vec<u8>, u32, u32) {
    const W: usize = 256;
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    let hue2 = (hue + 0.33).fract();
    let mut bgra = vec![0u8; W * W * 4];
    for y in 0..W {
        for x in 0..W {
            let u = x as f32 / W as f32;
            let v = y as f32 / W as f32;
            let t = (u * 0.7 + v * 0.3 + ((u * 9.0).sin() * 0.03)).fract();
            let hh = hue + (hue2 - hue) * t;
            let band = 1.0 - ((u + v - 1.0).abs() * 2.5).min(1.0);
            let (r, g, b) = hsv(hh, 0.75, 0.28 + 0.62 * band);
            let i = (y * W + x) * 4;
            bgra[i] = (b * 255.0) as u8;
            bgra[i + 1] = (g * 255.0) as u8;
            bgra[i + 2] = (r * 255.0) as u8;
            bgra[i + 3] = 0xff;
        }
    }
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, 88);
    let _ = encoder.encode(&bgra, W as u16, W as u16, jpeg_encoder::ColorType::Bgra);
    (out, W as u32, W as u32)
}

fn hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as u32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mp-observe-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_alias_is_the_file_stem_so_editing_a_file_republishes_its_asset() {
        let dir = tmp("alias");
        let a = dir.join("42_my_look.splash");
        std::fs::write(&a, b"{}").unwrap();
        assert_eq!(
            alias_for("vjfx", &a).as_deref(),
            Some("vjfx/42_my_look"),
            "the alias must be the stem, so a bundled preset's file edits that preset"
        );
        // Charset gate: an alias segment is not a place for arbitrary text.
        let bad = dir.join("my look!.splash");
        std::fs::write(&bad, b"{}").unwrap();
        assert_eq!(alias_for("vjfx", &bad), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_visible_splash_documents_are_observed() {
        let dir = tmp("filter");
        for name in ["a.splash", ".hidden.splash", "b.txt", "c.SPLASH"] {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/d.splash"), b"{}").unwrap();
        let mut got: Vec<String> = sweep(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        got.sort();
        assert_eq!(got, vec!["a.splash", "c.SPLASH", "d.splash"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transition_is_declared_by_the_document_its_engine_or_the_embedder() {
        assert!(declares_transition("{ engine: \"transition\" }", "x", &[]));
        assert!(declares_transition("{ engine: \"screen\" }", "x", &[]));
        assert!(declares_transition("{ engine: \"tiles\" }", "x", &[]));
        assert!(declares_transition("{ transition: true }", "x", &[]));
        assert!(!declares_transition("{ engine: \"particles\" }", "x", &[]));
        // A real document's shape, and a comment that must not count.
        let doc = "// A LOOK that mentions engine: \"screen\" in prose.\n\
                   {\n    name: \"Thing\"\n    engine: \"particles\"\n}\n";
        assert!(!declares_transition(doc, "x", &[]));
        // The one thing an engine cannot tell you: a scene engine somebody
        // wrote as a transition. The embedder names those.
        assert!(declares_transition(
            "{ engine: \"raymarch\" }",
            "89_beat_lens",
            &["89_beat_lens".to_string()]
        ));
    }

    #[test]
    fn title_and_description_come_out_of_the_document() {
        let source = "// A LOOK — the leading block is the description.\n\
                      // Second line joins it.\n\
                      {\n    name: \"My Look\"\n    engine: \"particles\"\n}\n";
        assert_eq!(title_of(source, "42_fallback"), "My Look");
        assert_eq!(
            description_of(source),
            "A LOOK — the leading block is the description. Second line joins it."
        );
        assert_eq!(title_of("{ engine: \"particles\" }", "42_fallback"), "42_fallback");
    }

    #[test]
    fn the_placeholder_is_a_real_jpeg_and_differs_by_name() {
        let (a, w, h) = placeholder_thumbnail("42_my_look");
        let (b, _, _) = placeholder_thumbnail("43_other");
        assert_eq!((w, h), (256, 256));
        assert!(a.starts_with(&[0xff, 0xd8]), "not a jpeg");
        assert!(a.len() > 500);
        assert_ne!(a, b);
    }
}
