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
use makepad_asset_data::{AssetId, AssetKind, AssetRevisionId, BlobId, MediaType, ThumbnailCells};
use std::collections::{HashMap, VecDeque};

pub type CatGen = u64;

/// Tile-resolve pipelines (detail + manifest) in flight at once while the
/// operator is BROWSING — nobody is on stage, so the grid may take the
/// whole machine and fill in one breath. A page is [`PAGE_SIZE`] tiles and
/// every one of them needs its own detail + manifest round trip before its
/// thumbnail blob is even named; resolving four at a time turned a
/// warm-cache page into a two-second drip (measured: 48 tiles, 2.0-2.3s,
/// ~23 tiles a second, with the fetch and decode lanes idle the whole
/// time). A page's worth in flight is what makes a warm page land at once.
pub const MAX_RESOLVING: usize = 48;
/// Tile-resolve pipelines in flight while a set is RUNNING — the program
/// window is up or a deck is playing. The grid still fills; it just stops
/// competing with the picture on screen for the link and the CPU.
pub const MAX_RESOLVING_PERFORMING: usize = 6;
/// Search page size (server pages deterministically under this).
pub const PAGE_SIZE: u32 = 48;
/// Resolved tiles one surface remembers across listings. Each is a handful
/// of ids and two short strings, so a whole browsed library costs well under
/// a megabyte; the bound is here so a session that never stops browsing
/// cannot grow without one.
pub const MAX_CARRY: usize = 8192;
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
#[derive(Clone, Debug, PartialEq)]
pub struct TileThumb {
    pub blob: BlobId,
    pub len: u64,
    /// The cell layout the manifest DECLARED, when the picture is a packed
    /// sheet, and the rate its producer wrote down. `None` means the
    /// thumbnail says nothing about itself, which is what every revision
    /// published before the views contract means.
    pub anim: Option<(ThumbnailCells, f32)>,
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

/// LEGACY ONLY: whether a tile's THUMBNAIL may be a packed animation strip,
/// decided by catalog kind because the picture itself does not say.
///
/// A thumbnail now DECLARES its cell layout ([`TileThumb::anim`]), so this
/// question is answered from the manifest and nothing is guessed. What
/// remains is revisions published before that contract: their pictures carry
/// no declaration, and the only thing standing between a sprite actor's
/// 128²-tile strip and a 1024-square PBR map that is dimensionally the same
/// sheet is this kind gate. Delete it with the last un-declared revision.
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
    /// Server-side last-update stamp: the strip sorts newest-first on it,
    /// so tonight's generations lead from the left.
    pub updated_ms: u64,
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
        Some(AssetKind::VjEffect) => "effect",
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

pub struct BrowseModel<C: Clone = PageCursor> {
    /// Exact kinds this surface shows (1..=2, e.g. Mesh+Character for the
    /// dance lane). One bounded search per kind; results merge by asset.
    pub kinds: Vec<AssetKind>,
    pub text: String,
    pub category: String,
    /// Positive tag narrowing (the TRANSITION preset); empty = none.
    pub tag: String,
    /// Per-lane EXCLUDE tag override (the EFFECT lane drops transition-
    /// tagged docs). Empty = the default intermediate-artifact exclusion.
    /// (The query wire carries ONE exclude tag, so a lane that needs its
    /// own gives up the intermediate one — vjeffects never carry it.)
    pub exclude: String,
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
    /// Resolved state of tiles this surface has ALREADY resolved, by asset.
    ///
    /// A tile's picture is keyed by its revision, and its revision is only
    /// known once a detail + manifest round trip has landed. So a listing
    /// that forgets what it resolved shows blank tiles until the store
    /// answers again — even when every one of those pictures is sitting in
    /// texture memory. That is what "switching tabs doesn't cache the
    /// thumbnails" looks like from the operator's chair.
    ///
    /// It therefore ACCUMULATES rather than being swapped: a refresh, a
    /// filter change and a tab flip all re-list, and flipping VIDEO →
    /// EFFECT → VIDEO must find VIDEO's revisions still remembered, not
    /// just the one tab back. Entries are dropped when the asset is
    /// republished ([`Self::event_republished`]) or retired, and the oldest
    /// are trimmed at [`MAX_CARRY`].
    carry: HashMap<AssetId, Tile>,
    /// Insertion order for `carry`, so the trim drops the least recently
    /// remembered rather than an arbitrary one.
    carry_order: VecDeque<AssetId>,
    resolve_queue: VecDeque<AssetId>,
    resolving: usize,
    /// How many resolves this surface may run at once — [`MAX_RESOLVING`]
    /// while browsing, [`MAX_RESOLVING_PERFORMING`] during a set. The app
    /// sets it from the same politeness check the effect-thumbnail bank
    /// uses, so one rule governs both.
    resolve_width: usize,
    pub error: Option<String>,
    /// Raised by catalog events; the app refreshes on its debounce tick.
    pub refresh_wanted: bool,
    /// Display order of the SETTLED body. Once an asset has a place here
    /// it keeps it until the next re-sort — the operator's hand is on a
    /// pad, and a grid that renumbers itself under that hand is a grid
    /// that fires the wrong clip.
    order: Vec<AssetId>,
    /// updated_ms per placed asset, for the newest-first ordering.
    stamps: HashMap<AssetId, u64>,
    /// The PENDING head column: assets the generators published while the
    /// operator was watching, filling the leftmost column top to bottom.
    /// It merges into `order` when it fills (a new empty column starts) or
    /// on the next re-sort. Never longer than [`PENDING_COLUMN`].
    pending: Vec<AssetId>,
    /// The next arriving first page re-sorts the body. Set by a real query
    /// change (text, category, kinds) and by an explicit re-sort — never by
    /// the event-driven refresh that a publish triggers.
    resort: bool,
    /// TRANSITION lane: sort by the seed registry's everyday→rare rank
    /// instead of newest-first, so the lane is the same every night.
    pub rank_aliases: bool,
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
            AssetKind::VjEffect,
        ]
    }

    /// Program pads: all visual kinds.
    pub fn visual() -> BrowseModel<C> {
        Self::new_multi(Self::visual_kinds(), "")
    }

    /// Change the kind lanes (kind chips) and re-query from page one. A
    /// plain kind change drops any preset tag narrowing — the chips are a
    /// different gesture than the tag presets.
    pub fn set_kinds(&mut self, kinds: Vec<AssetKind>) -> Vec<CatCmd<C>> {
        self.set_lanes(kinds, String::new(), String::new())
    }

    /// Change the kind lanes AND the positive tag filter together (the
    /// EFFECT / TRANSITION presets) and re-query from page one.
    pub fn set_lanes(
        &mut self,
        kinds: Vec<AssetKind>,
        tag: String,
        exclude: String,
    ) -> Vec<CatCmd<C>> {
        let kinds = if kinds.is_empty() { Self::visual_kinds() } else { kinds };
        if self.kinds == kinds && self.tag == tag && self.exclude == exclude {
            return Vec::new();
        }
        self.kinds = kinds;
        self.tag = tag;
        self.exclude = exclude;
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
            tag: String::new(),
            exclude: String::new(),
            gen: 0,
            tiles: Vec::new(),
            index: HashMap::new(),
            total: 0,
            rank_aliases: false,
            next_cursors: vec![None; lanes],
            restarting: vec![false; lanes],
            pages_pending: 0,
            cleared_gen: 0,
            carry: HashMap::new(),
            carry_order: VecDeque::new(),
            resolve_queue: VecDeque::new(),
            resolving: 0,
            resolve_width: MAX_RESOLVING,
            error: None,
            refresh_wanted: false,
            order: Vec::new(),
            stamps: HashMap::new(),
            pending: Vec::new(),
            // The first page of a fresh model IS the sort.
            resort: true,
        }
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// How many tile resolves this surface may run at once. Returns the
    /// commands the widening frees, so raising the width does not have to
    /// wait for the next page to land.
    pub fn set_resolve_width(&mut self, width: usize) -> Vec<CatCmd<C>> {
        let width = width.max(1);
        if width == self.resolve_width {
            return Vec::new();
        }
        self.resolve_width = width;
        self.pump_resolves()
    }

    pub fn resolve_width(&self) -> usize {
        self.resolve_width
    }

    /// Tile resolves in flight right now (detail or manifest).
    pub fn resolving(&self) -> usize {
        self.resolving
    }

    /// Tiles waiting for a resolve slot.
    pub fn resolve_backlog(&self) -> usize {
        self.resolve_queue.len()
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
        if !self.tag.is_empty() {
            q.tag = Some(self.tag.clone());
        }
        if !self.exclude.is_empty() {
            q.exclude_tag = Some(self.exclude.clone());
            return q;
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

    /// The head column plus the body, in display order — CONTIGUOUS. The
    /// head used to reserve its full column height with `None` cells so the
    /// body never moved while it filled; on screen those reservations read
    /// as random holes punched into the grid (the operator's words), which
    /// is worse than the body stepping one cell per arrival. The `Option`
    /// stays in the signature for the callers' sake, but every cell is
    /// `Some` now.
    pub fn display_order(&self) -> Vec<Option<AssetId>> {
        let mut out: Vec<Option<AssetId>> =
            Vec::with_capacity(self.pending.len() + self.order.len());
        out.extend(self.pending.iter().map(|a| Some(*a)));
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
            let outgoing: Vec<Tile> = self.tiles.drain(..).collect();
            for tile in outgoing {
                self.remember(tile);
            }
            self.index.clear();
            if self.resort {
                // A real query change: the whole strip is re-derived from
                // the server's order.
                self.order.clear();
                self.pending.clear();
            }
        }
        let mut added = 0usize;
        let mut resorted = false;
        for hit in hits {
            if self.index.contains_key(&hit.asset) {
                continue; // keyset pages should not repeat; drop dupes anyway
            }
            added += 1;
            self.stamps.insert(hit.asset, hit.updated_ms);
            self.place(hit.asset);
            resorted = true;
            self.index.insert(hit.asset, self.tiles.len());
            let kind = hit.kind.or_else(|| self.kinds.get(slot).copied());
            // Cloned, not taken: the operator flips back and forth, and a
            // memory that empties itself on first use is no memory.
            match self.carry.get(&hit.asset).cloned() {
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
            // Newest first: the operator reads the strip left-to-right as
            // "what just came in" — tonight's generations lead.
            let stamps = &self.stamps;
            if self.rank_aliases {
                // Everyday→rare, the seed registry's order; ties (unknown
                // docs) fall back to newest-first.
                let tiles = &self.tiles;
                let index = &self.index;
                self.order.sort_by_key(|asset| {
                    let rank = index
                        .get(asset)
                        .and_then(|&i| tiles[i].alias.as_deref())
                        .map(crate::effects::seed::transition_rank)
                        .unwrap_or(usize::MAX);
                    (rank, std::cmp::Reverse(stamps.get(asset).copied().unwrap_or(0)))
                });
            } else {
                self.order.sort_by_key(|asset| {
                    std::cmp::Reverse(stamps.get(asset).copied().unwrap_or(0))
                });
            }
        }
        let _ = resorted;
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

    /// Remember a resolved tile so a later listing can paint it without a
    /// round trip. Unresolved tiles are not worth remembering.
    fn remember(&mut self, tile: Tile) {
        if tile.state != TileState::Ready {
            self.carry.remove(&tile.asset);
            return;
        }
        let asset = tile.asset;
        if self.carry.insert(asset, tile).is_none() {
            self.carry_order.push_back(asset);
        }
        while self.carry_order.len() > MAX_CARRY {
            if let Some(oldest) = self.carry_order.pop_front() {
                self.carry.remove(&oldest);
            }
        }
    }

    fn forget(&mut self, asset: AssetId) {
        if self.carry.remove(&asset).is_some() {
            self.carry_order.retain(|a| *a != asset);
        }
    }

    /// This asset was published again: whatever we remember about it is a
    /// revision out of date.
    ///
    /// The carried copy goes, so the next listing resolves it fresh instead
    /// of painting last night's picture. A tile of it that is on screen
    /// right now is re-resolved in place — it KEEPS its current picture
    /// until the new manifest lands, because a republish should refresh a
    /// grid, not blank it.
    pub fn event_republished(&mut self, asset: AssetId) -> Vec<CatCmd<C>> {
        self.forget(asset);
        let Some(&i) = self.index.get(&asset) else { return Vec::new() };
        if self.tiles[i].state != TileState::Ready {
            return Vec::new(); // already on its way
        }
        if self.resolve_queue.contains(&asset) {
            return Vec::new();
        }
        self.resolve_queue.push_back(asset);
        self.pump_resolves()
    }

    /// Start queued tile resolves up to the bound.
    fn pump_resolves(&mut self) -> Vec<CatCmd<C>> {
        let mut cmds = Vec::new();
        let width = self.resolve_width.max(1);
        while self.resolving < width {
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

    /// Move the tiles the operator can SEE to the front of the resolve
    /// queue, in the order given, and start what fits.
    ///
    /// The queue is otherwise filled in listing order, so a bank of three
    /// thousand clips resolves from the top whatever page is on screen —
    /// the operator scrolls to row forty and waits for rows one to thirty-
    /// nine to finish first. The store's resolve throughput is finite; what
    /// it spends it on is not.
    pub fn resolve_visible_first(&mut self, assets: &[AssetId]) -> Vec<CatCmd<C>> {
        let mut jumped: Vec<AssetId> = Vec::new();
        for asset in assets {
            let Some(&i) = self.index.get(asset) else { continue };
            if self.tiles[i].state != TileState::Listed {
                continue; // already resolving, resolved or failed
            }
            jumped.push(*asset);
        }
        if jumped.is_empty() {
            return Vec::new();
        }
        // Nothing already at the head of the queue is displaced further than
        // the visible window itself: retain, then push the window in front
        // in the order the eye reads it.
        self.resolve_queue.retain(|a| !jumped.contains(a));
        for asset in jumped.into_iter().rev() {
            self.resolve_queue.push_front(asset);
        }
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

    /// An asset left the catalog (retired/quarantined): schedule a full
    /// re-sorted refresh. The server's listing no longer contains it, and
    /// the resort path rebuilds order/tiles/index together — dropping the
    /// dead tile AND closing its hole without hand-surgery on invariants
    /// (a surgical compaction here once left duplicate tiles scattered on
    /// an 8-stride; rebuild beats scalpel).
    pub fn event_remove(&mut self, asset: AssetId) {
        // A retired asset must not be remembered, or the next listing would
        // paint a tile the store no longer has.
        self.forget(asset);
        self.resort = true;
        self.refresh_wanted = true;
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
            updated_ms: seed as u64,
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
        // Newest first: the strip leads with the freshest updated_ms.
        let body: Vec<AssetId> = (1..=6).rev().map(|s| hit(s).asset).collect();
        assert_eq!(m.display_order(), body.iter().map(|a| Some(*a)).collect::<Vec<_>>());

        // A publish event re-lists with two new assets at the FRONT (what
        // a newest-first server returns).
        let cmds = m.refresh_event();
        let gen = search_gen(&cmds);
        let listed: Vec<HitRow> = [7u8, 8].iter().chain(&[1, 2, 3, 4, 5, 6]).map(|s| hit(*s)).collect();
        m.page_arrived(gen, 0, true, listed, 8, None);

        let shown = m.display_order();
        assert_eq!(m.pending_len(), 2);
        // CONTIGUOUS: the head holds exactly the tiles that exist — no
        // reserved empty cells (those read as holes punched into the grid).
        assert_eq!(shown.len(), 2 + 6);
        assert_eq!(shown[0], Some(hit(7).asset));
        assert_eq!(shown[1], Some(hit(8).asset));
        assert!(shown.iter().all(|cell| cell.is_some()), "no gap cells ever");
        assert_eq!(&shown[2..], &body.iter().map(|a| Some(*a)).collect::<Vec<_>>()[..]);

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
            vec![Some(hit(9).asset), Some(hit(3).asset), Some(hit(1).asset)],
            "a re-sort orders newest-first by updated_ms"
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
        // Narrowed on purpose: this test is about the BOUND and the identity
        // guard, not the width the app browses at.
        m.set_resolve_width(4);
        let cmds = m.page_arrived(g, 0, true, hits, 7, None);
        // Only `resolve_width` details go out.
        assert_eq!(cmds.len(), 4);
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
            Some(TileThumb { blob: BlobId::from_bytes([5; 32]), len: 20, anim: None }),
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
        // An effect's SEEDED placeholder is a plain still; the animated
        // sheet that replaces it always declares its own cells.
        assert!(!kind_may_be_sheet(Some(AssetKind::VjEffect)));
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


    /// A resolved tile is remembered across EVERY listing, not just the one
    /// the operator came from. Flipping to another tab and back must paint
    /// from what is already known — a revision the grid has to ask for again
    /// is a blank tile, however warm the texture cache is.
    #[test]
    fn resolved_tiles_survive_a_round_trip_through_another_tab() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        let a = hit(1).asset;
        m.page_arrived(g, 0, true, vec![hit(1)], 1, None);
        m.detail_arrived(g, a, Some(rev(1)));
        m.manifest_arrived(
            g,
            a,
            rev(1),
            Some(media(1)),
            None,
            Some(TileThumb { blob: BlobId::from_bytes([5; 32]), len: 20, anim: None }),
        );
        assert_eq!(m.tile(&a).unwrap().state, TileState::Ready);

        // Tab away: a different query, none of the old assets in it.
        let g2 = search_gen(&m.set_text("transition".into()));
        m.page_arrived(g2, 0, true, vec![hit(2)], 1, None);
        assert!(m.tile(&a).is_none());

        // Tab back. The tile comes back RESOLVED — no detail, no manifest,
        // and its revision is there for the texture cache to key on.
        let g3 = search_gen(&m.set_text(String::new()));
        let cmds = m.page_arrived(g3, 0, true, vec![hit(1)], 1, None);
        let back = m.tile(&a).expect("the tile is listed again");
        assert_eq!(back.state, TileState::Ready);
        assert_eq!(back.revision, Some(rev(1)));
        assert!(back.thumb.is_some());
        assert!(
            !cmds.iter().any(|c| matches!(c, CatCmd::FetchDetail { asset, .. } if *asset == a)),
            "a remembered tile must not be resolved again"
        );

        // And once more, to prove the memory is not consumed by first use.
        let g4 = search_gen(&m.set_text("transition".into()));
        m.page_arrived(g4, 0, true, vec![hit(2)], 1, None);
        let g5 = search_gen(&m.set_text(String::new()));
        m.page_arrived(g5, 0, true, vec![hit(1)], 1, None);
        assert_eq!(m.tile(&a).unwrap().revision, Some(rev(1)));
    }

    /// A republish makes the memory wrong, so the memory goes — and the tile
    /// on screen re-resolves WITHOUT losing the picture it is showing.
    #[test]
    fn a_republish_forgets_the_remembered_revision() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        let a = hit(1).asset;
        m.page_arrived(g, 0, true, vec![hit(1)], 1, None);
        m.detail_arrived(g, a, Some(rev(1)));
        m.manifest_arrived(g, a, rev(1), Some(media(1)), None, None);

        let cmds = m.event_republished(a);
        assert!(
            cmds.iter().any(|c| matches!(c, CatCmd::FetchDetail { asset, .. } if *asset == a)),
            "the live tile asks the store what it is now"
        );
        // The picture it is already showing stays until the new one lands.
        assert_eq!(m.tile(&a).unwrap().revision, Some(rev(1)));

        // The new revision replaces it, and THAT is what gets remembered.
        m.detail_arrived(g, a, Some(rev(2)));
        m.manifest_arrived(g, a, rev(2), Some(media(2)), None, None);
        assert_eq!(m.tile(&a).unwrap().revision, Some(rev(2)));
        let g2 = search_gen(&m.set_text("transition".into()));
        m.page_arrived(g2, 0, true, vec![hit(9)], 1, None);
        let g3 = search_gen(&m.set_text(String::new()));
        m.page_arrived(g3, 0, true, vec![hit(1)], 1, None);
        assert_eq!(m.tile(&a).unwrap().revision, Some(rev(2)));
    }

    /// The resolve pipeline is as wide as the app says. Four at a time made
    /// a warm page a two-second drip; a page's worth in flight makes it land
    /// at once, and the politeness width narrows it again for a live set.
    #[test]
    fn resolve_width_bounds_and_reopens_the_pipeline() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        let hits: Vec<HitRow> = (1..=20u8).map(hit).collect();
        let cmds = m.page_arrived(g, 0, true, hits, 20, None);
        let details = cmds.iter().filter(|c| matches!(c, CatCmd::FetchDetail { .. })).count();
        assert_eq!(details, MAX_RESOLVING.min(20), "a page resolves as wide as it may");
        assert_eq!(m.resolving(), details);

        // Narrowing for a set does not cancel what is running; it just stops
        // starting more. Widening again starts what is waiting, now.
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        m.set_resolve_width(2);
        let hits: Vec<HitRow> = (1..=20u8).map(hit).collect();
        let cmds = m.page_arrived(g, 0, true, hits, 20, None);
        assert_eq!(cmds.iter().filter(|c| matches!(c, CatCmd::FetchDetail { .. })).count(), 2);
        assert_eq!(m.resolve_backlog(), 18);
        let opened = m.set_resolve_width(MAX_RESOLVING);
        assert_eq!(
            opened.iter().filter(|c| matches!(c, CatCmd::FetchDetail { .. })).count(),
            18,
            "widening starts the backlog without waiting for a page"
        );
        assert_eq!(m.resolve_backlog(), 0);
    }

    /// What is on screen resolves first: the store's throughput is finite,
    /// and a bank of thousands must not make the visible page wait behind
    /// every row above it.
    #[test]
    fn visible_tiles_jump_the_resolve_queue() {
        let mut m = BrowseModel::<u8>::new(AssetKind::Video, "");
        let g = search_gen(&m.refresh());
        m.set_resolve_width(2);
        let hits: Vec<HitRow> = (1..=10u8).map(hit).collect();
        let started = m.page_arrived(g, 0, true, hits, 10, None);
        let first_two: Vec<AssetId> = started
            .iter()
            .filter_map(|c| match c {
                CatCmd::FetchDetail { asset, .. } => Some(*asset),
                _ => None,
            })
            .collect();
        assert_eq!(first_two.len(), 2);

        // Rows 8 and 9 are what the operator scrolled to.
        let visible = [AssetId::from_bytes([8; 16]), AssetId::from_bytes([9; 16])];
        // No slot free yet, so nothing starts — but the ORDER changed.
        assert!(m.resolve_visible_first(&visible).is_empty());
        // A landed detail hands its own slot straight to its manifest; the
        // slot frees only when that tile is finished.
        let cmds = m.detail_arrived(g, first_two[0], Some(rev(1)));
        assert!(!cmds.iter().any(|c| matches!(c, CatCmd::FetchDetail { .. })));
        let cmds = m.manifest_arrived(g, first_two[0], rev(1), None, None, None);
        let next = cmds
            .iter()
            .find_map(|c| match c {
                CatCmd::FetchDetail { asset, .. } => Some(*asset),
                _ => None,
            })
            .expect("the freed slot starts something");
        assert_eq!(next, visible[0], "the visible row goes first, in reading order");

        // Tiles already resolving or resolved are left alone.
        assert!(m.resolve_visible_first(&[first_two[0]]).is_empty());
    }
}

#[cfg(test)]
mod shelf_tests {
    use super::*;

    #[test]
    fn a_doom_map_is_a_map_and_a_splat_is_a_scene() {
        // The bug this pins: `AssetKind::World` covers a Doom level AND a
        // Gaussian splat, so reading the kind alone shelved E1M1 as a splat.
        let alias = Some("doom/doom/worlds/doom1/e1m1");
        assert_eq!(shelf_of(Some(AssetKind::World), alias, None), "map");
        assert_eq!(shelf_of(Some(AssetKind::World), alias, Some("map")), "map");
        let splat = Some("gen/splat/office-scan");
        assert_eq!(shelf_of(Some(AssetKind::World), splat, Some("splat")), "3D scene");
        // Category beats the path: a splat published under a worlds/ alias
        // is a scene, not a level.
        assert_eq!(
            shelf_of(Some(AssetKind::World), Some("x/worlds/y"), Some("splat")),
            "3D scene"
        );
    }

    #[test]
    fn shelves_name_what_things_are() {
        assert_eq!(shelf_of(Some(AssetKind::VjEffect), None, None), "effect");
        assert_eq!(shelf_of(Some(AssetKind::Billboard), None, None), "sprite");
        // Audio is ONE catalog lane; music vs sfx is a tag/alias question.
        assert_eq!(shelf_of(Some(AssetKind::Audio), None, None), "sfx");
        assert_eq!(shelf_of(Some(AssetKind::Audio), None, Some("music")), "music");
        assert_eq!(shelf_of(Some(AssetKind::Audio), Some("gen/music/loop-a"), None), "music");
        assert_eq!(shelf_of(Some(AssetKind::Audio), Some("doom/doom/sfx/dsbfg"), None), "sfx");
    }
}
