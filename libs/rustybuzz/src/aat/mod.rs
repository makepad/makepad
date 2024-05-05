mod extended_kerning;
mod feature_mappings;
mod feature_selector;
mod map;
mod metamorphosis;
mod tracking;

pub use map::*;

use crate::Face;
use crate::buffer::Buffer;
use crate::plan::ShapePlan;

pub fn substitute(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    metamorphosis::apply(plan, face, buffer);
}

pub fn position(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    extended_kerning::apply(plan, face, buffer);
}

pub fn track(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    tracking::apply(plan, face, buffer);
}

pub fn zero_width_deleted_glyphs(buffer: &mut Buffer) {
    for i in 0..buffer.len {
        if buffer.info[i].glyph_id == 0xFFFF {
            buffer.pos[i].x_advance = 0;
            buffer.pos[i].y_advance = 0;
            buffer.pos[i].x_offset = 0;
            buffer.pos[i].y_offset = 0;
        }
    }
}

pub fn remove_deleted_glyphs(buffer: &mut Buffer) {
    buffer.delete_glyphs_inplace(|info| info.glyph_id == 0xFFFF )
}
