//! The font service: the host-owned glyph atlases and the rasterising workers behind them.
//!
//! Sessions open fonts (the host maps each file once, refcounted per session) and ask for
//! glyphs; the workers render them; the service puts them into the shared atlases, uploads the
//! changed rectangles to the host's textures and tells every session that asked where the glyph
//! is. The atlases belong to the host (session 0): a session's close drops its interest, never
//! the glyphs, so a relaunched app finds its text already rendered. Under pressure the least
//! recently used slots are taken back and the sessions that held them are told.

use super::atlas::{RasterAtlas, SlugStore};
use super::pool::{Claim, Done, HostFont, HostSource, Job, JobKind, Known, Payload, Pool, ReqKey, Work};
use super::wire::{self, atlas, font_status, glyph, kind, FontSource, GlyphEntry};
use super::{FrameProgress, RetireFence};
use crate::backend::exec::{self, Gpu};
use crate::cmds::HalCmd;
use crate::session::{LentRef, TextureObj, World, HOST};
use crate::types::*;
use makepad_font_core::{
    font_atlas::GlyphImageKey,
    geom::{Point, Rect, Size},
    raster::{default_text_atlas_size, Settings, UNRESOLVED_RASTER_RECT},
    MappedBytes,
};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};

/// Width and height of the slug curve texture, in texels (two per curve: 131072 curves).
const SLUG_WIDTH: usize = 2048;
const SLUG_ROWS: usize = 128;
/// How much of an atlas an eviction frees.
const EVICT_FRACTION: f32 = 0.25;
/// Entries per `GlyphsReady` event.
const ENTRIES_PER_EVENT: usize = 256;

/// The host's strings for its own labels (the ledger names the atlases by them).
const LABEL_RASTER: StrId = 1;
const LABEL_SLUG: StrId = 2;

/// What the service did, for tests, the ledger and measurements.
#[derive(Clone, Debug, Default)]
pub struct FontStats {
    /// fonts opened by sessions / files actually mapped / opens that found the file mapped
    pub opens: u64,
    pub maps: u64,
    pub shared_opens: u64,
    /// glyphs asked for, over all sessions
    pub glyph_requests: u64,
    /// glyph images / curve sets rendered by the workers, and the time that took (ns)
    pub rasters: u64,
    pub raster_ns: u64,
    /// jobs the atlas answered without rendering
    pub cached: u64,
    /// entries sent, multi-channel upgrades sent
    pub delivered: u64,
    pub upgrades: u64,
    /// slots taken back, and how many times the atlas had to
    pub evicted_slots: u64,
    pub evictions: u64,
    /// glyphs that could not be placed even in an empty atlas (delivered as empty)
    pub oversized: u64,
    pub bad: u64,
    /// bytes of the host's atlas textures, slots / glyphs in them
    pub raster_bytes: u64,
    pub slug_bytes: u64,
    pub raster_slots: usize,
    pub slug_glyphs: usize,
    /// curves the slug store holds (live glyphs), and how many it can hold
    pub slug_curves: u64,
    pub slug_capacity: u64,
    /// jobs queued or running
    pub in_flight: usize,
}

/// What one session used.
#[derive(Clone, Debug, Default)]
pub struct SessionFontStats {
    pub requested: u64,
    /// images this session was the first to ask for (it paid for them), and their time (ns)
    pub rastered: u64,
    pub raster_ns: u64,
    pub delivered: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct HostKey {
    source: String,
    index: u32,
    vars: Vec<(u32, u32)>,
}

struct FontEntry {
    host: Option<Arc<HostFont>>,
    /// the sessions that have it open, and how many times
    refs: HashMap<u32, u32>,
    len: u64,
    hash: u64,
}

pub struct FontService {
    settings: Settings,
    known: Arc<RwLock<Known>>,
    pool: Option<Pool>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    fonts: HashMap<u32, FontEntry>,
    by_key: HashMap<HostKey, u32>,
    next_font: u32,
    /// (session, the app's font id) -> the host's font id
    session_fonts: HashMap<(u32, u32), u32>,
    raster: Option<RasterAtlas>,
    slug: Option<SlugStore>,
    /// requests being rendered, and who is waiting: (session, the app's font id)
    inflight: HashMap<ReqKey, Vec<(u32, u32)>>,
    msdf_pending: HashSet<GlyphImageKey>,
    /// requests waiting for the worker that holds the claim on their image
    parked: HashMap<Claim, Vec<ReqKey>>,
    /// Finished rasters keep their pixels and claims while evicted storage drains on the GPU.
    deferred: Vec<Done>,
    /// A closed session may still have GPU frames sampling the host's atlas.
    closed_frames: Vec<Arc<FrameProgress>>,
    /// the lent ids sessions sample the atlases by
    aliases: HashMap<(u32, u32), Id>,
    next_host_texture: u32,
    next_slot: u32,
    /// The use rank a slot gets (its `last_use`; eviction takes the lowest first): a delivery
    /// ranks `tick`, a `GlyphTouch` (the session drew from it since its last touch) `tick + 1`,
    /// and each touch moves `tick` on by two. So a glyph a session still shows outranks one
    /// delivered in the same period and not drawn since, and the oldest unused go first.
    tick: u64,
    stats: FontStats,
    session_stats: HashMap<u32, SessionFontStats>,
    /// entries to deliver at the end of the pass: (session, font) -> entries
    out: HashMap<(u32, u32), Vec<GlyphEntry>>,
}

impl FontService {
    pub fn new(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> FontService {
        let mut settings = Settings::default();
        settings.atlas_size = default_text_atlas_size();
        FontService {
            settings,
            known: Arc::new(RwLock::new(Known::default())),
            pool: None,
            notify,
            fonts: HashMap::new(),
            by_key: HashMap::new(),
            next_font: 1,
            session_fonts: HashMap::new(),
            raster: None,
            slug: None,
            inflight: HashMap::new(),
            msdf_pending: HashSet::new(),
            parked: HashMap::new(),
            deferred: Vec::new(),
            closed_frames: Vec::new(),
            aliases: HashMap::new(),
            next_host_texture: 1,
            next_slot: 1,
            tick: 0,
            stats: FontStats::default(),
            session_stats: HashMap::new(),
            out: HashMap::new(),
        }
    }

    /// The atlas size to use (set before the first request; tests shrink it to force eviction).
    pub fn set_atlas_size(&mut self, size: Size<usize>) {
        if self.raster.is_none() {
            self.settings.atlas_size = size;
        }
    }

    pub fn stats(&self) -> FontStats {
        let mut s = self.stats.clone();
        s.raster_bytes = self.raster.as_ref().map_or(0, |a| a.bytes());
        s.slug_bytes = self.slug.as_ref().map_or(0, |a| a.bytes());
        s.raster_slots = self.raster.as_ref().map_or(0, |a| a.len());
        s.slug_glyphs = self.slug.as_ref().map_or(0, |a| a.len());
        s.slug_curves = self.slug.as_ref().map_or(0, |a| a.curves_held());
        s.slug_capacity = self.slug.as_ref().map_or(0, |a| a.capacity_curves() as u64);
        s.in_flight = self.in_flight();
        s.raster_ns = self.pool.as_ref().map_or(0, |p| p.busy_ns());
        s
    }

    /// The raster atlas's pixels in `rect` (x, y, w, h) as the host holds them (BGRA8 rows): what
    /// the texture shows once its uploads are done. For tests and diagnostics.
    pub fn raster_rect_bytes(&self, rect: [u32; 4]) -> Option<Vec<u8>> {
        let a = self.raster.as_ref()?;
        let r = makepad_font_core::geom::Rect::new(Point::new(rect[0] as usize, rect[1] as usize), Size::new(rect[2] as usize, rect[3] as usize));
        Some(a.rect_bytes(r))
    }

    pub fn session_stats(&self, session: u32) -> SessionFontStats {
        self.session_stats.get(&session).cloned().unwrap_or_default()
    }

    /// Whether a worker has a result or a retired allocation can be reclaimed.
    pub fn has_done(&self) -> bool {
        self.pool.as_ref().is_some_and(|p| p.has_done())
            || self.raster.as_ref().is_some_and(|a| a.can_reclaim())
            || self.slug.as_ref().is_some_and(|a| a.can_reclaim())
    }

    /// Jobs queued, running or holding finished pixels until atlas storage is safe to reuse.
    pub fn in_flight(&self) -> usize {
        self.pool.as_ref().map_or(0, |p| p.in_flight()) + self.deferred.len()
    }

    fn pool(&mut self) -> &Pool {
        let (settings, known, notify) = (self.settings, self.known.clone(), self.notify.clone());
        self.pool.get_or_insert_with(|| Pool::new(settings, known, notify))
    }

    // ---- commands

    pub fn command(&mut self, gpu: &mut dyn Gpu, world: &mut World, sid: u32, c: HalCmd<'_>) {
        match c {
            HalCmd::FontOpen { font, index, kind, nvars, desc } => self.font_open(world, sid, font, index, kind, nvars, desc),
            HalCmd::GlyphRequest { font, kind, count, glyphs } => self.glyph_request(gpu, world, sid, font, kind, count, glyphs),
            HalCmd::GlyphRelease { count, slots } => self.glyph_release(sid, count, slots),
            HalCmd::GlyphTouch { count, slots } => self.glyph_touch(count, slots),
            _ => {}
        }
        self.flush(world);
    }

    fn font_open(&mut self, world: &mut World, sid: u32, local: u32, index: u32, kind: u32, nvars: u32, desc: &[u8]) {
        self.stats.opens += 1;
        let answer = |world: &mut World, status: u32, len: u64, hash: u64, path: &str| {
            world.to_session(sid, |q| q.font_opened(local, status, len, hash, path.as_bytes()));
        };
        let Some((vars, source)) = wire::parse_font_open(kind, nvars, desc) else {
            self.stats.bad += 1;
            answer(world, font_status::BAD, 0, 0, "");
            return;
        };
        let var_bits: Vec<(u32, u32)> = vars.iter().map(|(t, v)| (*t, v.to_bits())).collect();
        // what the file is: a mapping we may already hold, or bytes
        let (key_source, path) = match source {
            FontSource::Path(p) => {
                let canon = std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string());
                (canon.clone(), canon)
            }
            FontSource::Bytes(b) => (format!("bytes:{}:{:016x}", b.len(), table_hash(b, index)), String::new()),
        };
        let key = HostKey { source: key_source, index, vars: var_bits };
        let id = match self.by_key.get(&key).copied() {
            Some(id) => id,
            None => {
                let id = self.next_font;
                self.next_font += 1;
                self.by_key.insert(key.clone(), id);
                self.fonts.insert(id, FontEntry { host: None, refs: HashMap::new(), len: 0, hash: 0 });
                id
            }
        };
        let entry = self.fonts.get_mut(&id).expect("font entry");
        if entry.host.is_some() {
            self.stats.shared_opens += 1;
        } else {
            // map the file (once, for every session and worker), or take the bytes
            let source = match source {
                FontSource::Path(_) => match open_font_file(&path) {
                    Ok(m) => {
                        entry.len = m.len() as u64;
                        entry.hash = table_hash(m.as_slice(), index);
                        HostSource::Mapped(m)
                    }
                    Err(_) => {
                        answer(world, font_status::MISSING, 0, 0, &path);
                        return;
                    }
                },
                FontSource::Bytes(b) => {
                    entry.len = b.len() as u64;
                    entry.hash = table_hash(b, index);
                    HostSource::Inline(Arc::new(b.to_vec()))
                }
            };
            let readable = match &source {
                HostSource::Mapped(m) => makepad_font_core::rustybuzz::ttf_parser::RawFace::parse(m.as_slice(), index).is_ok(),
                HostSource::Inline(v) => makepad_font_core::rustybuzz::ttf_parser::RawFace::parse(v.as_slice(), index).is_ok(),
            };
            if !readable {
                answer(world, font_status::BAD, entry.len, 0, &path);
                return;
            }
            self.stats.maps += 1;
            entry.host = Some(Arc::new(HostFont { id, source, index, vars }));
        }
        let (len, hash) = (entry.len, entry.hash);
        *entry.refs.entry(sid).or_default() += 1;
        // opening the same app id again replaces what it named (the old open is let go)
        if let Some(old) = self.session_fonts.insert((sid, local), id) {
            if old != id {
                self.release_font(sid, old);
            } else if let Some(e) = self.fonts.get_mut(&id) {
                // the same font under the same id: one reference only
                if let Some(n) = e.refs.get_mut(&sid) {
                    *n = n.saturating_sub(1).max(1);
                }
            }
        }
        answer(world, font_status::OPENED, len, hash, &path);
    }

    fn release_font(&mut self, sid: u32, id: u32) {
        let Some(e) = self.fonts.get_mut(&id) else { return };
        if let Some(n) = e.refs.get_mut(&sid) {
            *n -= 1;
            if *n == 0 {
                e.refs.remove(&sid);
            }
        }
        if e.refs.is_empty() {
            // nobody has it open: the mapping goes (the glyphs it rendered stay in the atlas)
            e.host = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn glyph_request(&mut self, gpu: &mut dyn Gpu, world: &mut World, sid: u32, local: u32, req_kind: u32, count: u32, glyphs: &[u8]) {
        let Some(gfont) = self.session_fonts.get(&(sid, local)).copied() else {
            if let Some(s) = world.session_mut(sid) {
                s.stale();
            }
            return;
        };
        let Some(host) = self.fonts.get(&gfont).and_then(|e| e.host.clone()) else { return };
        if req_kind > kind::SLUG {
            self.stats.bad += 1;
            return;
        }
        let atlas_kind = if req_kind == kind::SLUG { atlas::SLUG } else { atlas::RASTER };
        if !self.ensure_atlas(gpu, world, atlas_kind) || self.ensure_alias(world, sid, atlas_kind).is_none() {
            return;
        }
        for (glyph, dpxs) in wire::read_glyphs(glyphs, count) {
            let sizeless = req_kind == kind::SLUG;
            if glyph > u16::MAX as u32 || (!sizeless && (!dpxs.is_finite() || dpxs <= 0.0)) {
                self.stats.bad += 1;
                continue;
            }
            self.stats.glyph_requests += 1;
            self.session_stats.entry(sid).or_default().requested += 1;
            // curves do not depend on the size: one request for any size
            let dpxs_bits = if req_kind == kind::SLUG { 0 } else { dpxs.to_bits() };
            let req = ReqKey { font: gfont, glyph: glyph as u16, dpxs_bits, kind: req_kind };
            match self.inflight.get_mut(&req) {
                Some(waiting) => {
                    if !waiting.contains(&(sid, local)) {
                        waiting.push((sid, local));
                    }
                }
                None => {
                    self.inflight.insert(req, vec![(sid, local)]);
                    self.pool().submit(Job { kind: JobKind::Glyph { req }, font: host.clone(), glyph: glyph as u16, dpxs });
                }
            }
        }
    }

    /// The slots a session drew from recently: they are used now (eviction takes the least
    /// recently used first).
    fn glyph_touch(&mut self, count: u32, slots: &[u8]) {
        self.tick += 2;
        let tick = self.tick + 1;
        for (slot, gen) in wire::read_slots(slots, count) {
            if let Some(rec) = self.raster.as_mut().filter(|a| a.generation == gen).and_then(|a| a.slot_mut(slot)) {
                rec.last_use = tick;
                continue;
            }
            if let Some(rec) = self.slug.as_mut().filter(|a| a.generation == gen).and_then(|a| a.by_slot_mut(slot)) {
                rec.last_use = tick;
            }
        }
    }

    fn glyph_release(&mut self, sid: u32, count: u32, slots: &[u8]) {
        for (slot, gen) in wire::read_slots(slots, count) {
            if let Some(a) = self.raster.as_mut().filter(|a| a.generation == gen) {
                if let Some(rec) = a.slot_mut(slot) {
                    rec.sessions.retain(|s| *s != sid);
                    continue;
                }
            }
            if let Some(a) = self.slug.as_mut().filter(|a| a.generation == gen) {
                if let Some(rec) = a.by_slot_mut(slot) {
                    rec.sessions.retain(|s| *s != sid);
                }
            }
        }
    }

    /// A session ended: its interest in glyphs and its font references go; the glyphs stay.
    pub(crate) fn session_closed(&mut self, sid: u32, frames: Arc<FrameProgress>) {
        if frames.pending() {
            self.closed_frames.push(frames);
        }
        let mine: Vec<((u32, u32), u32)> = self.session_fonts.iter().filter(|((s, _), _)| *s == sid).map(|(k, v)| (*k, *v)).collect();
        for (key, gfont) in mine {
            self.session_fonts.remove(&key);
            if let Some(e) = self.fonts.get_mut(&gfont) {
                e.refs.remove(&sid);
                if e.refs.is_empty() {
                    e.host = None;
                }
            }
        }
        for waiting in self.inflight.values_mut() {
            waiting.retain(|(s, _)| *s != sid);
        }
        self.aliases.retain(|(s, _), _| *s != sid);
        if let Some(a) = self.raster.as_mut() {
            a.session_closed(sid);
        }
        if let Some(a) = self.slug.as_mut() {
            a.session_closed(sid);
        }
        self.out.retain(|(s, _), _| *s != sid);
        self.session_stats.remove(&sid);
    }

    // ---- the host's textures

    fn host_texture(&mut self, gpu: &mut dyn Gpu, world: &mut World, fmt: u32, w: usize, h: usize, label: StrId, now: u64) -> Option<Id> {
        let id = Id::new(self.next_host_texture, 1);
        self.next_host_texture += 1;
        let host = world.session_mut(HOST)?;
        host.strings.entry(label).or_insert_with(|| if label == LABEL_RASTER { "font glyph atlas".into() } else { "font slug curves".into() });
        let obj = TextureObj { format: fmt, w: w as u32, h: h as u32, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: exec::texture_bytes(fmt, w as u32, h as u32, 1, 1), imported: false, uploaded: 0, lent: None };
        exec::insert_texture(gpu, host, id, obj, Attr { label, ..Default::default() }, now);
        host.textures.get(HOST, id).is_ok().then_some(id)
    }

    /// Creates the atlas of `kind` on first use. False when the device refused it.
    fn ensure_atlas(&mut self, gpu: &mut dyn Gpu, world: &mut World, kind: u32) -> bool {
        let now = world.now();
        if kind == atlas::RASTER {
            if self.raster.is_some() {
                return true;
            }
            let size = self.settings.atlas_size;
            let Some(tex) = self.host_texture(gpu, world, format::BGRA8, size.width, size.height, LABEL_RASTER, now) else { return false };
            self.raster = Some(RasterAtlas::new(tex, size, self.known.clone()));
            // Texture allocation need not clear GPU storage. Initialise the reserved slot before
            // lending the atlas, so pending glyphs always sample transparent pixels.
            let rect = UNRESOLVED_RASTER_RECT;
            self.upload(
                gpu,
                world,
                tex,
                [
                    rect.origin.x as u32,
                    rect.origin.y as u32,
                    rect.size.width as u32,
                    rect.size.height as u32,
                ],
                &[0; 4],
            );
        } else {
            if self.slug.is_some() {
                return true;
            }
            // the curve store follows the atlas size (MAKEPAD_TEXT_ATLAS_SIZE shrinks both for tests)
            let rows = (SLUG_ROWS * self.settings.atlas_size.width / 2048).clamp(4, SLUG_ROWS * 4);
            let Some(tex) = self.host_texture(gpu, world, format::RGBA32F, SLUG_WIDTH, rows, LABEL_SLUG, now) else { return false };
            self.slug = Some(SlugStore::new(tex, SLUG_WIDTH, rows, self.known.clone()));
        }
        true
    }

    /// The id `sid` samples the atlas by; the first time, its `AtlasInfo` is queued.
    fn ensure_alias(&mut self, world: &mut World, sid: u32, kind: u32) -> Option<Id> {
        if let Some(id) = self.aliases.get(&(sid, kind)) {
            return Some(*id);
        }
        let (tex, fmt, w, h, generation) = if kind == atlas::RASTER {
            let a = self.raster.as_ref()?;
            (a.tex, format::BGRA8, a.size.width as u32, a.size.height as u32, a.generation)
        } else {
            let a = self.slug.as_ref()?;
            (a.tex, format::RGBA32F, a.width as u32, a.rows as u32, a.generation)
        };
        // the host samples its atlas directly, a session through a lent id
        let alias = if sid == HOST {
            tex
        } else {
            let alias = world.alloc_lent();
            let obj = TextureObj { format: fmt, w, h, depth: 1, mips: 1, usage: usage::SAMPLED, bytes: 0, imported: false, uploaded: 0, lent: Some(LentRef { child: HOST, tex, window: Id::NONE }) };
            let s = world.session_mut(sid)?;
            s.textures.insert(sid, alias, HOST, obj).ok()?;
            alias
        };
        self.aliases.insert((sid, kind), alias);
        world.to_session(sid, |q| q.atlas_info(kind, alias, w, h, generation));
        Some(alias)
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, world: &mut World, tex: Id, rect: [u32; 4], bytes: &[u8]) {
        let Some(host) = world.session_mut(HOST) else { return };
        let Ok(e) = host.textures.get_mut(HOST, tex) else { return };
        e.val.uploaded += bytes.len() as u64;
        let obj = e.val.clone();
        gpu.texture_update(host, tex, &obj, 0, rect, 0, bytes);
    }

    /// The device was replaced: the atlas textures have new, empty storage under their ids; the
    /// service uploads its own copy of both whole (sessions keep every slot they were told of).
    pub fn device_reset(&mut self, gpu: &mut dyn Gpu, world: &mut World) {
        if let Some(a) = self.raster.as_ref() {
            let (tex, size) = (a.tex, a.size);
            let bytes = a.rect_bytes(Rect::new(Point::new(0, 0), size));
            self.upload(gpu, world, tex, [0, 0, size.width as u32, size.height as u32], &bytes);
        }
        if let Some(a) = self.slug.as_ref() {
            let (tex, w, rows) = (a.tex, a.width as u32, a.rows as u32);
            let bytes = a.all_bytes();
            self.upload(gpu, world, tex, [0, 0, w, rows], &bytes);
        }
    }

    // ---- finished work

    /// Takes what the workers finished: places it in the atlases, uploads it, tells the sessions.
    pub fn service(&mut self, gpu: &mut dyn Gpu, world: &mut World) {
        let raster_reclaimed = self.raster.as_mut().is_some_and(|a| a.reclaim());
        let slug_reclaimed = self.slug.as_mut().is_some_and(|a| a.reclaim());
        self.closed_frames.retain(|frames| frames.pending());
        let done = match self.pool.as_ref() {
            Some(p) if p.has_done() => p.take_done(),
            _ => Vec::new(),
        };
        if raster_reclaimed || slug_reclaimed {
            for d in std::mem::take(&mut self.deferred) {
                self.apply(gpu, world, d, false);
            }
        }
        for d in done {
            self.apply(gpu, world, d, true);
        }
        self.flush(world);
    }

    fn apply(&mut self, gpu: &mut dyn Gpu, world: &mut World, mut d: Done, newly_done: bool) {
        if newly_done {
            if d.cached {
                self.stats.cached += 1;
            } else if !matches!(d.payload, Payload::Empty) {
                self.stats.rasters += 1;
            }
        }
        let deferred = match &d.job {
            JobKind::Msdf { key, sdf_slot, .. } => {
                let deferred = if let Payload::Msdf { key, pixels } = d.payload {
                    self.apply_msdf(gpu, world, key, pixels, *sdf_slot)
                } else {
                    None
                };
                if deferred.is_none() {
                    self.msdf_pending.remove(key);
                }
                deferred
            }
            JobKind::Glyph { req } => {
                // who paid for it
                if newly_done && !d.cached && !matches!(d.payload, Payload::Empty) {
                    if let Some((sid, _)) = self.inflight.get(req).and_then(|w| w.first()) {
                        let st = self.session_stats.entry(*sid).or_default();
                        st.rastered += 1;
                        st.raster_ns += d.ns;
                    }
                }
                self.apply_glyph(gpu, world, *req, d.payload)
            }
        };
        if let Some(payload) = deferred {
            d.payload = payload;
            self.deferred.push(d);
            return;
        }
        // the result is placed (or lost): requests that waited for this one look again
        if let Some(claim) = d.claim {
            known_release(&self.known, &claim);
            for req in self.parked.remove(&claim).unwrap_or_default() {
                self.retry(req);
            }
        }
    }

    /// Runs a request through the workers again (its entry vanished between the worker's look and
    /// the service's, or the one that was rendering it is done).
    fn retry(&mut self, req: ReqKey) {
        let Some(host) = self.fonts.get(&req.font).and_then(|e| e.host.clone()) else {
            self.inflight.remove(&req);
            return;
        };
        let dpxs = f32::from_bits(req.dpxs_bits);
        self.pool().submit(Job { kind: JobKind::Glyph { req }, font: host, glyph: req.glyph, dpxs });
    }

    /// Waits for the worker that holds `claim`; if it is already done, asks again at once.
    fn park(&mut self, claim: Claim, req: ReqKey) {
        if known_is_claimed(&self.known, &claim) {
            self.parked.entry(claim).or_default().push(req);
        } else {
            self.retry(req);
        }
    }

    fn apply_glyph(&mut self, gpu: &mut dyn Gpu, world: &mut World, req: ReqKey, payload: Payload) -> Option<Payload> {
        let tick = self.tick;
        let mut entry = GlyphEntry {
            glyph: req.glyph as u32,
            dpxs_bits: req.dpxs_bits,
            kind: req.kind,
            atlas_kind: glyph::EMPTY,
            plane: 0,
            rect: [0; 4],
            padding: 0,
            origin: [0.0; 2],
            scale: 0.0,
            size_h: 0.0,
            slot: 0,
            slot_gen: 0,
            replaces: 0,
            replaces_gen: 0,
        };
        let mut msdf_job = None;
        match payload {
            // nothing to draw: delivered as an empty entry, so the app stops asking
            Payload::Empty => {}
            Payload::Msdf { .. } => return None,
            Payload::Slug { origin_in_ems, size_in_ems, curves } => {
                let font = req.font;
                let existing = self.slug.as_ref().and_then(|s| s.find(font, req.glyph)).map(|r| r.slot);
                let slot = match (existing, curves) {
                    (Some(slot), _) => Some(slot),
                    (None, Work::Rendered(curves)) => match self.place_slug(gpu, world, font, req.glyph, &curves, origin_in_ems, size_in_ems) {
                        Ok(slot) => slot,
                        Err(()) => return Some(Payload::Slug { origin_in_ems, size_in_ems, curves: Work::Rendered(curves) }),
                    },
                    (None, Work::Elsewhere) => {
                        self.park(Claim::Slug(font, req.glyph), req);
                        return None;
                    }
                    // the worker found it in the atlas, and it has been taken back since
                    (None, Work::InAtlas) => {
                        self.retry(req);
                        return None;
                    }
                };
                if let Some(slot) = slot {
                    let generation = self.slug.as_ref().map_or(0, |s| s.generation);
                    if let Some(rec) = self.slug.as_mut().and_then(|s| s.by_slot_mut(slot)) {
                        rec.last_use = tick;
                        entry.atlas_kind = glyph::SLUG;
                        entry.rect = [rec.offset, rec.count, 0, 0];
                        entry.origin = [rec.origin.x, rec.origin.y];
                        entry.scale = rec.size.width;
                        entry.size_h = rec.size.height;
                        entry.slot = slot;
                        entry.slot_gen = generation;
                    }
                }
            }
            Payload::Sdf { key, pixels, padding, origin, dpxs, msdf } => {
                match self.place_image(gpu, world, key.clone(), glyph::GRAY, padding, Image::Sdf(pixels), tick, req) {
                    Place::Waiting => return None,
                    Place::Deferred(ImageData::Sdf(pixels)) => return Some(Payload::Sdf { key, pixels: Work::Rendered(pixels), padding, origin, dpxs, msdf }),
                    Place::Deferred(ImageData::Pixels(_)) => unreachable!("SDF placement retains SDF pixels"),
                    Place::None => {}
                    Place::Slot(slot) => {
                        self.fill_raster_entry(&mut entry, slot, origin, dpxs);
                        if let Some((mkey, outline, mdpxs)) = msdf {
                            msdf_job = Some((mkey, outline, mdpxs, slot));
                        }
                    }
                }
            }
            Payload::Color { key, pixels, padding, origin, dpxs } => {
                match self.place_image(gpu, world, key.clone(), glyph::COLOR, padding, Image::Pixels(pixels), tick, req) {
                    Place::Waiting => return None,
                    Place::Deferred(ImageData::Pixels(pixels)) => return Some(Payload::Color { key, pixels: Work::Rendered(pixels), padding, origin, dpxs }),
                    Place::Deferred(ImageData::Sdf(_)) => unreachable!("color placement retains color pixels"),
                    Place::None => {}
                    Place::Slot(slot) => self.fill_raster_entry(&mut entry, slot, origin, dpxs),
                }
            }
        }
        let waiting = self.inflight.remove(&req).unwrap_or_default();
        for (sid, local) in waiting {
            if entry.slot != 0 {
                self.mark_interest(entry.atlas_kind, entry.slot, sid);
            }
            self.stats.delivered += 1;
            self.session_stats.entry(sid).or_default().delivered += 1;
            self.out.entry((sid, local)).or_default().push(entry);
        }
        // the plain field is out; the multi-channel upgrade follows when it is ready
        if let Some((mkey, outline, mdpxs, sdf_slot)) = msdf_job {
            let host = self.fonts.get(&req.font).and_then(|e| e.host.clone());
            let in_atlas = self.raster.as_ref().is_some_and(|a| a.find(&mkey).is_some());
            if let (Some(host), false, true) = (host, in_atlas, self.msdf_pending.insert(mkey.clone())) {
                self.pool().submit(Job { kind: JobKind::Msdf { key: mkey, outline, dpxs: mdpxs, sdf_slot }, font: host, glyph: req.glyph, dpxs: mdpxs });
            }
        }
        None
    }

    fn mark_interest(&mut self, atlas_kind: u32, slot: u32, sid: u32) {
        if atlas_kind == glyph::SLUG {
            if let Some(rec) = self.slug.as_mut().and_then(|s| s.by_slot_mut(slot)) {
                if !rec.sessions.contains(&sid) {
                    rec.sessions.push(sid);
                }
            }
        } else if let Some(rec) = self.raster.as_mut().and_then(|a| a.slot_mut(slot)) {
            if !rec.sessions.contains(&sid) {
                rec.sessions.push(sid);
            }
        }
    }

    fn fill_raster_entry(&self, entry: &mut GlyphEntry, slot: u32, origin: Point<f32>, dpxs: f32) {
        let Some(a) = self.raster.as_ref() else { return };
        let Some(rec) = a.slot(slot) else { return };
        entry.atlas_kind = rec.kind;
        entry.plane = rec.slot.plane.index() as u32;
        let r = rec.slot.rect;
        entry.rect = [r.origin.x as u32, r.origin.y as u32, r.size.width as u32, r.size.height as u32];
        entry.padding = rec.padding as u32;
        entry.origin = [origin.x, origin.y];
        entry.scale = dpxs;
        entry.slot = slot;
        entry.slot_gen = a.generation;
    }

    #[allow(clippy::too_many_arguments)]
    fn place_image(&mut self, gpu: &mut dyn Gpu, world: &mut World, key: GlyphImageKey, kind: u32, padding: usize, image: Image, tick: u64, req: ReqKey) -> Place {
        let Some(a) = self.raster.as_mut() else { return Place::None };
        if let Some(slot) = a.find(&key) {
            if let Some(rec) = a.slot_mut(slot) {
                rec.last_use = tick;
            }
            return Place::Slot(slot);
        }
        // not in the atlas: the pixels are needed
        let image = match image {
            Image::Sdf(Work::Rendered(v)) => ImageData::Sdf(v),
            Image::Pixels(Work::Rendered(v)) => ImageData::Pixels(v),
            Image::Sdf(Work::Elsewhere) | Image::Pixels(Work::Elsewhere) => {
                self.park(Claim::Raster(key), req);
                return Place::Waiting;
            }
            // the worker found it in the atlas, and it has been taken back since
            Image::Sdf(Work::InAtlas) | Image::Pixels(Work::InAtlas) => {
                self.retry(req);
                return Place::Waiting;
            }
        };
        let shared = kind != glyph::GRAY;
        let slot = loop {
            let id = self.next_slot;
            let a = self.raster.as_mut().expect("raster atlas");
            if key.size.width > a.size.width || key.size.height > a.size.height {
                self.stats.oversized += 1;
                return Place::None;
            }
            if a.alloc(id, key.clone(), shared, kind, padding, tick).is_some() {
                self.next_slot += 1;
                break id;
            }
            if a.has_retired() {
                return Place::Deferred(image);
            }
            if !self.evict_raster(world) {
                self.stats.oversized += 1;
                return Place::None;
            }
        };
        let a = self.raster.as_mut().expect("raster atlas");
        match image {
            ImageData::Sdf(values) => a.write_sdf(slot, &values),
            ImageData::Pixels(pixels) => a.write_pixels(slot, &pixels),
        }
        let (tex, rect) = {
            let rec = a.slot(slot).expect("placed slot");
            (a.tex, rec.slot.rect)
        };
        let bytes = a.rect_bytes(rect);
        self.upload(gpu, world, tex, [rect.origin.x as u32, rect.origin.y as u32, rect.size.width as u32, rect.size.height as u32], &bytes);
        Place::Slot(slot)
    }

    #[allow(clippy::too_many_arguments)]
    fn place_slug(&mut self, gpu: &mut dyn Gpu, world: &mut World, font: u32, glyph: u16, curves: &[f32], origin: Point<f32>, size: Size<f32>) -> Result<Option<u32>, ()> {
        let tick = self.tick;
        let count = (curves.len() / 8) as u32;
        let slot = loop {
            let id = self.next_slot;
            let Some(s) = self.slug.as_mut() else { return Ok(None) };
            if count > s.capacity_curves() {
                self.stats.oversized += 1;
                return Ok(None);
            }
            if s.alloc(id, font, glyph, curves, origin, size, tick).is_some() {
                self.next_slot += 1;
                break id;
            }
            if s.has_retired() {
                return Err(());
            }
            let fence = RetireFence::capture(world, &self.closed_frames);
            let taken = self.slug.as_mut().expect("slug atlas").evict_for(count, fence);
            if taken.is_empty() {
                self.stats.oversized += 1;
                return Ok(None);
            }
            self.stats.evictions += 1;
            self.stats.evicted_slots += taken.len() as u64;
            let generation = self.slug.as_ref().expect("slug atlas").generation;
            let mut per_session: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
            for rec in taken {
                for sid in rec.sessions {
                    per_session.entry(sid).or_default().push((rec.slot, generation));
                }
            }
            self.send_evicted(world, atlas::SLUG, generation, per_session);
        };
        let s = self.slug.as_ref().expect("slug atlas");
        let rec = s.by_slot(slot).expect("placed slot");
        let (first, rows, bytes) = s.rows_bytes(rec.offset, rec.count);
        let (tex, width) = (s.tex, s.width as u32);
        self.upload(gpu, world, tex, [0, first, width, rows], &bytes);
        Ok(Some(slot))
    }

    /// Takes back a quarter of the raster atlas; false when it holds nothing to take.
    fn evict_raster(&mut self, world: &mut World) -> bool {
        let fence = RetireFence::capture(world, &self.closed_frames);
        let Some(a) = self.raster.as_mut() else { return false };
        let taken = a.evict(EVICT_FRACTION, fence);
        if taken.is_empty() {
            return false;
        }
        let generation = a.generation;
        self.stats.evictions += 1;
        self.stats.evicted_slots += taken.len() as u64;
        let mut per_session: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
        for (id, rec) in taken {
            for sid in rec.sessions {
                per_session.entry(sid).or_default().push((id, generation));
            }
        }
        self.send_evicted(world, atlas::RASTER, generation, per_session);
        true
    }

    fn send_evicted(&mut self, world: &mut World, kind: u32, generation: u32, per_session: HashMap<u32, Vec<(u32, u32)>>) {
        // Complete requests before withdrawing their slots. Dropping an unsent Ready would
        // leave its client waiting forever: it has no slot yet to associate with Evicted.
        self.flush(world);
        let mut sessions: Vec<u32> = per_session.keys().copied().collect();
        sessions.sort_unstable();
        for sid in sessions {
            let slots = &per_session[&sid];
            let bytes = wire::pack_slots(slots);
            world.to_session(sid, |q| q.atlas_evicted(kind, generation, slots.len() as u32, &bytes));
        }
    }

    fn apply_msdf(&mut self, gpu: &mut dyn Gpu, world: &mut World, key: GlyphImageKey, pixels: Vec<u32>, sdf_slot: u32) -> Option<Payload> {
        let tick = self.tick;
        let Some(a) = self.raster.as_mut() else { return None };
        // the plain field it upgrades must still be there, and so must someone who holds it
        let Some(sdf) = a.slot(sdf_slot) else { return None };
        let (padding, sessions) = (sdf.padding, sdf.sessions.clone());
        if sessions.is_empty() || a.find(&key).is_some() {
            return None;
        }
        let new_slot = loop {
            let id = self.next_slot;
            let a = self.raster.as_mut().expect("raster atlas");
            if a.alloc(id, key.clone(), true, glyph::MSDF, padding, tick).is_some() {
                self.next_slot += 1;
                break id;
            }
            if a.has_retired() {
                return Some(Payload::Msdf { key, pixels });
            }
            if !self.evict_raster(world) {
                return None;
            }
        };
        let a = self.raster.as_mut().expect("raster atlas");
        if !a.write_msdf(new_slot, sdf_slot, &pixels) {
            a.remove(new_slot);
            return None;
        }
        let (tex, rect, generation) = {
            let rec = a.slot(new_slot).expect("placed slot");
            (a.tex, rec.slot.rect, a.generation)
        };
        let bytes = a.rect_bytes(rect);
        self.upload(gpu, world, tex, [rect.origin.x as u32, rect.origin.y as u32, rect.size.width as u32, rect.size.height as u32], &bytes);
        // whoever got the plain field gets the upgrade (the plain slot may have been taken back
        // by the allocations above; then nobody holds it any more)
        let Some(sessions) = self.raster.as_ref().and_then(|a| a.slot(sdf_slot)).map(|r| r.sessions.clone()) else {
            return None;
        };
        if let Some(rec) = self.raster.as_mut().and_then(|a| a.slot_mut(new_slot)) {
            rec.sessions = sessions.clone();
        }
        let entry = GlyphEntry {
            glyph: key.glyph_id as u32,
            dpxs_bits: 0,
            kind: kind::AUTO,
            atlas_kind: glyph::MSDF,
            plane: 0,
            rect: [rect.origin.x as u32, rect.origin.y as u32, rect.size.width as u32, rect.size.height as u32],
            padding: padding as u32,
            origin: [0.0; 2],
            scale: 0.0,
            size_h: 0.0,
            slot: new_slot,
            slot_gen: generation,
            replaces: sdf_slot,
            replaces_gen: generation,
        };
        for sid in sessions {
            self.stats.upgrades += 1;
            self.out.entry((sid, 0)).or_default().push(entry);
        }
        None
    }

    /// Queues the collected entries to their sessions.
    fn flush(&mut self, world: &mut World) {
        if self.out.is_empty() {
            return;
        }
        let mut keys: Vec<(u32, u32)> = self.out.keys().copied().collect();
        keys.sort_unstable();
        for key in keys {
            let entries = self.out.remove(&key).unwrap_or_default();
            for chunk in entries.chunks(ENTRIES_PER_EVENT) {
                let bytes = wire::pack_entries(chunk);
                world.to_session(key.0, |q| q.glyphs_ready(key.1, chunk.len() as u32, &bytes));
            }
        }
    }
}

/// What a worker made of a glyph's image.
enum Image {
    Sdf(Work<Vec<u8>>),
    Pixels(Work<Vec<u32>>),
}

/// The pixels to place.
enum ImageData {
    Sdf(Vec<u8>),
    Pixels(Vec<u32>),
}

enum Place {
    Slot(u32),
    /// no room even in an empty atlas
    None,
    /// the request waits for another worker, or runs again: nothing to deliver yet
    Waiting,
    /// The pixels are ready, but a pre-eviction frame still owns the space they need.
    Deferred(ImageData),
}

fn known_release(known: &RwLock<Known>, claim: &Claim) {
    known.write().unwrap_or_else(|e| e.into_inner()).release(claim);
}

fn known_is_claimed(known: &RwLock<Known>, claim: &Claim) -> bool {
    known.read().unwrap_or_else(|e| e.into_inner()).is_claimed(claim)
}

/// Identifies a font's content without reading all of it: the face's table directory (tags,
/// checksums, lengths), which an sfnt keeps in step with every table's bytes.
fn table_hash(data: &[u8], index: u32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.len().hash(&mut h);
    if let Ok(face) = makepad_font_core::rustybuzz::ttf_parser::RawFace::parse(data, index) {
        for r in face.table_records {
            (r.tag.to_bytes(), r.check_sum, r.length).hash(&mut h);
        }
    }
    h.finish()
}

/// Maps a font file: a path, or on Android an `apk:<asset>` name for a stored APK asset (the app
/// maps the same asset for its own layout; neither holds a heap copy).
fn open_font_file(path: &str) -> std::io::Result<Arc<MappedBytes>> {
    #[cfg(target_os = "android")]
    if let Some(asset) = path.strip_prefix(crate::APK_ASSET_PREFIX) {
        return crate::backend::android::assets::map(asset);
    }
    MappedBytes::open(path).map(Arc::new)
}
