//! Library + Asset Store presentation: the gallery widgets and the flattened
//! row models behind the Library / Runs+Workers / Admin surfaces.
//!
//! Data honesty is the design rule here. Local rows (History payloads, the
//! app's own pipeline, LAN fleet boxes) render real state; everything
//! server-side renders from [`AssetStore`], which is fed only by the shared
//! `asset_client` session/runtime. While no verified session is connected the
//! builders emit explicit disconnected/loading/error rows — never sample
//! catalogs, never local data dressed up as a server.

use crate::asset_store_state::{
    hex16_string, server_kind_label, AssetStore, Remote,
};
use crate::library::LibraryMeta;
use crate::pipeline::{
    format_clock, format_music_duration, stage_display_name, CandidateSet, Pipeline, StageState,
};
use makepad_asset_ai::fleet::BoxSnapshot;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Shared gallery entries + thumbnail textures
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct GalleryEntry {
    pub meta: LibraryMeta,
    pub path: PathBuf,
    /// Image payload itself, persisted upstream render/matte sidecar, or None
    /// when the card intentionally uses a generated domain badge.
    pub preview_path: Option<PathBuf>,
    pub selected: bool,
}

impl GalleryEntry {
    pub fn is_sprite_preview(&self) -> bool {
        self.meta.domain.eq_ignore_ascii_case("billboard")
            || self
                .meta
                .content_type
                .to_ascii_lowercase()
                .contains("billboard")
            || self
                .path
                .extension()
                .and_then(|e| e.to_str())
                == Some("billboard")
    }

    pub fn is_file_drag_media(&self) -> bool {
        // Every library card is a real payload — the grab icon on the card
        // drags that file, never a preview sidecar.
        true
    }

    /// The managed payload that may leave the app as an operating-system
    /// file drag. Preview sidecars are deliberately never exposed here: an
    /// audio card's `.thumb` is a waveform PNG and a video's preview is not
    /// the playable attachment.
    pub fn file_drag_payload_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }
}

/// A short intentional movement starts the native file drag; a click on the
/// handle must remain inert, and an active OS session must never be started a
/// second time by another move event.
pub const FILE_DRAG_MIN_DISTANCE: f64 = 10.0;

pub fn should_start_file_drag(move_distance: f64, already_dragging: bool) -> bool {
    !already_dragging && move_distance >= FILE_DRAG_MIN_DISTANCE
}

pub struct CachedGalleryTexture {
    pub source: Option<PathBuf>,
    pub texture: Texture,
    pub frames: Vec<Texture>,
    pub fps: f32,
}

/// Which artifact kinds may inherit the nearest upstream pipeline image as
/// their persisted preview. Images are their own preview, and AUDIO is
/// contract-bound to its own waveform — assigning a neighbouring image to a
/// WAV is exactly the provenance bug this gate exists to prevent.
pub fn upstream_preview_allowed(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    !ct.starts_with("image/") && !ct.starts_with("audio/")
}

/// The background work one card without a cached texture needs. Draw code
/// never performs this work itself — it records the miss, shows the typed
/// badge, and the app routes the read+decode through the IO worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewWork {
    /// Read + decode an encoded image: the persisted sidecar, or an image
    /// payload that is its own preview.
    Encoded(PathBuf),
    /// Legacy sidecarless WAV: read + parse + min/max scan the payload.
    WavPayload(PathBuf),
    /// `.billboard` manifest: decode the preview state's native-size frames.
    StatefulBillboard(PathBuf),
}

/// Classify what (if any) background work produces this entry's preview.
/// `None` = the typed badge IS the preview (text and friends).
pub fn preview_work(entry: &GalleryEntry) -> Option<PreviewWork> {
    let ct = entry.meta.content_type.to_ascii_lowercase();
    let ext = entry
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    // Only real manifests go through the frame decoder. Duke tile PNGs are
    // domain=billboard but the payload is the icon itself.
    let manifest = ext.eq_ignore_ascii_case("billboard")
        || ct.contains("stateful-billboard")
        || ct == "text/x-stateful-billboard";
    if manifest {
        return Some(PreviewWork::StatefulBillboard(entry.path.clone()));
    }
    if let Some(path) = &entry.preview_path {
        return Some(PreviewWork::Encoded(path.clone()));
    }
    if entry.meta.domain.eq_ignore_ascii_case("billboard")
        && (ext.eq_ignore_ascii_case("png") || ct.starts_with("image/"))
    {
        return Some(PreviewWork::Encoded(entry.path.clone()));
    }
    if entry
        .meta
        .content_type
        .to_ascii_lowercase()
        .starts_with("audio/")
    {
        return Some(PreviewWork::WavPayload(entry.path.clone()));
    }
    None
}

/// Per-widget preview store: completed async textures keyed by stable file
/// id (validated against the entry's current source path), one shared badge
/// texture per domain, and the misses recorded during the last draws.
/// NOTHING in here reads files or decodes images — that is the point.
#[derive(Default)]
pub struct PreviewCache {
    map: HashMap<String, CachedGalleryTexture>,
    badges: HashMap<String, Texture>,
    pending: Vec<(String, PreviewWork)>,
}

impl PreviewCache {
    /// Content changed: drop decoded textures (badges stay — they are pure
    /// domain color and never stale).
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The texture to draw NOW. On a cache miss with pending work the badge
    /// shows and the miss is recorded (deduplicated) for the app's async
    /// pump; badge-only kinds cache their badge permanently.
    pub fn texture_for(&mut self, cx: &mut Cx, entry: &GalleryEntry, time: f64) -> Texture {
        if let Some(cached) = self.map.get(&entry.meta.file) {
            if cached.source == entry.preview_path {
                if cached.frames.len() > 1 {
                    let fps = cached.fps.max(1.0) as f64;
                    let i = ((time * fps).floor() as usize) % cached.frames.len();
                    return cached.frames[i].clone();
                }
                return cached.texture.clone();
            }
        }
        match preview_work(entry) {
            Some(work) => {
                // Last drawn miss is last in `pending` — the pump/stack
                // treats that as last-requested-first.
                self.pending.retain(|(file, _)| file != &entry.meta.file);
                self.pending.push((entry.meta.file.clone(), work));
                self.badge(cx, &entry.meta.domain)
            }
            None => {
                let texture = self.badge(cx, &entry.meta.domain);
                self.map.insert(
                    entry.meta.file.clone(),
                    CachedGalleryTexture {
                        source: None,
                        texture: texture.clone(),
                        frames: Vec::new(),
                        fps: 0.0,
                    },
                );
                texture
            }
        }
    }

    /// The decoded still for `file` if this cache holds one (animated
    /// previews return their first frame; badges count as decoded).
    pub fn cached(&self, file: &str) -> Option<Texture> {
        self.map.get(file).map(|cached| cached.texture.clone())
    }

    /// Store a completed (or failure-pinned) texture under its validation
    /// source.
    pub fn install(&mut self, file: String, source: Option<PathBuf>, texture: Texture) {
        self.map.insert(
            file,
            CachedGalleryTexture {
                source,
                texture,
                frames: Vec::new(),
                fps: 0.0,
            },
        );
    }

    pub fn install_anim(
        &mut self,
        file: String,
        source: Option<PathBuf>,
        frames: Vec<Texture>,
        fps: f32,
    ) {
        let Some(texture) = frames.first().cloned() else {
            return;
        };
        self.map.insert(
            file,
            CachedGalleryTexture {
                source,
                texture,
                frames,
                fps,
            },
        );
    }

    pub fn has_anims(&self) -> bool {
        self.map.values().any(|c| c.frames.len() > 1)
    }

    pub fn take_pending(&mut self) -> Vec<(String, PreviewWork)> {
        std::mem::take(&mut self.pending)
    }

    fn badge(&mut self, cx: &mut Cx, domain: &str) -> Texture {
        if let Some(texture) = self.badges.get(domain) {
            return texture.clone();
        }
        let texture = badge_texture(cx, domain);
        self.badges.insert(domain.to_string(), texture.clone());
        texture
    }
}

/// Flat color tile for gallery entries without a natural thumbnail.
pub fn badge_texture(cx: &mut Cx, domain: &str) -> Texture {
    let color: u32 = match domain {
        "video" => 0xff6a4a8c,
        "mesh" | "rig" | "motion" => 0xffb06a3a,
        "world" => 0xff3a6ab0,
        "text" => 0xff3d4750,
        _ => 0xff2a3238,
    };
    Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width: 8,
            height: 8,
            data: Some(vec![color; 64]),
            updated: TextureUpdated::Full,
        },
    )
}

// ---------------------------------------------------------------------------
// Selected-input tray (seeded transform runs)
// ---------------------------------------------------------------------------

/// One managed asset pinned as the next run's input. Everything here is
/// display + provenance metadata; the exact payload BYTES are read from
/// `path` only when a run is enqueued (the thumbnail is display-only and
/// never leaves the app as an input).
#[derive(Clone, Debug, PartialEq)]
pub struct InputAsset {
    /// Stable managed file id (the future `AssetRevision` id slot).
    pub file: String,
    pub label: String,
    pub domain: String,
    /// Exact stored content type — the seeded request sends this verbatim.
    pub content_type: String,
    /// Managed payload path (never a preview sidecar).
    pub path: PathBuf,
    /// Display thumbnail source, if one exists.
    pub preview_path: Option<PathBuf>,
}

/// The Create surface's persistent input tray. Mutated ONLY by an explicit
/// user pick (`select`), the tray's own × (`clear`), or the deletion sweep
/// (`retain_existing`) — never by async viewer commits or artifacts landing
/// from finished runs, so a queued transform can never silently switch to
/// "whatever showed up last".
#[derive(Clone, Debug, PartialEq, Default)]
pub struct InputTray {
    current: Option<InputAsset>,
    /// Extra references for multi-reference editors (⇧ double-click adds
    /// one); ordered, deduped, capped at [`Self::MAX_EXTRAS`].
    extras: Vec<InputAsset>,
}

impl InputTray {
    pub const MAX_EXTRAS: usize = 3;

    pub fn current(&self) -> Option<&InputAsset> {
        self.current.as_ref()
    }

    pub fn extras(&self) -> &[InputAsset] {
        &self.extras
    }

    /// Explicit user pick; returns whether the tray changed. A new primary
    /// that was pinned as an extra reference leaves the extras list.
    pub fn select(&mut self, asset: InputAsset) -> bool {
        let removed = self.extras.iter().position(|a| a.file == asset.file);
        if let Some(index) = removed {
            self.extras.remove(index);
        }
        if self.current.as_ref() == Some(&asset) {
            return removed.is_some();
        }
        self.current = Some(asset);
        true
    }

    /// Add an extra reference. Without a primary input the asset becomes
    /// the primary instead (a reference list needs something to refer to).
    /// Returns whether the tray changed; a duplicate or a full list is a
    /// no-op.
    pub fn add_extra(&mut self, asset: InputAsset) -> bool {
        if self.current.is_none() {
            return self.select(asset);
        }
        if self.current.as_ref().is_some_and(|c| c.file == asset.file)
            || self.extras.iter().any(|a| a.file == asset.file)
            || self.extras.len() >= Self::MAX_EXTRAS
        {
            return false;
        }
        self.extras.push(asset);
        true
    }

    pub fn remove_extra(&mut self, index: usize) -> bool {
        if index < self.extras.len() {
            self.extras.remove(index);
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) -> bool {
        let had_extras = !self.extras.is_empty();
        self.extras.clear();
        self.current.take().is_some() || had_extras
    }

    /// Deletion sweep: drop the pinned asset when its managed record no
    /// longer exists (single delete, group delete, cap eviction — every
    /// mutation path revalidates). Returns whether the tray changed.
    pub fn retain_existing(&mut self, exists: impl Fn(&str) -> bool) -> bool {
        let before = self.extras.len();
        self.extras.retain(|asset| exists(&asset.file));
        let mut changed = self.extras.len() != before;
        if let Some(asset) = &self.current {
            if !exists(&asset.file) {
                self.current = None;
                changed = true;
            }
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// Human-choice candidate contact sheet
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateCard {
    pub id: String,
    pub title: String,
    pub status: String,
    pub meta: String,
    pub progress: f32,
    pub failed: bool,
    pub selected: bool,
}

/// Pure projection from keyed pipeline state into contact-sheet cards.
/// Arrival order never participates: every row is tied to the stable id that
/// was allocated from the coherent fleet snapshot.
pub fn candidate_cards(set: &CandidateSet) -> Vec<CandidateCard> {
    set.candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let (status, failed) = match &candidate.state {
                StageState::Waiting => ("waiting".to_string(), false),
                StageState::FanOut => (candidate.detail.clone(), false),
                StageState::AwaitingChoice => (candidate.detail.clone(), false),
                StageState::Submitting => ("submitting…".to_string(), false),
                StageState::Polling => (
                    if candidate.detail.is_empty() {
                        candidate.service_state.clone()
                    } else {
                        candidate.detail.clone()
                    },
                    false,
                ),
                StageState::Fetching => ("fetching image…".to_string(), false),
                StageState::Done => ("ready · click to preview/select".to_string(), false),
                StageState::Failed(error) => (format!("FAILED: {error}"), true),
            };
            let meta = if candidate.endpoint.is_empty() || candidate.model.is_empty() {
                format!("GPU wave pending · seed {}", candidate.seed)
            } else {
                format!(
                    "{} @ {} · seed {}",
                    candidate.model,
                    candidate.endpoint.trim_start_matches("http://"),
                    candidate.seed,
                )
            };
            CandidateCard {
                id: candidate.id.clone(),
                title: format!("Candidate {}", index + 1),
                status,
                meta,
                progress: candidate.progress.clamp(0.0, 1.0) as f32,
                failed,
                selected: set.selected.as_deref() == Some(candidate.id.as_str()),
            }
        })
        .collect()
}

/// Responsive fixed-eight contact sheet: four columns × two rows at normal
/// width, two columns × four rows in a narrow pane. Textures are installed
/// once when candidate bytes land; draw only binds cached GPU handles.
#[derive(Script, ScriptHook, Widget)]
pub struct CandidateSheet {
    #[deref]
    view: View,
    #[rust]
    cards: Vec<CandidateCard>,
    #[rust]
    textures: HashMap<String, Texture>,
    #[rust]
    placeholder: Option<Texture>,
    #[rust(4usize)]
    pub last_cols: usize,
}

impl CandidateSheet {
    pub fn clear_textures(&mut self, cx: &mut Cx) {
        self.textures.clear();
        self.view.redraw(cx);
    }

    pub fn set_cards(&mut self, cx: &mut Cx, cards: Vec<CandidateCard>) {
        if self.cards != cards {
            self.cards = cards;
            self.view.redraw(cx);
        }
    }

    pub fn install_texture(&mut self, cx: &mut Cx, candidate_id: String, texture: Texture) {
        self.textures.insert(candidate_id, texture);
        self.view.redraw(cx);
    }

    pub fn candidate_at_cell(&self, row: usize, slot: usize) -> Option<String> {
        self.cards
            .get(row * self.last_cols.max(1) + slot)
            .map(|card| card.id.clone())
    }
}

pub fn candidate_columns(width: f64) -> usize {
    if width >= 700.0 { 4 } else { 2 }
}

impl Widget for CandidateSheet {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let width = self.view.area().rect(cx).size.x;
        if width > 0.0 {
            self.last_cols = candidate_columns(width);
        }
        let cols = self.last_cols.max(1);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            if self.cards.is_empty() {
                list.set_item_range(cx, 0, 1);
                while let Some(item_id) = list.next_visible_item(cx) {
                    if item_id == 0 {
                        list.item(cx, item_id, id!(Empty)).draw_all_unscoped(cx);
                    }
                }
                continue;
            }
            let rows = self.cards.len().div_ceil(cols);
            list.set_item_range(cx, 0, rows);
            while let Some(row_id) = list.next_visible_item(cx) {
                if row_id >= rows {
                    continue;
                }
                let item = list.item(cx, row_id, id!(Row));
                let slots = [ids!(c1), ids!(c2), ids!(c3), ids!(c4)];
                let titles = [
                    ids!(c1.candidate_title), ids!(c2.candidate_title),
                    ids!(c3.candidate_title), ids!(c4.candidate_title),
                ];
                let statuses = [
                    ids!(c1.candidate_status), ids!(c2.candidate_status),
                    ids!(c3.candidate_status), ids!(c4.candidate_status),
                ];
                let metas = [
                    ids!(c1.candidate_meta), ids!(c2.candidate_meta),
                    ids!(c3.candidate_meta), ids!(c4.candidate_meta),
                ];
                let progresses = [
                    ids!(c1.candidate_progress), ids!(c2.candidate_progress),
                    ids!(c3.candidate_progress), ids!(c4.candidate_progress),
                ];
                let thumbs = [
                    ids!(c1.candidate_thumb), ids!(c2.candidate_thumb),
                    ids!(c3.candidate_thumb), ids!(c4.candidate_thumb),
                ];
                for slot in 0..4 {
                    let index = row_id * cols + slot;
                    let visible = slot < cols && index < self.cards.len();
                    item.view(cx, slots[slot]).set_visible(cx, visible);
                    if !visible {
                        continue;
                    }
                    let card = self.cards[index].clone();
                    item.label(cx, titles[slot]).set_text(cx, &card.title);
                    item.label(cx, statuses[slot]).set_text(cx, &card.status);
                    item.label(cx, metas[slot]).set_text(cx, &card.meta);
                    item.view(cx, slots[slot]).toggle_state(
                        cx,
                        card.selected,
                        Animate::No,
                        ids!(select.on),
                        ids!(select.off),
                    );
                    let progress = item.view(cx, progresses[slot]);
                    progress.set_uniform(cx, live_id!(progress), &[card.progress]);
                    let fill: [f32; 4] = if card.failed {
                        [0.85, 0.35, 0.32, 1.0]
                    } else {
                        [0.24, 0.61, 0.94, 1.0]
                    };
                    progress.set_uniform(cx, live_id!(color_fill), &fill);
                    let texture = self
                        .textures
                        .get(&card.id)
                        .cloned()
                        .unwrap_or_else(|| {
                            self.placeholder
                                .get_or_insert_with(|| badge_texture(cx, "image"))
                                .clone()
                        });
                    item.image(cx, thumbs[slot]).set_texture(cx, Some(texture));
                }
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ---------------------------------------------------------------------------
// History strip (horizontal recency gallery on the Create surface)
// ---------------------------------------------------------------------------

/// One compact slot of the History strip: a whole pipeline run (or one
/// ungrouped legacy record) represented by its end product. History is about
/// end products — intermediates stay durably tied to the run on disk and
/// remain inspectable on the Library surface, but never explode into their
/// own strip cards.
#[derive(Clone)]
pub struct HistoryTile {
    /// Representative artifact: thumbnail, type badge, title, selection ring
    /// and outbound file drag all come from this member.
    pub entry: GalleryEntry,
    /// Persisted run group behind this tile's ×. `None` = one ungrouped
    /// legacy record standing alone.
    pub group_id: Option<String>,
    /// How many managed records the tile's × removes.
    pub count: usize,
}

/// What one tile's single visible × means. The ungrouped "Earlier imports"
/// bucket is deliberately NOT addressable: legacy records tile and delete
/// one by one, so no single × can sweep them together.
#[derive(Clone, Debug, PartialEq)]
pub enum TileDelete {
    /// Remove the whole persisted run group atomically
    /// (`Library::remove_group`).
    Group(String),
    /// Remove exactly this one ungrouped legacy record.
    Single(String),
}

impl HistoryTile {
    pub fn delete_action(&self) -> TileDelete {
        match &self.group_id {
            Some(id) => TileDelete::Group(id.clone()),
            None => TileDelete::Single(self.entry.meta.file.clone()),
        }
    }
}

/// Project newest-first entries into ONE tile per persisted run group —
/// grouping STRICTLY by id, never adjacency, so artifacts of concurrently-
/// finishing runs stay with their own run even when interleaved on disk.
/// The representative is the group's newest member: the library only holds
/// persisted (successful) artifacts and a run appends in chain order, so
/// that is the final artifact — or the newest remaining one when the final
/// was individually deleted or has not landed yet. The tile shows selected
/// when ANY member is open, and tiles order by their group's newest member.
/// Ungrouped legacy records each get their own single-record tile in place.
pub fn history_tiles(entries: Vec<GalleryEntry>) -> Vec<HistoryTile> {
    let mut tiles: Vec<HistoryTile> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(id) = entry.meta.group_id.as_deref() else {
            tiles.push(HistoryTile {
                entry: entry.clone(),
                group_id: None,
                count: 1,
            });
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        // First occurrence in a newest-first scan: every member lives at or
        // after `index`, and `entry` itself is the newest one.
        let members: Vec<&GalleryEntry> = entries[index..]
            .iter()
            .filter(|member| member.meta.group_id.as_deref() == Some(id))
            .collect();
        // Newest-first, but prefer the textured mesh over paint maps / JSON
        // so clicking the run tile opens something you can judge in 3D.
        let mut representative = group_tile_representative(&members).clone();
        representative.selected = members.iter().any(|member| member.selected);
        tiles.push(HistoryTile {
            entry: representative,
            group_id: Some(id.to_string()),
            count: members.len(),
        });
    }
    tiles
}

fn group_tile_representative<'a>(members: &[&'a GalleryEntry]) -> &'a GalleryEntry {
    let ct = |entry: &&GalleryEntry| entry.meta.content_type.to_ascii_lowercase();
    // Final product first: the mesh of a mesh run, the clip of a video run,
    // the take of an audio run — an image is the source of those, and only
    // represents runs that produced nothing further.
    members
        .iter()
        .copied()
        .find(|entry| ct(entry).contains("gltf"))
        .or_else(|| {
            members.iter().copied().find(|entry| {
                let ct = ct(entry);
                ct.starts_with("video/") || ct.starts_with("audio/")
            })
        })
        .or_else(|| {
            members
                .iter()
                .copied()
                .find(|entry| ct(entry).starts_with("image/"))
        })
        .or_else(|| {
            members.iter().copied().find(|entry| {
                let ct = ct(entry);
                !ct.starts_with("application/json") && !ct.starts_with("text/")
            })
        })
        .unwrap_or(members[0])
}

/// Virtualized, horizontally scrolling History strip: one compact tile per
/// pipeline-run group (see [`history_tiles`]). Encoded thumbnails are read
/// only when a tile first becomes visible. The cache is keyed by stable
/// managed file id, not PortalList index: list items are routinely
/// discarded/recreated while scrolling and must always have their texture
/// rebound.
#[derive(Script, ScriptHook, Widget)]
pub struct LibraryGallery {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<HistoryTile>,
    #[rust]
    cache: PreviewCache,
    /// Files whose preview landed since their tile last rendered. Tiles are
    /// `CachedView`s: binding a new texture on the card's Image does nothing
    /// visible until the cached content is forced to re-render, so a landed
    /// preview used to stay a badge until scrolling moved the tile.
    #[rust]
    stale_tiles: HashSet<String>,
}

impl LibraryGallery {
    /// Replace the visible entries. Selection-only updates keep decoded
    /// textures resident; payload/sidecar changes opt into a cache clear.
    pub fn set_entries(
        &mut self,
        cx: &mut Cx,
        entries: Vec<GalleryEntry>,
        clear_thumbnails: bool,
    ) {
        self.rows = history_tiles(entries);
        if clear_thumbnails {
            // Content changes may replace a sidecar at the same stable path.
            self.cache.clear();
        }
        self.view.redraw(cx);
    }

    /// Preview work recorded by draws since the last drain (app → worker).
    pub fn take_preview_work(&mut self) -> Vec<(String, PreviewWork)> {
        self.cache.take_pending()
    }

    /// A worker-decoded (or failure-pinned) preview texture landed.
    pub fn install_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        texture: Texture,
    ) {
        self.stale_tiles.insert(file.clone());
        self.cache.install(file, source, texture);
        self.view.redraw(cx);
    }

    pub fn install_anim_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        frames: Vec<Texture>,
        fps: f32,
    ) {
        self.stale_tiles.insert(file.clone());
        self.cache.install_anim(file, source, frames, fps);
        self.view.redraw(cx);
        cx.new_next_frame();
    }

    /// A member's decoded preview if the strip already has it (lets other
    /// widgets — the run tray — reuse the decode instead of re-reading).
    pub fn cached_texture(&self, file: &str) -> Option<Texture> {
        self.cache.cached(file)
    }

    /// Stable file id of tile `index`'s representative artifact — what a
    /// click opens in the viewer.
    pub fn file_at(&self, index: usize) -> Option<String> {
        self.rows.get(index).map(|tile| tile.entry.meta.file.clone())
    }

    /// Actual managed audio/video payload of tile `index`'s representative.
    /// Never a preview sidecar; None for non-media representatives.
    pub fn file_drag_payload_path_at(&self, index: usize) -> Option<PathBuf> {
        self.rows
            .get(index)
            .and_then(|tile| tile.entry.file_drag_payload_path())
    }

    /// Exact scope of tile `index`'s single × (whole run vs one legacy
    /// record).
    pub fn delete_at(&self, index: usize) -> Option<TileDelete> {
        self.rows.get(index).map(HistoryTile::delete_action)
    }
}

impl Widget for LibraryGallery {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            if self.rows.is_empty() {
                // next_visible_item keeps yielding ids while the viewport is
                // unfilled; drawing only id 0 (and skipping the rest) is what
                // terminates the walk with a single empty-state card.
                list.set_item_range(cx, 0, 1);
                while let Some(item_id) = list.next_visible_item(cx) {
                    if item_id == 0 {
                        list.item(cx, item_id, id!(Empty)).draw_all_unscoped(cx);
                    }
                }
                continue;
            }

            list.set_item_range(cx, 0, self.rows.len());
            while let Some(item_id) = list.next_visible_item(cx) {
                let Some(tile) = self.rows.get(item_id).cloned() else {
                    continue;
                };
                // A newly-created/recycled PortalList widget starts from its
                // template and therefore has no prior Image texture. Always
                // bind the texture below, even when this stable file was
                // already decoded on an earlier scroll pass.
                let (item, _existed) = list.item_with_existed(cx, item_id, id!(Item));
                item.button(cx, ids!(card.title))
                    .set_text(cx, &tile.entry.meta.label);
                item.view(cx, ids!(card.file_drag))
                    .set_visible(cx, tile.entry.is_file_drag_media());
                // The member-count badge doubles as the "× removes the whole
                // run" cue; a single-record tile needs neither.
                let badge = item.view(cx, ids!(card.run_count));
                badge.set_visible(cx, tile.count > 1);
                if tile.count > 1 {
                    item.label(cx, ids!(card.run_count_label))
                        .set_text(cx, &format!("{} items", tile.count));
                }
                item.view(cx, ids!(card)).toggle_state(
                    cx,
                    tile.entry.selected,
                    Animate::No,
                    ids!(select.on),
                    ids!(select.off),
                );

                // Cache hit or typed badge — NEVER a synchronous file
                // read/decode inside a draw.
                let now = cx.time();
                let texture = self.cache.texture_for(cx, &tile.entry, now);
                item.image(cx, ids!(card.thumb))
                    .set_texture(cx, Some(texture));
                // A preview that landed since this tile last rendered: the
                // tile is a CachedView, so force its offscreen re-render or
                // the new texture never shows (until a scroll moves it).
                if self.stale_tiles.remove(&tile.entry.meta.file) || self.cache.has_anims() {
                    item.as_view().redraw_texture_cache();
                }
                item.draw_all_unscoped(cx);
            }
        }
        if self.cache.has_anims() {
            cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::NextFrame(_)) && self.cache.has_anims() {
            self.view.redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// Run tray (one horizontal, virtualized row of the selected run's artifacts)
// ---------------------------------------------------------------------------

/// One member of the selected run as the tray shows it.
#[derive(Clone)]
pub struct RunTrayMember {
    pub entry: GalleryEntry,
    /// Stage-ish label under the thumb ("matte", "mesh", "paint", "image").
    pub kind: String,
}

/// The selected run spread out: ONE row, horizontally scrolling and
/// virtualized (imports bring hundreds of members), each chip a click-to-view
/// / double-click-to-pin artifact. Same async preview rules as the History
/// strip: decoded textures keyed by stable file id, never a synchronous read.
#[derive(Script, ScriptHook, Widget)]
pub struct RunTray {
    #[deref]
    view: View,
    #[rust]
    members: Vec<RunTrayMember>,
    #[rust]
    selected: Option<String>,
    #[rust]
    cache: PreviewCache,
    #[rust]
    stale_chips: HashSet<String>,
}

impl RunTray {
    pub fn set_members(
        &mut self,
        cx: &mut Cx,
        members: Vec<RunTrayMember>,
        selected: Option<String>,
        clear_previews: bool,
    ) {
        self.members = members;
        self.selected = selected;
        if clear_previews {
            self.cache.clear();
        }
        self.view.redraw(cx);
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    pub fn file_at(&self, index: usize) -> Option<String> {
        self.members.get(index).map(|m| m.entry.meta.file.clone())
    }

    pub fn take_preview_work(&mut self) -> Vec<(String, PreviewWork)> {
        self.cache.take_pending()
    }

    /// Reuse a decode another widget already has (the History strip usually
    /// holds every member of the selected run).
    pub fn seed_texture(&mut self, file: &str, texture: Texture) {
        if self.cache.cached(file).is_none() {
            self.cache.install(file.to_string(), None, texture);
            self.stale_chips.insert(file.to_string());
        }
    }

    pub fn install_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        texture: Texture,
    ) {
        self.stale_chips.insert(file.clone());
        self.cache.install(file, source, texture);
        self.view.redraw(cx);
    }

    pub fn install_anim_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        frames: Vec<Texture>,
        fps: f32,
    ) {
        self.stale_chips.insert(file.clone());
        self.cache.install_anim(file, source, frames, fps);
        self.view.redraw(cx);
        cx.new_next_frame();
    }
}

impl Widget for RunTray {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, self.members.len());
            while let Some(item_id) = list.next_visible_item(cx) {
                let Some(member) = self.members.get(item_id).cloned() else {
                    continue;
                };
                let item = list.item(cx, item_id, id!(Chip));
                item.label(cx, ids!(kind)).set_text(cx, &member.kind);
                let selected = self.selected.as_deref() == Some(member.entry.meta.file.as_str());
                item.as_view()
                    .toggle_state(cx, selected, Animate::No, ids!(select.on), ids!(select.off));
                let now = cx.time();
                let texture = self.cache.texture_for(cx, &member.entry, now);
                item.image(cx, ids!(thumb)).set_texture(cx, Some(texture));
                self.stale_chips.remove(&member.entry.meta.file);
                item.draw_all_unscoped(cx);
            }
        }
        if self.cache.has_anims() {
            cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::NextFrame(_)) && self.cache.has_anims() {
            self.view.redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// Library grid (wrapping thumbnail gallery on the Library surface)
// ---------------------------------------------------------------------------

/// Grid card layout constants; `grid_columns` derives the per-row card count
/// from the width the grid actually received.
pub const GRID_CARD_W: f64 = 150.0;
pub const GRID_GAP: f64 = 8.0;
/// Card slots available in the Row template (c1..c8); extra width on very
/// wide windows becomes margin instead of a ninth column.
pub const GRID_MAX_COLS: usize = 8;

pub fn grid_columns(width: f64) -> usize {
    (((width + GRID_GAP) / (GRID_CARD_W + GRID_GAP)) as usize).clamp(1, GRID_MAX_COLS)
}

/// Vertically scrolling thumbnail grid over the filtered local History. Rows
/// are virtualized; each PortalList row holds up to [`GRID_MAX_COLS`] card
/// slots and the visible column count follows the widget's real width.
#[derive(Script, ScriptHook, Widget)]
pub struct LibraryGrid {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<GalleryEntry>,
    #[rust]
    cache: PreviewCache,
    /// Columns used on the last draw — the click handler needs the same
    /// row-major mapping that layout used.
    #[rust(4usize)]
    pub last_cols: usize,
}

impl LibraryGrid {
    /// `clear_thumbnails` drops decoded textures so a replaced sidecar at the
    /// same path re-decodes; filter-only refreshes keep the cache warm.
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<GalleryEntry>, clear_thumbnails: bool) {
        self.entries = entries;
        if clear_thumbnails {
            self.cache.clear();
        }
        self.view.redraw(cx);
    }

    pub fn file_at(&self, index: usize) -> Option<String> {
        self.entries.get(index).map(|entry| entry.meta.file.clone())
    }

    /// Actual managed audio payload for a filtered grid card. This is never
    /// the waveform preview sidecar.
    pub fn file_drag_payload_path_at(&self, index: usize) -> Option<PathBuf> {
        self.entries
            .get(index)
            .and_then(GalleryEntry::file_drag_payload_path)
    }

    pub fn take_preview_work(&mut self) -> Vec<(String, PreviewWork)> {
        self.cache.take_pending()
    }

    pub fn install_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        texture: Texture,
    ) {
        self.cache.install(file, source, texture);
        self.view.redraw(cx);
    }

    pub fn install_anim_preview(
        &mut self,
        cx: &mut Cx,
        file: String,
        source: Option<PathBuf>,
        frames: Vec<Texture>,
        fps: f32,
    ) {
        self.cache.install_anim(file, source, frames, fps);
        self.view.redraw(cx);
        cx.new_next_frame();
    }
}

const GRID_SLOTS: usize = GRID_MAX_COLS;

impl Widget for LibraryGrid {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The widget's area rect is one frame behind, which is fine: a resize
        // re-chunks the rows on the next frame.
        let width = self.view.area().rect(cx).size.x;
        if width > GRID_CARD_W {
            self.last_cols = grid_columns(width - 14.0); // scrollbar gutter
        }
        let cols = self.last_cols.max(1);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            if self.entries.is_empty() {
                // Draw the empty card once; skipping later yielded ids ends
                // the viewport-fill walk (see LibraryGallery).
                list.set_item_range(cx, 0, 1);
                while let Some(item_id) = list.next_visible_item(cx) {
                    if item_id == 0 {
                        list.item(cx, item_id, id!(Empty)).draw_all_unscoped(cx);
                    }
                }
                continue;
            }
            let rows = self.entries.len().div_ceil(cols);
            list.set_item_range(cx, 0, rows);
            while let Some(row_id) = list.next_visible_item(cx) {
                // PortalList may ask past the declared item range while it
                // tries to fill a viewport that is taller than the available
                // content. Never instantiate those synthetic ids: an empty
                // Row has zero-height hidden cells, so drawing it makes the
                // list request another id forever (unbounded widget growth).
                if row_id >= rows {
                    continue;
                }
                let (item, _existed) = list.item_with_existed(cx, row_id, id!(Row));
                let slots = [
                    ids!(c1), ids!(c2), ids!(c3), ids!(c4),
                    ids!(c5), ids!(c6), ids!(c7), ids!(c8),
                ];
                let thumbs = [
                    ids!(c1.grid_thumb), ids!(c2.grid_thumb), ids!(c3.grid_thumb),
                    ids!(c4.grid_thumb), ids!(c5.grid_thumb), ids!(c6.grid_thumb),
                    ids!(c7.grid_thumb), ids!(c8.grid_thumb),
                ];
                let sprites = [
                    ids!(c1.grid_sprite), ids!(c2.grid_sprite), ids!(c3.grid_sprite),
                    ids!(c4.grid_sprite), ids!(c5.grid_sprite), ids!(c6.grid_sprite),
                    ids!(c7.grid_sprite), ids!(c8.grid_sprite),
                ];
                let titles = [
                    ids!(c1.grid_title), ids!(c2.grid_title), ids!(c3.grid_title),
                    ids!(c4.grid_title), ids!(c5.grid_title), ids!(c6.grid_title),
                    ids!(c7.grid_title), ids!(c8.grid_title),
                ];
                let drag_handles = [
                    ids!(c1.file_drag), ids!(c2.file_drag), ids!(c3.file_drag),
                    ids!(c4.file_drag), ids!(c5.file_drag), ids!(c6.file_drag),
                    ids!(c7.file_drag), ids!(c8.file_drag),
                ];
                for slot in 0..GRID_SLOTS {
                    let index = row_id * cols + slot;
                    let visible = slot < cols && index < self.entries.len();
                    item.view(cx, slots[slot]).set_visible(cx, visible);
                    if !visible {
                        continue;
                    }
                    let entry = self.entries[index].clone();
                    item.label(cx, titles[slot]).set_text(cx, &entry.meta.label);
                    item.view(cx, drag_handles[slot]).set_visible(cx, true);
                    item.view(cx, slots[slot]).toggle_state(
                        cx,
                        entry.selected,
                        Animate::No,
                        ids!(select.on),
                        ids!(select.off),
                    );
                    // Cache hit or typed badge — never a synchronous decode.
                    let now = cx.time();
                    let texture = self.cache.texture_for(cx, &entry, now);
                    let sprite = entry.is_sprite_preview();
                    item.image(cx, thumbs[slot]).set_visible(cx, !sprite);
                    item.image(cx, sprites[slot]).set_visible(cx, sprite);
                    if sprite {
                        item.image(cx, sprites[slot]).set_texture(cx, Some(texture));
                    } else {
                        item.image(cx, thumbs[slot]).set_texture(cx, Some(texture));
                    }
                }
                item.draw_all_unscoped(cx);
            }
        }
        if self.cache.has_anims() {
            cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if matches!(event, Event::NextFrame(_)) && self.cache.has_anims() {
            self.view.redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// Flattened rows for Runs+Workers / Admin / server catalog lists
// ---------------------------------------------------------------------------

/// What a click on a row's action affordance means. The widget stays dumb;
/// the app maps actions back onto its own state.
#[derive(Clone, Debug, PartialEq)]
pub enum RowAction {
    /// Stop run `id`'s active stage (service cancel) — each concurrent run
    /// cancels independently.
    CancelRun(u64),
    /// Drop entry `index` from the app-side run queue.
    CancelQueued(usize),
    /// Select a server catalog asset for the detail panel.
    SelectAsset(String),
    /// Stop the import that is running now.
    StopImport,
    /// Drop a waiting import by queue id.
    RemoveQueuedImport(u64),
}

/// One concurrent pipeline run as the Runs surface renders it.
pub struct RunView<'a> {
    pub id: u64,
    pub label: &'a str,
    pub pipeline: &'a Pipeline,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StoreRow {
    /// Section heading.
    Section(String),
    /// Dim explanatory / status line.
    Note(String),
    /// A pipeline stage or remote job with a live progress bar.
    Stage {
        title: String,
        meta: String,
        progress: f32,
        failed: bool,
        cancel: Option<RowAction>,
    },
    /// App-side queued run (waiting for the active pipeline to finish).
    Queued { title: String, cancel: RowAction },
    /// A generator worker (LAN fleet box, or a server worker when connected).
    Worker {
        title: String,
        state: String,
        meta: String,
        models: String,
        online: bool,
    },
    /// Generic titled record (server operation, game, room, namespace).
    Record { title: String, meta: String },
    /// Server catalog asset row (Library → Server, connected).
    Asset {
        title: String,
        meta: String,
        selected: bool,
        action: RowAction,
    },
    /// Honest big empty-state block for missing server data.
    Disconnected { title: String, detail: String },
}

/// PortalList renderer over a caller-provided `Vec<StoreRow>`. One widget
/// serves the Runs+Workers surface, the Admin surface and the Library's
/// server catalog; the row model is the entire contract.
#[derive(Script, ScriptHook, Widget)]
pub struct StoreListPanel {
    #[deref]
    view: View,
    #[rust]
    pub rows: Vec<StoreRow>,
}

impl StoreListPanel {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<StoreRow>) {
        if self.rows != rows {
            self.rows = rows;
            self.view.redraw(cx);
        }
    }

    pub fn row_at(&self, index: usize) -> Option<StoreRow> {
        self.rows.get(index).cloned()
    }
}

impl Widget for StoreListPanel {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, self.rows.len().max(1));
            while let Some(item_id) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(item_id).cloned() else {
                    continue;
                };
                let item = match row {
                    StoreRow::Section(text) => {
                        let item = list.item(cx, item_id, id!(SectionR));
                        item.label(cx, ids!(section_label)).set_text(cx, &text);
                        item
                    }
                    StoreRow::Note(text) => {
                        let item = list.item(cx, item_id, id!(NoteR));
                        item.label(cx, ids!(note_label)).set_text(cx, &text);
                        item
                    }
                    StoreRow::Stage {
                        title,
                        meta,
                        progress,
                        failed,
                        cancel,
                    } => {
                        let item = list.item(cx, item_id, id!(StageR));
                        item.label(cx, ids!(stage_title)).set_text(cx, &title);
                        item.label(cx, ids!(stage_meta)).set_text(cx, &meta);
                        let bar = item.view(cx, ids!(stage_bar));
                        bar.set_uniform(cx, live_id!(progress), &[progress]);
                        let fill: [f32; 4] = if failed {
                            [0.85, 0.35, 0.32, 1.0]
                        } else {
                            [0.24, 0.61, 0.94, 1.0]
                        };
                        bar.set_uniform(cx, live_id!(color_fill), &fill);
                        item.button(cx, ids!(stage_cancel))
                            .set_visible(cx, cancel.is_some());
                        item
                    }
                    StoreRow::Queued { title, .. } => {
                        let item = list.item(cx, item_id, id!(QueuedR));
                        item.label(cx, ids!(queued_title)).set_text(cx, &title);
                        item
                    }
                    StoreRow::Worker {
                        title,
                        state,
                        meta,
                        models,
                        online,
                    } => {
                        let item = list.item(cx, item_id, id!(WorkerR));
                        item.label(cx, ids!(worker_title)).set_text(cx, &title);
                        item.label(cx, ids!(worker_state)).set_text(cx, &state);
                        item.label(cx, ids!(worker_meta)).set_text(cx, &meta);
                        let models_label = item.label(cx, ids!(worker_models));
                        models_label.set_text(cx, &models);
                        models_label.set_visible(cx, !models.is_empty());
                        item.view(cx, ids!(worker_dot)).set_uniform(
                            cx,
                            live_id!(online),
                            &[if online { 1.0 } else { 0.0 }],
                        );
                        item
                    }
                    StoreRow::Record { title, meta } => {
                        let item = list.item(cx, item_id, id!(RecordR));
                        item.label(cx, ids!(record_title)).set_text(cx, &title);
                        item.label(cx, ids!(record_meta)).set_text(cx, &meta);
                        item
                    }
                    StoreRow::Asset {
                        title,
                        meta,
                        selected,
                        ..
                    } => {
                        let item = list.item(cx, item_id, id!(AssetR));
                        item.label(cx, ids!(asset_title)).set_text(cx, &title);
                        item.label(cx, ids!(asset_meta)).set_text(cx, &meta);
                        item.view(cx, ids!(asset_card)).toggle_state(
                            cx,
                            selected,
                            Animate::No,
                            ids!(select.on),
                            ids!(select.off),
                        );
                        item
                    }
                    StoreRow::Disconnected { title, detail } => {
                        let item = list.item(cx, item_id, id!(DiscR));
                        item.label(cx, ids!(disc_title)).set_text(cx, &title);
                        item.label(cx, ids!(disc_detail)).set_text(cx, &detail);
                        item
                    }
                };
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ---------------------------------------------------------------------------
// Row builders (pure over app state — unit-tested below)
// ---------------------------------------------------------------------------

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{cut}…")
    }
}

fn stage_row(run_id: u64, pipeline: &Pipeline, index: usize) -> StoreRow {
    let stage = &pipeline.stages[index];
    let where_ = if stage.box_url.is_empty() {
        String::new()
    } else {
        format!(
            "{} @ {}",
            stage.model,
            stage.box_url.trim_start_matches("http://")
        )
    };
    let elapsed = match (stage.started, stage.finished) {
        (Some(t0), Some(t1)) => format_clock((t1 - t0).as_secs_f64()),
        (Some(t0), None) => {
            let gone = t0.elapsed().as_secs_f64();
            let frac = stage.progress.clamp(0.0, 1.0);
            if frac > 0.02 && frac < 0.999 && gone > 1.0 {
                format!(
                    "{} · ~{} left",
                    format_clock(gone),
                    format_clock(gone * (1.0 - frac) / frac)
                )
            } else {
                format!("{}…", format_clock(gone))
            }
        }
        _ => String::new(),
    };
    let (state, progress, failed) = match &stage.state {
        StageState::Waiting => ("waiting".to_string(), stage.progress, false),
        StageState::FanOut => (stage.detail.clone(), stage.progress, false),
        StageState::AwaitingChoice => (stage.detail.clone(), 1.0, false),
        StageState::Submitting => ("submitting…".to_string(), stage.progress, false),
        StageState::Polling => (
            if stage.detail.is_empty() {
                stage.service_state.clone()
            } else {
                stage.detail.clone()
            },
            stage.progress,
            false,
        ),
        StageState::Fetching => ("fetching artifacts…".to_string(), stage.progress, false),
        StageState::Done => ("done".to_string(), 1.0, false),
        StageState::Failed(error) => (format!("FAILED: {error}"), 1.0, true),
    };
    let percent = format!("{:.1}%", progress.clamp(0.0, 1.0) * 100.0);
    let target = (stage.domain == "music").then(|| {
        format!(
            "{} target (may end early)",
            format_music_duration(pipeline.gen.music_seconds)
        )
    });
    let meta = [where_, target.unwrap_or_default(), percent, state, elapsed]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    StoreRow::Stage {
        title: format!("{} · {}", index + 1, stage_display_name(&stage.domain)),
        meta,
        progress: progress as f32,
        failed,
        cancel: (index == pipeline.current && pipeline.can_cancel_current())
            .then_some(RowAction::CancelRun(run_id)),
    }
}

/// Runs + Workers surface. Every concurrent local run lists with its own
/// stages, progress and Stop; LAN fleet is live data; the server section is
/// explicit about being disconnected.
pub fn runs_rows(
    runs: &[RunView],
    queued: &[String],
    fleet: &[BoxSnapshot],
    latency_ms: &[Option<u64>],
    store: &AssetStore,
) -> Vec<StoreRow> {
    let mut rows = vec![StoreRow::Section("THIS APP · LOCAL RUNS".into())];
    if runs.is_empty() {
        rows.push(StoreRow::Note(
            "Idle — nothing has run yet. Generate from the Create surface.".into(),
        ));
    }
    for run in runs {
        let state = if run.pipeline.is_running() {
            "running"
        } else {
            "finished"
        };
        rows.push(StoreRow::Note(format!(
            "{} · {state} · \u{201c}{}\u{201d}",
            run.label,
            truncate(&run.pipeline.prompt, 72)
        )));
        for index in 0..run.pipeline.stages.len() {
            rows.push(stage_row(run.id, run.pipeline, index));
        }
    }
    if !queued.is_empty() {
        rows.push(StoreRow::Section("UP NEXT · RUN QUEUE".into()));
        for (index, title) in queued.iter().enumerate() {
            rows.push(StoreRow::Queued {
                title: format!("{} · {title}", index + 1),
                cancel: RowAction::CancelQueued(index),
            });
        }
    }

    rows.push(StoreRow::Section("FLEET WORKERS · LAN".into()));
    if fleet.is_empty() {
        rows.push(StoreRow::Note(
            "No GPU boxes on the LAN — start a makepad-asset-ai fleet service.".into(),
        ));
    }
    // One row per PHYSICAL host; several service instances on one box list
    // as endpoints inside it (display grouping only — each endpoint keeps
    // its own health, queue and model set; see fleet_poll::host_groups).
    let endpoint_state = |index: usize| -> String {
        match &fleet[index].health {
            Some(_) => {
                let latency = latency_ms
                    .get(index)
                    .copied()
                    .flatten()
                    .map(|ms| format!(" · {ms}ms"))
                    .unwrap_or_default();
                format!("queue {}{latency}", fleet[index].jobs_pending())
            }
            None => "offline".to_string(),
        }
    };
    let model_lines = |index: usize| -> String {
        fleet[index]
            .models
            .iter()
            .filter(|model| model.available)
            .map(|model| {
                let badge = match model.state.as_str() {
                    "loaded" => "◆ loaded",
                    "ready" => "◇ ready",
                    other => other,
                };
                format!("{:<7} {}  {}", model.domain, model.id, badge)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (host, indices) in crate::fleet_poll::host_groups(fleet) {
        let any_up = indices.iter().any(|&index| fleet[index].is_up());
        let meta = indices
            .iter()
            .find_map(|&index| fleet[index].health.as_ref())
            .map(|health| {
                let vram = match (health.vram_free_mb, health.vram_total_mb) {
                    (Some(free), Some(total)) => format!(" · vram {free}/{total} MB"),
                    _ => String::new(),
                };
                format!("{}{vram}", health.gpu.as_deref().unwrap_or("no gpu info"))
            })
            .unwrap_or_else(|| {
                "unreachable — kept in the fleet, recovers on a later poll".to_string()
            });
        let (state, models) = if indices.len() == 1 {
            (endpoint_state(indices[0]), model_lines(indices[0]))
        } else {
            let blocks = indices
                .iter()
                .map(|&index| {
                    let port = crate::fleet_poll::port_of(&fleet[index].base_url);
                    let lines = model_lines(index);
                    if lines.is_empty() {
                        format!(":{port} · {}", endpoint_state(index))
                    } else {
                        format!(":{port} · {}\n{lines}", endpoint_state(index))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (format!("{} service endpoints", indices.len()), blocks)
        };
        rows.push(StoreRow::Worker {
            title: host,
            state,
            meta,
            models,
            online: any_up,
        });
    }

    rows.push(StoreRow::Section("SERVER GENERATION".into()));
    if !store.connected() {
        rows.push(StoreRow::Note(
            format!("{} — remote generation profiles unavailable.", store.status_label()),
        ));
    } else {
        match &store.profiles {
            Remote::Idle => rows.push(StoreRow::Note(
                "Generation profiles have not been requested yet.".into(),
            )),
            Remote::Loading => rows.push(StoreRow::Note(
                "Loading advertised generation profiles…".into(),
            )),
            Remote::Failed(error) => rows.push(StoreRow::Note(format!(
                "Generation profiles failed: {error}"
            ))),
            Remote::Ready(profiles) if profiles.is_empty() => rows.push(StoreRow::Note(
                "This server advertises no generation profiles.".into(),
            )),
            Remote::Ready(profiles) => {
                for profile in profiles {
                    rows.push(StoreRow::Record {
                        title: format!("{} · {}", profile.label, profile.domain),
                        meta: format!(
                            "profile {} · job {} · namespace {}",
                            profile.id, profile.kind, profile.namespace
                        ),
                    });
                }
            }
        }
        rows.push(StoreRow::Note(
            "Global executing-job enumeration is not exposed by the current Asset Server API; this panel does not invent remote jobs."
                .into(),
        ));
    }
    rows
}

/// Admin + Audit surface over routes the shared client actually exposes:
/// verified server identity, generation profiles and the committed catalog
/// event feed. Games/rooms/namespaces are deliberately not synthesized when
/// there is no corresponding client request.
pub fn admin_rows(store: &AssetStore) -> Vec<StoreRow> {
    if !store.connected() {
        return vec![
            StoreRow::Disconnected {
                title: "No server data".into(),
                detail: store.status_label(),
            },
            StoreRow::Note(
                "Discovery, identity verification and authentication run through the shared Asset Server session.".into(),
            ),
        ];
    }
    let server = store.server.as_ref().expect("connected implies server identity");
    let mut rows = vec![
        StoreRow::Section("VERIFIED ASSET SERVER".into()),
        StoreRow::Record {
            title: server.label.clone(),
            meta: format!("server id {}", hex16_string(&server.server_id)),
        },
        StoreRow::Section("CATALOG EVENT FEED · NEWEST FIRST".into()),
    ];
    if let Some(note) = &store.event_note {
        rows.push(StoreRow::Note(note.clone()));
    }
    if store.events.is_empty() {
        rows.push(StoreRow::Note(if store.events_live {
            "Following committed catalog changes · no events received yet.".into()
        } else {
            "Catalog event feed is starting…".into()
        }));
    }
    for event in store.events.iter().take(50) {
        let subject = event
            .alias
            .clone()
            .or_else(|| event.asset_id.map(|id| id.to_string()))
            .or_else(|| event.game_id.map(|id| id.to_string()))
            .unwrap_or_else(|| event.namespace.clone());
        rows.push(StoreRow::Record {
            title: format!("#{} · {}", event.seq, event.kind.as_str()),
            meta: format!("{} · {} · {} ms", event.namespace, subject, event.ts_ms),
        });
    }
    rows
}

/// Library → Server catalog rows over the real typed search response.
pub fn catalog_rows(store: &AssetStore) -> Vec<StoreRow> {
    if !store.connected() {
        return vec![
            StoreRow::Disconnected {
                title: "No server catalog".into(),
                detail: store.status_label(),
            },
            StoreRow::Note(
                "Local History stays fully usable on the Local tab; nothing local is promoted into this view.".into(),
            ),
        ];
    }
    match &store.search {
        Remote::Idle => vec![StoreRow::Note("Catalog browse has not started yet.".into())],
        Remote::Loading => vec![StoreRow::Note("Searching the server catalog…".into())],
        Remote::Failed(error) => vec![StoreRow::Disconnected {
            title: "Catalog request failed".into(),
            detail: error.clone(),
        }],
        Remote::Ready(results) if results.hits.is_empty() => vec![StoreRow::Note(
            "No catalog assets match the current filters.".into(),
        )],
        Remote::Ready(results) => {
            let mut rows = results
                .hits
                .iter()
                .map(|hit| {
                    let kind = hit
                        .kind
                        .map(server_kind_label)
                        .unwrap_or("kind unknown");
                    let alias = hit
                        .alias
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "no alias".into());
                    let lifecycle = if hit.live { "published" } else { "not live" };
                    StoreRow::Asset {
                        title: format!("{} · {kind}", hit.title),
                        meta: format!(
                            "{alias} · {} · {lifecycle} · {}",
                            hit.namespace, hit.snippet
                        ),
                        selected: store.selected == Some(hit.asset_id),
                        action: RowAction::SelectAsset(hit.asset_id.to_string()),
                    }
                })
                .collect::<Vec<_>>();
            if results.more {
                rows.push(StoreRow::Note(format!(
                    "Showing {} of {} matches · more available on the server.",
                    results.hits.len(), results.total
                )));
            }
            rows
        }
    }
}

pub fn short_digest(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_store_state::{
        AssetKind, SearchResults, ServerInfo,
    };
    use makepad_asset_client::CatalogHit;
    use makepad_asset_data::AssetId;
    use crate::pipeline::GenParams;

    fn local_pipeline() -> Pipeline {
        Pipeline::new(
            "a weathered fishing trawler",
            &["text", "image"],
            &[],
            vec![],
            None,
            None,
            GenParams::default(),
        )
    }

    #[test]
    fn runs_rows_show_each_concurrent_run_and_honest_server_state() {
        let first = local_pipeline();
        let second = local_pipeline();
        let fleet = vec![BoxSnapshot::new("http://10.0.0.217:8767")];
        let runs = [
            RunView {
                id: 2,
                label: "video (small) — \"b\"",
                pipeline: &second,
            },
            RunView {
                id: 1,
                label: "video (small) — \"a\"",
                pipeline: &first,
            },
        ];
        let rows = runs_rows(
            &runs,
            &["image — \"queued run\"".to_string()],
            &fleet,
            &[None],
            &AssetStore::default(),
        );
        // One Stage row per stage PER RUN — concurrent runs itemize
        // independently instead of collapsing into one "active" pipeline.
        let stages = rows
            .iter()
            .filter(|row| matches!(row, StoreRow::Stage { .. }))
            .count();
        assert_eq!(stages, 4, "two runs × two stages");
        assert!(rows
            .iter()
            .any(|row| matches!(row, StoreRow::Note(text) if text.contains("video (small) — \"b\""))));
        assert!(rows
            .iter()
            .any(|row| matches!(row, StoreRow::Queued { cancel: RowAction::CancelQueued(0), .. })));
        // The configured-but-unpolled box renders as offline — never invented
        // as healthy.
        assert!(rows.iter().any(|row| matches!(
            row,
            StoreRow::Worker { online: false, .. }
        )));
        // Disconnected server: a note, and zero server records.
        assert!(rows
            .iter()
            .any(|row| matches!(row, StoreRow::Note(text) if text.contains("not started"))));
        assert!(!rows.iter().any(|row| matches!(row, StoreRow::Record { .. })));
    }

    #[test]
    fn admin_rows_are_one_honest_block_when_disconnected() {
        let rows = admin_rows(&AssetStore::default());
        assert!(matches!(rows[0], StoreRow::Disconnected { .. }));
        assert!(!rows.iter().any(|row| matches!(
            row,
            StoreRow::Record { .. } | StoreRow::Section(_)
        )));
    }

    #[test]
    fn catalog_rows_are_disconnected_without_session_and_real_with_search_page() {
        let mut store = AssetStore::default();
        assert!(matches!(
            catalog_rows(&store)[0],
            StoreRow::Disconnected { .. }
        ));

        let asset_id = AssetId::from_bytes([7; 16]);
        store.server = Some(ServerInfo {
            label: "asset.example:443".into(),
            server_id: [9; 16],
        });
        store.search = Remote::Ready(SearchResults {
            hits: vec![CatalogHit {
                asset_id,
                namespace: "game".into(),
                kind: Some(AssetKind::Vehicle),
                title: "Fishing Trawler".into(),
                snippet: "weathered coast vehicle".into(),
                score: 42,
                live: true,
                alias: Some("game/trawler".parse().unwrap()),
            }],
            total: 1,
            more: false,
        });
        let rows = catalog_rows(&store);
        assert!(matches!(
            &rows[0],
            StoreRow::Asset { title, action: RowAction::SelectAsset(id), .. }
                if title.contains("Fishing Trawler") && id == &asset_id.to_string()
        ));
    }

    fn entry(file: &str, group_id: Option<&str>, group_label: Option<&str>) -> GalleryEntry {
        GalleryEntry {
            meta: crate::library::LibraryMeta {
                file: file.into(),
                label: file.into(),
                domain: "image".into(),
                content_type: "image/png".into(),
                prompt: String::new(),
                group_id: group_id.map(Into::into),
                group_label: group_label.map(Into::into),
                tags: None,
                enhanced_tags: None,
            },
            path: PathBuf::from(file),
            preview_path: None,
            selected: false,
        }
    }

    #[test]
    fn history_tiles_are_one_compact_tile_per_run_group_never_adjacency() {
        // Interleaved on purpose: out-of-order completion of concurrent runs
        // writes A, B, A, legacy, B, A — grouping must follow the ids alone,
        // and a run's intermediates must never explode into their own tiles.
        let tiles = history_tiles(vec![
            entry("lib-9.mp4", Some("run-a"), Some("video — \"storm\"")),
            entry("lib-8.wav", Some("run-b"), Some("sfx — \"clash\"")),
            entry("lib-7.png", Some("run-a"), Some("video — \"storm\"")),
            entry("lib-6.txt", None, None),
            entry("lib-5.png", Some("run-b"), Some("sfx — \"clash\"")),
            entry("lib-4.txt", Some("run-a"), Some("video — \"storm\"")),
        ]);
        let describe: Vec<String> = tiles
            .iter()
            .map(|tile| {
                format!(
                    "{}:{:?}:{}",
                    tile.entry.meta.file,
                    tile.group_id.as_deref(),
                    tile.count
                )
            })
            .collect();
        // Six records → three tiles, ordered by each group's newest member;
        // no header slots, no intermediate cards.
        assert_eq!(
            describe,
            vec![
                "lib-9.mp4:Some(\"run-a\"):3",
                "lib-8.wav:Some(\"run-b\"):2",
                "lib-6.txt:None:1",
            ]
        );
    }

    #[test]
    fn paint_run_tile_opens_the_textured_glb_not_the_manifest() {
        let mut manifest = entry("lib-5.json", Some("run-pbr"), Some("pbr"));
        manifest.meta.content_type = "application/json".into();
        manifest.meta.domain = "paint".into();
        let mut albedo = entry("lib-4.png", Some("run-pbr"), Some("pbr"));
        albedo.meta.domain = "paint".into();
        let mut painted = entry("lib-3.glb", Some("run-pbr"), Some("pbr"));
        painted.meta.content_type = "model/gltf-binary".into();
        painted.meta.domain = "paint".into();
        let mut mesh = entry("lib-2.glb", Some("run-pbr"), Some("pbr"));
        mesh.meta.content_type = "model/gltf-binary".into();
        mesh.meta.domain = "mesh".into();
        let tiles = history_tiles(vec![manifest, albedo, painted, mesh]);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].count, 4);
        assert_eq!(tiles[0].entry.meta.file, "lib-3.glb");
        assert_eq!(tiles[0].entry.meta.domain, "paint");
    }

    #[test]
    fn representative_is_the_final_artifact_then_newest_remaining() {
        // Newest-first input: the run's final persisted artifact leads.
        let full = history_tiles(vec![
            entry("lib-3.mp4", Some("run-a"), Some("video")),
            entry("lib-2.png", Some("run-a"), Some("video")),
            entry("lib-1.txt", Some("run-a"), Some("video")),
        ]);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].entry.meta.file, "lib-3.mp4");
        assert_eq!(full[0].count, 3);

        // Final artifact individually deleted (or not landed yet): the
        // newest remaining member represents the same run.
        let partial = history_tiles(vec![
            entry("lib-2.png", Some("run-a"), Some("video")),
            entry("lib-1.txt", Some("run-a"), Some("video")),
        ]);
        assert_eq!(partial[0].entry.meta.file, "lib-2.png");
        assert_eq!(partial[0].group_id.as_deref(), Some("run-a"));
        assert_eq!(partial[0].count, 2);
    }

    #[test]
    fn tile_delete_routes_the_exact_group_and_single_legacy_records() {
        let tiles = history_tiles(vec![
            entry("lib-4.mp4", Some("run-a"), Some("video")),
            entry("lib-3.txt", None, None),
            entry("lib-2.png", Some("run-a"), Some("video")),
            entry("lib-1.txt", None, None),
        ]);
        let deletes: Vec<TileDelete> =
            tiles.iter().map(HistoryTile::delete_action).collect();
        // The grouped tile's × targets its persisted group id — the whole
        // run, atomically. Each ungrouped legacy record keeps its OWN tile
        // whose × removes exactly that file: no single × can sweep the
        // "Earlier imports" bucket, which is deliberately unaddressable.
        assert_eq!(
            deletes,
            vec![
                TileDelete::Group("run-a".into()),
                TileDelete::Single("lib-3.txt".into()),
                TileDelete::Single("lib-1.txt".into()),
            ]
        );
    }

    #[test]
    fn any_open_member_highlights_its_run_tile() {
        let newest = entry("lib-2.mp4", Some("run-a"), Some("video"));
        let mut intermediate = entry("lib-1.png", Some("run-a"), Some("video"));
        intermediate.selected = true;
        let tiles = history_tiles(vec![newest, intermediate]);
        // The intermediate was opened from the Library surface; the run's
        // one tile carries the selection ring even though the representative
        // itself is not the selected file.
        assert!(tiles[0].entry.selected);
        assert_eq!(tiles[0].entry.meta.file, "lib-2.mp4");

        let unselected = history_tiles(vec![
            entry("lib-2.mp4", Some("run-a"), Some("video")),
            entry("lib-1.png", Some("run-a"), Some("video")),
        ]);
        assert!(!unselected[0].entry.selected);
    }

    #[test]
    fn file_drag_lookup_returns_audio_and_video_payloads_never_previews() {
        let mut wav = entry("lib-3.wav", Some("run-a"), Some("music"));
        wav.meta.content_type = "AUDIO/WAV".into();
        wav.path = PathBuf::from("/library/lib-3.wav");
        wav.preview_path = Some(PathBuf::from("/library/lib-3.wav.thumb"));
        assert_eq!(
            wav.file_drag_payload_path(),
            Some(PathBuf::from("/library/lib-3.wav"))
        );

        let mut video = entry("lib-4.mp4", Some("run-b"), Some("video"));
        video.meta.content_type = "VIDEO/MP4".into();
        video.path = PathBuf::from("/library/lib-4.mp4");
        video.preview_path = Some(PathBuf::from("/library/lib-4.mp4.thumb"));
        assert_eq!(
            video.file_drag_payload_path(),
            Some(PathBuf::from("/library/lib-4.mp4"))
        );

        // Extension is not a gate — the grab icon drags the managed payload
        // path stored on the card (never a preview sidecar).
        let mut image_named_wav = entry("mislabelled.wav", None, None);
        image_named_wav.path = PathBuf::from("/library/mislabelled.wav");
        assert_eq!(
            image_named_wav.file_drag_payload_path(),
            Some(PathBuf::from("/library/mislabelled.wav"))
        );

        // The compact run tiles drag their REPRESENTATIVE's managed payload
        // — the final wav/mp4 the run produced, never a preview sidecar.
        let mut text_intermediate = entry("lib-2.txt", Some("run-a"), Some("music"));
        text_intermediate.meta.content_type = "text/plain".into();
        text_intermediate.path = PathBuf::from("/library/lib-2.txt");
        let tiles = history_tiles(vec![wav, video, text_intermediate, image_named_wav]);
        let payloads: Vec<Option<PathBuf>> = tiles
            .iter()
            .map(|tile| tile.entry.file_drag_payload_path())
            .collect();
        assert_eq!(
            payloads,
            vec![
                Some(PathBuf::from("/library/lib-3.wav")),
                Some(PathBuf::from("/library/lib-4.mp4")),
                Some(PathBuf::from("/library/mislabelled.wav")),
            ]
        );
    }

    fn input_asset(entry: &GalleryEntry) -> InputAsset {
        InputAsset {
            file: entry.meta.file.clone(),
            label: entry.meta.label.clone(),
            domain: entry.meta.domain.clone(),
            content_type: entry.meta.content_type.clone(),
            path: entry.path.clone(),
            preview_path: entry.preview_path.clone(),
        }
    }

    #[test]
    fn input_tray_survives_unrelated_changes_and_clears_on_delete() {
        let mut tray = InputTray::default();
        let mut image = entry("lib-2.png", Some("run-a"), Some("video"));
        image.meta.content_type = "image/png".into();
        image.path = PathBuf::from("/library/lib-2.png");
        assert!(tray.select(input_asset(&image)));
        // Re-picking the same asset is a no-op, not a churn event.
        assert!(!tray.select(input_asset(&image)));

        // A NEW artifact landing from a finished run changes the app's
        // selection/viewer — the tray has no API for that, so the pinned
        // input survives untouched (no stale async substitution).
        assert_eq!(tray.current().map(|a| a.file.as_str()), Some("lib-2.png"));

        // The pinned record still exists → sweep keeps it.
        assert!(!tray.retain_existing(|file| file == "lib-2.png"));
        assert_eq!(tray.current().map(|a| a.file.as_str()), Some("lib-2.png"));

        // Its record was deleted (single or whole-group) → tray clears.
        assert!(tray.retain_existing(|_| false));
        assert!(tray.current().is_none());
        // Idempotent afterwards.
        assert!(!tray.retain_existing(|_| false));
        assert!(!tray.clear());
    }

    #[test]
    fn input_tray_extra_references_dedupe_cap_and_sweep() {
        let mut tray = InputTray::default();
        let mk = |name: &str| {
            let mut e = entry(name, Some("run-a"), Some("image"));
            e.meta.content_type = "image/png".into();
            e.path = PathBuf::from(format!("/library/{name}"));
            input_asset(&e)
        };
        // No primary yet: the first "add reference" becomes the primary.
        assert!(tray.add_extra(mk("a.png")));
        assert_eq!(tray.current().map(|a| a.file.as_str()), Some("a.png"));
        assert!(tray.extras().is_empty());
        // Extras dedupe against the primary and each other, and cap.
        assert!(!tray.add_extra(mk("a.png")));
        assert!(tray.add_extra(mk("b.png")));
        assert!(!tray.add_extra(mk("b.png")));
        assert!(tray.add_extra(mk("c.png")));
        assert!(tray.add_extra(mk("d.png")));
        assert!(!tray.add_extra(mk("e.png")), "capped at MAX_EXTRAS");
        assert_eq!(tray.extras().len(), InputTray::MAX_EXTRAS);
        // Promoting an extra to primary removes it from the extras.
        assert!(tray.select(mk("c.png")));
        assert_eq!(tray.current().map(|a| a.file.as_str()), Some("c.png"));
        assert_eq!(
            tray.extras().iter().map(|a| a.file.as_str()).collect::<Vec<_>>(),
            vec!["b.png", "d.png"]
        );
        // Deletion sweep drops only the vanished reference.
        assert!(tray.retain_existing(|file| file != "b.png"));
        assert_eq!(tray.extras().len(), 1);
        assert!(tray.remove_extra(0));
        assert!(!tray.remove_extra(0));
        assert!(tray.clear());
        assert!(tray.current().is_none() && tray.extras().is_empty());
    }

    #[test]
    fn input_tray_pins_the_exact_payload_path_and_type_from_a_history_tile() {
        // Clicking the compact History tile selects the run's REPRESENTATIVE
        // (final) artifact; pinning it as input must carry the exact managed
        // payload path + stored content type — the preview sidecar is
        // display-only.
        let mut final_video = entry("lib-9.mp4", Some("run-a"), Some("video"));
        final_video.meta.content_type = "video/mp4".into();
        final_video.path = PathBuf::from("/library/lib-9.mp4");
        final_video.preview_path = Some(PathBuf::from("/library/lib-9.mp4.thumb"));
        let intermediate = entry("lib-8.png", Some("run-a"), Some("video"));
        let tiles = history_tiles(vec![final_video.clone(), intermediate]);
        assert_eq!(tiles[0].entry.meta.file, "lib-9.mp4");

        let mut tray = InputTray::default();
        tray.select(input_asset(&tiles[0].entry));
        let pinned = tray.current().unwrap();
        assert_eq!(pinned.file, "lib-9.mp4");
        assert_eq!(pinned.content_type, "video/mp4");
        assert_eq!(pinned.path, PathBuf::from("/library/lib-9.mp4"));
        assert_eq!(
            pinned.preview_path.as_deref(),
            Some(std::path::Path::new("/library/lib-9.mp4.thumb"))
        );
    }

    #[test]
    fn file_drag_threshold_is_inclusive_and_one_shot() {
        assert!(!should_start_file_drag(9.999, false));
        assert!(should_start_file_drag(FILE_DRAG_MIN_DISTANCE, false));
        assert!(!should_start_file_drag(50.0, true));
    }

    #[test]
    fn preview_work_classification_keeps_payload_reads_off_the_draw_path() {
        // Image payloads are their own encoded preview.
        let mut image = entry("lib-1.png", None, None);
        image.preview_path = Some(PathBuf::from("/lib/lib-1.png"));
        assert_eq!(
            preview_work(&image),
            Some(PreviewWork::Encoded(PathBuf::from("/lib/lib-1.png")))
        );
        // A persisted sidecar decodes as an encoded preview whatever the kind.
        let mut model = entry("lib-2.glb", None, None);
        model.meta.content_type = "model/gltf-binary".into();
        model.path = PathBuf::from("/lib/lib-2.glb");
        model.preview_path = Some(PathBuf::from("/lib/lib-2.glb.thumb"));
        assert_eq!(
            preview_work(&model),
            Some(PreviewWork::Encoded(PathBuf::from("/lib/lib-2.glb.thumb")))
        );
        // Legacy sidecarless WAV: the PAYLOAD is scanned — on the worker.
        let mut wav = entry("lib-3.wav", None, None);
        wav.meta.content_type = "audio/wav".into();
        wav.path = PathBuf::from("/lib/lib-3.wav");
        wav.preview_path = None;
        assert_eq!(
            preview_work(&wav),
            Some(PreviewWork::WavPayload(PathBuf::from("/lib/lib-3.wav")))
        );
        // A WAV WITH a sidecar decodes the sidecar, never rescans the WAV.
        wav.preview_path = Some(PathBuf::from("/lib/lib-3.wav.thumb"));
        assert_eq!(
            preview_work(&wav),
            Some(PreviewWork::Encoded(PathBuf::from("/lib/lib-3.wav.thumb")))
        );
        // Badge-only kinds have no background work at all.
        let mut text = entry("lib-4.txt", None, None);
        text.meta.content_type = "text/plain".into();
        assert_eq!(preview_work(&text), None);
        // Duke tile PNGs are domain=billboard but the file is the icon.
        let mut tile = entry("lib-72.png", None, None);
        tile.meta.domain = "billboard".into();
        tile.meta.content_type = "image/png".into();
        tile.path = PathBuf::from("/lib/lib-72.png");
        tile.preview_path = Some(PathBuf::from("/lib/lib-72.png"));
        assert_eq!(
            preview_work(&tile),
            Some(PreviewWork::Encoded(PathBuf::from("/lib/lib-72.png")))
        );
        let mut manifest = entry("lib-8.billboard", None, None);
        manifest.meta.domain = "billboard".into();
        manifest.meta.content_type = "text/x-stateful-billboard".into();
        manifest.path = PathBuf::from("/lib/lib-8.billboard");
        assert_eq!(
            preview_work(&manifest),
            Some(PreviewWork::StatefulBillboard(PathBuf::from(
                "/lib/lib-8.billboard"
            )))
        );
    }

    #[test]
    fn upstream_preview_policy_blocks_audio_and_images() {
        // The gorilla-thumbnail bug: a WAV must never inherit a pipeline
        // image. Images are their own preview; everything else may.
        assert!(!upstream_preview_allowed("audio/wav"));
        assert!(!upstream_preview_allowed("AUDIO/WAV"));
        assert!(!upstream_preview_allowed("image/png"));
        assert!(upstream_preview_allowed("model/gltf-binary"));
        assert!(upstream_preview_allowed("video/mp4"));
        assert!(upstream_preview_allowed("text/plain"));
    }

    #[test]
    fn candidate_cards_keep_stable_identity_selection_and_failure_provenance() {
        use crate::pipeline::{CandidateSetState, ImageCandidate};

        let mut ready = ImageCandidate::new(
            "set-1:candidate-a".into(),
            "http://10.0.0.1:8000".into(),
            "flux1-schnell".into(),
            11,
        );
        ready.state = StageState::Done;
        ready.progress = 1.0;
        let mut failed = ImageCandidate::new(
            "set-1:candidate-b".into(),
            "http://10.0.0.2:8000".into(),
            "flux1-dev".into(),
            22,
        );
        failed.state = StageState::Failed("GPU disconnected".into());
        failed.progress = 1.0;
        let set = CandidateSet {
            id: "set-1".into(),
            stage: 0,
            state: CandidateSetState::AwaitingChoice,
            candidates: vec![ready, failed],
            selected: Some("set-1:candidate-a".into()),
            chosen: None,
        };

        let cards = candidate_cards(&set);
        assert_eq!(cards[0].id, "set-1:candidate-a");
        assert!(cards[0].selected);
        assert!(cards[0]
            .meta
            .contains("flux1-schnell @ 10.0.0.1:8000"));
        assert!(cards[0].meta.contains("seed 11"));
        assert_eq!(cards[0].status, "ready · click to preview/select");
        assert!(cards[1].failed);
        assert!(cards[1].status.contains("GPU disconnected"));
        assert!(!cards[1].selected);
    }

    #[test]
    fn grid_columns_track_width_within_slot_bounds() {
        assert_eq!(grid_columns(0.0), 1);
        assert_eq!(grid_columns(GRID_CARD_W), 1);
        assert_eq!(grid_columns(2.0 * GRID_CARD_W + GRID_GAP), 2);
        assert_eq!(grid_columns(4.0 * (GRID_CARD_W + GRID_GAP)), 4);
        assert_eq!(grid_columns(10_000.0), GRID_MAX_COLS);
    }

    #[test]
    fn format_bytes_is_compact() {
        assert_eq!(format_bytes(42), "42 B");
        assert_eq!(format_bytes(120 * 1024), "120 KB");
        assert_eq!(format_bytes(34 * 1024 * 1024), "34.0 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5.00 GB");
    }
}
