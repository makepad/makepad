//! Games import: a folder of splash games → `AssetKind::Game` assets.
//!
//! A game is a directory `<slug>/game.splash` with an optional
//! `manifest.toml` (`name`, `description`, `players`). The splash source is
//! published as the retained `Source` text file; everything the game
//! references (models, audio) is resolved through the catalog at play time,
//! so a game asset never embeds bytes.
//!
//! Idempotent by content: the alias `<namespace>/games/<slug>` is the
//! publication marker, and a game whose head revision already holds the
//! same source digest is skipped. A changed source publishes a NEW revision
//! of the SAME asset id (the alias head moves), so players keep the
//! identity they pinned.

use makepad_asset_client::{
    AssetClient, ClientError, PublishBundle, PublishBundleFile, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetFile, AssetId, AssetKind, BlobId, DeviceTier, FileRole, MediaType,
    ThumbnailMedia,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Catalog category every imported game carries.
pub const GAME_CATEGORY: &str = "game";
/// Catalog tag every imported game carries (beside its slug).
pub const GAME_TAG: &str = "game";
/// Thumbnail edge in pixels.
pub const THUMB: u32 = 256;

/// Logical name of the exterior source. Interiors use
/// `interiors/<sub>.splash` in the same immutable revision.
pub const MAIN_SUB: &str = "main";
pub const MAIN_FILE: &str = "game.splash";
const WORLD_INDEX_HEADER: &str = "makepad-game-worlds 2";
const WORLD_INDEX_TIER: DeviceTier = DeviceTier::High;
const WORLD_INDEX_LOD: u8 = 7;

/// All named world sources carried by one game revision. The file set is
/// the door-link truth: an interior exists exactly when its `sub` is here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldSources {
    files: BTreeMap<String, Vec<u8>>,
}

impl WorldSources {
    pub fn main(source: Vec<u8>) -> Result<Self, String> {
        let mut files = BTreeMap::new();
        files.insert(MAIN_SUB.to_string(), source);
        let worlds = Self { files };
        worlds.validate()?;
        Ok(worlds)
    }

    pub fn get(&self, sub: &str) -> Option<&[u8]> {
        self.files.get(sub).map(Vec::as_slice)
    }

    pub fn insert(&mut self, sub: &str, source: Vec<u8>) -> Result<(), String> {
        validate_sub(sub)?;
        if source.is_empty() || std::str::from_utf8(&source).is_err() {
            return Err(format!("{} must be non-empty UTF-8", world_file(sub)?));
        }
        self.files.insert(sub.to_string(), source);
        self.validate()
    }

    pub fn remove(&mut self, sub: &str) -> Option<Vec<u8>> {
        if sub == MAIN_SUB { None } else { self.files.remove(sub) }
    }

    pub fn subs(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn file(&self, sub: &str) -> Result<String, String> {
        world_file(sub)
    }

    pub fn blob(&self, sub: &str) -> Option<BlobId> {
        self.get(sub).map(BlobId::hash_of)
    }

    fn validate(&self) -> Result<(), String> {
        if !self.files.contains_key(MAIN_SUB) {
            return Err("game revision has no game.splash".into());
        }
        // There are 32 Source slots. The index consumes one; the other 31
        // carry game.splash plus at most 30 interiors.
        if self.files.len() > 31 {
            return Err("game revision exceeds the 30-interior source format cap".into());
        }
        for (sub, bytes) in &self.files {
            validate_sub(sub)?;
            if bytes.is_empty() || std::str::from_utf8(bytes).is_err() {
                return Err(format!("{} must be non-empty UTF-8", world_file(sub)?));
            }
        }
        Ok(())
    }
}

pub fn validate_sub(sub: &str) -> Result<(), String> {
    if sub == MAIN_SUB {
        return Ok(());
    }
    if sub.is_empty()
        || sub.len() > 64
        || !sub.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        || sub == "."
        || sub == ".."
    {
        return Err("sub must be 1..64 ASCII letters/digits/dash/underscore/dot (not . or ..)".into());
    }
    Ok(())
}

pub fn world_file(sub: &str) -> Result<String, String> {
    validate_sub(sub)?;
    Ok(if sub == MAIN_SUB {
        MAIN_FILE.to_string()
    } else {
        format!("interiors/{sub}.splash")
    })
}

fn source_slots() -> impl Iterator<Item = (DeviceTier, u8)> {
    [DeviceTier::Any, DeviceTier::Low, DeviceTier::Medium, DeviceTier::High]
        .into_iter()
        .flat_map(|tier| (0..=7).map(move |lod| (tier, lod)))
        .filter(|slot| *slot != (DeviceTier::Any, 0) && *slot != (WORLD_INDEX_TIER, WORLD_INDEX_LOD))
}

fn assigned_slots(worlds: &WorldSources) -> BTreeMap<String, (DeviceTier, u8)> {
    let mut out = BTreeMap::new();
    out.insert(MAIN_SUB.to_string(), (DeviceTier::Any, 0));
    for (sub, slot) in worlds.subs().filter(|sub| *sub != MAIN_SUB).zip(source_slots()) {
        out.insert(sub.to_string(), slot);
    }
    out
}

fn tier_name(tier: DeviceTier) -> &'static str {
    match tier {
        DeviceTier::Any => "any",
        DeviceTier::Low => "low",
        DeviceTier::Medium => "medium",
        DeviceTier::High => "high",
    }
}

fn parse_tier(text: &str) -> Option<DeviceTier> {
    Some(match text {
        "any" => DeviceTier::Any,
        "low" => DeviceTier::Low,
        "medium" => DeviceTier::Medium,
        "high" => DeviceTier::High,
        _ => return None,
    })
}

fn world_index(worlds: &WorldSources) -> String {
    let slots = assigned_slots(worlds);
    let mut out = format!("{WORLD_INDEX_HEADER}\n");
    for sub in worlds.subs() {
        let (tier, lod) = slots[sub];
        out.push_str(&format!("{sub}\t{}\t{}\t{lod}\n", world_file(sub).unwrap(), tier_name(tier)));
    }
    out
}

fn parse_world_index(text: &str) -> Result<Vec<(String, DeviceTier, u8)>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(WORLD_INDEX_HEADER) {
        return Err("game world manifest has an unknown header".into());
    }
    let mut out = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 4 {
            return Err("game world manifest row is malformed".into());
        }
        validate_sub(fields[0])?;
        if world_file(fields[0])? != fields[1] {
            return Err("game world manifest path does not match its sub id".into());
        }
        let tier = parse_tier(fields[2]).ok_or("game world manifest has an invalid tier")?;
        let lod: u8 = fields[3].parse().map_err(|_| "game world manifest has an invalid lod")?;
        if lod > 7 || (tier, lod) == (WORLD_INDEX_TIER, WORLD_INDEX_LOD) {
            return Err("game world manifest names a reserved source slot".into());
        }
        out.push((fields[0].to_string(), tier, lod));
    }
    if !out.iter().any(|(sub, tier, lod)| sub == MAIN_SUB && *tier == DeviceTier::Any && *lod == 0) {
        return Err("game world manifest has no canonical game.splash row".into());
    }
    Ok(out)
}

fn exact_source<'a>(manifest: &'a makepad_asset_data::AssetManifest, tier: DeviceTier, lod: u8) -> Option<&'a AssetFile> {
    manifest.files.iter().find(|file| file.role == FileRole::Source && file.tier == tier && file.lod == lod)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldSourceRef {
    pub sub: String,
    pub file: String,
    pub blob: BlobId,
    pub byte_len: u64,
}

pub fn world_index_ref(manifest: &makepad_asset_data::AssetManifest) -> Option<&AssetFile> {
    exact_source(manifest, WORLD_INDEX_TIER, WORLD_INDEX_LOD)
}

/// Resolve the logical names in an already fetched world-index text to the
/// exact immutable source blobs in its revision manifest.
pub fn world_source_refs(
    manifest: &makepad_asset_data::AssetManifest,
    index: Option<&str>,
) -> Result<Vec<WorldSourceRef>, String> {
    let rows = match index {
        Some(index) => parse_world_index(index)?,
        None => {
            let source = exact_source(manifest, DeviceTier::Any, 0)
                .or_else(|| manifest.files.iter().find(|file| file.role == FileRole::Source))
                .ok_or("head revision has no source file")?;
            return Ok(vec![WorldSourceRef {
                sub: MAIN_SUB.into(),
                file: MAIN_FILE.into(),
                blob: source.blob,
                byte_len: source.byte_len,
            }]);
        }
    };
    rows.into_iter()
        .map(|(sub, tier, lod)| {
            let source = exact_source(manifest, tier, lod)
                .ok_or_else(|| format!("game world manifest points at missing {}", world_file(&sub).unwrap()))?;
            Ok(WorldSourceRef {
                file: world_file(&sub)?,
                sub,
                blob: source.blob,
                byte_len: source.byte_len,
            })
        })
        .collect()
}

/// Read the complete named source set at an alias head. Old one-source game
/// revisions transparently become a `{main}` set until their next publish.
pub fn head_world_sources(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
) -> Result<(AssetId, WorldSources), String> {
    let alias = AssetAlias::from_str(&game_alias(namespace, slug)).map_err(|e| e.to_string())?;
    let head = client.resolve_alias(&alias).map_err(|e| format!("alias probe: {e}"))?;
    let manifest = crate::readable_head(client, &head.head_revision)
        .map_err(|e| format!("head manifest: {e}"))?
        .ok_or("head manifest unreadable (older schema)")?;
    let Some(index_file) = exact_source(&manifest, WORLD_INDEX_TIER, WORLD_INDEX_LOD) else {
        let main = exact_source(&manifest, DeviceTier::Any, 0)
            .or_else(|| manifest.files.iter().find(|file| file.role == FileRole::Source))
            .ok_or("head revision has no source file")?;
        let bytes = client.fetch_blob_bytes(&main.blob, Some(main.byte_len))
            .map_err(|e| format!("head source: {e}"))?;
        return Ok((head.asset_id, WorldSources::main(bytes)?));
    };
    let index = client.fetch_blob_bytes(&index_file.blob, Some(index_file.byte_len))
        .map_err(|e| format!("world manifest: {e}"))?;
    let index = std::str::from_utf8(&index).map_err(|_| "game world manifest is not UTF-8")?;
    let rows = parse_world_index(index)?;
    let mut files = BTreeMap::new();
    for (sub, tier, lod) in rows {
        if files.contains_key(&sub) {
            return Err(format!("game world manifest repeats sub '{sub}'"));
        }
        let file = exact_source(&manifest, tier, lod)
            .ok_or_else(|| format!("game world manifest points at missing {}", world_file(&sub).unwrap()))?;
        let bytes = client.fetch_blob_bytes(&file.blob, Some(file.byte_len))
            .map_err(|e| format!("{}: {e}", world_file(&sub).unwrap()))?;
        files.insert(sub, bytes);
    }
    let worlds = WorldSources { files };
    worlds.validate()?;
    Ok((head.asset_id, worlds))
}

/// Alias of one game: `<namespace>/games/<slug>`.
pub fn game_alias(namespace: &str, slug: &str) -> String {
    format!("{namespace}/games/{slug}")
}

/// One game folder on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameDir {
    pub slug: String,
    pub dir: PathBuf,
    pub name: String,
    pub description: String,
    pub players: u32,
    pub splash: Vec<u8>,
    /// Optional `interiors/<door>.splash` files discovered beside the main
    /// source. Keys are door/sub ids, not paths.
    pub interiors: BTreeMap<String, Vec<u8>>,
}

/// Scan `root` for `<slug>/game.splash` folders, sorted by display name.
/// Unreadable or empty splash files are skipped; a missing root is an empty
/// list, not an error.
pub fn scan_games(root: &Path) -> Vec<GameDir> {
    let mut games = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return games;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let splash_path = dir.join("game.splash");
        if !splash_path.is_file() {
            continue;
        }
        let slug = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !slug_ok(&slug) {
            continue;
        }
        let Ok(splash) = std::fs::read(&splash_path) else {
            continue;
        };
        if splash.is_empty() || std::str::from_utf8(&splash).is_err() {
            continue;
        }
        let manifest = std::fs::read_to_string(dir.join("manifest.toml")).unwrap_or_default();
        let (name, description, players) = parse_manifest(&manifest, &slug);
        let mut interiors = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("interiors")) {
            for interior in entries.flatten() {
                let path = interior.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
                let Some(sub) = name.strip_suffix(".splash") else { continue };
                if validate_sub(sub).is_err() || sub == MAIN_SUB {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                if !bytes.is_empty() && std::str::from_utf8(&bytes).is_ok() {
                    interiors.insert(sub.to_string(), bytes);
                }
            }
        }
        games.push(GameDir {
            slug,
            dir,
            name,
            description,
            players,
            splash,
            interiors,
        });
    }
    games.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    games
}

/// Alias segments are `[a-z0-9_-]`, lowercase, non-empty.
fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Minimal TOML reader for the game manifest: top-level `key = "value"`
/// lines before the first table. A manifest that fails to parse degrades to
/// "a game named after its folder", never to an error.
pub fn parse_manifest(text: &str, fallback_name: &str) -> (String, String, u32) {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut players = 1u32;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "name" if !value.is_empty() => name = value.to_string(),
            "description" => description = value.to_string(),
            "players" => players = value.parse().unwrap_or(1),
            _ => {}
        }
    }
    (name, description, players)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GamesReport {
    pub published: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Publish every game under `root` into `namespace`. `rights` is the
/// explicit declaration for the whole folder (the stock sandbox games are
/// own-authored and use the named CC0 grant).
pub fn import_games(
    client: &mut AssetClient,
    root: &Path,
    namespace: &str,
    rights: &PublishRights,
    log: bool,
) -> Result<GamesReport, String> {
    let games = scan_games(root);
    if games.is_empty() {
        return Err(format!("no <slug>/game.splash folders under {}", root.display()));
    }
    let mut report = GamesReport::default();
    for game in games {
        match publish_game(client, &game, namespace, rights) {
            Ok(GameOutcome::Published) => {
                if log {
                    eprintln!("[games-import] published {}", game.slug);
                }
                report.published.push(game.slug);
            }
            Ok(GameOutcome::Updated) => {
                if log {
                    eprintln!("[games-import] new revision {}", game.slug);
                }
                report.updated.push(game.slug);
            }
            Ok(GameOutcome::Unchanged) => {
                if log {
                    eprintln!("[games-import] unchanged {}", game.slug);
                }
                report.unchanged.push(game.slug);
            }
            Err(error) => {
                if log {
                    eprintln!("[games-import] FAILED {}: {error}", game.slug);
                }
                report.failed.push((game.slug, error));
            }
        }
    }
    Ok(report)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameOutcome {
    Published,
    Updated,
    Unchanged,
}

/// What [`publish_game_detailed`] left on the store: the outcome plus the
/// identity a caller needs to enter the game right away (the sandbox's
/// New-game / `world.new_level` path plays the asset before the catalog
/// feed has listed it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GamePublished {
    pub outcome: GameOutcome,
    pub asset_id: AssetId,
    pub alias: String,
}

/// Publish one game: skip when the alias head already carries this exact
/// source, re-publish as a new revision of the same asset when it changed.
pub fn publish_game(
    client: &mut AssetClient,
    game: &GameDir,
    namespace: &str,
    rights: &PublishRights,
) -> Result<GameOutcome, String> {
    publish_game_detailed(client, game, namespace, rights).map(|p| p.outcome)
}

/// [`publish_game`], answering the asset id and alias as well.
pub fn publish_game_detailed(
    client: &mut AssetClient,
    game: &GameDir,
    namespace: &str,
    rights: &PublishRights,
) -> Result<GamePublished, String> {
    let alias_text = game_alias(namespace, &game.slug);
    let alias = AssetAlias::from_str(&alias_text).map_err(|e| e.to_string())?;
    let mut worlds = WorldSources::main(game.splash.clone())?;
    for (sub, source) in &game.interiors {
        worlds.insert(sub, source.clone())?;
    }
    let existing = match client.resolve_alias(&alias) {
        Ok(head) => {
            let same = head_world_sources(client, namespace, &game.slug)
                .map(|(_, current)| current == worlds)
                .unwrap_or(false);
            if same {
                return Ok(GamePublished {
                    outcome: GameOutcome::Unchanged,
                    asset_id: head.asset_id,
                    alias: alias_text,
                });
            }
            Some(head.asset_id)
        }
        Err(ClientError::NotFound { .. }) => None,
        Err(error) => return Err(format!("alias probe: {error}")),
    };
    let published_id = publish_world_bundle(
        client,
        namespace,
        &game.slug,
        &game.name,
        &game.description,
        existing,
        &worlds,
        PublishThumbnail {
            bytes: game_thumbnail_png(&game.slug),
            media: ThumbnailMedia::Png,
            width: THUMB,
            height: THUMB,
            views: Vec::new(),
        },
        rights,
        if game.players > 1 { &["multiplayer"] } else { &[] },
        "games-import",
        &format!("games-import {}", game.dir.display()),
    )?;
    Ok(GamePublished {
        outcome: if existing.is_some() {
            GameOutcome::Updated
        } else {
            GameOutcome::Published
        },
        asset_id: published_id,
        alias: alias_text,
    })
}

fn bundle_files(worlds: &WorldSources) -> Vec<PublishBundleFile> {
    let slots = assigned_slots(worlds);
    let mut files = Vec::with_capacity(worlds.files.len() + 1);
    for (sub, bytes) in &worlds.files {
        let (tier, lod) = slots[sub];
        let mut file = PublishBundleFile::bytes(FileRole::Source, MediaType::Text, bytes.clone(), None);
        file.tier = tier;
        file.lod = lod;
        files.push(file);
    }
    let mut index = PublishBundleFile::bytes(
        FileRole::Source,
        MediaType::Text,
        world_index(worlds).into_bytes(),
        None,
    );
    index.tier = WORLD_INDEX_TIER;
    index.lod = WORLD_INDEX_LOD;
    files.push(index);
    files
}

fn publish_world_bundle(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
    title: &str,
    description: &str,
    asset_id: Option<AssetId>,
    worlds: &WorldSources,
    thumbnail: PublishThumbnail,
    rights: &PublishRights,
    extra_tags: &[&str],
    creator: &str,
    provenance: &str,
) -> Result<AssetId, String> {
    worlds.validate()?;
    let alias = AssetAlias::from_str(&game_alias(namespace, slug)).map_err(|e| e.to_string())?;
    let mut request = PublishBundle::new(
        namespace,
        AssetKind::Game,
        title,
        bundle_files(worlds),
        thumbnail,
        rights.clone(),
    );
    request.description = description.to_string();
    request.alias = Some(alias);
    request.asset_id = asset_id;
    request.categories = vec![GAME_CATEGORY.into()];
    request.tags = vec![GAME_TAG.into(), slug.to_string()];
    request.tags.extend(extra_tags.iter().map(|tag| (*tag).to_string()));
    request.creator = creator.to_string();
    request.provenance = provenance.to_string();
    client.publish_bundle(&request).map(|published| published.asset_id).map_err(|e| format!("publish: {e}"))
}

/// Republish `slug` as a NEW revision of the same asset. Two callers:
///
/// - an AI EDIT that evaluated OK in the sandbox passes `splash =
///   Some(new source)` — the store is the world's history, and this is the
///   only way a level changes (the sandbox keeps no durable copy);
/// - a thumbnail capture passes `None`: the head revision's own source is
///   fetched HERE at publish time, so the picture is the only change and
///   nothing stashed earlier can be stale or missing.
///
/// `thumbnail` is the picture to carry either way (an edit re-uses the
/// current one — see [`head_thumbnail`]). Unlike [`publish_game`] this never
/// skips on an unchanged source. The alias head moves, so every client's
/// next listing shows the new revision.
pub fn publish_game_revision(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
    title: &str,
    description: &str,
    splash: Option<Vec<u8>>,
    thumbnail: PublishThumbnail,
    rights: &PublishRights,
) -> Result<(), String> {
    let (_, mut worlds) = head_world_sources(client, namespace, slug)?;
    if let Some(bytes) = splash {
        worlds.insert(MAIN_SUB, bytes)?;
    }
    publish_game_worlds_revision(
        client, namespace, slug, title, description, &worlds, thumbnail, rights,
    )
}

/// Publish a complete named world set as the parent's next revision. This is
/// the structural edit path used for ordinary source edits, first-open room
/// creation, and legacy-child folding.
pub fn publish_game_worlds_revision(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
    title: &str,
    description: &str,
    worlds: &WorldSources,
    thumbnail: PublishThumbnail,
    rights: &PublishRights,
) -> Result<(), String> {
    let alias = AssetAlias::from_str(&game_alias(namespace, slug)).map_err(|e| e.to_string())?;
    let head = client.resolve_alias(&alias).map_err(|e| format!("alias probe: {e}"))?;
    publish_world_bundle(
        client,
        namespace,
        slug,
        title,
        description,
        Some(head.asset_id),
        worlds,
        thumbnail,
        rights,
        &[],
        "sandbox",
        &format!("sandbox world-set revision of {slug}"),
    )?;
    Ok(())
}

/// The head revision's current SOURCE blob. The sandbox's thumbnail
/// publisher compares this against the source its photo was taken of: a
/// head that moved since the frame means the picture no longer shows the
/// game, and publishing it anyway is how one game's content once landed
/// on another (Chicane Circuit, 2026-08-26).
pub fn head_source_blob(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
) -> Result<BlobId, String> {
    head_world_source_blob(client, namespace, slug, MAIN_SUB)
}

pub fn head_world_source_blob(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
    sub: &str,
) -> Result<BlobId, String> {
    let (_, worlds) = head_world_sources(client, namespace, slug)?;
    worlds.blob(sub).ok_or_else(|| format!("head revision has no {}", world_file(sub).unwrap_or_else(|_| sub.into())))
}

/// The head revision's current thumbnail, as a `PublishThumbnail` an edit
/// can carry forward unchanged. Falls back to the slug's placeholder card
/// when the head has none the client can read (older schema): a game
/// never loses its picture over a source edit, and never blocks on it.
pub fn head_thumbnail(
    client: &mut AssetClient,
    namespace: &str,
    slug: &str,
) -> Result<PublishThumbnail, String> {
    let alias = AssetAlias::from_str(&game_alias(namespace, slug)).map_err(|e| e.to_string())?;
    let head = client.resolve_alias(&alias).map_err(|e| format!("alias probe: {e}"))?;
    let manifest = crate::readable_head(client, &head.head_revision)
        .map_err(|e| format!("head manifest: {e}"))?;
    let png = manifest.and_then(|m| {
        let thumb = m.thumbnail?;
        if thumb.media != ThumbnailMedia::Png {
            return None;
        }
        let bytes = client.fetch_blob_bytes(&thumb.blob, Some(thumb.byte_len)).ok()?;
        Some((bytes, thumb.width, thumb.height))
    });
    Ok(match png {
        Some((bytes, width, height)) => PublishThumbnail {
            bytes,
            media: ThumbnailMedia::Png,
            width,
            height,
            views: Vec::new(),
        },
        None => PublishThumbnail {
            bytes: game_thumbnail_png(slug),
            media: ThumbnailMedia::Png,
            width: THUMB,
            height: THUMB,
            views: Vec::new(),
        },
    })
}

/// True when `blob` is the placeholder card this module publishes for
/// `slug` — i.e. the game has never had a real picture.
pub fn is_placeholder_thumbnail(slug: &str, blob: &BlobId) -> bool {
    *blob == BlobId::hash_of(&game_thumbnail_png(slug))
}

/// Deterministic per-slug placeholder thumbnail: a slug-coloured card with
/// a darker border. The sandbox replaces nothing here — a real capture is a
/// later derivation, and the catalog requires SOME thumbnail now.
pub fn game_thumbnail_png(slug: &str) -> Vec<u8> {
    let (r, g, b) = slug_rgb(slug);
    let mut rgb = vec![0u8; (THUMB * THUMB * 3) as usize];
    for y in 0..THUMB {
        for x in 0..THUMB {
            let i = ((y * THUMB + x) * 3) as usize;
            let edge = x < 10 || y < 10 || x + 10 >= THUMB || y + 10 >= THUMB;
            if edge {
                rgb[i] = r / 3;
                rgb[i + 1] = g / 3;
                rgb[i + 2] = b / 3;
            } else {
                let shade = 180u16.saturating_sub((y / 2) as u16) as u8;
                rgb[i] = ((r as u16 * shade as u16) / 255) as u8;
                rgb[i + 1] = ((g as u16 * shade as u16) / 255) as u8;
                rgb[i + 2] = ((b as u16 * shade as u16) / 255) as u8;
            }
        }
    }
    encode_png_rgb(&rgb, THUMB, THUMB)
}

fn slug_rgb(slug: &str) -> (u8, u8, u8) {
    let mut h = 2166136261u32;
    for b in slug.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as i32;
    hsv(hue, 0.55, 0.85)
}

fn hsv(h: i32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - (((h as f32 / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// Uncompressed RGB PNG (stored deflate blocks). No extra crate; the bytes
/// are the contract and every decoder accepts stored blocks.
pub fn encode_png_rgb(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let bpp = 3usize;
    let raw_stride = 1 + w as usize * bpp;
    let mut raw = vec![0u8; raw_stride * h as usize];
    for y in 0..h as usize {
        let dst = y * raw_stride + 1;
        let src = y * w as usize * bpp;
        raw[dst..dst + w as usize * bpp].copy_from_slice(&rgb[src..src + w as usize * bpp]);
    }
    let mut zlib = Vec::new();
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut off = 0;
    while off < raw.len() {
        let n = (raw.len() - off).min(65535);
        let last = off + n == raw.len();
        zlib.push(if last { 0x01 } else { 0x00 });
        zlib.extend_from_slice(&(n as u16).to_le_bytes());
        zlib.extend_from_slice(&(!n as u16).to_le_bytes());
        zlib.extend_from_slice(&raw[off..off + n]);
        off += n;
    }
    let adler = adler32(&raw);
    zlib.extend_from_slice(&adler.to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    write_chunk(&mut out, b"IHDR", &{
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        ihdr
    });
    write_chunk(&mut out, b"IDAT", &zlib);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let mut crc = crc32(ty);
    crc = crc32_update(crc, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xffff_ffff, data) ^ 0xffff_ffff
}

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "games-import-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&root);
        root
    }

    fn write_game(root: &Path, slug: &str, splash: &str, manifest: Option<&str>) {
        let dir = root.join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("game.splash"), splash).unwrap();
        if let Some(m) = manifest {
            std::fs::write(dir.join("manifest.toml"), m).unwrap();
        }
    }

    #[test]
    fn scan_reads_manifest_and_skips_junk() {
        let root = temp_root("scan");
        write_game(
            &root,
            "race",
            "game {}\n",
            Some("name = \"Race\"\ndescription = \"cars\"\nplayers = 4\n[knobs.speed]\nvalue = 2\n"),
        );
        write_game(&root, "bare", "game {}\n", None);
        write_game(&root, "empty", "", None);
        write_game(&root, "Bad Slug", "game {}\n", None);
        std::fs::create_dir_all(root.join("no-splash")).unwrap();
        let games = scan_games(&root);
        let slugs: Vec<&str> = games.iter().map(|g| g.slug.as_str()).collect();
        assert_eq!(slugs, ["bare", "race"], "{slugs:?}");
        let race = games.iter().find(|g| g.slug == "race").unwrap();
        assert_eq!(race.name, "Race");
        assert_eq!(race.description, "cars");
        assert_eq!(race.players, 4);
        let bare = games.iter().find(|g| g.slug == "bare").unwrap();
        assert_eq!(bare.name, "bare");
        assert_eq!(bare.players, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real server round trip: publish, skip unchanged, new revision on a
    /// changed source, and the kind=game catalog query the sandbox runs.
    #[test]
    fn games_publish_and_list_by_kind() {
        use makepad_asset_client::{ApiEndpoints, CatalogQuery, ClientConfig};
        use makepad_asset_store::{AssetServer, ServerConfig};

        let base = temp_root("e2e");
        let server_root = base.join("server");
        std::fs::create_dir_all(&server_root).unwrap();
        let mut config = ServerConfig::new(server_root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.discovery = None;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(server_root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        let mut client_config = ClientConfig::new(base.join("cache"));
        client_config.token = Some(token);
        let endpoints = ApiEndpoints {
            control: server.control_addr(),
            data: server.data_addr(),
        };
        let mut client = AssetClient::connect(client_config, endpoints, Some(server.server_id()))
            .expect("connect");

        let games = base.join("games");
        write_game(&games, "arena", "game { arena: 1 }\n", Some("name = \"Arena\"\nplayers = 2\n"));
        write_game(&games, "race", "game { race: 1 }\n", None);
        let rights = PublishRights::generated_cc0();

        let first = import_games(&mut client, &games, "sandbox", &rights, false).expect("import");
        assert_eq!(first.published, ["arena", "race"]);
        assert!(first.failed.is_empty(), "{:?}", first.failed);

        let second = import_games(&mut client, &games, "sandbox", &rights, false).expect("import");
        assert_eq!(second.unchanged, ["arena", "race"]);
        assert!(second.published.is_empty() && second.updated.is_empty());

        std::fs::write(games.join("race").join("game.splash"), "game { race: 2 }\n").unwrap();
        std::fs::create_dir_all(games.join("race").join("interiors")).unwrap();
        std::fs::write(
            games.join("race").join("interiors/dogshop.splash"),
            "game { room: \"dogshop\" }\n",
        )
        .unwrap();
        let third = import_games(&mut client, &games, "sandbox", &rights, false).expect("import");
        assert_eq!(third.updated, ["race"]);
        assert_eq!(third.unchanged, ["arena"]);

        let alias = AssetAlias::from_str(&game_alias("sandbox", "race")).unwrap();
        let head = client.resolve_alias(&alias).expect("race alias");
        let manifest = client.fetch_asset_manifest(&head.head_revision).expect("manifest");
        assert_eq!(manifest.kind, AssetKind::Game);
        assert!(manifest
            .files
            .iter()
            .any(|f| f.role == FileRole::Source && f.blob == BlobId::hash_of(b"game { race: 2 }\n")));
        let (_, worlds) = head_world_sources(&mut client, "sandbox", "race").unwrap();
        assert_eq!(worlds.subs().collect::<Vec<_>>(), ["dogshop", "main"]);
        assert_eq!(worlds.get("dogshop"), Some(b"game { room: \"dogshop\" }\n".as_slice()));

        let page = client
            .catalog_search(
                &CatalogQuery {
                    text: String::new(),
                    namespace: None,
                    kind: Some(AssetKind::Game),
                    category: None,
                    tag: Some(GAME_TAG.into()),
                    exclude_tag: None,
                    creator: None,
                    live_only: true,
                    page_size: 50,
                    facets: 0,
                },
                None,
            )
            .expect("search");
        let mut titles: Vec<&str> = page.hits.iter().map(|h| h.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["Arena", "race"], "{titles:?}");
        // Same asset id across revisions: the alias head moved, identity stayed.
        assert_eq!(
            page.hits.iter().filter(|h| h.title == "race").count(),
            1,
            "a changed source must not mint a second game"
        );

        // The sandbox's edit path: a new SOURCE revision of the same asset,
        // carrying the head's own picture. The store is the history — the
        // next Play resolves the edited text, and the identity never moves.
        let before = client.resolve_alias(&alias).expect("race head before edit");
        let thumb = head_thumbnail(&mut client, "sandbox", "race").expect("head thumbnail");
        assert_eq!(thumb.bytes, game_thumbnail_png("race"), "the placeholder card rides along");
        publish_game_revision(
            &mut client,
            "sandbox",
            "race",
            "race",
            "",
            Some(b"game { race: 3 } // edited in the sandbox\n".to_vec()),
            thumb,
            &rights,
        )
        .expect("edit revision");
        let after = client.resolve_alias(&alias).expect("race head after edit");
        assert_eq!(after.asset_id, before.asset_id, "an edit is a revision, not a new game");
        assert_ne!(after.head_revision, before.head_revision, "the head moved");
        let manifest = client.fetch_asset_manifest(&after.head_revision).expect("edited manifest");
        let source = manifest
            .files
            .iter()
            .find(|f| f.role == FileRole::Source)
            .expect("edited source");
        let bytes = client
            .fetch_blob_bytes(&source.blob, Some(source.byte_len))
            .expect("edited source bytes");
        assert_eq!(bytes, b"game { race: 3 } // edited in the sandbox\n");
        assert_eq!(
            head_world_sources(&mut client, "sandbox", "race").unwrap().1.get("dogshop"),
            Some(b"game { room: \"dogshop\" }\n".as_slice()),
            "editing game.splash must carry the interior file forward"
        );
        // The thumbnail-only republish (`None`) now carries the EDITED
        // source: nothing stale can ride in from an earlier fetch.
        let capture = PublishThumbnail {
            bytes: game_thumbnail_png("race-capture"),
            media: ThumbnailMedia::Png,
            width: THUMB,
            height: THUMB,
            views: Vec::new(),
        };
        publish_game_revision(&mut client, "sandbox", "race", "race", "", None, capture, &rights)
            .expect("thumbnail revision");
        let pictured = client.resolve_alias(&alias).expect("race head after capture");
        let manifest = client.fetch_asset_manifest(&pictured.head_revision).expect("manifest");
        let source = manifest.files.iter().find(|f| f.role == FileRole::Source).unwrap();
        assert_eq!(
            source.blob,
            BlobId::hash_of(b"game { race: 3 } // edited in the sandbox\n"),
            "a capture republish keeps the edited source"
        );
        assert!(!is_placeholder_thumbnail("race", &manifest.thumbnail.unwrap().blob));
        assert!(head_world_sources(&mut client, "sandbox", "race").unwrap().1.get("dogshop").is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn alias_and_thumb_shape() {
        assert_eq!(game_alias("sandbox", "arena"), "sandbox/games/arena");
        assert!(AssetAlias::from_str(&game_alias("sandbox", "arena")).is_ok());
        let png = game_thumbnail_png("arena");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // IHDR width/height big-endian at fixed offsets.
        assert_eq!(&png[16..20], &THUMB.to_be_bytes());
        assert_eq!(&png[20..24], &THUMB.to_be_bytes());
        assert_ne!(game_thumbnail_png("arena"), game_thumbnail_png("race"));
    }
}
