//! The rasteriser under the HAL: glyphs are not rasterised here. This session asks
//! the host's font service for the glyphs it draws and keeps what it answered (an atlas
//! rectangle, or slug curves, per glyph and size); the atlases, the distance fields and the
//! workers that make them are the host's. A glyph that has not arrived draws its usual rectangle
//! with zero coverage and reports that draw's pending keys through `Fonts::prepare_textures`.
//! The Cx client pumps for those requests and rebuilds before presentation (bounded at 250 ms).

use super::{
    font::{Font, FontId, GlyphId},
    geom::{Point, Rect, Size},
    sdfer::Sdfer,
    msdfer::Msdfer,
    slug_atlas::SlugGlyphInfo,
};
use crate::makepad_platform::{hal_font_wire as wire, HalFontSource, HalGlyphEntry};
use fxhash::{FxHashMap, FxHashSet};
pub use makepad_font_core::raster::{
    AtlasKind, GlyphSource, MsdfComplexitySettings, MsdfResolutionSettings, OutlineRasterizationMode,
    RasterizedGlyph, Settings,
};
use makepad_font_core::raster::{plan_color, plan_outline_sdf, COLOR_PADDING};

/// Frames a glyph may go undrawn before the session lets go of its atlas slot.
const IDLE_FRAMES: u64 = 600;
/// How often (frames) the idle glyphs are looked for.
const SWEEP_FRAMES: u64 = 120;

/// How a font is named to the host.
#[derive(Clone, Debug)]
pub enum OpenSource {
    Path(String),
    /// a font that is not a file (assets): its bytes
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct OpenSpec {
    pub local: u32,
    pub index: u32,
    pub vars: Vec<(u32, f32)>,
    pub source: OpenSource,
}

impl OpenSpec {
    pub fn source(&self) -> HalFontSource<'_> {
        match &self.source {
            OpenSource::Path(p) => HalFontSource::Path(p),
            OpenSource::Bytes(b) => HalFontSource::Bytes(b),
        }
    }
}

/// What the client has to say to the host this turn.
#[derive(Default)]
pub struct Outbox {
    pub opens: Vec<OpenSpec>,
    /// (the session's font id, request kind, glyphs with their size)
    pub requests: Vec<(u32, u32, Vec<(u32, f32)>)>,
    pub releases: Vec<(u32, u32)>,
    /// Slots in use since the last sweep: the host keeps them over unused ones when it evicts.
    pub touches: Vec<(u32, u32)>,
    /// Requests referenced by this draw, including ones sent by an earlier draw.
    /// (font, glyph, kind, size bits); the Cx client decides which must precede presentation.
    pub pending: Vec<(u32, u32, u32, u32)>,
}

impl Outbox {
    pub fn is_empty(&self) -> bool {
        self.opens.is_empty() && self.requests.is_empty() && self.releases.is_empty() && self.touches.is_empty() && self.pending.is_empty()
    }
}

/// A glyph key: the session's font id, glyph, request kind.
type Key = (u32, u16, u32);

/// A glyph a draw list draws: (session font id, glyph, request kind). A list keeps these with
/// its instances; while the list is alive its glyphs are in use (see `GlyphClient::sweep`).
pub type GlyphUse = [u32; 3];

struct Held {
    dpxs_bits: u32,
    entry: HalGlyphEntry,
    used: u64,
}

/// What `GlyphClient::slug` answers: the glyph's curves (or the zero-curve box while they are on
/// their way), or nothing to draw.
pub enum SlugLookup {
    Ready(SlugGlyphInfo),
    Empty,
}

pub enum Lookup {
    Ready(HalGlyphEntry),
    /// the host knows the glyph has no image
    Empty,
    /// asked for, not here yet
    Pending,
}

#[derive(Default)]
pub struct GlyphClient {
    locals: FxHashMap<FontId, u32>,
    /// the font file (and face) each local font is an instance of, and the locals per file:
    /// instances of one variable font (animated weight/width) stand in for each other
    group_of: FxHashMap<u32, u64>,
    groups: FxHashMap<u64, Vec<u32>>,
    /// each local font's variation axis values: the nearest instance stands in for a new one
    coords_of: FxHashMap<u32, Vec<(u32, f32)>>,
    fonts: FxHashMap<u32, FontId>,
    failed: FxHashSet<u32>,
    next_local: u32,
    ready: FxHashMap<Key, Vec<Held>>,
    /// atlas slot -> the held entries that sit in it, and its generation
    slots: FxHashMap<u32, (u32, Vec<(Key, u32)>)>,
    requested: FxHashSet<(Key, u32)>,
    draw_pending: FxHashSet<(Key, u32)>,
    out_opens: Vec<OpenSpec>,
    out_requests: FxHashMap<(u32, u32), Vec<(u32, f32)>>,
    out_releases: Vec<(u32, u32)>,
    /// The pending list's storage, handed back by [`GlyphClient::recycle`]: a draw that
    /// waits on glyphs lists them every frame, and that list allocates once, not per frame.
    out_pending: Vec<(u32, u32, u32, u32)>,
    raster_size: Size<usize>,
    raster_generation: u32,
    slug_generation: u32,
    /// bumped when entries are taken away (eviction, a new atlas): runs built on them are stale
    epoch: u64,
    /// Glyphs looked up since they were last handed to a draw list (`take_uses`): the keys
    /// the list that is drawing records, so a list kept on screen keeps its glyphs.
    uses: Vec<GlyphUse>,
    out_touches: Vec<(u32, u32)>,
    frame: u64,
}

impl GlyphClient {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Glyphs asked for and not answered.
    pub fn pending(&self) -> usize {
        self.requested.len()
    }

    /// Font instances opened with the host (each variable-axis value is one).
    pub fn fonts_open(&self) -> usize {
        self.locals.len()
    }

    /// Entries held, for the memory report.
    pub fn held(&self) -> usize {
        self.ready.values().map(|v| v.len()).sum()
    }

    fn local_of(&mut self, font: &Font) -> u32 {
        if let Some(local) = self.locals.get(&font.id()) {
            return *local;
        }
        self.next_local += 1;
        let local = self.next_local;
        self.locals.insert(font.id(), local);
        self.fonts.insert(local, font.id());
        let data = font.data();
        {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            match data.path() {
                Some(path) => path.hash(&mut h),
                None => (data.as_slice().len(), data.as_slice().as_ptr() as usize).hash(&mut h),
            }
            font.face().index().hash(&mut h);
            let g = h.finish();
            self.group_of.insert(local, g);
            self.groups.entry(g).or_default().push(local);
        }
        let source = match data.path() {
            Some(path) => OpenSource::Path(path.to_string_lossy().into_owned()),
            None => OpenSource::Bytes(data.as_slice().to_vec()),
        };
        let vars = font.variation_coords();
        self.coords_of.insert(local, vars.clone());
        self.out_opens.push(OpenSpec { local, index: font.face().index(), vars, source });
        local
    }

    /// The glyph at about this size. A glyph of another size stands in while the exact one is on its
    /// way (the text scales it): zooming never blanks the text. Slug curves have no size.
    pub fn lookup(&mut self, font: &Font, glyph: GlyphId, dpxs: f32, kind: u32) -> Lookup {
        let local = self.local_of(font);
        self.lookup_local(local, glyph, dpxs, kind)
    }

    /// `lookup` by the session's font id (the font is open already).
    fn lookup_local(&mut self, local: u32, glyph: GlyphId, dpxs: f32, kind: u32) -> Lookup {
        if self.failed.contains(&local) {
            return Lookup::Empty;
        }
        let sizeless = kind == wire::kind::SLUG;
        let bits = if sizeless { 0 } else { dpxs.to_bits() };
        let key: Key = (local, glyph, kind);
        self.uses.push([local, glyph as u32, kind]);
        let frame = self.frame;
        let mut stand_in = None;
        if let Some(held) = self.ready.get_mut(&key) {
            if let Some(h) = held.iter_mut().find(|h| h.dpxs_bits == bits) {
                h.used = frame;
                return match h.entry.atlas_kind {
                    wire::glyph::EMPTY => Lookup::Empty,
                    _ => Lookup::Ready(h.entry),
                };
            }
            // the nearest size, by ratio
            let want = dpxs.max(f32::MIN_POSITIVE);
            let near = held
                .iter_mut()
                .filter(|h| h.entry.atlas_kind != wire::glyph::EMPTY)
                .min_by(|a, b| ratio(f32::from_bits(a.dpxs_bits), want).total_cmp(&ratio(f32::from_bits(b.dpxs_bits), want)));
            if let Some(h) = near {
                h.used = frame;
                stand_in = Some(h.entry);
            }
        }
        // A new instance of a variable font (an animated axis) has nothing of its own yet: the
        // same glyph of another instance of the file stands in, so the text never blanks or
        // stalls the frame while the exact instance is made. An atlas image comes from the
        // nearest size; slug curves (sizeless) from the instance nearest on the axes, which for
        // an animated weight is the value drawn a frame before. A draw that records once into a
        // kept draw list (a Motion slide, a render target) is not drawn again when the answer
        // arrives, so without a stand-in it presented the letter blank: the motion deck's weight
        // wave flickered, a letter vanishing on each frame its weight stepped to a new value.
        if stand_in.is_none() {
            if let Some(members) = self.group_of.get(&local).and_then(|g| self.groups.get(g)) {
                let want = dpxs.max(f32::MIN_POSITIVE);
                let coords = self.coords_of.get(&local);
                let mut best: Option<(f32, HalGlyphEntry)> = None;
                for &other in members.iter().filter(|o| **o != local) {
                    let Some(held) = self.ready.get(&(other, glyph, kind)) else { continue };
                    for h in held.iter().filter(|h| h.entry.atlas_kind != wire::glyph::EMPTY) {
                        let r = if sizeless {
                            axis_distance(coords, self.coords_of.get(&other))
                        } else {
                            ratio(f32::from_bits(h.dpxs_bits), want)
                        };
                        if best.as_ref().is_none_or(|b| r < b.0) {
                            best = Some((r, h.entry));
                        }
                    }
                }
                stand_in = best.map(|b| b.1);
            }
        }
        if self.requested.insert((key, bits)) {
            self.out_requests.entry((local, kind)).or_default().push((glyph as u32, dpxs));
        }
        // a glyph with a stand-in does not hold the frame back: only a draw with nothing to show
        // for it waits
        if stand_in.is_none() {
            self.draw_pending.insert((key, bits));
        }
        match stand_in {
            Some(entry) => Lookup::Ready(entry),
            None => Lookup::Pending,
        }
    }

    pub fn raster(
        &mut self,
        font: &Font,
        glyph: GlyphId,
        dpxs: f32,
        kind: u32,
        settings: &Settings,
    ) -> Option<RasterizedGlyph> {
        let local = self.local_of(font);
        let sizeless = kind == wire::kind::SLUG;
        let source = GlyphSource { font: local, glyph, kind, dpxs_bits: if sizeless { 0 } else { dpxs.to_bits() } };
        match self.lookup_local(local, glyph, dpxs, kind) {
            Lookup::Ready(e) => Some(self.rasterized_from(&e, source)),
            Lookup::Empty => None,
            Lookup::Pending => {
                // Use the host's metric calculation, including bitmap strike scaling and SDF
                // padding/minimum resolution. Planning does not render pixels or allocate a slot.
                // AUTO starts with this same SDF plan; its MSDF upgrade preserves the rectangle.
                let (size, padding, origin, scale) = if let Some(plan) = plan_color(font, glyph, dpxs) {
                    (
                        plan.key.size,
                        COLOR_PADDING,
                        plan.origin_in_dpxs,
                        plan.dpxs_per_em,
                    )
                } else {
                    let plan = plan_outline_sdf(font, glyph, dpxs, settings)?;
                    (
                        plan.key.size,
                        plan.padding,
                        plan.origin_in_dpxs,
                        plan.dpxs_per_em,
                    )
                };
                Some(RasterizedGlyph {
                    atlas_kind: AtlasKind::Unresolved,
                    // No AtlasInfo is needed to draw an unresolved instance. Its UVs are set
                    // separately from these bounds, which describe the eventual glyph rectangle.
                    atlas_size: Size::new(1, 1),
                    atlas_image_bounds: Rect::new(Point::new(0, 0), size),
                    atlas_image_padding: padding,
                    atlas_plane: 0,
                    origin_in_dpxs: origin,
                    dpxs_per_em: scale,
                    source,
                })
            }
        }
    }

    /// A glyph kept from an earlier draw (a prepared run, a cached cell), looked up again: the
    /// answer that arrived since, an upgraded slot, or a new request when its entry was taken
    /// back. Kept glyphs are used glyphs: this keeps their slots from being let go. A glyph still
    /// on its way stays as it was (its rectangle is already the eventual one) and is reported
    /// pending for this draw, like any text drawn this frame.
    pub fn refresh(&mut self, glyph: RasterizedGlyph) -> RasterizedGlyph {
        let source = glyph.source;
        if source.font == 0 {
            return glyph;
        }
        match self.lookup_local(source.font, source.glyph, f32::from_bits(source.dpxs_bits), source.kind) {
            Lookup::Ready(e) => self.rasterized_from(&e, source),
            // nothing there (any more): its old rectangle may hold another glyph now
            Lookup::Empty | Lookup::Pending => RasterizedGlyph { atlas_kind: AtlasKind::Unresolved, ..glyph },
        }
    }

    fn rasterized_from(&self, e: &HalGlyphEntry, source: GlyphSource) -> RasterizedGlyph {
        RasterizedGlyph { source, ..self.rasterized(e) }
    }

    pub fn rasterized(&self, e: &HalGlyphEntry) -> RasterizedGlyph {
        RasterizedGlyph {
            atlas_kind: match e.atlas_kind {
                wire::glyph::COLOR => AtlasKind::Color,
                wire::glyph::MSDF => AtlasKind::Msdf,
                _ => AtlasKind::Grayscale,
            },
            atlas_size: self.raster_size,
            atlas_image_bounds: Rect::new(Point::new(e.rect[0] as usize, e.rect[1] as usize), Size::new(e.rect[2] as usize, e.rect[3] as usize)),
            atlas_image_padding: e.padding as usize,
            atlas_plane: e.plane as u8,
            origin_in_dpxs: Point::new(e.origin[0], e.origin[1]),
            dpxs_per_em: e.scale,
            source: GlyphSource::default(),
        }
    }

    /// A slug glyph kept from an earlier draw (a prepared run), looked up again (see `refresh`).
    pub fn refresh_slug(&mut self, glyph: SlugGlyphInfo) -> SlugGlyphInfo {
        let source = glyph.source;
        if source.font == 0 {
            return glyph;
        }
        match self.lookup_local(source.font, source.glyph, 0.0, wire::kind::SLUG) {
            Lookup::Ready(e) => SlugGlyphInfo { source, ..Self::slug_info(&e) },
            // nothing there (any more): its old curves may be another glyph's now
            Lookup::Empty | Lookup::Pending => SlugGlyphInfo { curve_offset: 0, curve_count: 0, ..glyph },
        }
    }

    /// A slug glyph of `font` as the client holds it, with the key a kept copy is looked up by.
    pub fn slug(&mut self, font: &Font, glyph: GlyphId) -> SlugLookup {
        let local = self.local_of(font);
        let source = GlyphSource { font: local, glyph, kind: wire::kind::SLUG, dpxs_bits: 0 };
        match self.lookup_local(local, glyph, 0.0, wire::kind::SLUG) {
            Lookup::Ready(e) => SlugLookup::Ready(SlugGlyphInfo { source, ..Self::slug_info(&e) }),
            Lookup::Empty => SlugLookup::Empty,
            Lookup::Pending => match Self::unresolved_slug_info(font, glyph) {
                Some(info) => SlugLookup::Ready(SlugGlyphInfo { source, ..info }),
                None => SlugLookup::Empty,
            },
        }
    }

    pub fn slug_info(e: &HalGlyphEntry) -> SlugGlyphInfo {
        SlugGlyphInfo {
            origin_in_ems: Point::new(e.origin[0], e.origin[1]),
            size_in_ems: Size::new(e.scale, e.size_h),
            curve_offset: e.rect[0] as usize,
            curve_count: e.rect[1] as usize,
            band_offset: 0,
            band_count: 0,
            fill_flags: 0,
            source: GlyphSource::default(),
        }
    }

    /// The zero-curve slot covers nothing, but has the same box as the host's slug curves.
    pub fn unresolved_slug_info(font: &Font, glyph: GlyphId) -> Option<SlugGlyphInfo> {
        let bounds = font.glyph_outline_rc(glyph)?.bounds_in_ems();
        if bounds.size.width <= 0.000001 || bounds.size.height <= 0.000001 {
            return None;
        }
        Some(SlugGlyphInfo {
            origin_in_ems: bounds.origin,
            size_in_ems: bounds.size,
            curve_offset: 0,
            curve_count: 0,
            band_offset: 0,
            band_count: 0,
            fill_flags: 0,
            source: GlyphSource::default(),
        })
    }

    // ---- the host's answers

    pub fn on_opened(&mut self, local: u32, status: u32) {
        if status != wire::font_status::OPENED {
            self.failed.insert(local);
            // nothing will come for what was asked of it
            self.requested.retain(|((l, _, _), _)| *l != local);
        }
    }

    pub fn on_atlas(&mut self, kind: u32, width: u32, height: u32, generation: u32) {
        let old = if kind == wire::atlas::SLUG { &mut self.slug_generation } else { &mut self.raster_generation };
        let changed = *old != 0 && *old != generation;
        *old = generation;
        if kind == wire::atlas::RASTER {
            self.raster_size = Size::new(width as usize, height as usize);
        }
        if changed {
            // every entry of the older generation is void
            let slug = kind == wire::atlas::SLUG;
            self.ready.retain(|(_, _, k), _| (*k == wire::kind::SLUG) != slug);
            self.slots.retain(|_, (_, keys)| keys.first().is_none_or(|((_, _, k), _)| (*k == wire::kind::SLUG) != slug));
            self.epoch += 1;
        }
    }

    pub fn on_ready(&mut self, local: u32, entries: &[HalGlyphEntry]) {
        let frame = self.frame;
        for e in entries {
            if e.replaces != 0 {
                self.upgrade(e);
                continue;
            }
            let key: Key = (local, e.glyph as u16, e.kind);
            self.requested.remove(&(key, e.dpxs_bits));
            let held = self.ready.entry(key).or_default();
            match held.iter_mut().find(|h| h.dpxs_bits == e.dpxs_bits) {
                Some(h) => {
                    h.entry = *e;
                    h.used = frame;
                }
                None => held.push(Held { dpxs_bits: e.dpxs_bits, entry: *e, used: frame }),
            }
            if e.slot != 0 {
                self.slots.entry(e.slot).or_insert_with(|| (e.slot_gen, Vec::new())).1.push((key, e.dpxs_bits));
            }
        }
    }

    /// The plain field of slot `e.replaces` became a multi-channel one: its entries now name the
    /// new slot and rectangle (size, padding and metrics are the same).
    fn upgrade(&mut self, e: &HalGlyphEntry) {
        let Some((_, keys)) = self.slots.remove(&e.replaces) else { return };
        for (key, bits) in &keys {
            if let Some(h) = self.ready.get_mut(key).and_then(|v| v.iter_mut().find(|h| h.dpxs_bits == *bits)) {
                h.entry.atlas_kind = e.atlas_kind;
                h.entry.plane = e.plane;
                h.entry.rect = e.rect;
                h.entry.padding = e.padding;
                h.entry.slot = e.slot;
                h.entry.slot_gen = e.slot_gen;
            }
        }
        self.slots.insert(e.slot, (e.slot_gen, keys));
    }

    pub fn on_evicted(&mut self, slots: &[(u32, u32)]) {
        for (slot, gen) in slots {
            let Some((held_gen, keys)) = self.slots.remove(slot) else { continue };
            if held_gen != *gen {
                self.slots.insert(*slot, (held_gen, keys));
                continue;
            }
            for (key, bits) in keys {
                if let Some(v) = self.ready.get_mut(&key) {
                    v.retain(|h| h.dpxs_bits != bits);
                    if v.is_empty() {
                        self.ready.remove(&key);
                    }
                }
            }
            self.epoch += 1;
        }
    }

    /// The glyphs looked up since the last call, for the draw list that drew them.
    pub fn take_uses(&mut self, into: &mut Vec<GlyphUse>) {
        into.append(&mut self.uses);
    }

    /// Whether this frame's end sweeps (`sweep`): the caller then names the glyphs its kept
    /// draw lists still show.
    pub fn sweep_due(&self) -> bool {
        (self.frame + 1) % SWEEP_FRAMES == 0
    }

    /// A glyph a live draw list shows: in use, every size of it held.
    pub fn retained(&mut self, glyph: GlyphUse) {
        let frame = self.frame;
        if let Some(held) = self.ready.get_mut(&(glyph[0], glyph[1] as u16, glyph[2])) {
            for h in held.iter_mut() {
                h.used = frame;
            }
        }
    }

    /// The end of a draw. Every `SWEEP_FRAMES`: glyphs not in use for `IDLE_FRAMES` are let go
    /// of (their slots become the host's first candidates when its atlas is full), and the slots
    /// in use are touched, so a full atlas takes back what is least recently DRAWN, not what was
    /// delivered first.
    ///
    /// In use = looked up by a draw, or kept on screen by a live draw list (the caller reports
    /// those with `retained` before this, when `sweep_due`). A list that is not redrawn (a
    /// render target painted again from its kept instances, a code pane at rest) shows its
    /// glyphs without looking them up. Before, they were let go after IDLE_FRAMES, the host
    /// drew other glyphs into their slots, and words went blank or garbled ("Re ax").
    pub fn end_frame(&mut self) {
        self.frame += 1;
        // lookups no draw list took (a measurement) are no one's
        self.uses.clear();
        if self.frame % SWEEP_FRAMES != 0 {
            return;
        }
        let limit = self.frame.saturating_sub(IDLE_FRAMES);
        self.release_idle(limit);
        let since = self.frame.saturating_sub(SWEEP_FRAMES);
        let mut touched: Vec<(u32, u32)> = Vec::new();
        for held in self.ready.values() {
            for h in held {
                if h.used >= since && h.entry.slot != 0 {
                    touched.push((h.entry.slot, h.entry.slot_gen));
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        self.out_touches = touched;
    }

    fn release_idle(&mut self, limit: u64) {
        let mut dropped: Vec<(Key, u32, u32)> = Vec::new();
        self.ready.retain(|key, held| {
            held.retain(|h| {
                let keep = h.used >= limit;
                if !keep {
                    dropped.push((*key, h.dpxs_bits, h.entry.slot));
                }
                keep
            });
            !held.is_empty()
        });
        for (key, bits, slot) in dropped {
            if let Some((gen, keys)) = self.slots.get_mut(&slot) {
                keys.retain(|k| *k != (key, bits));
                if keys.is_empty() {
                    self.out_releases.push((slot, *gen));
                    self.slots.remove(&slot);
                }
            }
        }
    }

    pub fn take_outbox(&mut self) -> Outbox {
        let mut requests: Vec<(u32, u32, Vec<(u32, f32)>)> = self.out_requests.drain().map(|((local, kind), glyphs)| (local, kind, glyphs)).collect();
        requests.sort_by_key(|r| (r.0, r.1));
        let mut pending = std::mem::take(&mut self.out_pending);
        pending.extend(self.draw_pending.drain().map(|((font, glyph, kind), bits)| (font, glyph as u32, kind, bits)));
        Outbox { opens: std::mem::take(&mut self.out_opens), requests, releases: std::mem::take(&mut self.out_releases), touches: std::mem::take(&mut self.out_touches), pending }
    }

    /// Takes back an outbox's pending list once the host has been told, for the next frame's.
    pub fn recycle(&mut self, outbox: Outbox) {
        let mut pending = outbox.pending;
        pending.clear();
        self.out_pending = pending;
    }
}

/// How far apart two instances of a font are on their variation axes (an axis one of them
/// does not name counts as its full value).
fn axis_distance(a: Option<&Vec<(u32, f32)>>, b: Option<&Vec<(u32, f32)>>) -> f32 {
    let (a, b) = (a.map_or(&[][..], |v| v.as_slice()), b.map_or(&[][..], |v| v.as_slice()));
    let value = |v: &[(u32, f32)], tag: u32| v.iter().find(|(t, _)| *t == tag).map(|(_, x)| *x);
    let mut d = 0.0;
    for &(tag, x) in a {
        d += (x - value(b, tag).unwrap_or(0.0)).abs();
    }
    for &(tag, x) in b {
        if value(a, tag).is_none() {
            d += x.abs();
        }
    }
    d
}

fn ratio(have: f32, want: f32) -> f32 {
    let r = have.max(f32::MIN_POSITIVE) / want;
    if r < 1.0 { 1.0 / r } else { r }
}

#[derive(Debug)]
pub struct Rasterizer {
    settings: Settings,
    sdfer: Sdfer,
    msdfer: Msdfer,
    client: GlyphClient,
}

impl std::fmt::Debug for GlyphClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphClient").field("held", &self.held()).field("pending", &self.pending()).finish()
    }
}

impl Rasterizer {
    pub fn new(settings: Settings) -> Self {
        Self { settings, sdfer: Sdfer::new(settings.sdfer), msdfer: Msdfer::new(settings.msdfer), client: GlyphClient::default() }
    }

    /// Bumped when glyph entries were taken away: what was built on them is stale.
    pub fn epoch(&self) -> u64 {
        self.client.epoch()
    }

    pub fn client(&self) -> &GlyphClient {
        &self.client
    }

    pub fn client_mut(&mut self) -> &mut GlyphClient {
        &mut self.client
    }

    pub fn sdfer(&self) -> &Sdfer {
        &self.sdfer
    }

    pub fn msdfer(&self) -> &Msdfer {
        &self.msdfer
    }

    pub fn msdf_resolution(&self) -> MsdfResolutionSettings {
        self.settings.msdf_resolution
    }

    pub fn msdf_complexity(&self) -> MsdfComplexitySettings {
        self.settings.msdf_complexity
    }

    pub fn outline_rasterization_mode(&self) -> OutlineRasterizationMode {
        self.settings.outline_rasterization_mode
    }

    pub fn set_outline_rasterization_mode(&mut self, mode: OutlineRasterizationMode) {
        self.settings.outline_rasterization_mode = mode;
    }

    pub fn rasterize_glyph(&mut self, font: &Font, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<RasterizedGlyph> {
        let kind = match self.settings.outline_rasterization_mode {
            OutlineRasterizationMode::Sdf => wire::kind::SDF,
            OutlineRasterizationMode::Msdf => wire::kind::AUTO,
        };
        self.client
            .raster(font, glyph_id, dpxs_per_em, kind, &self.settings)
    }

    /// See `GlyphClient::refresh`: what a kept glyph draws as now.
    pub fn refresh_glyph(&mut self, glyph: RasterizedGlyph) -> RasterizedGlyph {
        self.client.refresh(glyph)
    }

    /// See `GlyphClient::refresh_slug`.
    pub fn refresh_slug_glyph(&mut self, glyph: SlugGlyphInfo) -> SlugGlyphInfo {
        self.client.refresh_slug(glyph)
    }

    /// See `GlyphClient::take_uses`.
    pub fn take_glyph_uses(&mut self, into: &mut Vec<GlyphUse>) {
        self.client.take_uses(into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        makepad_platform::SharedBytes,
        text::{font::FontId as Fid, font_face::FontFace, layouter, rasterizer::Rasterizer as Rz},
    };
    use std::{cell::RefCell, rc::Rc};

    fn instance(id: u64, wght: f32) -> Font {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources/RobotoFlex.ttf");
        let data = SharedBytes::from_file_mmap_or_read(path).expect("font");
        let mut face = FontFace::from_data_and_index(data, 0).expect("face");
        face.set_variations(&[(u32::from_be_bytes(*b"wght"), wght)]);
        Font::new(Fid::from(id), Rc::new(RefCell::new(Rz::new(layouter::Settings::default().loader.rasterizer))), face, 0.0, 0.0)
    }

    fn entry(glyph: u32, size: f32, slot: u32) -> HalGlyphEntry {
        HalGlyphEntry {
            glyph, dpxs_bits: size.to_bits(), kind: wire::kind::SDF, atlas_kind: wire::glyph::GRAY, plane: 0,
            rect: [8, 8, 12, 14], padding: 2, origin: [1.0, 9.0], scale: size, size_h: 0.0, slot, slot_gen: 1, replaces: 0, replaces_gen: 0,
        }
    }

    /// A glyph kept from a draw while it was still on its way (a prepared run a widget caches,
    /// as Sandbox's HUD figures do) draws with its pixels once they arrive: the draw looks it up
    /// again by its key. Before, it stayed a zero-coverage placeholder forever ('STRIKE' drew
    /// as 'STR  E' on macOS: I and K were not in the atlas at that size when the run was shaped).
    #[test]
    fn a_kept_glyph_that_was_pending_draws_once_its_answer_arrives() {
        let a = instance(0xB1, 400.0);
        let settings = layouter::Settings::default().loader.rasterizer;
        let mut client = GlyphClient::default();
        let kept = client.raster(&a, 44, 24.0, wire::kind::SDF, &settings).expect("planned");
        assert_eq!(kept.atlas_kind, AtlasKind::Unresolved);
        assert_ne!(kept.source.font, 0, "a lent-atlas glyph carries its key");
        // still on its way: it stays the placeholder and holds this draw
        let _ = client.take_outbox();
        assert_eq!(client.refresh(kept).atlas_kind, AtlasKind::Unresolved);
        assert!(!client.take_outbox().pending.is_empty(), "a kept pending glyph is reported pending");
        let local = kept.source.font;
        client.on_ready(local, &[entry(44, 24.0, 7)]);
        let now = client.refresh(kept);
        assert_eq!(now.atlas_kind, AtlasKind::Grayscale);
        assert_eq!(now.atlas_image_bounds, Rect::new(Point::new(8, 8), Size::new(12, 14)));
        assert_eq!(now.source, kept.source);
    }

    /// A glyph drawn only from a kept snapshot is in use: its slot is not let go after the idle
    /// frames (it was, and the host could give the slot to another glyph under the kept rect).
    #[test]
    fn a_kept_glyph_drawn_every_frame_keeps_its_slot() {
        let a = instance(0xB2, 400.0);
        let settings = layouter::Settings::default().loader.rasterizer;
        let mut client = GlyphClient::default();
        let first = client.raster(&a, 45, 30.0, wire::kind::SDF, &settings).expect("planned");
        client.on_ready(first.source.font, &[entry(45, 30.0, 9)]);
        let kept = client.raster(&a, 45, 30.0, wire::kind::SDF, &settings).expect("ready");
        assert_eq!(kept.atlas_kind, AtlasKind::Grayscale);
        for _ in 0..(IDLE_FRAMES + 2 * SWEEP_FRAMES) {
            let _ = client.refresh(kept);
            client.end_frame();
        }
        assert!(client.take_outbox().releases.is_empty(), "a glyph drawn every frame is not released");
        assert_eq!(client.held(), 1);
    }

    /// A glyph on screen in a draw list that is not redrawn (a render target painted again from
    /// its kept instances, a code pane at rest) is not looked up for a long while, yet still
    /// shown: the draw list keeps its glyphs (`take_uses`), and at the sweep the caller names
    /// them (`retained`). They keep their slots; a glyph no list shows is let go. Before, they
    /// were released after IDLE_FRAMES and the host drew other glyphs into their slots (the
    /// Relax deck title read "Re ax" on 149).
    #[test]
    fn a_glyph_in_a_retained_list_keeps_its_slot_and_an_unshown_one_is_let_go() {
        let a = instance(0xB3, 400.0);
        let settings = layouter::Settings::default().loader.rasterizer;
        let mut client = GlyphClient::default();
        let shown = client.raster(&a, 46, 18.0, wire::kind::SDF, &settings).expect("planned");
        let mut list = Vec::new();
        client.take_uses(&mut list);
        let _ = client.raster(&a, 47, 18.0, wire::kind::SDF, &settings).expect("planned");
        client.on_ready(shown.source.font, &[entry(46, 18.0, 11), entry(47, 18.0, 12)]);
        let _ = client.take_outbox();
        assert_eq!(list, vec![[shown.source.font, 46, wire::kind::SDF]], "the list keeps what it drew");
        let mut releases = Vec::new();
        for _ in 0..(IDLE_FRAMES + 2 * SWEEP_FRAMES) {
            if client.sweep_due() {
                for glyph in &list {
                    client.retained(*glyph);
                }
            }
            client.end_frame();
            releases.extend(client.take_outbox().releases);
        }
        assert_eq!(releases, vec![(12, 1)], "only the glyph no list shows is let go");
        assert_eq!(client.held(), 1);
    }

    /// The slots in use are touched at every sweep, so the host's eviction takes the least
    /// recently DRAWN glyphs, not the ones delivered first (a title on screen for minutes was
    /// the oldest delivery, so the first taken when the store filled).
    #[test]
    fn the_sweep_touches_the_slots_in_use() {
        let a = instance(0xB4, 400.0);
        let settings = layouter::Settings::default().loader.rasterizer;
        let mut client = GlyphClient::default();
        let g = client.raster(&a, 48, 18.0, wire::kind::SDF, &settings).expect("planned");
        let _ = client.raster(&a, 49, 18.0, wire::kind::SDF, &settings).expect("planned");
        client.on_ready(g.source.font, &[entry(48, 18.0, 21), entry(49, 18.0, 22)]);
        let _ = client.take_outbox();
        let mut touches = Vec::new();
        // the first sweep still sees both deliveries; the second only what was drawn since
        for _ in 0..2 * SWEEP_FRAMES {
            let _ = client.raster(&a, 48, 18.0, wire::kind::SDF, &settings);
            client.end_frame();
            let out = client.take_outbox();
            if !out.touches.is_empty() {
                touches = out.touches;
            }
        }
        assert_eq!(touches, vec![(21, 1)], "only the slot drawn since the last sweep is touched");
    }

    /// A slug glyph kept in a prepared run is looked up again when drawn: curves taken back and
    /// delivered again elsewhere draw from where they are now.
    #[test]
    fn a_kept_slug_glyph_follows_its_curves() {
        let a = instance(0xB5, 400.0);
        let mut client = GlyphClient::default();
        let SlugLookup::Ready(kept) = client.slug(&a, 50) else { panic!("an outline glyph") };
        assert_eq!(kept.curve_count, 0, "on its way");
        let local = kept.source.font;
        let slug = |offset: u32, slot: u32| HalGlyphEntry {
            glyph: 50, dpxs_bits: 0, kind: wire::kind::SLUG, atlas_kind: wire::glyph::SLUG, plane: 0,
            rect: [offset, 12, 0, 0], padding: 0, origin: [0.0; 2], scale: 0.5, size_h: 0.7, slot, slot_gen: 1, replaces: 0, replaces_gen: 0,
        };
        client.on_ready(local, &[slug(100, 31)]);
        assert_eq!(client.refresh_slug(kept).curve_offset, 100);
        client.on_evicted(&[(31, 1)]);
        assert_eq!(client.refresh_slug(kept).curve_count, 0, "taken back: drawn as nothing (not someone else's curves), asked for again");
        client.on_ready(local, &[slug(400, 32)]);
        assert_eq!(client.refresh_slug(kept).curve_offset, 400);
    }

    /// An animated axis makes a new font instance per value: until its own glyph arrives the
    /// same glyph of another instance of the file stands in (no blank, no frame held back).
    #[test]
    fn a_new_variable_instance_draws_with_a_sibling_until_its_own_glyph_arrives() {
        let (a, b) = (instance(0xA1, 300.0), instance(0xA2, 301.0));
        let mut client = GlyphClient::default();
        // the first instance is asked for and answered
        assert!(matches!(client.lookup(&a, 5, 20.0, wire::kind::SDF), Lookup::Pending));
        let la = client.local_of(&a);
        let entry = HalGlyphEntry {
            glyph: 5, dpxs_bits: 20.0f32.to_bits(), kind: wire::kind::SDF, atlas_kind: wire::glyph::GRAY, plane: 0,
            rect: [4, 4, 16, 16], padding: 2, origin: [0.0; 2], scale: 20.0, size_h: 0.0, slot: 9, slot_gen: 1, replaces: 0, replaces_gen: 0,
        };
        client.on_ready(la, &[entry]);
        let _ = client.take_outbox();
        // the second instance: nothing of its own, a sibling's glyph stands in and holds nothing back
        assert!(matches!(client.lookup(&b, 5, 20.0, wire::kind::SDF), Lookup::Ready(_)));
        let out = client.take_outbox();
        assert!(out.pending.is_empty(), "a glyph with a stand-in does not hold the frame");
        assert!(out.requests.iter().any(|r| !r.2.is_empty()), "its own glyph is still asked for");
    }

    /// Slug curves (all text by default) of a new variable instance: the curves of the instance
    /// nearest on the axis stand in until its own arrive, never the zero-curve box. Before, a
    /// letter whose animated weight stepped to a value not drawn yet drew blank for that frame
    /// (the motion deck's weight wave flickered: Motion records a frame once, so the HAL's wait
    /// for the answer redrew nothing).
    #[test]
    fn a_new_variable_instance_draws_slug_curves_of_the_nearest_weight_until_its_own_arrive() {
        let (light, heavy, mid) = (instance(0xC1, 300.0), instance(0xC2, 800.0), instance(0xC3, 760.0));
        let mut client = GlyphClient::default();
        let slug = |offset: u32, slot: u32| HalGlyphEntry {
            glyph: 50, dpxs_bits: 0, kind: wire::kind::SLUG, atlas_kind: wire::glyph::SLUG, plane: 0,
            rect: [offset, 12, 0, 0], padding: 0, origin: [0.0; 2], scale: 0.5, size_h: 0.7, slot, slot_gen: 1, replaces: 0, replaces_gen: 0,
        };
        let SlugLookup::Ready(first) = client.slug(&light, 50) else { panic!("an outline glyph") };
        assert_eq!(first.curve_count, 0, "the first instance of the file has nothing to stand in");
        client.on_ready(first.source.font, &[slug(100, 41)]);
        let SlugLookup::Ready(second) = client.slug(&heavy, 50) else { panic!("an outline glyph") };
        assert_eq!(second.curve_offset, 100, "the only other instance stands in");
        client.on_ready(second.source.font, &[slug(200, 42)]);
        let _ = client.take_outbox();
        // a weight not drawn before: the nearest weight's curves, and nothing holds the frame
        let SlugLookup::Ready(now) = client.slug(&mid, 50) else { panic!("an outline glyph") };
        assert_eq!((now.curve_offset, now.curve_count), (200, 12), "760 draws with 800's curves, not blank");
        let out = client.take_outbox();
        assert!(out.pending.is_empty(), "a stand-in does not hold the frame");
        assert!(out.requests.iter().any(|r| r.1 == wire::kind::SLUG && r.2.iter().any(|g| g.0 == 50)), "its own curves are asked for");
        // its own curves, once here
        client.on_ready(now.source.font, &[slug(300, 43)]);
        assert_eq!(client.refresh_slug(now).curve_offset, 300);
    }
}
