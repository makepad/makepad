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
    pub revision: Option<AssetRevisionId>,
    pub media: Option<TileMedia>,
    pub thumb: Option<TileThumb>,
    pub state: TileState,
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
    pages_pending: usize,
    /// Tiles were already replaced for this generation's first pages.
    cleared_gen: CatGen,
    resolve_queue: VecDeque<AssetId>,
    resolving: usize,
    pub error: Option<String>,
    /// Raised by catalog events; the app refreshes on its debounce tick.
    pub refresh_wanted: bool,
}

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

    /// Program pads: video clips plus stills and 3D that can land on A/B.
    pub fn visual() -> BrowseModel<C> {
        Self::new_multi(
            vec![
                AssetKind::Video,
                AssetKind::Mesh,
                AssetKind::Character,
                AssetKind::Prop,
                AssetKind::Weapon,
                AssetKind::Vehicle,
                AssetKind::Texture,
                AssetKind::Billboard,
            ],
            "",
        )
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
            pages_pending: 0,
            cleared_gen: 0,
            resolve_queue: VecDeque::new(),
            resolving: 0,
            error: None,
            refresh_wanted: false,
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
        q
    }

    /// New query text/category, or an event-driven refresh: bump the
    /// generation (all in-flight completions die) and request page one. The
    /// visible tiles stay until that page arrives.
    pub fn refresh(&mut self) -> Vec<CatCmd<C>> {
        self.gen += 1;
        self.error = None;
        self.refresh_wanted = false;
        self.next_cursors = vec![None; self.kinds.len()];
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
            self.cleared_gen = gen;
            self.tiles.clear();
            self.index.clear();
        }
        for hit in hits {
            if self.index.contains_key(&hit.asset) {
                continue; // keyset pages should not repeat; drop dupes anyway
            }
            self.index.insert(hit.asset, self.tiles.len());
            self.tiles.push(Tile {
                asset: hit.asset,
                title: hit.title,
                alias: hit.alias,
                live: hit.live,
                revision: None,
                media: None,
                thumb: None,
                state: TileState::Listed,
            });
            self.resolve_queue.push_back(hit.asset);
        }
        self.pump_resolves()
    }

    pub fn page_failed(&mut self, gen: CatGen, error: String) {
        if gen != self.gen {
            return;
        }
        self.pages_pending = self.pages_pending.saturating_sub(1);
        self.error = Some(error);
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
    /// the app passes the selected playable file + thumbnail meta.
    pub fn manifest_arrived(
        &mut self,
        gen: CatGen,
        asset: AssetId,
        revision: AssetRevisionId,
        media: Option<TileMedia>,
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
            .manifest_arrived(g1, hit(2).asset, rev(9), Some(media(1)), None)
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
        let stale = m.manifest_arrived(g, a1, rev(99), Some(media(1)), None);
        assert!(m.tile(&a1).unwrap().media.is_none());
        let _ = stale;
        // The right revision completes the tile and requests its thumb.
        let cmds = m.manifest_arrived(
            g,
            a1,
            rev(1),
            Some(media(1)),
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
