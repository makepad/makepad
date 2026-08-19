//! Per-surface catalog browse model: search → tiles → resolve pipeline.
//!
//! Pure command/completion state machine (no sockets): the app maps the
//! returned [`CatCmd`]s onto `ClientRuntime` requests and feeds typed
//! completions back, each stamped with the generation that issued it.
//!
//! Guarantees:
//! - deterministic pagination: server cursors chain page after page under
//!   one query generation,
//! - latest-query-wins: every query change/refresh bumps the generation and
//!   all older completions are ignored,
//! - refreshes double-buffer: the old tile list stays on screen until the
//!   new first page arrives (live generators republish constantly),
//! - tile → revision → manifest resolution is bounded (at most
//!   [`MAX_RESOLVING`] in flight) and keyed by exact asset+revision, so a
//!   virtualized grid can never show another asset's thumbnail: textures are
//!   keyed by the immutable revision, never by list position.

use makepad_asset_client::{CatalogQuery, PageCursor};
use makepad_asset_data::{AssetId, AssetKind, AssetRevisionId, BlobId, MediaType};
use std::collections::{HashMap, VecDeque};

pub type CatGen = u64;

/// Most tile-resolve pipelines (detail + manifest) in flight at once.
pub const MAX_RESOLVING: usize = 4;
/// Search page size (server pages deterministically under this).
pub const PAGE_SIZE: u32 = 48;
/// Catalog tag the asset-ui puts on non-product run artifacts.
pub const INTERMEDIATE_TAG: &str = "intermediate";

/// The playable file a tile's manifest selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileMedia {
    pub blob: BlobId,
    pub len: u64,
    pub media: MediaType,
}

/// The manifest's typed thumbnail blob (immutable per revision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileThumb {
    pub blob: BlobId,
    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileState {
    /// Hit only; revision not resolved yet.
    Listed,
    Resolving,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub asset: AssetId,
    pub title: String,
    pub alias: Option<String>,
    pub live: bool,
    /// Catalog kind of the hit (the lane's kind when the server omits it).
    /// Playback decisions that must not be guessed from pixels — sheet vs
    /// still, sprite actor vs texture — key on this.
    pub kind: Option<AssetKind>,
    pub revision: Option<AssetRevisionId>,
    pub media: Option<TileMedia>,
    /// Companion file the primary needs to be playable: for a grouped
    /// `Billboard` actor, the `stateful-billboard` manifest that cuts its
    /// packed sheet into frames and states.
    pub source: Option<TileMedia>,
    pub thumb: Option<TileThumb>,
    pub state: TileState,
}

/// Whether a tile's THUMBNAIL may be a packed animation strip. Decided by
/// catalog kind, never by pixel dimensions: a 1024² PBR map or Flux still is
/// dimensionally a 64-tile sheet and must never cycle, while a sprite actor's
/// 128²-tile strip must.
pub fn kind_may_be_sheet(kind: Option<AssetKind>) -> bool {
    matches!(
        kind,
        Some(
            AssetKind::Billboard
                | AssetKind::Mesh
                | AssetKind::Character
                | AssetKind::Prop
                | AssetKind::Weapon
                | AssetKind::Vehicle
        )
    )
}

/// Hide the pre-grouping per-lump sprite assets (`…/billboards/<wad>/trooa1`,
/// one frame each, no `stateful` companion) once the same actors are
/// published as one `Billboard` per prefix (`…/billboards/<wad>/troo`).
/// Flip to `false` — or delete the one call in `grid_entries` — when the
/// server retires the legacy rows.
pub const HIDE_LEGACY_LUMP_SPRITES: bool = true;

/// True for a legacy per-lump sprite: a `Billboard` whose alias segment is a
/// Doom lump name (4-char prefix + letter/rotation pairs, e.g. `trooa2a8`)
/// and which carries no stateful manifest. A grouped actor's alias ends in
/// the bare 4-char prefix, so the two shapes never collide.
pub fn is_legacy_lump_sprite(
    kind: Option<AssetKind>,
    alias: Option<&str>,
    has_stateful_source: bool,
) -> bool {
    if kind != Some(AssetKind::Billboard) || has_stateful_source {
        return false;
    }
    let Some(alias) = alias else { return false };
    if !alias.contains("/billboards/") {
        return false;
    }
    let Some(name) = alias.rsplit('/').next() else { return false };
    is_doom_lump_name(name)
}

/// `trooa1`, `trooa2a8`, `posse1` — a 4-character sprite prefix followed by
/// one or more (letter, rotation-digit) pairs.
fn is_doom_lump_name(name: &str) -> bool {
    if name.len() < 6 || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let bytes = name.as_bytes();
    if !bytes[..4].iter().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let rest = &bytes[4..];
    if rest.len() % 2 != 0 {
        return false;
    }
    rest.chunks_exact(2)
        .all(|pair| pair[0].is_ascii_alphabetic() && pair[1].is_ascii_digit())
}

/// Generic over the pagination cursor so the model is hermetically testable:
/// the app instantiates with the client's server-bound [`PageCursor`], tests
/// with a plain value ([`PageCursor`] is deliberately unconstructible outside
/// the client crate).
#[derive(Clone, Debug, PartialEq)]
pub enum CatCmd<C: Clone = PageCursor> {
    SearchPage { gen: CatGen, slot: usize, query: CatalogQuery, cursor: Option<C>, first: bool },
    FetchDetail { gen: CatGen, asset: AssetId },
    FetchManifest { gen: CatGen, asset: AssetId, revision: AssetRevisionId },
    /// Fetch the immutable thumbnail blob (texture cache key = revision).
    FetchThumb { asset: AssetId, revision: AssetRevisionId, blob: BlobId, len: u64 },
}

/// A search hit as the model needs it (projected from `CatalogHit`).
#[derive(Clone, Debug)]
pub struct HitRow {
    pub asset: AssetId,
    pub title: String,
    pub alias: Option<String>,
    pub live: bool,
    /// Kind as the server reported it; `None` falls back to the lane's kind
    /// (each lane is an exact-kind search).
    pub kind: Option<AssetKind>,
}

/// The four buckets an operator actually thinks in, and the ones the Asset
/// UI's Library shelves reduce to. HOT PRESETS: picking one SETS the filter
/// rather than toggling a lane, which is what makes them feel like buttons
/// on a deck instead of a checkbox farm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Video,
    /// Meshes, props, vehicles, weapons, characters, MAPS and 3D scenes
    /// (splats) — everything the slot renders in three dimensions.
    ThreeD,
    Image,
    Audio,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Preset::Video => "VIDEO",
            Preset::ThreeD => "3D",
            Preset::Image => "IMAGE",
            Preset::Audio => "AUDIO",
        }
    }

    /// Catalog lanes to ask the server for.
    pub fn kinds(self) -> Vec<AssetKind> {
        match self {
            Preset::Video => vec![AssetKind::Video],
            Preset::ThreeD => vec![
                AssetKind::Mesh,
                AssetKind::Character,
                AssetKind::Prop,
                AssetKind::Weapon,
                AssetKind::Vehicle,
                AssetKind::World,
            ],
            Preset::Image => vec![AssetKind::Texture, AssetKind::Billboard],
            Preset::Audio => vec![AssetKind::Audio],
        }
    }
}

/// Which shelf a catalog row belongs on, classified the way the Asset UI's
/// Library does it — by what the thing IS, not by the lane it arrived in.
///
/// The distinction that matters: `AssetKind::World` covers BOTH a Doom map
/// and a Gaussian splat, so the kind alone never decides. A splat publishes
/// with category `splat`; a map's alias runs through `…/worlds/…` (or its
/// category says so). Reading "kind = World" as "splat" is what put E1M1 in
/// the splat shelf.
pub fn shelf_of(kind: Option<AssetKind>, alias: Option<&str>, category: Option<&str>) -> &'static str {
    let cat = category.unwrap_or("").to_ascii_lowercase();
    let path = alias.unwrap_or("").to_ascii_lowercase();
    let in_path = |seg: &str| path.split('/').any(|p| p == seg);
    match kind {
        Some(AssetKind::Video) => "video",
        Some(AssetKind::Texture) => "image",
        Some(AssetKind::Billboard) => "sprite",
        Some(AssetKind::Character) => "character",
        Some(AssetKind::Prop) => "prop",
        Some(AssetKind::Weapon) => "weapon",
        Some(AssetKind::Vehicle) => "vehicle",
        // One catalog lane, two shelves: the split is by tag/alias, the way
        // the Library does it, never by kind.
        Some(AssetKind::Audio) => {
            if cat == "music" || in_path("music") || path.contains("music") {
                "music"
            } else {
                "sfx"
            }
        }
        Some(AssetKind::World) => {
            if cat == "splat" {
                "3D scene"
            } else if cat == "map" || cat == "world" || in_path("worlds") || in_path("maps") {
                "map"
            } else {
                "3D scene"
            }
        }
        Some(AssetKind::Mesh) => {
            if cat == "splat" {
                "3D scene"
            } else {
                "mesh"
            }
        }
        _ => "other",
    }
}

/// Which hot preset a row answers to.
pub fn preset_of(
    kind: Option<AssetKind>,
    alias: Option<&str>,
    category: Option<&str>,
) -> Option<Preset> {
    match shelf_of(kind, alias, category) {
        "video" => Some(Preset::Video),
        "image" | "sprite" => Some(Preset::Image),
        "music" | "sfx" => Some(Preset::Audio),
        "mesh" | "character" | "prop" | "weapon" | "vehicle" | "map" | "3D scene" => {
            Some(Preset::ThreeD)
        }
        _ => None,
    }
}

pub struct BrowseModel<C: Clone = PageCursor> {
    /// Exact kinds this surface shows (1..=2, e.g. Mesh+Character for the
    /// dance lane). One bounded search per kind; results merge by asset.
    pub kinds: Vec<AssetKind>,
    pub text: String,
    pub category: String,
    gen: CatGen,
    tiles: Vec<Tile>,
    index: HashMap<AssetId, usize>,
    pub total: u64,
    next_cursors: Vec<Option<C>>,
    /// Lanes whose cursor the server declared stale (the index mutated
    /// under it — an import landing): paging restarts from page one in the
    /// SAME generation, known hits are skipped, and pages keep coming until
    /// one adds something new. The loaded window and the operator's scroll
    /// position survive a catalog that is being imported into.
    restarting: Vec<bool>,
    pages_pending: usize,
    /// Tiles were already replaced for this generation's first pages.
    cleared_gen: CatGen,
    /// Resolved state of the tiles a refresh replaced, by asset. A refresh
    /// re-lists the catalog; it must not throw away manifests/thumbnails we
    /// already hold for assets that are still listed — otherwise a stream
    /// of publish events (an import running) keeps the grid forever blank.
    carry: HashMap<AssetId, Tile>,
    resolve_queue: VecDeque<AssetId>,
    resolving: usize,
    pub error: Option<String>,
    /// Raised by catalog events; the app refreshes on its debounce tick.
    pub refresh_wanted: bool,
    /// Display order of the SETTLED body. Once an asset has a place here
    /// it keeps it until the next re-sort — the operator's hand is on a
    /// pad, and a grid that renumbers itself under that hand is a grid
    /// that fires the wrong clip.
    order: Vec<AssetId>,
    /// The PENDING head column: assets the generators published while the
    /// operator was watching, filling the leftmost column top to bottom.
    /// It merges into `order` when it fills (a new empty column starts) or
    /// on the next re-sort. Never longer than [`PENDING_COLUMN`].
    pending: Vec<AssetId>,
    /// The next arriving first page re-sorts the body. Set by a real query
    /// change (text, category, kinds) and by an explicit re-sort — never by
    /// the event-driven refresh that a publish triggers.
    resort: bool,
}

/// Rows in one grid column — the height of the PENDING head column. Must
/// match the pad matrix's row count (asserted in the tests).
pub const PENDING_COLUMN: usize = 5;

impl<C: Clone> BrowseModel<C> {
    /// `category` empty = no category filter.
    pub fn new(kind: AssetKind, category: &str) -> BrowseModel<C> {
        Self::new_multi(vec![kind], category)
    }

    /// The dance lane: generated skinned dancers publish as Mesh OR
    /// Character; the surface shows both.
    pub fn dance() -> BrowseModel<C> {
        Self::new_multi(vec![AssetKind::Mesh, AssetKind::Character], "")
    }

    /// Every kind that can land on A/B: video clips, stills, 3D, sprites
    /// and splat/world scenes.
    pub fn visual_kinds() -> Vec<AssetKind> {
        vec![
            AssetKind::Video,
            AssetKind::Mesh,
            AssetKind::Character,
            AssetKind::Prop,
            AssetKind::Weapon,
            AssetKind::Vehicle,
            AssetKind::Texture,
            AssetKind::Billboard,
            AssetKind::World,
        ]
    }

    /// Program pads: all visual kinds.
    pub fn visual() -> BrowseModel<C> {
        Self::new_multi(Self::visual_kinds(), "")
    }

    /// Change the kind lanes (kind chips) and re-query from page one.
    pub fn set_kinds(&mut self, kinds: Vec<AssetKind>) -> Vec<CatCmd<C>> {
        let kinds = if kinds.is_empty() { Self::visual_kinds() } else { kinds };
        if self.kinds == kinds {
            return Vec::new();
        }
        self.kinds = kinds;
        self.next_cursors = vec![None; self.kinds.len()];
        self.refresh()
    }

    /// Bounded multi-kind surface (each kind is a separate exact-filter
    /// search; pages merge, deduped by asset).
    pub fn new_multi(kinds: Vec<AssetKind>, category: &str) -> BrowseModel<C> {
        let kinds = if kinds.is_empty() { vec![AssetKind::Video] } else { kinds };
        let lanes = kinds.len();
        BrowseModel {
            kinds,
            text: String::new(),
            category: category.to_string(),
            gen: 0,
            tiles: Vec::new(),
            index: HashMap::new(),
            total: 0,
            next_cursors: vec![None; lanes],
            restarting: vec![false; lanes],
            pages_pending: 0,
            cleared_gen: 0,
            carry: HashMap::new(),
            resolve_queue: VecDeque::new(),
            resolving: 0,
            error: None,
            refresh_wanted: false,
            order: Vec::new(),
            pending: Vec::new(),
            // The first page of a fresh model IS the sort.
            resort: true,
        }
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn tile(&self, asset: &AssetId) -> Option<&Tile> {
        self.index.get(asset).map(|&i| &self.tiles[i])
    }

    pub fn has_more(&self) -> bool {
        self.next_cursors.iter().any(Option::is_some)
    }

    pub fn is_loading(&self) -> bool {
        self.pages_pending > 0
    }

    fn query(&self, slot: usize) -> CatalogQuery {
        let mut q = CatalogQuery::text(self.text.clone(), PAGE_SIZE);
        q.kind = Some(self.kinds[slot.min(self.kinds.len() - 1)]);
        if !self.category.is_empty() {
            q.category = Some(self.category.clone());
        }
        // Program surfaces show a run's PRODUCT only. The asset-ui tags the
        // source image, the untextured mesh, mattes and PBR maps of a
        // generated model `intermediate`; the server drops them — the pads
        // never receive (and never sift) a full dump.
        q.exclude_tag = Some(INTERMEDIATE_TAG.to_string());
        q
    }

    /// New query text/category, or an event-driven refresh: bump the
    /// generation (all in-flight completions die) and request page one. The
    /// visible tiles stay until that page arrives.
    pub fn refresh(&mut self) -> Vec<CatCmd<C>> {
        self.resort = true;
        self.merge_pending();
        self.refresh_keeping_order()
    }

    /// Re-list without re-sorting: what a publish event triggers. Tiles the
    /// operator can already see keep their cells; anything new lands in the
    /// PENDING head column.
    pub fn refresh_event(&mut self) -> Vec<CatCmd<C>> {
        self.resort = false;
        self.refresh_keeping_order()
    }

    /// Fold the head column into the body and start an empty one. Called
    /// when the column fills and before every re-sort.
    fn merge_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut merged = std::mem::take(&mut self.pending);
        merged.extend(self.order.drain(..));
        self.order = merged;
    }

    /// The head column plus the body, in display order. `None` is a
    /// reserved-but-empty pending cell: the column keeps its full height
    /// from the moment it opens, so filling it never moves the body.
    pub fn display_order(&self) -> Vec<Option<AssetId>> {
        let mut out: Vec<Option<AssetId>> = Vec::with_capacity(
            if self.pending.is_empty() { 0 } else { PENDING_COLUMN } + self.order.len(),
        );
        if !self.pending.is_empty() {
            for slot in 0..PENDING_COLUMN {
                out.push(self.pending.get(slot).copied());
            }
        }
        out.extend(self.order.iter().map(|a| Some(*a)));
        out
    }

    /// How many freshly published tiles are waiting in the head column.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn refresh_keeping_order(&mut self) -> Vec<CatCmd<C>> {
        self.gen += 1;
        self.error = None;
        self.refresh_wanted = false;
        self.next_cursors = vec![None; self.kinds.len()];
        self.restarting = vec![false; self.kinds.len()];
        // In-flight resolves are all stale now.
        self.resolve_queue.clear();
        self.resolving = 0;
        self.pages_pending = self.kinds.len();
        (0..self.kinds.len())
            .map(|slot| CatCmd::SearchPage {
                gen: self.gen,
                slot,
                query: self.query(slot),
                cursor: None,
                first: true,
            })
            .collect()
    }

    pub fn set_text(&mut self, text: String) -> Vec<CatCmd<C>> {
        if self.text == text {
            return Vec::new();
        }
        self.text = text;
        self.refresh()
    }

    pub fn set_category(&mut self, category: String) -> Vec<CatCmd<C>> {
        if self.category == category {
            return Vec::new();
        }
        self.category = category;
        self.refresh()
    }

    pub fn load_more(&mut self) -> Vec<CatCmd<C>> {
        if self.pages_pending > 0 {
            return Vec::new();
        }
        let mut cmds = Vec::new();
        for slot in 0..self.kinds.len() {
            let Some(cursor) = self.next_cursors[slot].clone() else { continue };
            self.pages_pending += 1;
            cmds.push(CatCmd::SearchPage {
                gen: self.gen,
                slot,
                query: self.query(slot),
                cursor: Some(cursor),
                first: false,
            });
        }
        cmds
    }

    /// A search page arrived. Stale generations are dropped whole.
    pub fn page_arrived(
        &mut self,
        gen: CatGen,
        slot: usize,
        first: bool,
        hits: Vec<HitRow>,
        total: u64,
        next: Option<C>,
    ) -> Vec<CatCmd<C>> {
        if gen != self.gen || slot >= self.kinds.len() {
            return Vec::new();
        }
        self.pages_pending = self.pages_pending.saturating_sub(1);
        if first {
            // Totals sum across kind lanes; reset on the generation's first
            // arriving page.
            if self.cleared_gen != gen {
                self.total = 0;
            }
            self.total += total;
        }
        self.next_cursors[slot] = next;
        if first && self.cleared_gen != gen {
            // Double-buffered swap: the FIRST first-page of a generation
            // replaces the old tiles; the other kind lane then merges in.
            // Resolved tiles are carried, not dropped.
            self.cleared_gen = gen;
            self.carry = self.tiles.drain(..).map(|t| (t.asset, t)).collect();
            self.index.clear();
            if self.resort {
                // A real query change: the whole strip is re-derived from
                // the server's order.
                self.order.clear();
                self.pending.clear();
            }
        }
        let mut added = 0usize;
        for hit in hits {
            if self.index.contains_key(&hit.asset) {
                continue; // keyset pages should not repeat; drop dupes anyway
            }
            added += 1;
            self.place(hit.asset);
            self.index.insert(hit.asset, self.tiles.len());
            let kind = hit.kind.or_else(|| self.kinds.get(slot).copied());
            match self.carry.remove(&hit.asset) {
                Some(mut known) if known.state == TileState::Ready => {
                    known.title = hit.title;
                    known.alias = hit.alias;
                    known.live = hit.live;
                    known.kind = kind;
                    self.tiles.push(known);
                }
                _ => {
                    self.tiles.push(Tile {
                        asset: hit.asset,
                        title: hit.title,
                        alias: hit.alias,
                        live: hit.live,
                        kind,
                        revision: None,
                        media: None,
                        source: None,
                        thumb: None,
                        state: TileState::Listed,
                    });
                    self.resolve_queue.push_back(hit.asset);
                }
            }
        }
        // Anything the server no longer lists is gone from the strip, but
        // only a re-sort is allowed to close the gap — otherwise a delete
        // shuffles every pad under the operator's hand.
        if self.resort {
            let listed = &self.index;
            self.order.retain(|asset| listed.contains_key(asset));
            self.pending.retain(|asset| listed.contains_key(asset));
        }
        let mut cmds = self.pump_resolves();
        if self.restarting[slot] {
            // Re-walking after a stale cursor: a page of only-known hits is
            // not the frontier yet — keep going; the first page that adds
            // something is.
            if added == 0 && self.next_cursors[slot].is_some() {
                self.pages_pending += 1;
                cmds.push(CatCmd::SearchPage {
                    gen: self.gen,
                    slot,
                    query: self.query(slot),
                    cursor: self.next_cursors[slot].clone(),
                    first: false,
                });
            } else {
                self.restarting[slot] = false;
            }
        }
        cmds
    }

    /// Give a newly listed asset a cell.
    ///
    /// On a re-sort it simply appends, so the strip follows the server's
    /// order. Otherwise it is a tile that appeared while the operator was
    /// watching — a generator publishing — and it goes into the PENDING
    /// head column instead, where it cannot move anything that is already
    /// on screen. A full column folds into the body and a fresh, empty one
    /// opens; that fold is the ONE moment the body shifts, and it costs a
    /// whole column, not a tile.
    fn place(&mut self, asset: AssetId) {
        if self.order.contains(&asset) || self.pending.contains(&asset) {
            return;
        }
        if self.resort {
            self.order.push(asset);
            return;
        }
        self.pending.push(asset);
        if self.pending.len() >= PENDING_COLUMN {
            self.merge_pending();
        }
    }

    /// True for the server's "the index changed under your cursor" refusal.
    pub fn is_stale_cursor(error: &str) -> bool {
        error.contains("stale search cursor")
    }

    /// A page request failed. A stale cursor is not an operator-visible
    /// error: the lane re-walks from page one (see `restarting`).
    pub fn page_failed(&mut self, gen: CatGen, slot: usize, error: String) -> Vec<CatCmd<C>> {
        if gen != self.gen {
            return Vec::new();
        }
        self.pages_pending = self.pages_pending.saturating_sub(1);
        if Self::is_stale_cursor(&error) && slot < self.kinds.len() {
            self.next_cursors[slot] = None;
            self.restarting[slot] = true;
            self.pages_pending += 1;
            return vec![CatCmd::SearchPage {
                gen: self.gen,
                slot,
                query: self.query(slot),
                cursor: None,
                first: false,
            }];
        }
        self.error = Some(error);
        Vec::new()
    }

    /// Start queued tile resolves up to the bound.
    fn pump_resolves(&mut self) -> Vec<CatCmd<C>> {
        let mut cmds = Vec::new();
        while self.resolving < MAX_RESOLVING {
            let Some(asset) = self.resolve_queue.pop_front() else { break };
            let Some(&i) = self.index.get(&asset) else { continue };
            self.tiles[i].state = TileState::Resolving;
            self.resolving += 1;
            cmds.push(CatCmd::FetchDetail { gen: self.gen, asset });
        }
        cmds
    }

    fn resolve_done(&mut self) {
        self.resolving = self.resolving.saturating_sub(1);
    }

    /// Move an unresolved tile to the front of the resolve queue (the
    /// operator clicked it) and start it if a slot is free. A tile that
    /// is already resolving or resolved is left alone.
    pub fn resolve_first(&mut self, asset: AssetId) -> Vec<CatCmd<C>> {
        let Some(&i) = self.index.get(&asset) else { return Vec::new() };
        if self.tiles[i].state != TileState::Listed {
            return Vec::new();
        }
        self.resolve_queue.retain(|a| *a != asset);
        self.resolve_queue.push_front(asset);
        self.pump_resolves()
    }

    /// Asset detail arrived: pick the latest published revision.
    pub fn detail_arrived(
        &mut self,
        gen: CatGen,
        asset: AssetId,
        latest_published: Option<AssetRevisionId>,
    ) -> Vec<CatCmd<C>> {
        if gen != self.gen {
            return Vec::new();
        }
        self.resolve_done();
        let mut cmds = Vec::new();
        if let Some(&i) = self.index.get(&asset) {
            match latest_published {
                Some(revision) => {
                    self.tiles[i].revision = Some(revision);
                    self.resolving += 1;
                    cmds.push(CatCmd::FetchManifest { gen: self.gen, asset, revision });
                }
                None => {
                    self.tiles[i].state =
                        TileState::Failed("no published revision".to_string());
                }
            }
        }
        cmds.extend(self.pump_resolves());
        cmds
    }

    /// Manifest arrived (already digest-verified and decoded by the client);
    /// the app passes the selected playable file, its companion source file
    /// (grouped sprite actors) + thumbnail meta.
    pub fn manifest_arrived(
        &mut self,
        gen: CatGen,
        asset: AssetId,
        revision: AssetRevisionId,
        media: Option<TileMedia>,
        source: Option<TileMedia>,
        thumb: Option<TileThumb>,
    ) -> Vec<CatCmd<C>> {
        if gen != self.gen {
            return Vec::new();
        }
        self.resolve_done();
        let mut cmds = Vec::new();
        if let Some(&i) = self.index.get(&asset) {
            let tile = &mut self.tiles[i];
            // Identity guard: the completion must describe the revision this
            // tile currently resolves to.
            if tile.revision == Some(revision) {
                tile.thumb = thumb.clone();
                tile.source = source;
                match media {
                    Some(media) => {
                        tile.media = Some(media);
                        tile.state = TileState::Ready;
                    }
                    None => {
                        tile.state = TileState::Failed("no playable file".to_string());
                    }
                }
                if let Some(thumb) = thumb {
                    cmds.push(CatCmd::FetchThumb {
                        asset,
                        revision,
                        blob: thumb.blob,
                        len: thumb.len,
                    });
                }
            }
        }
        cmds.extend(self.pump_resolves());
        cmds
    }

    /// Any resolve-chain step failed for this asset.
    pub fn resolve_failed(&mut self, gen: CatGen, asset: AssetId, error: String) -> Vec<CatCmd<C>> {
        if gen != self.gen {
            return Vec::new();
        }
        self.resolve_done();
        if let Some(&i) = self.index.get(&asset) {
            self.tiles[i].state = TileState::Failed(error);
        }
        self.pump_resolves()
    }

    /// A committed catalog event touched this surface's kind (or an unknown
    /// kind): schedule a debounced refresh.
    pub fn event_touch(&mut self, content_kind: Option<AssetKind>) {
        match content_kind {
            None => self.refresh_wanted = true,
            Some(k) if self.kinds.contains(&k) => self.refresh_wanted = true,
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(seed: u8) -> HitRow {
        HitRow {
            asset: AssetId::from_bytes([seed; 16]),
            title: format!("asset {seed}"),
            alias: None,
            live: true,
            kind: None,
        }
    }

    fn rev(seed: u8) -> AssetRevisionId {
        AssetRevisionId::from_bytes([seed; 32])
    }

    fn media(seed: u8) -> TileMedia {
        TileMedia { blob: BlobId::from_bytes([seed; 32]), len: 10, media: MediaType::Mp4 }
    }

    fn search_gen<C: Clone>(cmds: &[CatCmd<C>]) -> CatGen {
        cmds.iter()
            .find_map(|c| match c {
                CatCmd::SearchPage { gen, .. } => Some(*gen),
                _ => None,
            })
            .expect("search cmd")
    }

    #[test]
    fn query_change_invalidates_in_flight_completions() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g1 = search_gen(&m.refresh());
        // Old page arrives after the query changed: dropped whole.
        let g2 = search_gen(&m.set_text("fire".into()));
        assert!(g2 > g1);
        assert!(m.page_arrived(g1, 0, true, vec![hit(1)], 1, None).is_empty());
        assert!(m.tiles().is_empty());
        // The current generation lands.
        let cmds = m.page_arrived(g2, 0, true, vec![hit(2)], 1, None);
        assert_eq!(m.tiles().len(), 1);
        assert!(matches!(cmds[0], CatCmd::FetchDetail { gen, .. } if gen == g2));
        // Stale detail/manifest completions are ignored too.
        assert!(m.detail_arrived(g1, hit(2).asset, Some(rev(9))).is_empty());
        assert!(m
            .manifest_arrived(g1, hit(2).asset, rev(9), Some(media(1)), None, None)
            .is_empty());
        assert_eq!(m.tiles()[0].state, TileState::Resolving);
    }

    #[test]
    fn refresh_keeps_old_tiles_until_first_page_arrives() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g1 = search_gen(&m.refresh());
        m.page_arrived(g1, 0, true, vec![hit(1), hit(2)], 2, None);
        assert_eq!(m.tiles().len(), 2);
        // Event-driven refresh: tiles stay while the new page loads.
        let g2 = search_gen(&m.refresh());
        assert_eq!(m.tiles().len(), 2, "double-buffered refresh");
        m.page_arrived(g2, 0, true, vec![hit(2), hit(3)], 2, None);
        let names: Vec<_> = m.tiles().iter().map(|t| t.title.clone()).collect();
        assert_eq!(names, vec!["asset 2", "asset 3"]);
    }

    #[test]
    fn stale_cursor_rewalks_the_lane_and_keeps_the_loaded_window() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        m.page_arrived(g, 0, true, vec![hit(1), hit(2)], 5, Some(1u8));
        m.page_arrived(g, 0, false, vec![hit(3), hit(4)], 5, Some(2u8));
        assert_eq!(m.tiles().len(), 4);
        let _ = m.load_more();
        // The import landed under the cursor: the server refuses page three.
        let cmds = m.page_failed(g, 0, "server refused: 400 invalid input: stale search cursor".into());
        assert!(m.error.is_none(), "a stale cursor is not an operator error");
        assert!(matches!(&cmds[0], CatCmd::SearchPage { slot: 0, cursor: None, first: false, .. }));
        // Page one again: nothing new → keep walking; the window is intact.
        let cmds = m.page_arrived(g, 0, false, vec![hit(1), hit(2)], 6, Some(11u8));
        assert_eq!(m.tiles().len(), 4);
        assert!(matches!(&cmds[0], CatCmd::SearchPage { slot: 0, cursor: Some(11), .. }));
        // Page two carries a fresh hit: the frontier is found, walking stops.
        let cmds = m.page_arrived(g, 0, false, vec![hit(3), hit(4), hit(5)], 6, Some(12u8));
        assert_eq!(m.tiles().len(), 5);
        assert!(!cmds.iter().any(|c| matches!(c, CatCmd::SearchPage { .. })));
        assert!(m.has_more());
        // A real refusal still surfaces.
        m.page_failed(g, 0, "server refused: 400 invalid input: bad".into());
        assert!(m.error.is_some());
    }

    /// The law the operator's hands depend on: a tile that is on screen
    /// keeps its cell. Publishes land in a reserved head column and fill it
    /// top to bottom; the body only moves when that column is full (one
    /// shift per five arrivals instead of one per arrival) or on a re-sort.
    #[test]
    fn published_tiles_fill_a_head_column_without_moving_the_body() {
        let mut m: BrowseModel = BrowseModel::new(AssetKind::Video, "");
        m.refresh();
        // Six settled tiles.
        m.page_arrived(1, 0, true, (1..=6).map(hit).collect(), 6, None);
        let body: Vec<AssetId> = (1..=6).map(|s| hit(s).asset).collect();
        assert_eq!(m.display_order(), body.iter().map(|a| Some(*a)).collect::<Vec<_>>());

        // A publish event re-lists with two new assets at the FRONT (what
        // a newest-first server returns).
        let cmds = m.refresh_event();
        let gen = search_gen(&cmds);
        let listed: Vec<HitRow> = [7u8, 8].iter().chain(&[1, 2, 3, 4, 5, 6]).map(|s| hit(*s)).collect();
        m.page_arrived(gen, 0, true, listed, 8, None);

        let shown = m.display_order();
        assert_eq!(m.pending_len(), 2);
        // A whole column is reserved from the moment it opens, so the body
        // sits at a fixed offset while the column fills.
        assert_eq!(shown.len(), PENDING_COLUMN + 6);
        assert_eq!(shown[0], Some(hit(7).asset));
        assert_eq!(shown[1], Some(hit(8).asset));
        assert_eq!(&shown[2..PENDING_COLUMN], &[None, None, None]);
        assert_eq!(&shown[PENDING_COLUMN..], &body.iter().map(|a| Some(*a)).collect::<Vec<_>>()[..]);

        // Three more publishes fill the column exactly; it folds into the
        // body and a fresh empty one opens.
        let cmds = m.refresh_event();
        let gen = search_gen(&cmds);
        let listed: Vec<HitRow> = [9u8, 10, 11, 7, 8].iter().chain(&[1, 2, 3, 4, 5, 6]).map(|s| hit(*s)).collect();
        m.page_arrived(gen, 0, true, listed, 11, None);
        assert_eq!(m.pending_len(), 0, "a full column folds into the body");
        let shown = m.display_order();
        assert_eq!(shown.len(), 11, "no reserved column while none is open");
        // The five that arrived while watching lead, in arrival order, and
        // the original six are still in their original order behind them.
        let head: Vec<AssetId> = shown[..5].iter().map(|a| a.unwrap()).collect();
        assert_eq!(head.len(), 5);
        assert_eq!(&shown[5..], &body.iter().map(|a| Some(*a)).collect::<Vec<_>>()[..]);
    }

    /// A query change is a re-sort: the strip is rebuilt in the server's
    /// order and any open head column is folded away first.
    #[test]
    fn a_query_change_resorts_and_retires_the_head_column() {
        let mut m: BrowseModel = BrowseModel::new(AssetKind::Video, "");
        m.refresh();
        m.page_arrived(1, 0, true, (1..=3).map(hit).collect(), 3, None);
        let cmds = m.refresh_event();
        let gen = search_gen(&cmds);
        m.page_arrived(gen, 0, true, [9u8, 1, 2, 3].iter().map(|s| hit(*s)).collect(), 4, None);
        assert_eq!(m.pending_len(), 1);

        // Typing in the filter re-sorts.
        let cmds = m.set_text("cats".into());
        let gen = search_gen(&cmds);
        assert_eq!(m.pending_len(), 0);
        m.page_arrived(gen, 0, true, [3u8, 9, 1].iter().map(|s| hit(*s)).collect(), 3, None);
        assert_eq!(
            m.display_order(),
            vec![Some(hit(3).asset), Some(hit(9).asset), Some(hit(1).asset)],
            "a re-sort follows the server's order exactly"
        );
        // A tile the server dropped is gone after a re-sort...
        let cmds = m.refresh();
        let gen = search_gen(&cmds);
        m.page_arrived(gen, 0, true, [3u8, 1].iter().map(|s| hit(*s)).collect(), 2, None);
        assert_eq!(m.display_order(), vec![Some(hit(3).asset), Some(hit(1).asset)]);
    }

    /// A delete arriving on an EVENT refresh must not close the gap: the
    /// pads under the operator's hand keep their numbers until a re-sort.
    #[test]
    fn an_event_refresh_never_renumbers_the_body() {
        let mut m: BrowseModel = BrowseModel::new(AssetKind::Video, "");
        m.refresh();
        m.page_arrived(1, 0, true, (1..=4).map(hit).collect(), 4, None);
        let before = m.display_order();
        let cmds = m.refresh_event();
        let gen = search_gen(&cmds);
        // The server no longer lists asset 2.
        m.page_arrived(gen, 0, true, [1u8, 3, 4].iter().map(|s| hit(*s)).collect(), 3, None);
        assert_eq!(m.display_order(), before, "cells are frozen between re-sorts");
    }

    /// The reserved column has to be exactly as tall as a grid column, or
    /// the body lands mid-column and every tile still moves.
    #[test]
    fn the_head_column_is_one_grid_column_tall() {
        assert_eq!(PENDING_COLUMN, crate::views::PAD_ROWS);
    }

    #[test]
    fn pagination_appends_deterministically_and_dedupes() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        // No cursor yet: load_more is a no-op while page one is pending.
        assert!(m.load_more().is_empty());
        m.page_arrived(g, 0, true, vec![hit(1), hit(2)], 4, Some(77u8));
        assert!(m.has_more());
        let cmds = m.load_more();
        assert!(matches!(
            &cmds[0],
            CatCmd::SearchPage { gen, slot: 0, cursor: Some(77), first: false, .. } if *gen == g
        ));
        // Page two appends; a repeated hit is dropped.
        m.page_arrived(g, 0, false, vec![hit(2), hit(3), hit(4)], 4, None);
        assert_eq!(m.tiles().len(), 4);
        assert!(!m.has_more());
        assert!(m.load_more().is_empty());
    }

    #[test]
    fn resolve_pipeline_is_bounded_and_identity_guarded() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Audio, "music");
        let g = search_gen(&m.refresh());
        let hits: Vec<HitRow> = (1..=7).map(hit).collect();
        let cmds = m.page_arrived(g, 0, true, hits, 7, None);
        // Only MAX_RESOLVING details go out.
        assert_eq!(cmds.len(), MAX_RESOLVING);
        // Completing one admits the next.
        let a1 = hit(1).asset;
        let cmds = m.detail_arrived(g, a1, Some(rev(1)));
        assert!(cmds.iter().any(|c| matches!(c, CatCmd::FetchManifest { asset, .. } if *asset == a1)));
        // Manifest for the WRONG revision is ignored by the identity guard.
        let stale = m.manifest_arrived(g, a1, rev(99), Some(media(1)), None, None);
        assert!(m.tile(&a1).unwrap().media.is_none());
        let _ = stale;
        // The right revision completes the tile and requests its thumb.
        let cmds = m.manifest_arrived(
            g,
            a1,
            rev(1),
            Some(media(1)),
            None,
            Some(TileThumb { blob: BlobId::from_bytes([5; 32]), len: 20 }),
        );
        assert_eq!(m.tile(&a1).unwrap().state, TileState::Ready);
        assert!(cmds.iter().any(|c| matches!(
            c,
            CatCmd::FetchThumb { asset, revision, .. } if *asset == a1 && *revision == rev(1)
        )));
        // Failures are per-tile and free a pipeline slot.
        let a2 = hit(2).asset;
        m.detail_arrived(g, a2, None);
        assert_eq!(
            m.tile(&a2).unwrap().state,
            TileState::Failed("no published revision".to_string())
        );
    }

    #[test]
    fn sheet_decision_follows_kind_not_dimensions() {
        // Sprite actors and mesh icons publish packed 128² strips.
        assert!(kind_may_be_sheet(Some(AssetKind::Billboard)));
        assert!(kind_may_be_sheet(Some(AssetKind::Mesh)));
        assert!(kind_may_be_sheet(Some(AssetKind::Character)));
        // A 1024² PBR map / Flux still is a 64-tile sheet by dimension and
        // must NEVER cycle; splats and clips never do either.
        assert!(!kind_may_be_sheet(Some(AssetKind::Texture)));
        assert!(!kind_may_be_sheet(Some(AssetKind::World)));
        assert!(!kind_may_be_sheet(Some(AssetKind::Video)));
        assert!(!kind_may_be_sheet(Some(AssetKind::Audio)));
        assert!(!kind_may_be_sheet(None));
    }

    #[test]
    fn tiles_carry_the_lane_kind_when_the_server_omits_it() {
        let mut m = BrowseModel::<u8>::new_multi(
            vec![AssetKind::Video, AssetKind::Billboard],
            "",
        );
        let g = search_gen(&m.refresh());
        m.page_arrived(g, 0, true, vec![hit(1)], 1, None);
        m.page_arrived(g, 1, true, vec![hit(2)], 1, None);
        assert_eq!(m.tile(&hit(1).asset).unwrap().kind, Some(AssetKind::Video));
        assert_eq!(m.tile(&hit(2).asset).unwrap().kind, Some(AssetKind::Billboard));
        // An explicit server kind wins over the lane's.
        let mut typed = hit(3);
        typed.kind = Some(AssetKind::Character);
        m.page_arrived(g, 0, false, vec![typed], 1, None);
        assert_eq!(m.tile(&hit(3).asset).unwrap().kind, Some(AssetKind::Character));
    }

    #[test]
    fn legacy_per_lump_sprites_are_recognised_by_alias_shape() {
        let doom = Some(AssetKind::Billboard);
        // One frame per lump, no stateful manifest: legacy.
        assert!(is_legacy_lump_sprite(doom, Some("doom/doom/billboards/doom1/trooa1"), false));
        assert!(is_legacy_lump_sprite(doom, Some("doom/doom/billboards/doom1/trooa2a8"), false));
        // The grouped actor is the bare 4-char prefix: kept.
        assert!(!is_legacy_lump_sprite(doom, Some("doom/doom/billboards/doom1/troo"), false));
        assert!(!is_legacy_lump_sprite(doom, Some("doom/doom/billboards/doom1/troo"), true));
        // A stateful companion always wins over the alias shape.
        assert!(!is_legacy_lump_sprite(doom, Some("doom/doom/billboards/doom1/trooa1"), true));
        // Duke/Quake names are not Doom lumps.
        for alias in [
            "duke/duke3d/billboards/duke3d/liztroop",
            "duke/duke3d/billboards/duke3d/tile-1405",
            "duke/duke3d/billboards/duke3d/strip-2066",
            "quake/id1/billboards/flame/frame-01",
        ] {
            assert!(!is_legacy_lump_sprite(doom, Some(alias), false), "{alias}");
        }
        // Other kinds and unaliased assets are never touched.
        assert!(!is_legacy_lump_sprite(
            Some(AssetKind::Texture),
            Some("doom/doom/billboards/doom1/trooa1"),
            false
        ));
        assert!(!is_legacy_lump_sprite(doom, None, false));
        assert!(!is_legacy_lump_sprite(doom, Some("trooa1"), false));
    }

    #[test]
    fn event_touch_filters_by_kind() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        m.event_touch(Some(AssetKind::Audio));
        assert!(!m.refresh_wanted);
        m.event_touch(Some(AssetKind::Video));
        assert!(m.refresh_wanted);
        m.refresh_wanted = false;
        // Unknown content kind conservatively refreshes.
        m.event_touch(None);
        assert!(m.refresh_wanted);
        // Refresh clears the flag.
        m.refresh();
        assert!(!m.refresh_wanted);
    }

    #[test]
    fn multi_kind_dance_lane_merges_mesh_and_character() {
        let mut m = BrowseModel::<u8>::new_multi(
            vec![AssetKind::Mesh, AssetKind::Character],
            "",
        );
        let cmds = m.refresh();
        // One exact-kind search per lane, same generation.
        assert_eq!(cmds.len(), 2);
        let kinds: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                CatCmd::SearchPage { query, slot, .. } => Some((*slot, query.kind)),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec![(0, Some(AssetKind::Mesh)), (1, Some(AssetKind::Character))]);
        // Every lane asks the SERVER to drop run intermediates (PBR maps,
        // source stills, untextured meshes): no client-side sifting.
        assert!(cmds.iter().all(|c| matches!(
            c,
            CatCmd::SearchPage { query, .. } if query.exclude_tag.as_deref() == Some(INTERMEDIATE_TAG)
        )));
        let g = search_gen(&cmds[..1].to_vec());
        // Mesh page replaces (first of the generation), character page
        // merges; the shared hit dedupes.
        m.page_arrived(g, 0, true, vec![hit(1), hit(2)], 2, None);
        assert_eq!(m.tiles().len(), 2);
        assert!(m.is_loading(), "character lane still pending");
        m.page_arrived(g, 1, true, vec![hit(2), hit(3)], 2, None);
        assert_eq!(m.tiles().len(), 3, "merged + deduped");
        assert_eq!(m.total, 4, "totals sum across lanes");
        assert!(!m.is_loading());
        // Events for either kind schedule a refresh; foreign kinds do not.
        m.event_touch(Some(AssetKind::Character));
        assert!(m.refresh_wanted);
        m.refresh_wanted = false;
        m.event_touch(Some(AssetKind::Mesh));
        assert!(m.refresh_wanted);
        m.refresh_wanted = false;
        m.event_touch(Some(AssetKind::Audio));
        assert!(!m.refresh_wanted);
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    #[test]
    fn a_doom_map_is_3d_and_a_map_not_a_splat() {
        // The bug this pins: `AssetKind::World` covers a Doom level AND a
        // Gaussian splat, so reading the kind alone shelved E1M1 as a splat.
        let alias = Some("doom/doom/worlds/doom1/e1m1");
        assert_eq!(shelf_of(Some(AssetKind::World), alias, None), "map");
        assert_eq!(shelf_of(Some(AssetKind::World), alias, Some("map")), "map");
        assert_eq!(preset_of(Some(AssetKind::World), alias, None), Some(Preset::ThreeD));
    }

    #[test]
    fn a_splat_is_a_3d_scene_by_its_category() {
        let alias = Some("gen/splat/office-scan");
        assert_eq!(shelf_of(Some(AssetKind::World), alias, Some("splat")), "3D scene");
        assert_eq!(
            preset_of(Some(AssetKind::World), alias, Some("splat")),
            Some(Preset::ThreeD),
            "a splat is still 3D"
        );
        // Category beats the path: a splat published under a worlds/ alias
        // is a scene, not a level.
        assert_eq!(
            shelf_of(Some(AssetKind::World), Some("x/worlds/y"), Some("splat")),
            "3D scene"
        );
    }

    #[test]
    fn every_3d_lane_answers_to_the_3d_preset() {
        for kind in [
            AssetKind::Mesh,
            AssetKind::Character,
            AssetKind::Prop,
            AssetKind::Weapon,
            AssetKind::Vehicle,
            AssetKind::World,
        ] {
            assert_eq!(preset_of(Some(kind), None, None), Some(Preset::ThreeD), "{kind:?}");
            assert!(Preset::ThreeD.kinds().contains(&kind), "{kind:?} is queried by the chip");
        }
    }

    #[test]
    fn video_images_sprites_and_audio_land_where_they_belong() {
        assert_eq!(preset_of(Some(AssetKind::Video), None, None), Some(Preset::Video));
        assert_eq!(preset_of(Some(AssetKind::Texture), None, None), Some(Preset::Image));
        // A sprite actor is a picture as far as browsing goes.
        assert_eq!(preset_of(Some(AssetKind::Billboard), None, None), Some(Preset::Image));
        assert_eq!(shelf_of(Some(AssetKind::Billboard), None, None), "sprite");
        // Audio is ONE catalog lane; music vs sfx is a tag/alias question.
        assert_eq!(preset_of(Some(AssetKind::Audio), None, None), Some(Preset::Audio));
        assert_eq!(shelf_of(Some(AssetKind::Audio), None, None), "sfx");
        assert_eq!(shelf_of(Some(AssetKind::Audio), None, Some("music")), "music");
        assert_eq!(shelf_of(Some(AssetKind::Audio), Some("gen/music/loop-a"), None), "music");
        assert_eq!(shelf_of(Some(AssetKind::Audio), Some("doom/doom/sfx/dsbfg"), None), "sfx");
        assert_eq!(preset_of(None, None, None), None);
    }

    #[test]
    fn the_preset_lanes_cover_every_visual_kind() {
        let visual = BrowseModel::<u32>::visual_kinds();
        for kind in visual {
            let covered = [Preset::Video, Preset::ThreeD, Preset::Image]
                .iter()
                .any(|p| p.kinds().contains(&kind));
            assert!(covered, "{kind:?} is in no hot preset");
        }
    }
}
