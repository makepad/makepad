use ttf_parser::GlyphId;
use ttf_parser::opentype_layout::LookupIndex;

use crate::{Face, Mask};
use crate::buffer::{Buffer, GlyphInfo, GlyphPropsFlags};
use super::{lookup_flags, TableIndex, MAX_NESTING_LEVEL};
use super::layout::{LayoutLookup, LayoutTable};

/// Find out whether a lookup would be applied.
pub trait WouldApply {
    /// Whether the lookup would be applied.
    fn would_apply(&self, ctx: &WouldApplyContext) -> bool;
}

/// Apply a lookup.
pub trait Apply {
    /// Apply the lookup.
    fn apply(&self, ctx: &mut ApplyContext) -> Option<()>;
}

pub struct WouldApplyContext<'a> {
    pub glyphs: &'a [GlyphId],
    pub zero_context: bool,
}

pub struct ApplyContext<'a, 'b> {
    pub table_index: TableIndex,
    pub face: &'a Face<'b>,
    pub buffer: &'a mut Buffer,
    pub lookup_mask: Mask,
    pub lookup_index: LookupIndex,
    pub lookup_props: u32,
    pub nesting_level_left: usize,
    pub auto_zwnj: bool,
    pub auto_zwj: bool,
    pub random: bool,
    pub random_state: u32,
}

impl<'a, 'b> ApplyContext<'a, 'b> {
    pub fn new(table_index: TableIndex, face: &'a Face<'b>, buffer: &'a mut Buffer) -> Self {
        Self {
            table_index,
            face,
            buffer,
            lookup_mask: 1,
            lookup_index: u16::MAX,
            lookup_props: 0,
            nesting_level_left: MAX_NESTING_LEVEL,
            auto_zwnj: true,
            auto_zwj: true,
            random: false,
            random_state: 1,
        }
    }

    pub fn random_number(&mut self) -> u32 {
        // http://www.cplusplus.com/reference/random/minstd_rand/
        self.random_state = self.random_state.wrapping_mul(48271) % 2147483647;
        self.random_state
    }

    pub fn recurse(&mut self, sub_lookup_index: LookupIndex) -> Option<()> {
        if self.nesting_level_left == 0 {
            return None;
        }

        self.buffer.max_ops -= 1;
        if self.buffer.max_ops < 0 {
            return None;
        }

        self.nesting_level_left -= 1;
        let saved_props = self.lookup_props;
        let saved_index = self.lookup_index;

        self.lookup_index = sub_lookup_index;
        let applied = match self.table_index {
            TableIndex::GSUB => {
                self.face.gsub
                    .as_ref()
                    .and_then(|table| table.get_lookup(sub_lookup_index))
                    .and_then(|lookup| {
                        self.lookup_props = lookup.props();
                        lookup.apply(self)
                    })
            }
            TableIndex::GPOS => {
                self.face.gpos
                    .as_ref()
                    .and_then(|table| table.get_lookup(sub_lookup_index))
                    .and_then(|lookup| {
                        self.lookup_props = lookup.props();
                        lookup.apply(self)
                    })
            }
        };

        self.lookup_props = saved_props;
        self.lookup_index = saved_index;
        self.nesting_level_left += 1;

        applied
    }

    pub fn check_glyph_property(&self, info: &GlyphInfo, match_props: u32) -> bool {
        let glyph_props = info.glyph_props();

        // Lookup flags are lower 16-bit of match props.
        let lookup_flags = match_props as u16;

        // Not covered, if, for example, glyph class is ligature and
        // match_props includes LookupFlags::IgnoreLigatures
        if glyph_props & lookup_flags & lookup_flags::IGNORE_FLAGS != 0 {
            return false;
        }

        if glyph_props & GlyphPropsFlags::MARK.bits() != 0 {
            // If using mark filtering sets, the high short of
            // match_props has the set index.
            if lookup_flags & lookup_flags::USE_MARK_FILTERING_SET != 0 {
                let set_index = (match_props >> 16) as u16;
                if let Some(table) = self.face.tables().gdef {
                    return table.is_mark_glyph(info.as_glyph(), Some(set_index));
                } else {
                    return false;
                }
            }

            // The second byte of match_props has the meaning
            // "ignore marks of attachment type different than
            // the attachment type specified."
            if lookup_flags & lookup_flags::MARK_ATTACHMENT_TYPE_MASK != 0 {
                return (lookup_flags & lookup_flags::MARK_ATTACHMENT_TYPE_MASK)
                    == (glyph_props & lookup_flags::MARK_ATTACHMENT_TYPE_MASK);
            }
        }

        true
    }

    fn set_glyph_class(
        &mut self,
        glyph_id: GlyphId,
        class_guess: GlyphPropsFlags,
        ligature: bool,
        component: bool,
    ) {
        let cur = self.buffer.cur_mut(0);
        let mut props = cur.glyph_props();

        props |= GlyphPropsFlags::SUBSTITUTED.bits();

        if ligature {
            // In the only place that the MULTIPLIED bit is used, Uniscribe
            // seems to only care about the "last" transformation between
            // Ligature and Multiple substitutions.  Ie. if you ligate, expand,
            // and ligate again, it forgives the multiplication and acts as
            // if only ligation happened.  As such, clear MULTIPLIED bit.
            props &= !GlyphPropsFlags::MULTIPLIED.bits();
            props |= GlyphPropsFlags::LIGATED.bits();
        }

        if component {
            props |= GlyphPropsFlags::MULTIPLIED.bits();
        }

        let has_glyph_classes = self.face.tables().gdef
            .map_or(false, |table| table.has_glyph_classes());

        if has_glyph_classes {
            props = (props & !GlyphPropsFlags::CLASS_MASK.bits()) | self.face.glyph_props(glyph_id);
        } else if !class_guess.is_empty() {
            props = (props & !GlyphPropsFlags::CLASS_MASK.bits()) | class_guess.bits();
        }

        cur.set_glyph_props(props);
    }

    pub fn replace_glyph(&mut self, glyph_id: GlyphId) {
        self.set_glyph_class(glyph_id, GlyphPropsFlags::empty(), false, false);
        self.buffer.replace_glyph(u32::from(glyph_id.0));
    }

    pub fn replace_glyph_inplace(&mut self, glyph_id: GlyphId) {
        self.set_glyph_class(glyph_id, GlyphPropsFlags::empty(), false, false);
        self.buffer.cur_mut(0).glyph_id = u32::from(glyph_id.0);
    }

    pub fn replace_glyph_with_ligature(&mut self, glyph_id: GlyphId, class_guess: GlyphPropsFlags) {
        self.set_glyph_class(glyph_id, class_guess, true, false);
        self.buffer.replace_glyph(u32::from(glyph_id.0));
    }

    pub fn output_glyph_for_component(&mut self, glyph_id: GlyphId, class_guess: GlyphPropsFlags) {
        self.set_glyph_class(glyph_id, class_guess, false, true);
        self.buffer.output_glyph(u32::from(glyph_id.0));
    }
}
