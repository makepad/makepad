//! The two atlases the font service owns, on the CPU: the glyph image atlas (BGRA8, four
//! distance-field planes that colour and multi-channel glyphs share) and the slug curve store
//! (RGBA32F texels, two per curve). Both are bounded by their texture and give slots back under
//! pressure, least recently used first; a slot's id is never reused, so an event that names a
//! slot can never mean another glyph.

use super::pool::Known;
use super::RetireFence;
use makepad_font_core::{
    font_atlas::GlyphImageKey,
    geom::{Point, Rect, Size},
    image::{Bgra, Image},
    raster::{AtlasPlane, AtlasSlot, MultiPlaneAllocator, UNRESOLVED_RASTER_RECT},
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::types::Id;

fn known_write(known: &RwLock<Known>) -> std::sync::RwLockWriteGuard<'_, Known> {
    known.write().unwrap_or_else(|e| e.into_inner())
}

/// One glyph image in the raster atlas.
pub struct SlotRec {
    pub key: GlyphImageKey,
    pub slot: AtlasSlot,
    /// the rectangle is taken in all four planes (colour, multi-channel)
    pub shared: bool,
    /// `wire::glyph::GRAY` / `COLOR` / `MSDF`
    pub kind: u32,
    pub padding: usize,
    pub last_use: u64,
    /// the sessions that were told about this slot and have not let go of it
    pub sessions: Vec<u32>,
}

pub struct RasterAtlas {
    pub tex: Id,
    pub size: Size<usize>,
    pub generation: u32,
    image: Image<Bgra>,
    alloc: MultiPlaneAllocator,
    slots: HashMap<u32, SlotRec>,
    /// Evicted rectangles remain allocated until every pre-eviction frame has completed.
    retired: Vec<(AtlasSlot, bool, Arc<RetireFence>)>,
    by_key: HashMap<GlyphImageKey, u32>,
    known: Arc<RwLock<Known>>,
}

impl RasterAtlas {
    pub fn new(tex: Id, size: Size<usize>, known: Arc<RwLock<Known>>) -> RasterAtlas {
        let mut atlas = RasterAtlas {
            tex,
            size,
            generation: 1,
            image: Image::new(size),
            alloc: MultiPlaneAllocator::new(size),
            slots: HashMap::new(),
            retired: Vec::new(),
            by_key: HashMap::new(),
            known,
        };
        atlas.rebuild();
        atlas
    }

    pub fn bytes(&self) -> u64 {
        (self.size.width * self.size.height * 4) as u64
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn slot(&self, id: u32) -> Option<&SlotRec> {
        self.slots.get(&id)
    }

    pub fn slot_mut(&mut self, id: u32) -> Option<&mut SlotRec> {
        self.slots.get_mut(&id)
    }

    pub fn find(&self, key: &GlyphImageKey) -> Option<u32> {
        self.by_key.get(key).copied()
    }

    /// Takes room for `key`'s image as slot `id` (the service's counter); `None` when the atlas
    /// has none.
    pub fn alloc(&mut self, id: u32, key: GlyphImageKey, shared: bool, kind: u32, padding: usize, tick: u64) -> Option<u32> {
        let (slot, shared) = if shared {
            let rect = self.alloc.allocate_shared_slot(key.size)?;
            (AtlasSlot { rect, plane: AtlasPlane::R }, true)
        } else {
            (self.alloc.allocate_sdf_slot(key.size)?, false)
        };
        self.by_key.insert(key.clone(), id);
        known_write(&self.known).raster.insert(key.clone());
        self.slots.insert(id, SlotRec { key, slot, shared, kind, padding, last_use: tick, sessions: Vec::new() });
        Some(id)
    }

    /// Writes a distance field (one byte per pixel) into the slot's plane.
    pub fn write_sdf(&mut self, id: u32, values: &[u8]) {
        let Some(rec) = self.slots.get(&id) else { return };
        let (rect, plane) = (rec.slot.rect, rec.slot.plane);
        let w = rect.size.width;
        for y in 0..rect.size.height {
            for x in 0..w {
                let at = Point::new(rect.origin.x + x, rect.origin.y + y);
                let old = self.image[at];
                self.image[at] = plane.set(old, values[y * w + x]);
            }
        }
    }

    /// Writes whole pixels (colour bitmaps) into the slot.
    pub fn write_pixels(&mut self, id: u32, pixels: &[u32]) {
        let Some(rec) = self.slots.get(&id) else { return };
        let rect = rec.slot.rect;
        let w = rect.size.width;
        for y in 0..rect.size.height {
            for x in 0..w {
                self.image[Point::new(rect.origin.x + x, rect.origin.y + y)] = Bgra { bits: pixels[y * w + x] };
            }
        }
    }

    /// Writes the multi-channel field `msdf` into slot `id`; its alpha is the plain distance field
    /// of slot `sdf` (the shader keeps using it for coverage). False when `sdf` is gone.
    pub fn write_msdf(&mut self, id: u32, sdf: u32, msdf: &[u32]) -> bool {
        let (Some(dst), Some(src)) = (self.slots.get(&id), self.slots.get(&sdf)) else { return false };
        let (dst_rect, src_rect, plane) = (dst.slot.rect, src.slot.rect, src.slot.plane);
        if dst_rect.size != src_rect.size {
            return false;
        }
        let w = dst_rect.size.width;
        for y in 0..dst_rect.size.height {
            for x in 0..w {
                let v = plane.get(self.image[Point::new(src_rect.origin.x + x, src_rect.origin.y + y)]);
                let m = Bgra { bits: msdf[y * w + x] };
                self.image[Point::new(dst_rect.origin.x + x, dst_rect.origin.y + y)] = Bgra::new(m.b(), m.g(), m.r(), v);
            }
        }
        true
    }

    /// The rectangle's pixels as the texture holds them (BGRA8 rows).
    pub fn rect_bytes(&self, rect: Rect<usize>) -> Vec<u8> {
        let mut out = Vec::with_capacity(rect.size.width * rect.size.height * 4);
        for y in 0..rect.size.height {
            for x in 0..rect.size.width {
                out.extend_from_slice(&self.image[Point::new(rect.origin.x + x, rect.origin.y + y)].bits.to_le_bytes());
            }
        }
        out
    }

    /// Retires least recently used slots totalling `fraction` of the atlas (slots no session
    /// holds go first). Their storage is free only after `fence` completes.
    pub(super) fn evict(&mut self, fraction: f32, fence: Arc<RetireFence>) -> Vec<(u32, SlotRec)> {
        if self.slots.is_empty() {
            return Vec::new();
        }
        let mut order: Vec<(bool, u64, u32)> = self.slots.iter().map(|(id, r)| (!r.sessions.is_empty(), r.last_use, *id)).collect();
        order.sort();
        let total = (self.size.width * self.size.height * 4) as f64;
        let target = total * fraction as f64;
        let mut freed = 0.0;
        let mut taken = Vec::new();
        for (_, _, id) in order {
            let rec = self.slots.remove(&id).expect("listed slot");
            freed += (rec.key.size.width * rec.key.size.height * if rec.shared { 4 } else { 1 }) as f64;
            self.by_key.remove(&rec.key);
            if !fence.complete() {
                self.retired.push((rec.slot, rec.shared, fence.clone()));
            }
            taken.push((id, rec));
            if freed >= target {
                break;
            }
        }
        {
            let mut known = known_write(&self.known);
            for (_, rec) in &taken {
                known.raster.remove(&rec.key);
            }
        }
        self.rebuild();
        taken
    }

    fn rebuild(&mut self) {
        let used = self.slots.values().map(|r| (r.slot.rect, (!r.shared).then_some(r.slot.plane)));
        let retired = self.retired.iter().map(|(slot, shared, _)| (slot.rect, (!*shared).then_some(slot.plane)));
        // Unresolved glyphs sample a permanently transparent texel, never an evictable slot.
        let unresolved = std::iter::once((UNRESOLVED_RASTER_RECT, None));
        self.alloc
            .rebuild(self.size, unresolved.chain(used).chain(retired));
    }

    pub fn has_retired(&self) -> bool {
        !self.retired.is_empty()
    }

    pub fn can_reclaim(&self) -> bool {
        self.retired.iter().any(|(_, _, fence)| fence.complete())
    }

    pub fn reclaim(&mut self) -> bool {
        let before = self.retired.len();
        self.retired.retain(|(_, _, fence)| !fence.complete());
        if self.retired.len() == before {
            return false;
        }
        self.rebuild();
        true
    }

    /// Removes an unpublished allocation (a failed MSDF upgrade). Published slots use `evict`.
    pub fn remove(&mut self, id: u32) -> Option<SlotRec> {
        let rec = self.slots.remove(&id)?;
        self.by_key.remove(&rec.key);
        known_write(&self.known).raster.remove(&rec.key);
        self.rebuild();
        Some(rec)
    }

    pub fn session_closed(&mut self, session: u32) {
        for rec in self.slots.values_mut() {
            rec.sessions.retain(|s| *s != session);
        }
    }
}

/// One glyph's curves in the slug store.
pub struct SlugRec {
    pub slot: u32,
    pub font: u32,
    pub glyph: u16,
    /// first curve, and how many
    pub offset: u32,
    pub count: u32,
    pub origin: Point<f32>,
    pub size: Size<f32>,
    pub last_use: u64,
    pub sessions: Vec<u32>,
}

pub struct SlugStore {
    pub tex: Id,
    pub width: usize,
    pub rows: usize,
    pub generation: u32,
    data: Vec<f32>,
    /// free runs of curves (start, length), sorted by start, never adjacent
    free: Vec<(u32, u32)>,
    retired: Vec<(u32, u32, Arc<RetireFence>)>,
    glyphs: HashMap<(u32, u16), SlugRec>,
    by_slot: HashMap<u32, (u32, u16)>,
    known: Arc<RwLock<Known>>,
}

const CURVE_TEXELS: usize = 2;
const CURVE_FLOATS: usize = 8;

impl SlugStore {
    pub fn new(tex: Id, width: usize, rows: usize, known: Arc<RwLock<Known>>) -> SlugStore {
        let curves = (width * rows / CURVE_TEXELS) as u32;
        SlugStore { tex, width, rows, generation: 1, data: vec![0.0; width * rows * 4], free: vec![(0, curves)], retired: Vec::new(), glyphs: HashMap::new(), by_slot: HashMap::new(), known }
    }

    pub fn bytes(&self) -> u64 {
        (self.width * self.rows * 16) as u64
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Curves of the glyphs held (retired and free runs not counted).
    pub fn curves_held(&self) -> u64 {
        self.glyphs.values().map(|r| r.count as u64).sum()
    }

    pub fn capacity_curves(&self) -> u32 {
        (self.width * self.rows / CURVE_TEXELS) as u32
    }

    pub fn find(&self, font: u32, glyph: u16) -> Option<&SlugRec> {
        self.glyphs.get(&(font, glyph))
    }

    pub fn by_slot(&self, slot: u32) -> Option<&SlugRec> {
        self.by_slot.get(&slot).and_then(|k| self.glyphs.get(k))
    }

    pub fn by_slot_mut(&mut self, slot: u32) -> Option<&mut SlugRec> {
        let key = *self.by_slot.get(&slot)?;
        self.glyphs.get_mut(&key)
    }

    fn take_run(&mut self, count: u32) -> Option<u32> {
        let i = self.free.iter().position(|(_, len)| *len >= count)?;
        let (start, len) = self.free[i];
        if len == count {
            self.free.remove(i);
        } else {
            self.free[i] = (start + count, len - count);
        }
        Some(start)
    }

    fn give_run(free: &mut Vec<(u32, u32)>, start: u32, len: u32) {
        let at = free.partition_point(|(s, _)| *s < start);
        free.insert(at, (start, len));
        if at + 1 < free.len() && free[at].0 + free[at].1 == free[at + 1].0 {
            free[at].1 += free[at + 1].1;
            free.remove(at + 1);
        }
        if at > 0 && free[at - 1].0 + free[at - 1].1 == free[at].0 {
            free[at - 1].1 += free[at].1;
            free.remove(at);
        }
    }

    /// Stores a glyph's curves; `None` when no run of that length is free.
    #[allow(clippy::too_many_arguments)]
    pub fn alloc(&mut self, slot: u32, font: u32, glyph: u16, curves: &[f32], origin: Point<f32>, size: Size<f32>, tick: u64) -> Option<u32> {
        let count = (curves.len() / CURVE_FLOATS) as u32;
        let offset = self.take_run(count)?;
        let at = offset as usize * CURVE_FLOATS;
        self.data[at..at + curves.len()].copy_from_slice(curves);
        self.by_slot.insert(slot, (font, glyph));
        known_write(&self.known).slug.insert((font, glyph));
        self.glyphs.insert((font, glyph), SlugRec { slot, font, glyph, offset, count, origin, size, last_use: tick, sessions: Vec::new() });
        Some(slot)
    }

    /// Every row of the texture (a device reset uploads the store whole).
    pub fn all_bytes(&self) -> Vec<u8> {
        self.data.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// The rows of the texture a run of curves touches: (first row, row count, their bytes).
    pub fn rows_bytes(&self, offset: u32, count: u32) -> (u32, u32, Vec<u8>) {
        let first = (offset as usize * CURVE_TEXELS) / self.width;
        let last = ((offset + count) as usize * CURVE_TEXELS).saturating_sub(1) / self.width;
        let mut out = Vec::with_capacity((last - first + 1) * self.width * 16);
        for f in &self.data[first * self.width * 4..(last + 1) * self.width * 4] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        (first as u32, (last - first + 1) as u32, out)
    }

    /// Retires least recently used glyphs (those no session holds first) until a run of `need`
    /// curves will be free after `fence` completes.
    pub(super) fn evict_for(&mut self, need: u32, fence: Arc<RetireFence>) -> Vec<SlugRec> {
        let mut order: Vec<(bool, u64, (u32, u16))> = self.glyphs.iter().map(|(k, r)| (!r.sessions.is_empty(), r.last_use, *k)).collect();
        order.sort();
        let mut taken = Vec::new();
        // Choose enough victims for a contiguous run without making that run reusable yet.
        let mut available = self.free.clone();
        for (_, _, key) in order {
            if available.iter().any(|(_, len)| *len >= need) {
                break;
            }
            let rec = self.glyphs.remove(&key).expect("listed glyph");
            self.by_slot.remove(&rec.slot);
            Self::give_run(&mut available, rec.offset, rec.count);
            if fence.complete() {
                Self::give_run(&mut self.free, rec.offset, rec.count);
            } else {
                self.retired.push((rec.offset, rec.count, fence.clone()));
            }
            taken.push(rec);
        }
        let mut known = known_write(&self.known);
        for rec in &taken {
            known.slug.remove(&(rec.font, rec.glyph));
        }
        taken
    }

    pub fn has_retired(&self) -> bool {
        !self.retired.is_empty()
    }

    pub fn can_reclaim(&self) -> bool {
        self.retired.iter().any(|(_, _, fence)| fence.complete())
    }

    pub fn reclaim(&mut self) -> bool {
        let before = self.retired.len();
        let free = &mut self.free;
        self.retired.retain(|(offset, count, fence)| {
            if !fence.complete() {
                return true;
            }
            Self::give_run(free, *offset, *count);
            false
        });
        self.retired.len() != before
    }

    pub fn session_closed(&mut self, session: u32) {
        for rec in self.glyphs.values_mut() {
            rec.sessions.retain(|s| *s != session);
        }
    }
}
