use ttf_parser::GlyphId;
use ttf_parser::opentype_layout::LookupIndex;
use ttf_parser::gpos::*;

use crate::{Direction, Face};
use crate::buffer::{Buffer, BufferScratchFlags, GlyphPosition};
use crate::plan::ShapePlan;

use super::{lookup_flags, LayoutLookup, LayoutTable, TableIndex, PositioningTable, PositioningLookup};
use super::apply::{Apply, ApplyContext};
use super::matching::SkippyIter;

pub fn position_start(_: &Face, buffer: &mut Buffer) {
    let len = buffer.len;
    for pos in &mut buffer.pos[..len] {
        pos.set_attach_chain(0);
        pos.set_attach_type(0);
    }
}

pub fn position(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    super::apply_layout_table(plan, face, buffer, face.gpos.as_ref());
}

pub fn position_finish_advances(_: &Face, _: &mut Buffer) {}

pub fn position_finish_offsets(_: &Face, buffer: &mut Buffer) {
    let len = buffer.len;
    let direction = buffer.direction;

    // Handle attachments
    if buffer.scratch_flags.contains(BufferScratchFlags::HAS_GPOS_ATTACHMENT) {
        for i in 0..len {
            propagate_attachment_offsets(&mut buffer.pos, len, i, direction);
        }
    }
}

fn propagate_attachment_offsets(
    pos: &mut [GlyphPosition],
    len: usize,
    i: usize,
    direction: Direction,
) {
    // Adjusts offsets of attached glyphs (both cursive and mark) to accumulate
    // offset of glyph they are attached to.
    let chain = pos[i].attach_chain();
    let kind = pos[i].attach_type();
    if chain == 0 {
        return;
    }

    pos[i].set_attach_chain(0);

    let j = (i as isize + isize::from(chain)) as _;
    if j >= len {
        return;
    }

    propagate_attachment_offsets(pos, len, j, direction);

    match kind {
        attach_type::MARK => {
            pos[i].x_offset += pos[j].x_offset;
            pos[i].y_offset += pos[j].y_offset;

            assert!(j < i);
            if direction.is_forward() {
                for k in j..i {
                    pos[i].x_offset -= pos[k].x_advance;
                    pos[i].y_offset -= pos[k].y_advance;
                }
            } else {
                for k in j+1..i+1 {
                    pos[i].x_offset += pos[k].x_advance;
                    pos[i].y_offset += pos[k].y_advance;
                }
            }
        }
        attach_type::CURSIVE => {
            if direction.is_horizontal() {
                pos[i].y_offset += pos[j].y_offset;
            } else {
                pos[i].x_offset += pos[j].x_offset;
            }
        }
        _ => {}
    }
}

impl<'a> LayoutTable for PositioningTable<'a> {
    const INDEX: TableIndex = TableIndex::GPOS;
    const IN_PLACE: bool = true;

    type Lookup = PositioningLookup<'a>;

    fn get_lookup(&self, index: LookupIndex) -> Option<&Self::Lookup> {
        self.lookups.get(usize::from(index))
    }
}

impl LayoutLookup for PositioningLookup<'_> {
    fn props(&self) -> u32 {
        self.props
    }

    fn is_reverse(&self) -> bool {
        false
    }

    fn covers(&self, glyph: GlyphId) -> bool {
        self.coverage.contains(glyph)
    }
}

impl Apply for PositioningLookup<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        if self.covers(ctx.buffer.cur(0).as_glyph()) {
            for subtable in &self.subtables {
                if subtable.apply(ctx).is_some() {
                    return Some(());
                }
            }
        }

        None
    }
}

impl Apply for PositioningSubtable<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        match self {
            Self::Single(t) => t.apply(ctx),
            Self::Pair(t) => t.apply(ctx),
            Self::Cursive(t) => t.apply(ctx),
            Self::MarkToBase(t) => t.apply(ctx),
            Self::MarkToLigature(t) => t.apply(ctx),
            Self::MarkToMark(t) => t.apply(ctx),
            Self::Context(t) => t.apply(ctx),
            Self::ChainContext(t) => t.apply(ctx),
        }
    }
}

impl Apply for SingleAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let glyph = ctx.buffer.cur(0).as_glyph();
        let record = match self {
            Self::Format1 { coverage, value } => {
                coverage.get(glyph)?;
                *value
            }
            Self::Format2 { coverage, values } => {
                let index = coverage.get(glyph)?;
                values.get(index)?
            }
        };
        record.apply(ctx, ctx.buffer.idx);
        ctx.buffer.idx += 1;
        Some(())
    }
}

impl Apply for PairAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let first = ctx.buffer.cur(0).as_glyph();
        let index = self.coverage().get(first)?;

        let mut iter = SkippyIter::new(ctx, ctx.buffer.idx, 1, false);
        if !iter.next() {
            return None;
        }

        let pos = iter.index();
        let second = ctx.buffer.info[pos].as_glyph();

        let records = match self {
            Self::Format1 { sets, .. } => {
                sets.get(index)?.get(second)
            }
            Self::Format2 { classes, matrix, .. } => {
                let classes = (classes.0.get(first), classes.1.get(second));
                matrix.get(classes)
            }
        }?;

        let flag1 = records.0.apply(ctx, ctx.buffer.idx);
        let flag2 = records.1.apply(ctx, pos);
        // Note the intentional use of "|" instead of short-circuit "||".
        if flag1 | flag2 {
            ctx.buffer.unsafe_to_break(ctx.buffer.idx, pos + 1);
        }

        ctx.buffer.idx = pos + usize::from(flag2);
        Some(())
    }
}

impl Apply for CursiveAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let this = ctx.buffer.cur(0).as_glyph();

        let index_this = self.coverage.get(this)?;
        let entry_this = self.sets.entry(index_this)?;

        let mut iter = SkippyIter::new(ctx, ctx.buffer.idx, 1, false);
        if !iter.prev() {
            return None;
        }

        let i = iter.index();
        let prev = ctx.buffer.info[i].as_glyph();
        let index_prev = self.coverage.get(prev)?;
        let exit_prev = self.sets.exit(index_prev)?;

        let (exit_x, exit_y) = exit_prev.get(ctx.face);
        let (entry_x, entry_y) = entry_this.get(ctx.face);

        let direction = ctx.buffer.direction;
        let j = ctx.buffer.idx;
        ctx.buffer.unsafe_to_break(i, j);

        let pos = &mut ctx.buffer.pos;
        match direction {
            Direction::LeftToRight => {
                pos[i].x_advance = exit_x + pos[i].x_offset;
                let d = entry_x + pos[j].x_offset;
                pos[j].x_advance -= d;
                pos[j].x_offset -= d;
            }
            Direction::RightToLeft => {
                let d = exit_x + pos[i].x_offset;
                pos[i].x_advance -= d;
                pos[i].x_offset -= d;
                pos[j].x_advance = entry_x + pos[j].x_offset;
            }
            Direction::TopToBottom => {
                pos[i].y_advance = exit_y + pos[i].y_offset;
                let d = entry_y + pos[j].y_offset;
                pos[j].y_advance -= d;
                pos[j].y_offset -= d;
            }
            Direction::BottomToTop => {
                let d = exit_y + pos[i].y_offset;
                pos[i].y_advance -= d;
                pos[i].y_offset -= d;
                pos[j].y_advance = entry_y;
            }
            Direction::Invalid => {}
        }

        // Cross-direction adjustment

        // We attach child to parent (think graph theory and rooted trees whereas
        // the root stays on baseline and each node aligns itself against its
        // parent.
        //
        // Optimize things for the case of RightToLeft, as that's most common in
        // Arabic.
        let mut child = i;
        let mut parent = j;
        let mut x_offset = entry_x - exit_x;
        let mut y_offset = entry_y - exit_y;

        // Low bits are lookup flags, so we want to truncate.
        if ctx.lookup_props as u16 & lookup_flags::RIGHT_TO_LEFT == 0 {
            core::mem::swap(&mut child, &mut parent);
            x_offset = -x_offset;
            y_offset = -y_offset;
        }

        // If child was already connected to someone else, walk through its old
        // chain and reverse the link direction, such that the whole tree of its
        // previous connection now attaches to new parent.  Watch out for case
        // where new parent is on the path from old chain...
        reverse_cursive_minor_offset(pos, child, direction, parent);

        pos[child].set_attach_type(attach_type::CURSIVE);
        pos[child].set_attach_chain((parent as isize - child as isize) as i16);

        ctx.buffer.scratch_flags |= BufferScratchFlags::HAS_GPOS_ATTACHMENT;
        if direction.is_horizontal() {
            pos[child].y_offset = y_offset;
        } else {
            pos[child].x_offset = x_offset;
        }

        // If parent was attached to child, break them free.
        // https://github.com/harfbuzz/harfbuzz/issues/2469
        if pos[parent].attach_chain() == -pos[child].attach_chain() {
            pos[parent].set_attach_chain(0);
        }

        ctx.buffer.idx += 1;
        Some(())
    }
}

fn reverse_cursive_minor_offset(
    pos: &mut [GlyphPosition],
    i: usize,
    direction: Direction,
    new_parent: usize,
) {
    let chain = pos[i].attach_chain();
    let attach_type = pos[i].attach_type();
    if chain == 0 || attach_type & attach_type::CURSIVE == 0 {
        return;
    }

    pos[i].set_attach_chain(0);

    // Stop if we see new parent in the chain.
    let j = (i as isize + isize::from(chain)) as _;
    if j == new_parent {
        return;
    }

    reverse_cursive_minor_offset(pos, j, direction, new_parent);

    if direction.is_horizontal() {
        pos[j].y_offset = -pos[i].y_offset;
    } else {
        pos[j].x_offset = -pos[i].x_offset;
    }

    pos[j].set_attach_chain(-chain);
    pos[j].set_attach_type(attach_type);
}

impl Apply for MarkToBaseAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let buffer = &ctx.buffer;
        let mark_glyph = ctx.buffer.cur(0).as_glyph();
        let mark_index = self.mark_coverage.get(mark_glyph)?;

        // Now we search backwards for a non-mark glyph
        let mut iter = SkippyIter::new(ctx, buffer.idx, 1, false);
        iter.set_lookup_props(u32::from(lookup_flags::IGNORE_MARKS));

        let info = &buffer.info;
        loop {
            if !iter.prev() {
                return None;
            }

            // We only want to attach to the first of a MultipleSubst sequence.
            // https://github.com/harfbuzz/harfbuzz/issues/740
            // Reject others...
            // ...but stop if we find a mark in the MultipleSubst sequence:
            // https://github.com/harfbuzz/harfbuzz/issues/1020
            let idx = iter.index();
            if !info[idx].is_multiplied()
                || info[idx].lig_comp() == 0
                || idx == 0
                || info[idx - 1].is_mark()
                || info[idx].lig_id() != info[idx - 1].lig_id()
                || info[idx].lig_comp() != info[idx - 1].lig_comp() + 1
            {
                break;
            }
            iter.reject();
        }

        // Checking that matched glyph is actually a base glyph by GDEF is too strong; disabled

        let idx = iter.index();
        let base_glyph = info[idx].as_glyph();
        let base_index = self.base_coverage.get(base_glyph)?;

        self.marks.apply(ctx, self.anchors, mark_index, base_index, idx)
    }
}

impl Apply for MarkToLigatureAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let buffer = &ctx.buffer;
        let mark_glyph = ctx.buffer.cur(0).as_glyph();
        let mark_index = self.mark_coverage.get(mark_glyph)?;

        // Now we search backwards for a non-mark glyph
        let mut iter = SkippyIter::new(ctx, buffer.idx, 1, false);
        iter.set_lookup_props(u32::from(lookup_flags::IGNORE_MARKS));
        if !iter.prev() {
            return None;
        }

        // Checking that matched glyph is actually a ligature by GDEF is too strong; disabled

        let idx = iter.index();
        let lig_glyph = buffer.info[idx].as_glyph();
        let lig_index = self.ligature_coverage.get(lig_glyph)?;
        let lig_attach = self.ligature_array.get(lig_index)?;

        // Find component to attach to
        let comp_count = lig_attach.rows;
        if comp_count == 0 {
            return None;
        }

        // We must now check whether the ligature ID of the current mark glyph
        // is identical to the ligature ID of the found ligature.  If yes, we
        // can directly use the component index.  If not, we attach the mark
        // glyph to the last component of the ligature.
        let lig_id = buffer.info[idx].lig_id();
        let mark_id = buffer.cur(0).lig_id();
        let mark_comp = u16::from(buffer.cur(0).lig_comp());
        let matches = lig_id != 0 && lig_id == mark_id && mark_comp > 0;
        let comp_index = if matches { mark_comp.min(comp_count) } else { comp_count } - 1;

        self.marks.apply(ctx, lig_attach, mark_index, comp_index, idx)
    }
}

impl Apply for MarkToMarkAdjustment<'_> {
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()> {
        let buffer = &ctx.buffer;
        let mark1_glyph = ctx.buffer.cur(0).as_glyph();
        let mark1_index = self.mark1_coverage.get(mark1_glyph)?;

        // Now we search backwards for a suitable mark glyph until a non-mark glyph
        let mut iter = SkippyIter::new(ctx, buffer.idx, 1, false);
        iter.set_lookup_props(ctx.lookup_props & !u32::from(lookup_flags::IGNORE_FLAGS));
        if !iter.prev() {
            return None;
        }

        let idx = iter.index();
        if !buffer.info[idx].is_mark() {
            return None;
        }

        let id1 = buffer.cur(0).lig_id();
        let id2 = buffer.info[idx].lig_id();
        let comp1 = buffer.cur(0).lig_comp();
        let comp2 = buffer.info[idx].lig_comp();

        let matches = if id1 == id2 {
            // Marks belonging to the same base
            // or marks belonging to the same ligature component.
            id1 == 0 || comp1 == comp2
        } else {
            // If ligature ids don't match, it may be the case that one of the marks
            // itself is a ligature.  In which case match.
            (id1 > 0 && comp1 == 0) || (id2 > 0 && comp2 == 0)
        };

        if !matches {
            return None;
        }

        let mark2_glyph = buffer.info[idx].as_glyph();
        let mark2_index = self.mark2_coverage.get(mark2_glyph)?;

        self.marks.apply(ctx, self.mark2_matrix, mark1_index, mark2_index, idx)
    }
}

trait ValueRecordExt {
    fn apply(&self, ctx: &mut ApplyContext, idx: usize) -> bool;
}

impl ValueRecordExt for ValueRecord<'_> {
    fn apply(&self, ctx: &mut ApplyContext, idx: usize) -> bool {
        let horizontal = ctx.buffer.direction.is_horizontal();
        let mut pos = ctx.buffer.pos[idx];
        let mut worked = false;

        if self.x_placement != 0 {
            pos.x_offset += i32::from(self.x_placement);
            worked = true;
        }

        if self.y_placement != 0 {
            pos.y_offset += i32::from(self.y_placement);
            worked = true;
        }

        if self.x_advance != 0 && horizontal {
            pos.x_advance += i32::from(self.x_advance);
            worked = true;
        }

        if self.y_advance != 0 && !horizontal {
            // y_advance values grow downward but font-space grows upward, hence negation
            pos.y_advance -= i32::from(self.y_advance);
            worked = true;
        }

        {
            let (ppem_x, ppem_y) = ctx.face.pixels_per_em().unwrap_or((0, 0));
            let coords = ctx.face.ttfp_face.variation_coordinates().len();
            let use_x_device = ppem_x != 0 || coords != 0;
            let use_y_device = ppem_y != 0 || coords != 0;

            if use_x_device {
                if let Some(device) = self.x_placement_device {
                    pos.x_offset += device.get_x_delta(ctx.face).unwrap_or(0);
                    worked = true; // TODO: even when 0?
                }
            }

            if use_y_device {
                if let Some(device) = self.y_placement_device {
                    pos.y_offset += device.get_y_delta(ctx.face).unwrap_or(0);
                    worked = true;
                }
            }

            if horizontal && use_x_device {
                if let Some(device) = self.x_advance_device {
                    pos.x_advance += device.get_x_delta(ctx.face).unwrap_or(0);
                    worked = true;
                }
            }

            if !horizontal && use_y_device {
                if let Some(device) = self.y_advance_device {
                    // y_advance values grow downward but face-space grows upward, hence negation
                    pos.y_advance -= device.get_y_delta(ctx.face).unwrap_or(0);
                    worked = true;
                }
            }
        }

        ctx.buffer.pos[idx] = pos;
        worked
    }
}

trait MarkArrayExt {
    fn apply(
        &self,
        ctx: &mut ApplyContext,
        anchors: AnchorMatrix,
        mark_index: u16,
        glyph_index: u16,
        glyph_pos: usize,
    ) -> Option<()>;
}

impl MarkArrayExt for MarkArray<'_> {
    fn apply(
        &self,
        ctx: &mut ApplyContext,
        anchors: AnchorMatrix,
        mark_index: u16,
        glyph_index: u16,
        glyph_pos: usize,
    ) -> Option<()> {
        // If this subtable doesn't have an anchor for this base and this class
        // return `None` such that the subsequent subtables have a chance at it.
        let (mark_class, mark_anchor) = self.get(mark_index)?;
        let base_anchor = anchors.get(glyph_index, mark_class)?;

        let (mark_x, mark_y) = mark_anchor.get(ctx.face);
        let (base_x, base_y) = base_anchor.get(ctx.face);

        ctx.buffer.unsafe_to_break(glyph_pos, ctx.buffer.idx);

        let idx = ctx.buffer.idx;
        let pos = ctx.buffer.cur_pos_mut();
        pos.x_offset = base_x - mark_x;
        pos.y_offset = base_y - mark_y;
        pos.set_attach_type(attach_type::MARK);
        pos.set_attach_chain((glyph_pos as isize - idx as isize) as i16);

        ctx.buffer.scratch_flags |= BufferScratchFlags::HAS_GPOS_ATTACHMENT;
        ctx.buffer.idx += 1;

        Some(())
    }
}

pub mod attach_type {
    pub const MARK: u8 = 1;
    pub const CURSIVE: u8 = 2;
}

/// Just like TryFrom<N>, but for numeric types not supported by the Rust's std.
pub trait TryNumFrom<T>: Sized {
    /// Casts between numeric types.
    fn try_num_from(_: T) -> Option<Self>;
}

impl TryNumFrom<f32> for i32 {
    #[inline]
    fn try_num_from(v: f32) -> Option<Self> {
        // Based on https://github.com/rust-num/num-traits/blob/master/src/cast.rs

        // Float as int truncates toward zero, so we want to allow values
        // in the exclusive range `(MIN-1, MAX+1)`.

        // We can't represent `MIN-1` exactly, but there's no fractional part
        // at this magnitude, so we can just use a `MIN` inclusive boundary.
        const MIN: f32 = core::i32::MIN as f32;
        // We can't represent `MAX` exactly, but it will round up to exactly
        // `MAX+1` (a power of two) when we cast it.
        const MAX_P1: f32 = core::i32::MAX as f32;
        if v >= MIN && v < MAX_P1 {
            Some(v as i32)
        } else {
            None
        }
    }
}

trait DeviceExt {
    fn get_x_delta(&self, face: &Face) -> Option<i32>;
    fn get_y_delta(&self, face: &Face) -> Option<i32>;
}

impl DeviceExt for Device<'_> {
    fn get_x_delta(&self, face: &Face) -> Option<i32> {
        match self {
            Device::Hinting(hinting) => hinting.x_delta(face.units_per_em, face.pixels_per_em()),
            Device::Variation(variation) => {
                face.tables().gdef?
                    .glyph_variation_delta(variation.outer_index, variation.inner_index, face.variation_coordinates())
                    .and_then(|float| i32::try_num_from(crate::round(float)))
            }
        }
    }

    fn get_y_delta(&self, face: &Face) -> Option<i32> {
        match self {
            Device::Hinting(hinting) => hinting.y_delta(face.units_per_em, face.pixels_per_em()),
            Device::Variation(variation) => {
                face.tables().gdef?
                    .glyph_variation_delta(variation.outer_index, variation.inner_index, face.variation_coordinates())
                    .and_then(|float| i32::try_num_from(crate::round(float)))
            }
        }
    }
}


trait AnchorExt {
    fn get(&self, face: &Face) -> (i32, i32);
}

impl AnchorExt for Anchor<'_> {
    fn get(&self, face: &Face) -> (i32, i32) {
        let mut x = i32::from(self.x);
        let mut y = i32::from(self.y);

        if self.x_device.is_some() || self.y_device.is_some() {
            let (ppem_x, ppem_y) = face.pixels_per_em().unwrap_or((0, 0));
            let coords = face.ttfp_face.variation_coordinates().len();

            if let Some(device) = self.x_device {
                if ppem_x != 0 || coords != 0 {
                    x += device.get_x_delta(face).unwrap_or(0);
                }
            }

            if let Some(device) = self.y_device {
                if ppem_y != 0 || coords != 0 {
                    y += device.get_y_delta(face).unwrap_or(0);
                }
            }
        }

        (x, y)
    }
}
