//! On-disk content library. Every accepted artifact is persisted under
//! `local/ai_content_library/` (payload file + `index.json`) so the library
//! survives app restarts. Payloads remain on disk and are loaded on demand.

use makepad_micro_serde::*;
use std::{
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

/// Newest items sit at the END of the index; the UI presents them first.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct LibraryMeta {
    /// Payload file name inside the library dir. It is always one normal path
    /// component; callers never get to escape the managed directory.
    pub file: String,
    pub label: String,
    pub domain: String,
    pub content_type: String,
    pub prompt: String,
    /// Stable pipeline-run / import group id. `Option` on purpose: indexes
    /// written before grouping deserialize with `None` (micro-serde treats a
    /// missing key as None and skips None on write), and those records render
    /// under one "Earlier imports" group. Grouping is NEVER inferred from
    /// adjacency — only from this persisted id.
    pub group_id: Option<String>,
    /// Human-readable run label ("video (small) — \"storm at sea\"").
    pub group_label: Option<String>,
    /// Import / pack tags (`darkmod`, `freedoom`, `kenney`, `space-kit`).
    /// `None` on indexes written before tags existed; [`Library::open`]
    /// backfills from `group_id` so old imports become filterable.
    pub tags: Option<Vec<String>>,
    /// Vision / enhance-metadata tags, written by a later Qwen-VL pass
    /// over thumbnails. Kept separate so a caption run cannot overwrite
    /// the import source tags the Library filter is built on.
    pub enhanced_tags: Option<Vec<String>>,
    /// Is this row the run's PRODUCT (the thing the user asked for) rather
    /// than an intermediate stage artifact (source image, untextured mesh,
    /// PBR map, sidecar)? Written at route time by the pipeline, where the
    /// stage index is known; `None` on rows from before the field existed
    /// and on non-run routes (drops, webcam, manual imports). Consumed by
    /// the publish loop, which tags non-products `intermediate` so program
    /// surfaces can exclude them.
    pub product: Option<bool>,
}

impl LibraryMeta {
    pub fn import_tags(&self) -> &[String] {
        self.tags.as_deref().unwrap_or(&[])
    }

    pub fn vision_tags(&self) -> &[String] {
        self.enhanced_tags.as_deref().unwrap_or(&[])
    }

    /// Shelf + import tags + vision tags. Type shelves (`characters`, `maps`)
    /// live in the same filter cloud as pack tags.
    pub fn filter_tags(&self) -> Vec<String> {
        let mut out = vec![asset_shelf(&self.domain, &self.content_type).to_string()];
        for tag in self.import_tags().iter().chain(self.vision_tags()) {
            if !out.iter().any(|have: &String| have.eq_ignore_ascii_case(tag)) {
                out.push(tag.clone());
            }
        }
        out
    }
}

/// Library type shelf. Same names the tag cloud lists (`maps`, `characters`).
pub fn asset_shelf(domain: &str, content_type: &str) -> &'static str {
    let d = domain.to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    if d == "map" || d == "world" || d == "maps" {
        "maps"
    } else if d == "character" || d == "characters" {
        "characters"
    } else if d == "prop" || d == "props" {
        "props"
    } else if d == "weapon" || d == "weapons" {
        "weapons"
    } else if d == "billboard" || ct.contains("billboard") {
        "billboards"
    } else if ct.starts_with("image/") || d == "image" || d == "matte" {
        "images"
    } else if ct.starts_with("video/") {
        "video"
    } else if d == "music" {
        "music"
    } else if d == "speech" {
        "speech"
    } else if ct.starts_with("audio/") || d == "sfx" || d == "audio" {
        "sfx"
    } else if ct.contains("ply") {
        "splats"
    } else if ct.contains("gltf") || ct.contains("model/") {
        "meshes"
    } else {
        "other"
    }
}

/// Pure keep-vs-rerender decision for a re-landed GLB's `.thumb` sidecar.
///
/// `created` is `Library::import_unique_with_thumbnail`'s own return:
/// `false` means the freshly landed bytes are byte-for-byte identical to a
/// payload already in the library (an exact-content dedupe hit found via
/// `find_exact_payload`), and `ensure_thumbnail` left that payload's
/// existing sidecar untouched. `true` means genuinely new or changed
/// content, whose old sidecar (if any) has already been overwritten or
/// discarded by the land itself.
///
/// `needs_render` is `Library::needs_model_thumbnail(file)` — true when no
/// `.thumb` is actually on disk for the landed file right now.
///
/// Re-rendering only pays for new/changed content, or for a dedupe hit that
/// never got a render in the first place; a byte-identical reimport (e.g.
/// re-running "Import all" over ~3400 unchanged Kenney GLBs) keeps every
/// existing icon instead of rebuilding it.
pub fn keep_existing_glb_thumbnail(created: bool, needs_render: bool) -> bool {
    !created && !needs_render
}

#[derive(Clone, Default, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct LibraryIndex {
    pub items: Vec<LibraryMeta>,
    /// Monotonic payload-file counter — survives deletes so names never
    /// collide with a file the OS has not fully released yet.
    pub next_id: u64,
}

/// Disk cap: oldest payloads are pruned past this.
/// Kenney kits exceed 64 (space-kit is 153) and "Import all" lands ~3800
/// models at once. The cap must hold everything ONE import lands, or landing
/// evicts members mid-thumbnail and later cards (and the server thumbnails
/// re-imports build from them) never get icons.
pub const LIBRARY_CAP: usize = 8192;

/// One unit of missing-preview regeneration work. The app drains these a
/// bounded slice at a time after the first frame; until a job lands its card
/// simply shows the domain badge (or the audio payload-derived fallback).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbnailBackfillJob {
    /// GLB payload with no sidecar: offscreen GPU model render (MeshView).
    ModelRender { file: String },
    /// WAV payload with no sidecar: CPU waveform strip persisted as PNG.
    AudioWaveform { file: String },
}

/// Derived-preview semantics versions, one per sidecar kind. Payloads and
/// the index stay source of truth; sidecars are disposable derivations, so a
/// version bump deletes ONLY the stale kind's sidecars once, and the bounded
/// background regenerator rebuilds them from the payloads.
///
/// model v12: yaw around AABB centre (v11 rotated pack-grid models off-card).
const MODEL_PREVIEW_VERSION: &str = "12-center-yaw";
/// audio v1: a WAV's preview is ALWAYS its own waveform strip. Earlier
/// sidecars could be a byte-copy of an upstream pipeline image (provenance
/// bug: lib-55.wav.thumb == lib-54.png) and are exactly what this discards.
const AUDIO_PREVIEW_VERSION: &str = "3-hd-spectrogram";
const PREVIEW_VERSIONS_FILE: &str = ".preview-versions";
/// Pre-split marker (model-only semantics); superseded and reaped on open.
const LEGACY_MODEL_VERSION_FILE: &str = ".model-thumbnail-version";

pub struct Library {
    dir: PathBuf,
    pub index: LibraryIndex,
}

impl Library {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        let index = fs::read_to_string(dir.join("index.json"))
            .ok()
            .and_then(|s| LibraryIndex::deserialize_json(&s).ok())
            .unwrap_or_default();
        // Crash recovery, decided by the DURABLE index alone: a tombstone
        // whose payload the committed index still references belongs to a
        // deletion that never committed — restore it (rollback). A tombstone
        // the index no longer references is a committed deletion's leftover
        // — reap it (roll forward). Runs before the liveness retain below so
        // restored payloads keep their index entries.
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some((is_payload, file)) = parse_tombstone(name) else {
                    continue;
                };
                let referenced = index.items.iter().any(|item| item.file == file);
                let target = if is_payload {
                    dir.join(&file)
                } else {
                    dir.join(format!("{file}.thumb"))
                };
                if referenced && !target.exists() {
                    let _ = fs::rename(entry.path(), &target);
                } else {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let mut library = Self { dir, index };
        // Drop stale or unsafe entries, but never follow an untrusted path out
        // of the managed directory.
        let dir = library.dir.clone();
        library.index.items.retain(|item| {
            is_safe_file_name(&item.file) && dir.join(&item.file).is_file()
        });
        // Opening the library is metadata-only: do not decode images or parse
        // every sidecar-missing GLB before the first window. A missing preview
        // renders as a typed badge and the app's bounded background pump
        // regenerates it. This cheap per-kind version migration discards
        // sidecars whose derivation semantics changed (file unlinks only).
        library.invalidate_stale_previews();
        let mut dirty = library.backfill_missing_tags();
        dirty |= library.backfill_missing_products();
        // The in-process publish loop reads index.json from DISK, so a
        // backfilled flag that only exists in memory would publish the
        // whole library as products. Commit once, here.
        if dirty {
            let _ = library.save();
        }
        library
    }

    pub fn len(&self) -> usize {
        self.index.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.items.is_empty()
    }

    /// Metadata in the exact order displayed by History.
    pub fn newest_items(&self) -> impl Iterator<Item = &LibraryMeta> {
        self.index.items.iter().rev()
    }

    pub fn get(&self, file: &str) -> Option<&LibraryMeta> {
        self.index.items.iter().find(|item| item.file == file)
    }

    /// File ids for every item `predicate` accepts, newest first — exactly
    /// the order and rule the Library grid renders with. Shared seam for
    /// "act on everything the current filter shows" UI (e.g. bulk delete)
    /// so the shown count and the acted-on set can never disagree: both
    /// come from this one pass over the index.
    pub fn files_matching(&self, mut predicate: impl FnMut(&LibraryMeta) -> bool) -> Vec<String> {
        self.newest_items()
            .filter(|item| predicate(item))
            .map(|item| item.file.clone())
            .collect()
    }

    pub fn payload_path(&self, file: &str) -> io::Result<PathBuf> {
        if !is_safe_file_name(file) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "library payload must be one normal file name",
            ));
        }
        Ok(self.dir.join(file))
    }

    pub fn load_bytes(&self, file: &str) -> io::Result<Vec<u8>> {
        fs::read(self.payload_path(file)?)
    }

    /// Persisted preview sidecar for a managed payload, when one exists.
    /// Image payloads remain their own preview and therefore need no sidecar.
    pub fn thumbnail_path(&self, file: &str) -> io::Result<Option<PathBuf>> {
        let path = self.thumbnail_sidecar_path(file)?;
        Ok(path.is_file().then_some(path))
    }

    /// Drop a persisted model preview so the next import/backfill must
    /// re-render. Used when reimporting a pack so stale icons cannot linger.
    pub fn discard_model_thumbnail(&self, file: &str) -> io::Result<bool> {
        let path = self.thumbnail_sidecar_path(file)?;
        if !path.is_file() {
            return Ok(false);
        }
        fs::remove_file(path)?;
        Ok(true)
    }

    /// True only for a managed model payload whose preview sidecar is
    /// missing (never written, discarded on reimport, or version-invalidated).
    pub fn needs_model_thumbnail(&self, file: &str) -> bool {
        let Some(item) = self.get(file) else {
            return false;
        };
        is_model_payload(item) && matches!(self.thumbnail_path(file), Ok(None))
    }

    /// Newest-first regeneration work list over payloads whose preview
    /// sidecar is missing. Metadata plus one stat per item — payload bytes
    /// are only read later, when the app feeds a job to the renderer.
    pub fn thumbnail_backfill_queue(&self) -> Vec<ThumbnailBackfillJob> {
        self.newest_items()
            .filter_map(|item| {
                let content_type = item.content_type.to_ascii_lowercase();
                if content_type.starts_with("image/")
                    || !matches!(self.thumbnail_path(&item.file), Ok(None))
                {
                    return None;
                }
                if is_model_payload(item) {
                    Some(ThumbnailBackfillJob::ModelRender {
                        file: item.file.clone(),
                    })
                } else if content_type.starts_with("audio/") {
                    Some(ThumbnailBackfillJob::AudioWaveform {
                        file: item.file.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Atomically replace a managed payload's preview with an encoded PNG
    /// rendered from the payload itself. The stable file id is checked again
    /// at commit time: a late GPU readback for an item deleted while it was in
    /// flight is rejected instead of recreating an orphan thumbnail.
    pub fn replace_thumbnail_png(&self, file: &str, png: &[u8]) -> io::Result<()> {
        const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
        if !png.starts_with(PNG_SIGNATURE) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "library thumbnail replacement must be an encoded PNG",
            ));
        }
        if self.get(file).is_none() || !self.payload_path(file)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "library payload was deleted before its thumbnail completed",
            ));
        }

        let target = self.thumbnail_sidecar_path(file)?;
        let temp = self.dir.join(format!(
            ".thumbnail.tmp-{}-{}-{file}",
            std::process::id(),
            self.index.next_id,
        ));
        let mut output = fs::File::create(&temp)?;
        output.write_all(png)?;
        output.sync_all()?;
        drop(output);

        #[cfg(not(windows))]
        {
            if let Err(error) = fs::rename(&temp, &target) {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        }
        #[cfg(windows)]
        {
            // Windows rename cannot replace an existing file. Keep the old
            // preview recoverable until the new one is in place.
            let backup = self.dir.join(format!(
                ".thumbnail.previous-{}-{}-{file}",
                std::process::id(),
                self.index.next_id,
            ));
            let _ = fs::remove_file(&backup);
            if target.is_file() {
                fs::rename(&target, &backup)?;
            }
            if let Err(error) = fs::rename(&temp, &target) {
                let _ = fs::rename(&backup, &target);
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
            let _ = fs::remove_file(backup);
        }
        Ok(())
    }

    /// Return the stable file id of an exact existing payload. This is used by
    /// managed imports so reopening an accepted playtest GLB cannot duplicate
    /// it on every app launch.
    pub fn find_exact_payload(&self, bytes: &[u8]) -> Option<String> {
        self.newest_items().find_map(|item| {
            let path = self.payload_path(&item.file).ok()?;
            let metadata = fs::metadata(&path).ok()?;
            if metadata.len() != bytes.len() as u64 {
                return None;
            }
            (fs::read(path).ok()?.as_slice() == bytes).then(|| item.file.clone())
        })
    }

    /// Add a payload, returning its stable managed file id. Payload and index
    /// are committed transactionally from the UI's perspective: an index-save
    /// failure restores the old in-memory index and removes the new payload.
    pub fn add(
        &mut self,
        domain: &str,
        content_type: &str,
        prompt: &str,
        label: &str,
        bytes: &[u8],
    ) -> io::Result<String> {
        self.add_with_thumbnail(domain, content_type, prompt, label, bytes, None, None, None)
    }

    /// Add a payload together with a visual preview. The preview is stored as
    /// encoded image bytes in a deterministic sidecar. If the caller has no
    /// upstream render, a GLB's embedded base-color image is used as a useful
    /// fallback instead of a blank history tile. `group` is the persisted
    /// (run id, run label) this artifact belongs to.
    pub fn add_with_thumbnail(
        &mut self,
        domain: &str,
        content_type: &str,
        prompt: &str,
        label: &str,
        bytes: &[u8],
        thumbnail: Option<&[u8]>,
        group: Option<(&str, &str)>,
        product: Option<bool>,
    ) -> io::Result<String> {
        let original = self.index.clone();
        self.index.next_id = self.index.next_id.saturating_add(1);
        let file = format!(
            "lib-{}.{}",
            self.index.next_id,
            ext_of(content_type, bytes)
        );
        let path = self.payload_path(&file)?;
        fs::write(&path, bytes)?;
        let thumbnail_path = self.thumbnail_sidecar_path(&file)?;
        // PROVENANCE, enforced at the persistence boundary (not only in the
        // routing layer): an audio payload's preview is ALWAYS derived from
        // its own WAV. Any caller-supplied bytes — e.g. an upstream pipeline
        // image — are discarded, so no future caller can reintroduce the
        // gorilla-thumbnail bug. Unparseable audio gets no sidecar; the
        // bounded backfill regenerates one later.
        let thumbnail = if is_audio_content(content_type) {
            own_waveform_thumbnail(bytes)
        } else {
            thumbnail
                .map(ToOwned::to_owned)
                .or_else(|| embedded_gltf_thumbnail(content_type, bytes))
        };
        if let Some(thumbnail) = thumbnail {
            if let Err(error) = fs::write(&thumbnail_path, thumbnail) {
                self.index = original;
                let _ = fs::remove_file(path);
                let _ = fs::remove_file(thumbnail_path);
                return Err(error);
            }
        }

        self.index.items.push(LibraryMeta {
            file: file.clone(),
            label: label.to_string(),
            domain: domain.to_string(),
            content_type: content_type.to_string(),
            prompt: prompt.to_string(),
            group_id: group.map(|(id, _)| id.to_string()),
            group_label: group.map(|(_, label)| label.to_string()),
            tags: Some(infer_import_tags(
                group.map(|(id, _)| id),
                group.map(|(_, label)| label),
                prompt,
            )),
            enhanced_tags: None,
            product,
        });
        let mut pruned = Vec::new();
        while self.index.items.len() > LIBRARY_CAP {
            pruned.push(self.index.items.remove(0));
        }
        if let Err(error) = self.save() {
            self.index = original;
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(thumbnail_path);
            return Err(error);
        }
        // Pruning happens only after the new index is durable. A failed unlink
        // can leave an orphan, but can never make the index point at no file.
        for item in pruned {
            self.reap_payload_files(&item.file);
        }
        Ok(file)
    }

    /// Copy AO/shadow sidecars from beside a source GLB onto the managed
    /// library payload (`lib-N.aomesh` / `lib-N.ao.png` / `lib-N.shadowsdf`).
    /// Missing sources are skipped; never invents AO for a mesh that has none.
    pub fn install_ao_sidecars(&self, file: &str, source_glb: &Path) -> io::Result<()> {
        let dest = self.payload_path(file)?;
        for ext in ["aomesh", "ao.png", "shadowsdf", "spawn", "place"] {
            let src = source_glb.with_extension(ext);
            if !src.is_file() {
                continue;
            }
            let dst = dest.with_extension(ext);
            fs::copy(&src, &dst)?;
        }
        Ok(())
    }

    /// Copy native-size sprite frames next to a landed `.billboard` and
    /// rewrite the manifest to those sibling names (`lib-N.f000.png`).
    ///
    /// A packed sheet (`SheetLayout`) puts many frames' `cell` entries on the
    /// SAME source file (`sheet_file`); a legacy manifest still gives every
    /// frame its own file. Either way, each distinct `frame.file` is copied
    /// exactly once — under the name of the FIRST frame index that
    /// references it — and every frame sharing that source is rewritten to
    /// the one landed name. Without the dedupe, a ~40-frame packed actor
    /// would copy its single shared sheet ~40 times.
    pub fn install_billboard_frames(&self, file: &str, source_manifest: &Path) -> io::Result<()> {
        use std::collections::BTreeMap;
        let dest = self.payload_path(file)?;
        let text = fs::read_to_string(source_manifest)?;
        let Ok(mut bb) = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&text)
        else {
            return Ok(());
        };
        let src_dir = source_manifest.parent().unwrap_or(source_manifest);
        let stem = dest
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("lib");
        let mut copied: BTreeMap<String, String> = BTreeMap::new();
        for (i, frame) in bb.frames.iter_mut().enumerate() {
            if let Some(name) = copied.get(&frame.file) {
                frame.file = name.clone();
                continue;
            }
            let src = src_dir.join(&frame.file);
            if !src.is_file() {
                continue;
            }
            let name = format!("{stem}.f{i:03}.png");
            fs::copy(&src, self.dir.join(&name))?;
            copied.insert(frame.file.clone(), name.clone());
            frame.file = name;
        }
        fs::write(&dest, bb.to_text())?;
        Ok(())
    }

    /// First-person spawn sidecar (`world-spawn 1` / pos / yaw pitch).
    pub fn world_spawn(&self, file: &str) -> Option<([f32; 3], f32, f32)> {
        let dest = self.payload_path(file).ok()?;
        parse_world_spawn(&dest.with_extension("spawn"))
    }

    /// Placement sidecar next to a World GLB, if import wrote one.
    pub fn world_place(&self, file: &str) -> Option<makepad_asset_importer::world_place::WorldPlace> {
        let dest = self.payload_path(file).ok()?;
        let text = fs::read_to_string(dest.with_extension("place")).ok()?;
        makepad_asset_importer::world_place::WorldPlace::parse(&text).ok()
    }

    /// Resolve a place asset key (`billboards/duke3d/tile-1405`) to a library file.
    pub fn find_place_asset(&self, asset_key: &str) -> Option<PathBuf> {
        if asset_key.is_empty() {
            return None;
        }
        let key = asset_key.replace('\\', "/").to_ascii_lowercase();
        let stem = key.rsplit('/').next().unwrap_or(key.as_str());
        if stem.is_empty() {
            return None;
        }
        let needle = format!(" · {stem} · ");
        let needle_end = format!(" · {stem}");
        let dotted = key.replace('/', " · ");
        let score = |item: &LibraryMeta| -> i32 {
            let prompt = item.prompt.to_ascii_lowercase();
            let label = item.label.to_ascii_lowercase();
            let ct = item.content_type.to_ascii_lowercase();
            let domain = item.domain.to_ascii_lowercase();
            let matched = label == stem
                || label == dotted
                || prompt.contains(&needle)
                || prompt.ends_with(&needle_end)
                || prompt.contains(&dotted);
            if !matched {
                return -1;
            }
            let mesh = ct.contains("gltf")
                || matches!(
                    domain.as_str(),
                    "mesh" | "character" | "weapon" | "prop" | "world" | "map"
                );
            if mesh {
                2
            } else {
                0
            }
        };
        let item = self
            .index
            .items
            .iter()
            .rev()
            .filter_map(|item| {
                let s = score(item);
                (s >= 0).then_some((s, item))
            })
            .max_by_key(|(s, _)| *s)?
            .1;
        self.payload_path(&item.file).ok()
    }

    /// Offline AO pair next to a managed GLB, if both bake files landed.
    pub fn ao_sidecar_bytes(&self, file: &str) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let Ok(dest) = self.payload_path(file) else {
            return (None, None);
        };
        (
            fs::read(dest.with_extension("aomesh")).ok(),
            fs::read(dest.with_extension("ao.png")).ok(),
        )
    }

    /// Where a playable rig's baked rest bundle (`.skinao`) lives for this
    /// payload. The viewer reads it when its hash matches the rig and writes
    /// it after an inline bake.
    pub fn rig_cache_path(&self, file: &str) -> Option<PathBuf> {
        self.payload_path(file).ok().map(|p| p.with_extension("skinao"))
    }

    fn reap_payload_files(&self, file: &str) {
        if let Ok(path) = self.payload_path(file) {
            let _ = fs::remove_file(&path);
            for ext in ["aomesh", "ao.png", "shadowsdf", "skinao", "spawn", "place"] {
                let _ = fs::remove_file(path.with_extension(ext));
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(rd) = fs::read_dir(&self.dir) {
                    for entry in rd.flatten() {
                        let name = entry.file_name();
                        let Some(name) = name.to_str() else { continue };
                        if name.starts_with(&format!("{stem}.f")) && name.ends_with(".png") {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
        if let Ok(path) = self.thumbnail_sidecar_path(file) {
            let _ = fs::remove_file(path);
        }
    }

    /// Import a caller-provided artifact exactly once. The source path is not
    /// moved or deleted; History owns a separate managed copy.
    pub fn import_unique(
        &mut self,
        domain: &str,
        content_type: &str,
        prompt: &str,
        label: &str,
        bytes: &[u8],
    ) -> io::Result<(String, bool)> {
        self.import_unique_with_thumbnail(domain, content_type, prompt, label, bytes, None, None)
    }

    pub fn import_unique_with_thumbnail(
        &mut self,
        domain: &str,
        content_type: &str,
        prompt: &str,
        label: &str,
        bytes: &[u8],
        thumbnail: Option<&[u8]>,
        group: Option<(&str, &str)>,
    ) -> io::Result<(String, bool)> {
        if let Some(file) = self.find_exact_payload(bytes) {
            self.ensure_thumbnail(&file, content_type, bytes, thumbnail)?;
            return Ok((file, false));
        }
        if let Some((group_id, _)) = group {
            let prefix = prompt.split('·').next().unwrap_or(prompt).trim();
            if let Some(file) = self.find_import_replace_target(group_id, label, prefix) {
                self.overwrite_payload(
                    &file,
                    domain,
                    content_type,
                    prompt,
                    label,
                    bytes,
                    thumbnail,
                    group,
                )?;
                self.collapse_duplicate_labels(group_id, label, &file, prefix)?;
                return Ok((file, true));
            }
        }
        let (file, created) = self
            .add_with_thumbnail(domain, content_type, prompt, label, bytes, thumbnail, group, None)
            .map(|file| (file, true))?;
        if let Some((group_id, _)) = group {
            let prefix = prompt.split('·').next().unwrap_or(prompt).trim();
            self.collapse_duplicate_labels(group_id, label, &file, prefix)?;
        }
        Ok((file, created))
    }

    /// Same `(group_id, label)` — or the same label under the same prompt
    /// prefix, so a Freedoom `MAP27` that previously landed in the Kenney
    /// group is replaced instead of duplicated.
    pub fn find_import_replace_target(
        &self,
        group_id: &str,
        label: &str,
        prompt_prefix: &str,
    ) -> Option<String> {
        if let Some(file) = self
            .index
            .items
            .iter()
            .rev()
            .find(|item| {
                item.group_id.as_deref() == Some(group_id) && import_labels_equiv(&item.label, label)
            })
            .map(|item| item.file.clone())
        {
            return Some(file);
        }
        let prefix = prompt_prefix.trim();
        if prefix.is_empty() {
            return None;
        }
        self.index
            .items
            .iter()
            .rev()
            .find(|item| {
                import_labels_equiv(&item.label, label) && item.prompt.starts_with(prefix)
            })
            .map(|item| item.file.clone())
    }

    /// Keep `keep_file`; delete every other library item with the same
    /// display label in this import group (or the same prompt prefix).
    fn collapse_duplicate_labels(
        &mut self,
        group_id: &str,
        label: &str,
        keep_file: &str,
        prompt_prefix: &str,
    ) -> io::Result<usize> {
        let prefix = prompt_prefix.trim();
        let extras: Vec<String> = self
            .index
            .items
            .iter()
            .filter(|item| {
                item.file != keep_file
                    && import_labels_equiv(&item.label, label)
                    && (item.group_id.as_deref() == Some(group_id)
                        || (!prefix.is_empty() && item.prompt.starts_with(prefix)))
            })
            .map(|item| item.file.clone())
            .collect();
        for file in &extras {
            let _ = self.remove_by_file(file);
        }
        Ok(extras.len())
    }

    fn overwrite_payload(
        &mut self,
        file: &str,
        domain: &str,
        content_type: &str,
        prompt: &str,
        label: &str,
        bytes: &[u8],
        thumbnail: Option<&[u8]>,
        group: Option<(&str, &str)>,
    ) -> io::Result<()> {
        let path = self.payload_path(file)?;
        fs::write(&path, bytes)?;
        if let Some(item) = self.index.items.iter_mut().find(|item| item.file == file) {
            item.domain = domain.to_string();
            item.content_type = content_type.to_string();
            item.prompt = prompt.to_string();
            item.label = label.to_string();
            if let Some((group_id, group_label)) = group {
                item.group_id = Some(group_id.to_string());
                item.group_label = Some(group_label.to_string());
            }
            item.tags = Some(infer_import_tags(
                item.group_id.as_deref(),
                item.group_label.as_deref(),
                &item.prompt,
            ));
        }
        if let Some(thumbnail) = thumbnail {
            fs::write(self.thumbnail_sidecar_path(file)?, thumbnail)?;
        } else if content_type.contains("gltf") {
            let _ = self.discard_model_thumbnail(file);
        }
        self.save()
    }

    /// Delete a managed payload by stable file id. The payload is first moved
    /// to a same-directory tombstone, then the index is atomically committed.
    /// If the index commit fails the payload and in-memory index are restored.
    pub fn remove_by_file(&mut self, file: &str) -> io::Result<bool> {
        let payload = self.payload_path(file)?;
        let Some(position) = self.index.items.iter().position(|item| item.file == file) else {
            return Ok(false);
        };
        let original = self.index.clone();
        let thumbnail = self.thumbnail_sidecar_path(file)?;
        let mut tombstones = Vec::new();
        let mut sources = vec![
            ("payload", payload.clone()),
            ("thumb", thumbnail),
        ];
        for ext in ["aomesh", "ao.png", "shadowsdf", "spawn", "place"] {
            sources.push((ext, payload.with_extension(ext)));
        }
        for (kind, source) in sources {
            if !source.is_file() {
                continue;
            }
            let tombstone = self.dir.join(format!(
                ".delete-{}-{}-{kind}-{file}",
                std::process::id(),
                self.index.next_id,
            ));
            if let Err(error) = fs::rename(&source, &tombstone) {
                for (source, tombstone) in tombstones.into_iter().rev() {
                    let _ = fs::rename(tombstone, source);
                }
                return Err(error);
            }
            tombstones.push((source, tombstone));
        }

        self.index.items.remove(position);
        if let Err(error) = self.save() {
            self.index = original;
            for (source, tombstone) in tombstones.into_iter().rev() {
                let _ = fs::rename(tombstone, source);
            }
            return Err(error);
        }
        for (_, tombstone) in tombstones {
            let _ = fs::remove_file(tombstone);
        }
        Ok(true)
    }

    /// Delete every member of one pipeline-run / import group in a single
    /// index transaction. `group = None` targets the "Earlier imports"
    /// pseudo-group (records that predate grouping). All member payloads and
    /// sidecars are tombstone-renamed first; ONE atomic index save commits
    /// the removal, and a failed save rolls every rename and the in-memory
    /// index back — other groups are never touched either way. Returns how
    /// many items were removed.
    pub fn remove_group(&mut self, group: Option<&str>) -> io::Result<usize> {
        let members: Vec<String> = self
            .index
            .items
            .iter()
            .filter(|item| item.group_id.as_deref() == group)
            .map(|item| item.file.clone())
            .collect();
        if members.is_empty() {
            return Ok(0);
        }
        let original = self.index.clone();
        let mut tombstones = Vec::new();
        for file in &members {
            let payload = self.payload_path(file)?;
            let mut sources = vec![
                ("payload", payload.clone()),
                ("thumb", self.thumbnail_sidecar_path(file)?),
            ];
            for ext in ["aomesh", "ao.png", "shadowsdf"] {
                sources.push((ext, payload.with_extension(ext)));
            }
            for (kind, source) in sources {
                if !source.is_file() {
                    continue;
                }
                let tombstone = self.dir.join(format!(
                    ".delete-{}-{}-{kind}-{file}",
                    std::process::id(),
                    self.index.next_id,
                ));
                if let Err(error) = fs::rename(&source, &tombstone) {
                    for (source, tombstone) in tombstones.into_iter().rev() {
                        let _ = fs::rename(tombstone, source);
                    }
                    return Err(error);
                }
                tombstones.push((source, tombstone));
            }
        }

        self.index
            .items
            .retain(|item| item.group_id.as_deref() != group);
        if let Err(error) = self.save() {
            self.index = original;
            for (source, tombstone) in tombstones.into_iter().rev() {
                let _ = fs::rename(tombstone, source);
            }
            return Err(error);
        }
        for (_, tombstone) in tombstones {
            let _ = fs::remove_file(tombstone);
        }
        Ok(members.len())
    }

    fn thumbnail_sidecar_path(&self, file: &str) -> io::Result<PathBuf> {
        if !is_safe_file_name(file) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "library thumbnail must belong to one normal payload file",
            ));
        }
        Ok(self.dir.join(format!("{file}.thumb")))
    }

    fn ensure_thumbnail(
        &self,
        file: &str,
        content_type: &str,
        bytes: &[u8],
        thumbnail: Option<&[u8]>,
    ) -> io::Result<()> {
        let path = self.thumbnail_sidecar_path(file)?;
        // Audio previews are contract-bound to the payload's own waveform —
        // the dedupe "upgrade" path must not smuggle a caller image in.
        if is_audio_content(content_type) {
            if !path.is_file() {
                if let Some(wave) = own_waveform_thumbnail(bytes) {
                    fs::write(path, wave)?;
                }
            }
            return Ok(());
        }
        // An explicit beauty render is authoritative and may intentionally
        // upgrade an older embedded-atlas fallback on a deduplicated import.
        if let Some(thumbnail) = thumbnail {
            fs::write(path, thumbnail)?;
            return Ok(());
        }
        if path.is_file() {
            return Ok(());
        }
        if let Some(thumbnail) = embedded_gltf_thumbnail(content_type, bytes) {
            fs::write(path, thumbnail)?;
        }
        Ok(())
    }

    fn invalidate_stale_previews(&self) {
        let marker = self.dir.join(PREVIEW_VERSIONS_FILE);
        let recorded = fs::read_to_string(&marker).unwrap_or_default();
        let recorded_version = |kind: &str| -> Option<String> {
            recorded.lines().find_map(|line| {
                line.trim()
                    .strip_prefix(kind)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .map(|version| version.trim().to_string())
            })
        };
        let model_stale = recorded_version("model").as_deref() != Some(MODEL_PREVIEW_VERSION);
        let audio_stale = recorded_version("audio").as_deref() != Some(AUDIO_PREVIEW_VERSION);
        for item in &self.index.items {
            let stale = (model_stale && is_model_payload(item))
                || (audio_stale && is_audio_payload(item));
            if !stale {
                continue;
            }
            if let Ok(sidecar) = self.thumbnail_sidecar_path(&item.file) {
                let _ = fs::remove_file(sidecar);
            }
        }
        // If this write fails, repeating the safe derived-preview
        // invalidation next launch beats trusting a known-bad preview.
        let _ = fs::write(
            marker,
            format!("model:{MODEL_PREVIEW_VERSION}\naudio:{AUDIO_PREVIEW_VERSION}\n"),
        );
        let _ = fs::remove_file(self.dir.join(LEGACY_MODEL_VERSION_FILE));
    }

    pub fn persist(&self) -> io::Result<()> {
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let target = self.dir.join("index.json");
        let temp = self.dir.join(format!(
            ".index.json.tmp-{}-{}",
            std::process::id(),
            self.index.next_id
        ));
        let mut output = fs::File::create(&temp)?;
        output.write_all(self.index.serialize_json().as_bytes())?;
        output.sync_all()?;
        drop(output);

        #[cfg(not(windows))]
        {
            fs::rename(&temp, &target)?;
        }
        #[cfg(windows)]
        {
            // Rust's Windows rename cannot replace an existing file. Keep a
            // rollback copy so a failed second rename cannot lose the index.
            let backup = self.dir.join(".index.json.previous");
            let _ = fs::remove_file(&backup);
            if target.is_file() {
                fs::rename(&target, &backup)?;
            }
            if let Err(error) = fs::rename(&temp, &target) {
                let _ = fs::rename(&backup, &target);
                return Err(error);
            }
            let _ = fs::remove_file(backup);
        }
        Ok(())
    }

    /// Write vision-generated tags for one item. Import tags stay untouched.
    #[allow(dead_code)]
    pub fn set_enhanced_tags(&mut self, file: &str, tags: Vec<String>) -> io::Result<bool> {
        let Some(item) = self.index.items.iter_mut().find(|item| item.file == file) else {
            return Ok(false);
        };
        let mut cleaned = Vec::new();
        for tag in tags {
            let tag = normalize_tag(&tag);
            if !tag.is_empty() && !cleaned.iter().any(|have| have == &tag) {
                cleaned.push(tag);
            }
        }
        item.enhanced_tags = Some(cleaned);
        self.save()?;
        Ok(true)
    }

    /// Fill `tags` on records written before the field existed, so the
    /// Library dropdown can list `freedoom` / `darkmod` without a re-import
    /// (and so the publish loop's generated-only scope gate sees the
    /// `generated` tag on legacy `run-…` rows). Returns whether anything
    /// changed; [`Library::open`] commits both backfills in one save.
    fn backfill_missing_tags(&mut self) -> bool {
        let mut dirty = false;
        for item in &mut self.index.items {
            if item.tags.is_some() {
                continue;
            }
            item.tags = Some(infer_import_tags(
                item.group_id.as_deref(),
                item.group_label.as_deref(),
                &item.prompt,
            ));
            dirty = true;
        }
        dirty
    }

    /// Fill `product` on records written before the field existed, using the
    /// importer's shared group classifier over the whole ordered index. The
    /// inference needs COMPLETE groups, which is exactly what a persisted
    /// index holds; live runs never reach here — they author the flag at
    /// route time, where the stage index is known.
    fn backfill_missing_products(&mut self) -> bool {
        if self.index.items.iter().all(|item| item.product.is_some()) {
            return false;
        }
        let flags = {
            let rows: Vec<makepad_asset_importer::import::ProductRow<'_>> = self
                .index
                .items
                .iter()
                .map(|item| makepad_asset_importer::import::ProductRow {
                    domain: &item.domain,
                    content_type: &item.content_type,
                    group_id: item.group_id.as_deref(),
                    product: item.product,
                })
                .collect();
            makepad_asset_importer::import::classify_products(&rows)
        };
        let mut dirty = false;
        for (item, flag) in self.index.items.iter_mut().zip(flags) {
            if item.product.is_none() {
                item.product = Some(flag);
                dirty = true;
            }
        }
        dirty
    }
}

/// `MAP27` and `freedoom2 MAP27` are the same world after a title tweak.
fn import_labels_equiv(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    a.strip_suffix(b).is_some_and(|prefix| prefix.ends_with(' '))
        || b.strip_suffix(a).is_some_and(|prefix| prefix.ends_with(' '))
}

/// One unique tag and how many library items carry it. Sorted most-used
/// first so a pack like `freedoom` stays near the top of a long dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagStat {
    pub name: String,
    pub count: usize,
    /// True when at least one item has this name in `enhanced_tags`.
    pub enhanced: bool,
}

pub fn collect_tag_stats<'a>(items: impl Iterator<Item = &'a LibraryMeta>) -> Vec<TagStat> {
    use std::collections::{BTreeMap, BTreeSet};
    struct Acc {
        count: usize,
        enhanced: bool,
    }
    let mut map: BTreeMap<String, Acc> = BTreeMap::new();
    for item in items {
        let mut seen = BTreeSet::new();
        let shelf = asset_shelf(&item.domain, &item.content_type);
        if seen.insert(shelf.to_string()) {
            map.entry(shelf.to_string())
                .or_insert(Acc {
                    count: 0,
                    enhanced: false,
                })
                .count += 1;
        }
        for tag in item.import_tags() {
            let key = normalize_tag(tag);
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            map.entry(key).or_insert(Acc {
                count: 0,
                enhanced: false,
            })
            .count += 1;
        }
        for tag in item.vision_tags() {
            let key = normalize_tag(tag);
            if key.is_empty() {
                continue;
            }
            let first = seen.insert(key.clone());
            let acc = map.entry(key).or_insert(Acc {
                count: 0,
                enhanced: true,
            });
            if first {
                acc.count += 1;
            }
            acc.enhanced = true;
        }
    }
    let mut out: Vec<TagStat> = map
        .into_iter()
        .map(|(name, acc)| TagStat {
            name,
            count: acc.count,
            enhanced: acc.enhanced,
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Source / pack tags inferred from the persisted group id (`import:freedoom:freedoom`)
/// or, for older records, from the landing prompt prefix. Pipeline runs get
/// `generated` so they can be sliced out of the game-import pile.
pub fn infer_import_tags(
    group_id: Option<&str>,
    _group_label: Option<&str>,
    prompt: &str,
) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(id) = group_id {
        if let Some(rest) = id.strip_prefix("import:") {
            let mut parts = rest.split(':');
            if let Some(source) = parts.next() {
                push_tag(&mut tags, source);
            }
            if let Some(pack) = parts.next() {
                push_tag(&mut tags, pack);
            }
        } else if id.starts_with("run-") {
            push_tag(&mut tags, "generated");
        }
    }
    if tags.is_empty() {
        push_prompt_source_tag(&mut tags, prompt);
    }
    tags
}

fn push_prompt_source_tag(tags: &mut Vec<String>, prompt: &str) {
    let p = prompt.to_ascii_lowercase();
    let source = if p.starts_with("the dark mod") {
        "darkmod"
    } else if p.starts_with("freedoom") {
        "freedoom"
    } else if p.starts_with("librequake") {
        "librequake"
    } else if p.starts_with("kaykit") {
        "kaykit"
    } else if p.starts_with("kenney") {
        "kenney"
    } else if p.starts_with("duke") {
        "duke3d"
    } else if p.starts_with("quake ii") || p.starts_with("quake 2") {
        "quake2"
    } else if p.starts_with("quake iii") || p.starts_with("quake 3") {
        "quake3"
    } else if p.starts_with("quake") {
        "quake"
    } else if p.starts_with("doom") {
        "doom"
    } else {
        return;
    };
    push_tag(tags, source);
}

fn push_tag(tags: &mut Vec<String>, raw: &str) {
    let tag = normalize_tag(raw);
    if tag.is_empty() {
        return;
    }
    if !tags.iter().any(|have| have == &tag) {
        tags.push(tag);
    }
}

pub fn normalize_tag(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_whitespace() { '-' } else { ch })
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

/// Allocate a new stable group id. Uniqueness comes from wall-clock millis
/// plus a process-local sequence, so two Generate clicks in the same
/// millisecond and ids from a relaunched app can never collide in one
/// library. The id is persisted on every artifact of the run; grouping is
/// never re-derived from anything else.
/// Whether a persisted group id names an import/pack landing group — the
/// `import:<source>:<pack>` convention `land_imported_pack` writes for
/// classic/Kenney bulk imports — rather than a generated pipeline run
/// (`run-…`) or another single-artifact grouping (`webcam-…`, `drop-…`, the
/// one-off `import-…` standalone-import id). Pack groups can hold thousands
/// of unrelated members (a whole Kenney kit, a whole Doom shareware pull);
/// the viewer's RUN tray exists to walk one run's own stage artifacts, so it
/// must never treat a pack import as a run.
pub fn is_import_pack_group(group_id: &str) -> bool {
    group_id.starts_with("import:")
}

pub fn new_group_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{millis}-{}-{seq}", std::process::id())
}

/// Audio payloads own the waveform-derived sidecar kind.
fn is_audio_payload(meta: &LibraryMeta) -> bool {
    is_audio_content(&meta.content_type)
}

fn is_audio_content(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().starts_with("audio/")
}

/// The ONLY legal audio preview: a waveform strip derived from the payload
/// itself. None for unparseable audio (no sidecar; backfill retries later).
fn own_waveform_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    crate::audio::parse_wav(bytes)
        .ok()
        .as_ref()
        .and_then(crate::audio::waveform_thumbnail_png)
}

/// The payload set produced/consumed by the offscreen model renderer.
/// Version invalidation, click gating and backfill queueing must all agree
/// on it, so this is the one definition. Content type is the source of
/// truth; the `.glb` extension covers payloads stored under a generic type.
pub fn parse_world_spawn(path: &Path) -> Option<([f32; 3], f32, f32)> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let header = lines.next()?.trim();
    if !header.starts_with("world-spawn") {
        return None;
    }
    let mut xyz = lines.next()?.split_whitespace();
    let pos = [
        xyz.next()?.parse().ok()?,
        xyz.next()?.parse().ok()?,
        xyz.next()?.parse().ok()?,
    ];
    let mut yp = lines.next()?.split_whitespace();
    let yaw = yp.next()?.parse().ok()?;
    let pitch = yp.next()?.parse().ok()?;
    Some((pos, yaw, pitch))
}

fn is_model_payload(meta: &LibraryMeta) -> bool {
    meta.content_type.to_ascii_lowercase().contains("gltf")
        || Path::new(&meta.file)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("glb"))
}

/// Material base-color image is a better GLB history preview than an opaque
/// domain badge. Fall back to image zero for minimal exporters without a
/// material texture binding.
fn embedded_gltf_thumbnail(content_type: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(b"glTF") && !content_type.to_ascii_lowercase().contains("gltf") {
        return None;
    }
    let loaded = makepad_gltf::load_gltf_from_bytes(bytes, None).ok()?;
    let document = &loaded.document;
    let image_index = document
        .materials_slice()
        .first()
        .and_then(|material| material.pbr_metallic_roughness.as_ref())
        .and_then(|pbr| pbr.base_color_texture.as_ref())
        .and_then(|info| document.textures_slice().get(info.index))
        .and_then(|texture| texture.source)
        .or((!document.images_slice().is_empty()).then_some(0))?;
    makepad_gltf::load_image_bytes(&loaded, image_index).ok()
}

/// Parse a `.delete-{pid}-{seq}-{kind}-{file}` tombstone name back to
/// `(is_payload, original file id)`. The trailing file id may itself contain
/// `-` (lib-N.ext), so only the three fixed fields are split off.
fn parse_tombstone(name: &str) -> Option<(bool, String)> {
    let rest = name.strip_prefix(".delete-")?;
    let mut parts = rest.splitn(4, '-');
    parts.next()?.parse::<u64>().ok()?;
    parts.next()?.parse::<u64>().ok()?;
    let is_payload = match parts.next()? {
        "payload" => true,
        "thumb" => false,
        _ => return None,
    };
    let file = parts.next()?;
    is_safe_file_name(file).then(|| (is_payload, file.to_string()))
}

fn is_safe_file_name(file: &str) -> bool {
    if file.is_empty() {
        return false;
    }
    let mut components = Path::new(file).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn ext_of(content_type: &str, bytes: &[u8]) -> &'static str {
    let ct = content_type.to_ascii_lowercase();
    if bytes.starts_with(b"glTF") || ct.contains("gltf") {
        "glb"
    } else if bytes.starts_with(b"ply") || ct.contains("ply") {
        "ply"
    } else if ct.starts_with("image/png") {
        "png"
    } else if ct.starts_with("image/") {
        "img"
    } else if ct.starts_with("audio/") {
        "wav"
    } else if ct.starts_with("video/") {
        "mp4"
    } else if ct.contains("billboard") {
        "billboard"
    } else if ct.starts_with("text/") || ct.starts_with("application/json") {
        "txt"
    } else {
        "bin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "makepad-ai-library-{name}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn keep_existing_glb_thumbnail_only_when_unchanged_and_rendered() {
        // Unchanged content (created == false) with a render already on
        // disk: keep it.
        assert!(keep_existing_glb_thumbnail(false, false));
        // Unchanged content but no render ever landed: still needs one.
        assert!(!keep_existing_glb_thumbnail(false, true));
        // New or changed content: always re-render, render state
        // notwithstanding.
        assert!(!keep_existing_glb_thumbnail(true, false));
        assert!(!keep_existing_glb_thumbnail(true, true));
    }

    #[test]
    fn reimporting_identical_glb_bytes_preserves_thumbnail() {
        let dir = TestDir::new("reimport-thumb-keep");
        let bytes = b"glTF unchanged model payload";
        let mut library = Library::open(&dir.0);
        let (file, created) = library
            .import_unique_with_thumbnail(
                "prop",
                "model/gltf-binary",
                "crate",
                "Crate",
                bytes,
                None,
                Some(("import:kenney:space-kit", "Kenney space-kit")),
            )
            .unwrap();
        assert!(created);
        // Simulate a rendered icon landing, as the GPU thumbnail pass would.
        fs::write(
            dir.0.join(format!("{file}.thumb")),
            b"\x89PNG\r\n\x1a\nrendered",
        )
        .unwrap();
        assert!(!library.needs_model_thumbnail(&file));

        // Re-land the exact same bytes under the exact same label/group, as
        // a second "Import all" pass over an unchanged pack would.
        let (file2, created2) = library
            .import_unique_with_thumbnail(
                "prop",
                "model/gltf-binary",
                "crate",
                "Crate",
                bytes,
                None,
                Some(("import:kenney:space-kit", "Kenney space-kit")),
            )
            .unwrap();
        assert_eq!(file2, file);
        assert!(
            !created2,
            "byte-identical reimport must be a dedupe hit, not a fresh add"
        );
        let needs_render = library.needs_model_thumbnail(&file2);
        assert!(
            !needs_render,
            "the existing sidecar must survive an unchanged reimport"
        );
        assert!(keep_existing_glb_thumbnail(created2, needs_render));
        // The sidecar bytes on disk are untouched by the re-land.
        assert_eq!(
            fs::read(dir.0.join(format!("{file}.thumb"))).unwrap(),
            b"\x89PNG\r\n\x1a\nrendered"
        );
    }

    #[test]
    fn reimporting_changed_glb_bytes_forces_rerender() {
        let dir = TestDir::new("reimport-thumb-rerender");
        let mut library = Library::open(&dir.0);
        let (file, created) = library
            .import_unique_with_thumbnail(
                "prop",
                "model/gltf-binary",
                "crate",
                "Crate",
                b"glTF v1",
                None,
                Some(("import:kenney:space-kit", "Kenney space-kit")),
            )
            .unwrap();
        assert!(created);
        fs::write(
            dir.0.join(format!("{file}.thumb")),
            b"\x89PNG\r\n\x1a\nrendered",
        )
        .unwrap();

        // Re-land the SAME label under the SAME group, but with different
        // bytes — a genuine content change (e.g. the upstream pack asset was
        // updated), not a no-op reimport.
        let (file2, created2) = library
            .import_unique_with_thumbnail(
                "prop",
                "model/gltf-binary",
                "crate",
                "Crate",
                b"glTF v2 - different payload bytes",
                None,
                Some(("import:kenney:space-kit", "Kenney space-kit")),
            )
            .unwrap();
        assert_eq!(file2, file);
        assert!(
            created2,
            "a genuinely changed payload must not be treated as a dedupe hit"
        );
        // overwrite_payload already drops the stale sidecar itself; the
        // landing code must also treat this as needing a fresh render.
        assert!(library.needs_model_thumbnail(&file2));
        assert!(!keep_existing_glb_thumbnail(
            created2,
            library.needs_model_thumbnail(&file2)
        ));
    }

    #[test]
    fn files_matching_selects_by_predicate_in_newest_first_order() {
        let dir = TestDir::new("files-matching");
        let mut library = Library::open(&dir.0);
        let (imp, _) = library
            .import_unique("prop", "model/gltf-binary", "p", "Doom Imp", b"a")
            .unwrap();
        let (zombie, _) = library
            .import_unique("prop", "model/gltf-binary", "p", "Doom Zombie", b"b")
            .unwrap();
        let (_crate_file, _) = library
            .import_unique("prop", "model/gltf-binary", "p", "Space Crate", b"c")
            .unwrap();

        let doom_files = library.files_matching(|item| item.label.to_ascii_lowercase().contains("doom"));
        // Newest first, same order the grid renders.
        assert_eq!(doom_files, vec![zombie, imp]);

        let none = library.files_matching(|item| item.label.contains("nonexistent"));
        assert!(none.is_empty());

        let all = library.files_matching(|_| true);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn import_is_deduplicated_and_delete_preserves_external_source() {
        let dir = TestDir::new("import-delete");
        let source = dir.0.with_extension("source.glb");
        let bytes = b"glTF accepted playable character";
        fs::write(&source, bytes).unwrap();
        let mut library = Library::open(&dir.0);
        let (file, added) = library
            .import_unique("motion", "model/gltf-binary", "elf", "elf", bytes)
            .unwrap();
        assert!(added);
        assert_eq!(library.len(), 1);
        let (same_file, added) = library
            .import_unique("motion", "model/gltf-binary", "elf", "elf", bytes)
            .unwrap();
        assert!(!added);
        assert_eq!(same_file, file);
        assert_eq!(library.len(), 1);

        drop(library);
        let mut reopened = Library::open(&dir.0);
        assert_eq!(reopened.load_bytes(&file).unwrap(), bytes);
        assert!(reopened.remove_by_file(&file).unwrap());
        assert!(!reopened.remove_by_file(&file).unwrap());
        assert!(source.is_file(), "managed delete must not touch import source");
        assert!(Library::open(&dir.0).is_empty());
    }

    #[test]
    fn reimport_replaces_same_group_label_instead_of_duplicating() {
        let dir = TestDir::new("import-replace");
        let mut library = Library::open(&dir.0);
        let (file, added) = library
            .import_unique_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "Freedoom freedoom · world · MAP27 · BSD-3-Clause",
                "MAP27",
                b"glTF first map bytes",
                None,
                Some(("import:freedoom:freedoom", "Freedoom · BSD-3-Clause")),
            )
            .unwrap();
        assert!(added);
        let (same, added) = library
            .import_unique_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "Freedoom freedoom · world · MAP27 · BSD-3-Clause",
                "MAP27",
                b"glTF rebuilt map bytes",
                None,
                Some(("import:freedoom:freedoom", "Freedoom · BSD-3-Clause")),
            )
            .unwrap();
        assert!(added, "new bytes must overwrite, not no-op");
        assert_eq!(same, file);
        assert_eq!(library.len(), 1);
        assert_eq!(
            library.load_bytes(&file).unwrap(),
            b"glTF rebuilt map bytes"
        );

        let (again, _) = library
            .import_unique_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "Freedoom freedoom · world · MAP27 · BSD-3-Clause",
                "freedoom2 MAP27",
                b"glTF third map bytes",
                None,
                Some(("import:freedoom:freedoom", "Freedoom · BSD-3-Clause")),
            )
            .unwrap();
        assert_eq!(again, file, "wad-prefixed title must replace MAP27");
        assert_eq!(library.len(), 1);
    }

    #[test]
    fn ao_sidecars_install_beside_payload_and_reap_on_delete() {
        let dir = TestDir::new("ao-sidecars");
        let src_dir = dir.0.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let src_glb = src_dir.join("crate.glb");
        let bytes = b"glTF ao sidecar host";
        fs::write(&src_glb, bytes).unwrap();
        fs::write(src_glb.with_extension("aomesh"), b"aomesh-bytes").unwrap();
        fs::write(src_glb.with_extension("ao.png"), b"ao-png-bytes").unwrap();
        fs::write(src_glb.with_extension("shadowsdf"), b"sdf-bytes").unwrap();

        let mut library = Library::open(&dir.0);
        let (file, added) = library
            .import_unique("mesh", "model/gltf-binary", "crate", "crate", bytes)
            .unwrap();
        assert!(added);
        library.install_ao_sidecars(&file, &src_glb).unwrap();
        let payload = library.payload_path(&file).unwrap();
        assert_eq!(
            fs::read(payload.with_extension("aomesh")).unwrap(),
            b"aomesh-bytes"
        );
        assert_eq!(
            fs::read(payload.with_extension("ao.png")).unwrap(),
            b"ao-png-bytes"
        );
        assert_eq!(
            fs::read(payload.with_extension("shadowsdf")).unwrap(),
            b"sdf-bytes"
        );

        assert!(library.remove_by_file(&file).unwrap());
        assert!(!payload.exists());
        assert!(!payload.with_extension("aomesh").exists());
        assert!(!payload.with_extension("ao.png").exists());
        assert!(!payload.with_extension("shadowsdf").exists());
        // Source pack sidecars are never touched by library delete.
        assert!(src_glb.with_extension("aomesh").is_file());
    }

    #[test]
    fn install_billboard_frames_dedupes_a_shared_packed_sheet() {
        let dir = TestDir::new("billboard-sheet-dedupe");
        let src_dir = dir.0.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let sheet_bytes = b"packed-sheet-png-bytes";
        fs::write(src_dir.join("trooper_sheet.png"), sheet_bytes).unwrap();
        let manifest_text = "\
stateful-billboard 1
prefix trooper
role character
preview idle
facings 1
sheet 8 64 64
frame 0 A 1 64 64 trooper_sheet.png cell 0
frame 1 B 1 64 64 trooper_sheet.png cell 1
";
        let manifest = src_dir.join("trooper.billboard");
        fs::write(&manifest, manifest_text).unwrap();

        let mut library = Library::open(&dir.0);
        let (file, added) = library
            .import_unique(
                "character",
                makepad_asset_importer::stateful_billboard::CONTENT_TYPE,
                "trooper",
                "Trooper",
                manifest_text.as_bytes(),
            )
            .unwrap();
        assert!(added);
        library.install_billboard_frames(&file, &manifest).unwrap();

        let payload = library.payload_path(&file).unwrap();
        let stem = payload.file_stem().and_then(|s| s.to_str()).unwrap();
        // The shared sheet is copied exactly once, under the FIRST frame
        // index that referenced it...
        let landed_sheet = dir.0.join(format!("{stem}.f000.png"));
        assert!(landed_sheet.is_file());
        assert_eq!(fs::read(&landed_sheet).unwrap(), sheet_bytes);
        // ...never a second time under the second frame's own index.
        assert!(!dir.0.join(format!("{stem}.f001.png")).is_file());

        // Both frames are rewritten to that one landed name; sheet/cell
        // metadata round-trips untouched.
        let landed_text = fs::read_to_string(&payload).unwrap();
        let bb = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&landed_text)
            .unwrap();
        assert_eq!(bb.frames.len(), 2);
        let landed_name = format!("{stem}.f000.png");
        assert_eq!(bb.frames[0].file, landed_name);
        assert_eq!(bb.frames[1].file, landed_name);
        assert_eq!(bb.frames[0].cell, Some(0));
        assert_eq!(bb.frames[1].cell, Some(1));
        assert!(bb.sheet.is_some());
    }

    #[test]
    fn install_billboard_frames_keeps_legacy_one_file_per_frame() {
        let dir = TestDir::new("billboard-legacy-per-frame");
        let src_dir = dir.0.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("trooper_a1.png"), b"frame-a1").unwrap();
        fs::write(src_dir.join("trooper_a2.png"), b"frame-a2").unwrap();
        let manifest_text = "\
stateful-billboard 1
prefix trooper
role character
preview idle
facings 1
frame 0 A 1 64 64 trooper_a1.png
frame 1 A 2 64 64 trooper_a2.png
";
        let manifest = src_dir.join("trooper.billboard");
        fs::write(&manifest, manifest_text).unwrap();

        let mut library = Library::open(&dir.0);
        let (file, _added) = library
            .import_unique(
                "character",
                makepad_asset_importer::stateful_billboard::CONTENT_TYPE,
                "trooper",
                "Trooper",
                manifest_text.as_bytes(),
            )
            .unwrap();
        library.install_billboard_frames(&file, &manifest).unwrap();

        let payload = library.payload_path(&file).unwrap();
        let stem = payload.file_stem().and_then(|s| s.to_str()).unwrap();
        assert!(dir.0.join(format!("{stem}.f000.png")).is_file());
        assert!(dir.0.join(format!("{stem}.f001.png")).is_file());
        let landed_text = fs::read_to_string(&payload).unwrap();
        let bb = makepad_asset_importer::stateful_billboard::StatefulBillboard::parse(&landed_text)
            .unwrap();
        assert_eq!(bb.frames[0].file, format!("{stem}.f000.png"));
        assert_eq!(bb.frames[1].file, format!("{stem}.f001.png"));
    }

    #[test]
    fn unsafe_index_entry_cannot_escape_managed_directory() {
        let dir = TestDir::new("unsafe-index");
        let sentinel = dir.0.with_extension("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let unsafe_index = LibraryIndex {
            items: vec![LibraryMeta {
                file: format!("../{}", sentinel.file_name().unwrap().to_string_lossy()),
                label: "bad".into(),
                domain: "mesh".into(),
                content_type: "model/gltf-binary".into(),
                prompt: String::new(),
                group_id: None,
                group_label: None,
                tags: None,
                enhanced_tags: None,
                product: None,
            }],
            next_id: 1,
        };
        fs::write(
            dir.0.join("index.json"),
            unsafe_index.serialize_json().as_bytes(),
        )
        .unwrap();
        let mut library = Library::open(&dir.0);
        assert!(library.is_empty());
        assert!(library.remove_by_file("../sentinel").is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
    }

    #[test]
    fn cap_retains_all_reachable_items_in_newest_first_order() {
        let dir = TestDir::new("cap");
        let mut library = Library::open(&dir.0);
        for index in 0..(LIBRARY_CAP + 5) {
            let thumbnail = format!("thumbnail-{index}");
            library
                .add_with_thumbnail(
                    "text",
                    "text/plain",
                    &format!("prompt-{index}"),
                    &format!("item-{index}"),
                    format!("payload-{index}").as_bytes(),
                    Some(thumbnail.as_bytes()),
                    None,
                    None,
                )
                .unwrap();
        }
        assert_eq!(library.len(), LIBRARY_CAP);
        let labels: Vec<_> = library
            .newest_items()
            .map(|item| item.label.as_str())
            .collect();
        let newest = format!("item-{}", LIBRARY_CAP + 4);
        assert_eq!(labels.first().copied(), Some(newest.as_str()));
        assert_eq!(labels.last(), Some(&"item-5"));
        assert!(library.index.next_id >= (LIBRARY_CAP + 5) as u64);
        assert!(!dir.0.join("lib-1.txt.thumb").exists());
        assert!(!dir.0.join("lib-5.txt.thumb").exists());
        assert!(dir.0.join("lib-6.txt.thumb").is_file());
        assert!(dir
            .0
            .join(format!("lib-{}.txt.thumb", LIBRARY_CAP + 5))
            .is_file());
    }

    #[test]
    fn thumbnail_persists_upgrades_on_dedupe_and_deletes_with_payload() {
        let dir = TestDir::new("thumbnail-lifecycle");
        let bytes = b"glTF accepted playable character";
        let mut library = Library::open(&dir.0);
        let (file, added) = library
            .import_unique_with_thumbnail(
                "motion",
                "model/gltf-binary",
                "elf",
                "elf",
                bytes,
                Some(b"first-preview"),
                None,
            )
            .unwrap();
        assert!(added);
        let thumbnail_path = library.thumbnail_path(&file).unwrap().unwrap();
        assert_eq!(fs::read(&thumbnail_path).unwrap(), b"first-preview");

        drop(library);
        let mut reopened = Library::open(&dir.0);
        assert_eq!(
            fs::read(reopened.thumbnail_path(&file).unwrap().unwrap()).unwrap(),
            b"first-preview"
        );
        let (same_file, added) = reopened
            .import_unique_with_thumbnail(
                "motion",
                "model/gltf-binary",
                "elf",
                "elf",
                bytes,
                Some(b"better-preview"),
                None,
            )
            .unwrap();
        assert_eq!(same_file, file);
        assert!(!added);
        assert_eq!(fs::read(&thumbnail_path).unwrap(), b"better-preview");

        assert!(reopened.remove_by_file(&file).unwrap());
        assert!(!thumbnail_path.exists());
        assert!(!reopened.payload_path(&file).unwrap().exists());
    }

    #[test]
    fn preview_versions_invalidate_only_the_stale_kind_and_queue_regeneration() {
        let dir = TestDir::new("preview-versions");
        let mut library = Library::open(&dir.0);
        let glb = library
            .add_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "p",
                "statue",
                b"glTF static statue",
                Some(b"model-render"),
                None,
                None,
            )
            .unwrap();
        let wav = library
            .add("sfx", "audio/wav", "p", "clash", b"RIFF....WAVE....")
            .unwrap();
        // Simulate the legacy provenance bug directly on disk: a sidecar
        // that is a byte-copy of an upstream image. (add_with_thumbnail now
        // refuses to write one — see the provenance test.)
        fs::write(dir.0.join(format!("{wav}.thumb")), b"upstream-image-bytes").unwrap();
        drop(library);

        // Current versions (written by the first open): reopening keeps
        // every sidecar untouched.
        let library = Library::open(&dir.0);
        assert!(library.thumbnail_path(&glb).unwrap().is_some());
        assert!(library.thumbnail_path(&wav).unwrap().is_some());
        drop(library);

        // Stale AUDIO semantics (the upstream-image provenance bug): only
        // audio sidecars are discarded; the model render survives. The
        // discarded WAV immediately enters the regeneration queue.
        fs::write(
            dir.0.join(PREVIEW_VERSIONS_FILE),
            format!("model:{MODEL_PREVIEW_VERSION}\naudio:0-upstream-image\n"),
        )
        .unwrap();
        let library = Library::open(&dir.0);
        assert!(library.thumbnail_path(&glb).unwrap().is_some());
        assert!(library.thumbnail_path(&wav).unwrap().is_none());
        assert_eq!(
            library.thumbnail_backfill_queue(),
            vec![ThumbnailBackfillJob::AudioWaveform { file: wav.clone() }]
        );
        drop(library);

        // Stale MODEL semantics (e.g. clear-color-only captures): the
        // model sidecar is discarded for re-render, audio untouched.
        let mut wave_png = b"\x89PNG\r\n\x1a\n".to_vec();
        wave_png.extend_from_slice(b"own-waveform");
        let library = Library::open(&dir.0);
        library.replace_thumbnail_png(&wav, &wave_png).unwrap();
        drop(library);
        fs::write(
            dir.0.join(PREVIEW_VERSIONS_FILE),
            format!("model:2-static-skin-classifier\naudio:{AUDIO_PREVIEW_VERSION}\n"),
        )
        .unwrap();
        let library = Library::open(&dir.0);
        assert!(library.thumbnail_path(&glb).unwrap().is_none());
        assert!(library.thumbnail_path(&wav).unwrap().is_some());
        assert_eq!(
            library.thumbnail_backfill_queue(),
            vec![ThumbnailBackfillJob::ModelRender { file: glb.clone() }]
        );
        assert!(library.needs_model_thumbnail(&glb));
        drop(library);

        // A legacy single-kind marker means pre-split semantics: both kinds
        // revalidate (model deleted here) and the legacy file is reaped.
        let mut model_png = b"\x89PNG\r\n\x1a\n".to_vec();
        model_png.extend_from_slice(b"good-render");
        let library = Library::open(&dir.0);
        library.replace_thumbnail_png(&glb, &model_png).unwrap();
        drop(library);
        fs::remove_file(dir.0.join(PREVIEW_VERSIONS_FILE)).unwrap();
        fs::write(dir.0.join(LEGACY_MODEL_VERSION_FILE), b"2-static-skin-classifier").unwrap();
        let library = Library::open(&dir.0);
        assert!(library.thumbnail_path(&glb).unwrap().is_none());
        assert!(!dir.0.join(LEGACY_MODEL_VERSION_FILE).exists());
        assert_eq!(
            fs::read_to_string(dir.0.join(PREVIEW_VERSIONS_FILE)).unwrap(),
            format!("model:{MODEL_PREVIEW_VERSION}\naudio:{AUDIO_PREVIEW_VERSION}\n")
        );

        // Healed versions: a fresh render persists across the next open.
        library.replace_thumbnail_png(&glb, &model_png).unwrap();
        drop(library);
        let library = Library::open(&dir.0);
        assert_eq!(
            fs::read(library.thumbnail_path(&glb).unwrap().unwrap()).unwrap(),
            model_png
        );
    }

    #[test]
    fn new_group_ids_are_unique_and_prefixed() {
        let one = new_group_id("run");
        let two = new_group_id("run");
        let import = new_group_id("import");
        assert!(one.starts_with("run-"));
        assert!(import.starts_with("import-"));
        assert_ne!(one, two, "same-millisecond ids must still differ");
    }

    #[test]
    fn is_import_pack_group_only_for_colon_prefixed_pack_ids() {
        // The `land_imported_pack` pack-landing convention: colon-separated,
        // can hold hundreds/thousands of members.
        assert!(is_import_pack_group("import:kenney:space-kit"));
        assert!(is_import_pack_group("import:doom:doom"));
        assert!(is_import_pack_group("import:duke3d:duke3d"));
        // Generated pipeline runs and other single-artifact groupings must
        // never be mistaken for a pack import.
        assert!(!is_import_pack_group("run-1755000000-123-4"));
        assert!(!is_import_pack_group("webcam-1755000000-123-4"));
        assert!(!is_import_pack_group("drop-1755000000-123-4"));
        // The one-off standalone-import id (`new_group_id("import")`) uses a
        // dash, not the pack's colon convention, and is deliberately NOT
        // treated as a pack group.
        assert!(!is_import_pack_group("import-1755000000-123-4"));
    }

    #[test]
    fn groups_persist_and_pre_group_indexes_read_as_earlier_imports() {
        let dir = TestDir::new("group-roundtrip");
        let mut library = Library::open(&dir.0);
        let grouped = library
            .add_with_thumbnail(
                "image",
                "image/png",
                "storm",
                "img storm",
                b"\x89PNG-a",
                None,
                Some(("run-1-2-3", "video (small) — \"storm\"")),
                None,
            )
            .unwrap();
        drop(library);

        let reopened = Library::open(&dir.0);
        let item = reopened.get(&grouped).unwrap();
        assert_eq!(item.group_id.as_deref(), Some("run-1-2-3"));
        assert_eq!(
            item.group_label.as_deref(),
            Some("video (small) — \"storm\"")
        );
        drop(reopened);

        // An index written before grouping existed (no group keys at all)
        // must parse — and those records land in the ungrouped bucket.
        let legacy = r#"{"items":[{"file":"lib-9.txt","label":"old","domain":"text","content_type":"text/plain","prompt":"p"}],"next_id":9}"#;
        fs::write(dir.0.join("index.json"), legacy).unwrap();
        fs::write(dir.0.join("lib-9.txt"), b"old payload").unwrap();
        let legacy_library = Library::open(&dir.0);
        let item = legacy_library.get("lib-9.txt").expect("legacy index parses");
        assert_eq!(item.group_id, None);
        assert_eq!(item.group_label, None);
        assert_eq!(item.tags.as_deref(), Some(&[][..]));
        assert_eq!(item.enhanced_tags, None);
    }

    #[test]
    fn import_group_stamps_source_and_pack_tags() {
        let dir = TestDir::new("import-tags");
        let mut library = Library::open(&dir.0);
        let file = library
            .add_with_thumbnail(
                "prop",
                "model/gltf-binary",
                "The Dark Mod darkmod · chair · CC-BY-NC-SA-3.0",
                "chair",
                b"glTF chair",
                None,
                Some(("import:darkmod:darkmod", "The Dark Mod · CC-BY-NC-SA-3.0")),
                None,
            )
            .unwrap();
        let kenney = library
            .add_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "Kenney space-kit · crate · CC-BY-4.0",
                "crate",
                b"glTF crate",
                None,
                Some(("import:kenney:space-kit", "Kenney space-kit · CC-BY-4.0")),
                None,
            )
            .unwrap();
        assert_eq!(library.get(&file).unwrap().import_tags(), ["darkmod"]);
        assert_eq!(
            library.get(&kenney).unwrap().import_tags(),
            ["kenney", "space-kit"]
        );
    }

    #[test]
    fn open_backfills_tags_from_legacy_group_id() {
        let dir = TestDir::new("tag-backfill");
        let legacy = r#"{"items":[{"file":"lib-3.glb","label":"MAP01","domain":"map","content_type":"model/gltf-binary","prompt":"Freedoom freedoom · world · MAP01","group_id":"import:freedoom:freedoom","group_label":"Freedoom · BSD-3-Clause"}],"next_id":3}"#;
        fs::write(dir.0.join("index.json"), legacy).unwrap();
        fs::write(dir.0.join("lib-3.glb"), b"glTF map").unwrap();
        let library = Library::open(&dir.0);
        let item = library.get("lib-3.glb").unwrap();
        assert_eq!(item.import_tags(), ["freedoom"]);
        let saved = fs::read_to_string(dir.0.join("index.json")).unwrap();
        assert!(saved.contains("\"tags\""), "backfill must persist");
    }

    #[test]
    fn open_backfills_products_over_complete_groups_and_persists_them() {
        let dir = TestDir::new("product-backfill");
        // One legacy `image → mesh → PBR` run, written before the field
        // existed, plus one authored row the backfill must not touch.
        let legacy = r#"{"items":[
            {"file":"lib-1.png","label":"src","domain":"image","content_type":"image/png","prompt":"p","group_id":"run-1","tags":["generated"]},
            {"file":"lib-2.glb","label":"mesh","domain":"mesh","content_type":"model/gltf-binary","prompt":"p","group_id":"run-1","tags":["generated"]},
            {"file":"lib-3.glb","label":"painted","domain":"paint","content_type":"model/gltf-binary","prompt":"p","group_id":"run-1","tags":["generated"]},
            {"file":"lib-4.png","label":"albedo","domain":"paint","content_type":"image/png","prompt":"p","group_id":"run-1","tags":["generated"]},
            {"file":"lib-5.png","label":"kept","domain":"image","content_type":"image/png","prompt":"p","group_id":"run-2","tags":["generated"],"product":false}
        ],"next_id":6}"#;
        fs::write(dir.0.join("index.json"), legacy).unwrap();
        for file in ["lib-1.png", "lib-2.glb", "lib-3.glb", "lib-4.png", "lib-5.png"] {
            fs::write(dir.0.join(file), b"payload").unwrap();
        }
        let library = Library::open(&dir.0);
        assert_eq!(library.get("lib-1.png").unwrap().product, Some(false));
        assert_eq!(library.get("lib-2.glb").unwrap().product, Some(false));
        assert_eq!(library.get("lib-3.glb").unwrap().product, Some(true));
        assert_eq!(library.get("lib-4.png").unwrap().product, Some(false));
        assert_eq!(
            library.get("lib-5.png").unwrap().product,
            Some(false),
            "an authored flag is never re-inferred"
        );
        // The publish loop reads index.json from disk, so the backfill has
        // to be durable before the watcher's first poll.
        let saved = fs::read_to_string(dir.0.join("index.json")).unwrap();
        assert!(saved.contains("\"product\""), "product backfill must persist");
        let reopened = Library::open(&dir.0);
        assert_eq!(reopened.get("lib-3.glb").unwrap().product, Some(true));
    }

    #[test]
    fn routed_product_flag_round_trips_through_the_index() {
        let dir = TestDir::new("product-roundtrip");
        let mut library = Library::open(&dir.0);
        let product = library
            .add_with_thumbnail(
                "paint",
                "model/gltf-binary",
                "p",
                "elf",
                b"glTF elf",
                None,
                Some(("run-9", "character")),
                Some(true),
            )
            .unwrap();
        let map = library
            .add_with_thumbnail(
                "paint",
                "image/png",
                "p",
                "albedo",
                b"png albedo",
                None,
                Some(("run-9", "character")),
                Some(false),
            )
            .unwrap();
        drop(library);
        let reopened = Library::open(&dir.0);
        assert_eq!(reopened.get(&product).unwrap().product, Some(true));
        assert_eq!(reopened.get(&map).unwrap().product, Some(false));
    }

    #[test]
    fn tag_stats_sort_by_count_then_name_and_mark_enhanced() {
        let items = [
            LibraryMeta {
                file: "a.glb".into(),
                label: "a".into(),
                domain: "prop".into(),
                content_type: "model/gltf-binary".into(),
                prompt: String::new(),
                group_id: None,
                group_label: None,
                tags: Some(vec!["freedoom".into(), "darkmod".into()]),
                enhanced_tags: Some(vec!["wooden".into()]),
                product: None,
            },
            LibraryMeta {
                file: "b.glb".into(),
                label: "b".into(),
                domain: "prop".into(),
                content_type: "model/gltf-binary".into(),
                prompt: String::new(),
                group_id: None,
                group_label: None,
                tags: Some(vec!["freedoom".into()]),
                enhanced_tags: None,
                product: None,
            },
            LibraryMeta {
                file: "c.glb".into(),
                label: "c".into(),
                domain: "prop".into(),
                content_type: "model/gltf-binary".into(),
                prompt: String::new(),
                group_id: None,
                group_label: None,
                tags: Some(vec!["freedoom".into()]),
                enhanced_tags: Some(vec!["wooden".into()]),
                product: None,
            },
        ];
        let stats = collect_tag_stats(items.iter());
        let names: Vec<_> = stats.iter().map(|s| (s.name.as_str(), s.count, s.enhanced)).collect();
        assert_eq!(
            names,
            vec![
                ("freedoom", 3, false),
                ("props", 3, false),
                ("wooden", 2, true),
                ("darkmod", 1, false),
            ]
        );
    }

    #[test]
    fn remove_group_removes_only_that_group_atomically() {
        let dir = TestDir::new("group-remove");
        let mut library = Library::open(&dir.0);
        let a1 = library
            .add_with_thumbnail(
                "image", "image/png", "p", "a1", b"payload-a1",
                Some(b"thumb-a1"), Some(("run-a", "run A")),
                None,
            )
            .unwrap();
        let a2 = library
            .add_with_thumbnail(
                "sfx", "audio/wav", "p", "a2", b"payload-a2",
                Some(b"thumb-a2"), Some(("run-a", "run A")),
                None,
            )
            .unwrap();
        let b1 = library
            .add_with_thumbnail(
                "image", "image/png", "p", "b1", b"payload-b1",
                Some(b"thumb-b1"), Some(("run-b", "run B")),
                None,
            )
            .unwrap();
        let ungrouped = library.add("text", "text/plain", "p", "old", b"old").unwrap();

        assert_eq!(library.remove_group(Some("run-a")).unwrap(), 2);
        assert!(library.get(&a1).is_none());
        assert!(library.get(&a2).is_none());
        assert!(!library.payload_path(&a1).unwrap().exists());
        assert!(!library.payload_path(&a2).unwrap().exists());
        assert!(library.thumbnail_path(&a1).unwrap().is_none());
        // Isolation: the other run and the ungrouped record are untouched.
        assert!(library.get(&b1).is_some());
        assert!(library.payload_path(&b1).unwrap().is_file());
        assert!(library.thumbnail_path(&b1).unwrap().is_some());
        assert!(library.get(&ungrouped).is_some());
        // Removing an absent group is a no-op.
        assert_eq!(library.remove_group(Some("run-a")).unwrap(), 0);
        // The Earlier-imports pseudo-group removes exactly the ungrouped.
        assert_eq!(library.remove_group(None).unwrap(), 1);
        assert!(library.get(&ungrouped).is_none());
        assert!(library.get(&b1).is_some());

        drop(library);
        let reopened = Library::open(&dir.0);
        assert_eq!(reopened.len(), 1);
        assert!(reopened.get(&b1).is_some());
    }

    #[test]
    fn remove_group_rolls_back_completely_when_the_index_save_fails() {
        let dir = TestDir::new("group-rollback");
        let mut library = Library::open(&dir.0);
        let a1 = library
            .add_with_thumbnail(
                "image", "image/png", "p", "a1", b"payload-a1",
                Some(b"thumb-a1"), Some(("run-a", "run A")),
                None,
            )
            .unwrap();
        let b1 = library
            .add_with_thumbnail(
                "image", "image/png", "p", "b1", b"payload-b1",
                None, Some(("run-b", "run B")),
                None,
            )
            .unwrap();

        // Make the atomic index rename fail: the save's rename target is now
        // a directory. Tombstone renames succeed first, so this exercises
        // the full restore path.
        fs::remove_file(dir.0.join("index.json")).unwrap();
        fs::create_dir(dir.0.join("index.json")).unwrap();
        assert!(library.remove_group(Some("run-a")).is_err());

        // In-memory index and every renamed file are restored.
        assert!(library.get(&a1).is_some());
        assert!(library.payload_path(&a1).unwrap().is_file());
        assert_eq!(
            fs::read(library.thumbnail_path(&a1).unwrap().unwrap()).unwrap(),
            b"thumb-a1"
        );
        assert!(library.get(&b1).is_some());
        assert_eq!(library.len(), 2);

        // With the obstruction cleared the same removal commits cleanly.
        fs::remove_dir(dir.0.join("index.json")).unwrap();
        assert_eq!(library.remove_group(Some("run-a")).unwrap(), 1);
        drop(library);
        let reopened = Library::open(&dir.0);
        assert_eq!(reopened.len(), 1);
        assert!(reopened.get(&b1).is_some());
    }

    #[test]
    fn backfill_queue_lists_only_sidecarless_model_and_audio_newest_first() {
        let dir = TestDir::new("backfill");
        let mut library = Library::open(&dir.0);
        let image = library
            .add("image", "image/png", "p", "img", b"\x89PNG-payload")
            .unwrap();
        let glb_done = library
            .add_with_thumbnail(
                "mesh",
                "model/gltf-binary",
                "p",
                "done",
                b"glTF with preview",
                Some(b"render"),
                None,
                None,
            )
            .unwrap();
        // Unparseable GLB bytes: no embedded-image fallback, so no sidecar.
        let glb_missing = library
            .add("mesh", "model/gltf-binary", "p", "todo", b"glTF broken")
            .unwrap();
        let wav_missing = library
            .add("sfx", "audio/wav", "p", "sfx", b"RIFF....WAVE....")
            .unwrap();
        let text = library.add("text", "text/plain", "p", "note", b"hi").unwrap();

        // Newest-first, image/text/preview-complete entries excluded.
        assert_eq!(
            library.thumbnail_backfill_queue(),
            vec![
                ThumbnailBackfillJob::AudioWaveform {
                    file: wav_missing.clone()
                },
                ThumbnailBackfillJob::ModelRender {
                    file: glb_missing.clone()
                },
            ]
        );

        // The click gate agrees with the queue: only the sidecar-less GLB
        // may queue a render, and a landed render retires both.
        assert!(library.needs_model_thumbnail(&glb_missing));
        assert!(!library.needs_model_thumbnail(&glb_done));
        assert!(!library.needs_model_thumbnail(&wav_missing));
        assert!(!library.needs_model_thumbnail(&image));
        assert!(!library.needs_model_thumbnail(&text));
        assert!(!library.needs_model_thumbnail("lib-999.glb"));
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(b"render");
        library.replace_thumbnail_png(&glb_missing, &png).unwrap();
        assert!(!library.needs_model_thumbnail(&glb_missing));
        assert_eq!(
            library.thumbnail_backfill_queue(),
            vec![ThumbnailBackfillJob::AudioWaveform {
                file: wav_missing.clone()
            }]
        );
    }

    #[test]
    fn audio_thumbnail_provenance_is_enforced_at_the_persistence_boundary() {
        let dir = TestDir::new("audio-provenance");
        let mut library = Library::open(&dir.0);
        let samples: Vec<f32> = (0..64).map(|i| (i as f32 / 8.0).sin() * 0.5).collect();
        let wav_bytes = makepad_asset_ai::wav::encode_wav_pcm16_mono(&samples, 24_000);

        // A poisoned caller thumbnail (e.g. the upstream pipeline image) is
        // DISCARDED; the sidecar is the payload's own waveform strip.
        let file = library
            .add_with_thumbnail(
                "sfx",
                "audio/wav",
                "p",
                "clash",
                &wav_bytes,
                Some(b"poisoned-upstream-image"),
                None,
                None,
            )
            .unwrap();
        let sidecar = library.thumbnail_path(&file).unwrap().expect("own waveform");
        let png = fs::read(&sidecar).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_ne!(png, b"poisoned-upstream-image".to_vec());
        assert_eq!(
            u32::from_be_bytes(png[16..20].try_into().unwrap()),
            crate::audio::WAVEFORM_THUMB_W as u32,
            "sidecar is the canonical waveform strip"
        );

        // The dedupe "upgrade" path must refuse a poisoned thumbnail too.
        let (same, added) = library
            .import_unique_with_thumbnail(
                "sfx",
                "audio/wav",
                "p",
                "clash",
                &wav_bytes,
                Some(b"still-poisoned"),
                None,
            )
            .unwrap();
        assert_eq!(same, file);
        assert!(!added);
        assert_eq!(fs::read(&sidecar).unwrap(), png, "waveform must survive dedupe");

        // Unparseable audio: no sidecar at all (never the poisoned bytes),
        // and the backfill queue picks the payload up for regeneration.
        let broken = library
            .add_with_thumbnail(
                "sfx",
                "audio/wav",
                "p",
                "hiss",
                b"RIFF....WAVE....",
                Some(b"poison"),
                None,
                None,
            )
            .unwrap();
        assert!(library.thumbnail_path(&broken).unwrap().is_none());
        assert!(library
            .thumbnail_backfill_queue()
            .contains(&ThumbnailBackfillJob::AudioWaveform { file: broken.clone() }));
    }

    #[test]
    fn interrupted_deletion_recovers_from_the_durable_index() {
        let dir = TestDir::new("tombstone-recovery");
        let mut library = Library::open(&dir.0);
        let keep = library
            .add_with_thumbnail(
                "image", "image/png", "p", "keep", b"\x89PNG-keep",
                Some(b"thumb-keep"), Some(("run-a", "run A")),
                None,
            )
            .unwrap();
        let victim = library
            .add_with_thumbnail(
                "image", "image/png", "p", "victim", b"\x89PNG-victim",
                Some(b"thumb-victim"), Some(("run-a", "run A")),
                None,
            )
            .unwrap();
        drop(library);

        // CRASH WINDOW 1: tombstone renames happened, the index commit did
        // NOT. The durable index still references the victim → roll back.
        let t_payload = dir.0.join(format!(".delete-999-1-payload-{victim}"));
        let t_thumb = dir.0.join(format!(".delete-999-1-thumb-{victim}"));
        fs::rename(dir.0.join(&victim), &t_payload).unwrap();
        fs::rename(dir.0.join(format!("{victim}.thumb")), &t_thumb).unwrap();
        let library = Library::open(&dir.0);
        assert!(library.get(&victim).is_some(), "uncommitted delete rolls back");
        assert_eq!(fs::read(dir.0.join(&victim)).unwrap(), b"\x89PNG-victim");
        assert_eq!(
            fs::read(library.thumbnail_path(&victim).unwrap().unwrap()).unwrap(),
            b"thumb-victim"
        );
        assert!(!t_payload.exists() && !t_thumb.exists(), "tombstones consumed");
        assert!(library.get(&keep).is_some());
        drop(library);

        // A stale DUPLICATE tombstone beside a live payload is reaped, not
        // restored over it.
        fs::write(&t_payload, b"stale-duplicate").unwrap();
        let library = Library::open(&dir.0);
        assert!(!t_payload.exists());
        assert_eq!(fs::read(dir.0.join(&victim)).unwrap(), b"\x89PNG-victim");
        drop(library);

        // CRASH WINDOW 2: the index commit happened (items gone) but the
        // tombstone unlinks never ran → roll forward (reap, stay deleted).
        let mut library = Library::open(&dir.0);
        assert_eq!(library.remove_group(Some("run-a")).unwrap(), 2);
        drop(library);
        fs::write(&t_payload, b"leftover-tombstone").unwrap();
        let library = Library::open(&dir.0);
        assert!(library.get(&victim).is_none(), "committed delete stays deleted");
        assert!(library.get(&keep).is_none());
        assert!(!t_payload.exists(), "unreferenced tombstone reaped");
        assert!(!dir.0.join(&victim).exists());
    }

    #[test]
    fn rendered_thumbnail_commit_is_identity_checked_and_atomic() {
        let dir = TestDir::new("rendered-thumbnail");
        let mut library = Library::open(&dir.0);
        let file = library
            .add(
                "mesh",
                "model/gltf-binary",
                "elf",
                "elf",
                b"glTF thumbnail target",
            )
            .unwrap();
        let mut first = b"\x89PNG\r\n\x1a\n".to_vec();
        first.extend_from_slice(b"first-render");
        library.replace_thumbnail_png(&file, &first).unwrap();
        let thumbnail = library.thumbnail_path(&file).unwrap().unwrap();
        assert_eq!(fs::read(&thumbnail).unwrap(), first);

        assert!(library
            .replace_thumbnail_png(&file, b"not a png")
            .is_err());
        assert_eq!(fs::read(&thumbnail).unwrap(), first);

        assert!(library.remove_by_file(&file).unwrap());
        let mut late = b"\x89PNG\r\n\x1a\n".to_vec();
        late.extend_from_slice(b"late-render");
        let error = library.replace_thumbnail_png(&file, &late).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!thumbnail.exists(), "late readback recreated a deleted sidecar");
    }
}
