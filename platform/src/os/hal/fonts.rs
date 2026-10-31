//! The font service as a session sees it (HAL.md 13): the host owns the font files and the glyph
//! atlases, this side opens fonts, asks for glyphs and samples the lent atlas textures. The text
//! stack (`makepad-draw`) drives it; the answers arrive as HAL events, are queued here and taken
//! by the next draw (`Cx::hal_take_font_events`).

use super::lent_id;
use crate::{event::LentId, Cx};
use std::collections::HashSet;
pub use makepad_hal::font::{wire as hal_font_wire, FontSource as HalFontSource, GlyphEntry as HalGlyphEntry};

/// The glyphs the current draw shows as blank gate its presentation (bounded by the event
/// loop's wait): a frame never presents a glyph that has nothing to show. The text client
/// reports only those (`Outbox::pending`): a glyph drawn with a stand-in (another size, another
/// instance of a variable font) shows ink and does not wait; background requests never wait.
#[derive(Default)]
pub(crate) struct GlyphWait {
    pub(crate) pending: HashSet<(u32, u32, u32, u32)>,
    pub(crate) presented: HashSet<makepad_hal::Id>,
    pub(crate) timed_out: bool,
    /// Glyphs the current draw shows blank (reported, answered or not).
    pub(crate) drawn_blank: usize,
    /// Frames presented with a blank glyph (only after the bounded wait ran out).
    pub(crate) blank_frames: u64,
}

impl GlyphWait {
    /// This draw shows these glyphs (font, glyph, kind, size bits) blank.
    pub(crate) fn note(&mut self, glyphs: &[(u32, u32, u32, u32)]) {
        self.drawn_blank += glyphs.len();
        self.pending.extend(glyphs.iter().copied());
    }

    /// The host answered these glyphs of `font` (any answer: an image or none).
    pub(crate) fn answered(&mut self, font: u32, entries: &[HalGlyphEntry]) {
        for entry in entries.iter().filter(|e| e.replaces == 0) {
            self.pending.remove(&(font, entry.glyph, entry.kind, entry.dpxs_bits));
        }
    }
}

/// What the font service told this session.
#[derive(Clone, Debug)]
pub enum HalFontEvent {
    /// `FontOpen` was answered (`hal_font_wire::font_status`).
    Opened { font: u32, status: u32, len: u64, hash: u64, path: String },
    /// An atlas the session may sample (`hal_font_wire::atlas`): draw it through `texture`
    /// (`Texture::set_hal_lent`). A new `generation` voids the entries of the older one.
    Atlas { kind: u32, texture: LentId, width: u32, height: u32, generation: u32 },
    /// Glyphs of the session's font `font` are in the atlas (an entry with `replaces` != 0
    /// upgrades the slot it names, for any font).
    Ready { font: u32, entries: Vec<HalGlyphEntry> },
    /// The host took atlas slots back (slot, generation): drop what names them.
    Evicted { kind: u32, generation: u32, slots: Vec<(u32, u32)> },
}

impl Cx {
    /// Opens a font under the app's own id `font`; answered by `HalFontEvent::Opened`.
    pub fn hal_font_open(&mut self, font: u32, index: u32, vars: &[(u32, f32)], source: HalFontSource<'_>) {
        self.hal.frames.w.open_font(font, index, vars, source);
    }

    /// Asks for glyphs (id, pixels per em) of an open font; `kind` is `hal_font_wire::kind`.
    pub fn hal_glyph_request(&mut self, font: u32, kind: u32, glyphs: &[(u32, f32)]) {
        self.hal.frames.w.request_glyphs(font, kind, glyphs);
    }

    /// Marks unresolved requests used by this draw: (font, glyph, kind, size bits).
    /// Repeated references are folded together; requests from older draws count only if used.
    pub fn hal_glyphs_pending(&mut self, glyphs: &[(u32, u32, u32, u32)]) {
        self.hal.glyph_wait.note(glyphs);
    }

    /// Frames this session presented with a glyph drawn blank (its pixels not there yet). The
    /// present waits for missing glyphs, so this stays 0 unless that bounded wait ran out.
    pub fn blank_glyph_frames(&self) -> u64 {
        self.hal.glyph_wait.blank_frames
    }

    /// Lets go of atlas slots this session no longer draws from.
    pub fn hal_glyph_release(&mut self, slots: &[(u32, u32)]) {
        self.hal.frames.w.release_glyphs(slots);
    }

    /// Names the atlas slots this session drew from since its last touch.
    pub fn hal_glyph_touch(&mut self, slots: &[(u32, u32)]) {
        self.hal.frames.w.touch_glyphs(slots);
    }

    /// The font service events that arrived since the last call.
    pub fn hal_take_font_events(&mut self) -> Vec<HalFontEvent> {
        std::mem::take(&mut self.hal.font_events)
    }

    pub(crate) fn hal_font_event(&mut self, event: makepad_hal::HalEvent<'_>) {
        use makepad_hal::HalEvent as E;
        let event = match event {
            E::FontOpened { font, status, len, hash, path } => HalFontEvent::Opened { font, status, len, hash, path: String::from_utf8_lossy(path).into_owned() },
            E::AtlasInfo { kind, texture, w, h, generation } => HalFontEvent::Atlas { kind, texture: lent_id(texture), width: w, height: h, generation },
            E::GlyphsReady { font, count, entries } => HalFontEvent::Ready { font, entries: hal_font_wire::read_entries(entries, count) },
            E::AtlasEvicted { kind, generation, count, slots } => HalFontEvent::Evicted { kind, generation, slots: hal_font_wire::read_slots(slots, count) },
            _ => return,
        };
        match &event {
            HalFontEvent::Opened { font, status, .. } if *status != hal_font_wire::font_status::OPENED => {
                self.hal.glyph_wait.pending.retain(|(f, _, _, _)| f != font);
            }
            HalFontEvent::Ready { font, entries } => self.hal.glyph_wait.answered(*font, entries),
            _ => {}
        }
        self.hal.font_events.push(event);
        // the text draws again: it takes the answers before it lays out glyphs
        self.redraw_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(glyph: u32, size: f32) -> HalGlyphEntry {
        HalGlyphEntry {
            glyph, dpxs_bits: size.to_bits(), kind: 1, atlas_kind: 1, plane: 0, rect: [0; 4], padding: 0,
            origin: [0.0; 2], scale: size, size_h: 0.0, slot: 3, slot_gen: 1, replaces: 0, replaces_gen: 0,
        }
    }

    /// A glyph answered once can be drawn blank again later (its slot was released after it
    /// went undrawn, or evicted, and no other size of it is held): that draw waits for it like
    /// for a glyph never seen. Before, a glyph seen once never held a frame again, and text
    /// scrolled back into view flashed blank for a frame or more (Stage's code pane, 2026-10-13).
    #[test]
    fn a_glyph_seen_before_still_holds_a_frame_that_shows_it_blank() {
        let mut wait = GlyphWait::default();
        let key = (7, 44, 1, 14.0f32.to_bits());
        wait.note(&[key]);
        wait.answered(7, &[answer(44, 14.0)]);
        assert!(wait.pending.is_empty());
        // later: the same glyph, blank again
        wait.note(&[key]);
        assert!(wait.pending.contains(&key), "a blank glyph holds the frame even when seen before");
        // an answer for another size does not release it
        wait.answered(7, &[answer(44, 20.0)]);
        assert!(wait.pending.contains(&key));
        wait.answered(7, &[answer(44, 14.0)]);
        assert!(wait.pending.is_empty());
    }
}
