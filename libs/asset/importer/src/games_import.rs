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
    AssetClient, ClientError, PublishFile, PublishRequest, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{AssetAlias, AssetKind, BlobId, FileRole, MediaType, ThumbnailMedia};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Catalog category every imported game carries.
pub const GAME_CATEGORY: &str = "game";
/// Catalog tag every imported game carries (beside its slug).
pub const GAME_TAG: &str = "game";
/// Thumbnail edge in pixels.
pub const THUMB: u32 = 256;

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
        games.push(GameDir {
            slug,
            dir,
            name,
            description,
            players,
            splash,
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

/// Publish one game: skip when the alias head already carries this exact
/// source, re-publish as a new revision of the same asset when it changed.
pub fn publish_game(
    client: &mut AssetClient,
    game: &GameDir,
    namespace: &str,
    rights: &PublishRights,
) -> Result<GameOutcome, String> {
    let alias = AssetAlias::from_str(&game_alias(namespace, &game.slug)).map_err(|e| e.to_string())?;
    let source_blob = BlobId::hash_of(&game.splash);
    let existing = match client.resolve_alias(&alias) {
        Ok(head) => {
            let manifest = client
                .fetch_asset_manifest(&head.head_revision)
                .map_err(|e| format!("head manifest: {e}"))?;
            let same = manifest
                .files
                .iter()
                .any(|f| f.role == FileRole::Source && f.blob == source_blob);
            if same {
                return Ok(GameOutcome::Unchanged);
            }
            Some(head.asset_id)
        }
        Err(ClientError::NotFound { .. }) => None,
        Err(error) => return Err(format!("alias probe: {error}")),
    };
    let mut request = PublishRequest::new(
        namespace,
        AssetKind::Game,
        game.name.clone(),
        PublishFile {
            bytes: game.splash.clone(),
            media: MediaType::Text,
            role: FileRole::Source,
            media_millis: 0,
            dims: None,
        },
        PublishThumbnail {
            bytes: game_thumbnail_png(&game.slug),
            media: ThumbnailMedia::Png,
            width: THUMB,
            height: THUMB,
        },
    );
    request.description = game.description.clone();
    request.alias = Some(alias);
    request.asset_id = existing;
    request.categories = vec![GAME_CATEGORY.into()];
    request.tags = vec![GAME_TAG.into(), game.slug.clone()];
    if game.players > 1 {
        request.tags.push("multiplayer".into());
    }
    request.creator = "games-import".into();
    request.provenance = format!("games-import {}", game.dir.display());
    request.rights = rights.clone();
    client
        .publish_artifact(&request)
        .map_err(|e| format!("publish: {e}"))?;
    Ok(if existing.is_some() {
        GameOutcome::Updated
    } else {
        GameOutcome::Published
    })
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
