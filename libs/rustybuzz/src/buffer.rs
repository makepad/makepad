use alloc::{string::String, vec::Vec};
use core::convert::TryFrom;

use ttf_parser::GlyphId;

use crate::{script, Direction, Face, Language, Mask, Script};
use crate::unicode::{CharExt, GeneralCategory, GeneralCategoryExt, Space};

const CONTEXT_LENGTH: usize = 5;


pub mod glyph_flag {
    /// Indicates that if input text is broken at the
    /// beginning of the cluster this glyph is part of,
    /// then both sides need to be re-shaped, as the
    /// result might be different.  On the flip side,
    /// it means that when this flag is not present,
    /// then it's safe to break the glyph-run at the
    /// beginning of this cluster, and the two sides
    /// represent the exact same result one would get
    /// if breaking input text at the beginning of
    /// this cluster and shaping the two sides
    /// separately.  This can be used to optimize
    /// paragraph layout, by avoiding re-shaping
    /// of each line after line-breaking, or limiting
    /// the reshaping to a small piece around the
    /// breaking point only.
    pub const UNSAFE_TO_BREAK: u32 = 0x00000001;

    /// All the currently defined flags.
    pub const DEFINED: u32 = 0x00000001; // OR of all defined flags
}


/// Holds the positions of the glyph in both horizontal and vertical directions.
///
/// All positions are relative to the current point.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct GlyphPosition {
    /// How much the line advances after drawing this glyph when setting text in
    /// horizontal direction.
    pub x_advance: i32,
    /// How much the line advances after drawing this glyph when setting text in
    /// vertical direction.
    pub y_advance: i32,
    /// How much the glyph moves on the X-axis before drawing it, this should
    /// not affect how much the line advances.
    pub x_offset: i32,
    /// How much the glyph moves on the Y-axis before drawing it, this should
    /// not affect how much the line advances.
    pub y_offset: i32,
    var: u32,
}

unsafe impl bytemuck::Zeroable for GlyphPosition {}
unsafe impl bytemuck::Pod for GlyphPosition {}

impl GlyphPosition {
    #[inline]
    pub(crate) fn attach_chain(&self) -> i16 {
        // glyph to which this attaches to, relative to current glyphs;
        // negative for going back, positive for forward.
        let v: &[i16; 2] = bytemuck::cast_ref(&self.var);
        v[0]
    }

    #[inline]
    pub(crate) fn set_attach_chain(&mut self, n: i16) {
        let v: &mut [i16; 2] = bytemuck::cast_mut(&mut self.var);
        v[0] = n;
    }

    #[inline]
    pub(crate) fn attach_type(&self) -> u8 {
        // attachment type
        // Note! if attach_chain() is zero, the value of attach_type() is irrelevant.
        let v: &[u8; 4] = bytemuck::cast_ref(&self.var);
        v[2]
    }

    #[inline]
    pub(crate) fn set_attach_type(&mut self, n: u8) {
        let v: &mut [u8; 4] = bytemuck::cast_mut(&mut self.var);
        v[2] = n;
    }
}


/// A glyph info.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct GlyphInfo {
    // NOTE: Stores a Unicode codepoint before shaping and a glyph ID after.
    //       Just like harfbuzz, we are using the same variable for two purposes.
    //       Occupies u32 as a codepoint and u16 as a glyph id.
    /// A selected glyph.
    ///
    /// Guarantee to be <= `u16::MAX`.
    pub glyph_id: u32,
    pub(crate) mask: Mask,
    /// An index to the start of the grapheme cluster in the original string.
    ///
    /// [Read more on clusters](https://harfbuzz.github.io/clusters.html).
    pub cluster: u32,
    pub(crate) var1: u32,
    pub(crate) var2: u32,
}

unsafe impl bytemuck::Zeroable for GlyphInfo {}
unsafe impl bytemuck::Pod for GlyphInfo {}

const IS_LIG_BASE: u8 = 0x10;

impl GlyphInfo {
    /// Indicates that if input text is broken at the beginning of the cluster this glyph
    /// is part of, then both sides need to be re-shaped, as the result might be different.
    ///
    /// On the flip side, it means that when this flag is not present,
    /// then it's safe to break the glyph-run at the beginning of this cluster,
    /// and the two sides represent the exact same result one would get if breaking input text
    /// at the beginning of this cluster and shaping the two sides separately.
    /// This can be used to optimize paragraph layout, by avoiding re-shaping of each line
    /// after line-breaking, or limiting the reshaping to a small piece around
    /// the breaking point only.
    pub fn unsafe_to_break(&self) -> bool {
        self.mask & glyph_flag::UNSAFE_TO_BREAK != 0
    }

    #[inline]
    pub(crate) fn as_char(&self) -> char {
        char::try_from(self.glyph_id).unwrap()
    }

    #[inline]
    pub(crate) fn as_glyph(&self) -> GlyphId {
        debug_assert!(self.glyph_id <= u32::from(u16::MAX));
        GlyphId(self.glyph_id as u16)
    }

    // Var allocation: unicode_props
    // Used during the entire shaping process to store unicode properties

    #[inline]
    fn unicode_props(&self) -> u16 {
        let v: &[u16; 2] = bytemuck::cast_ref(&self.var2);
        v[0]
    }

    #[inline]
    fn set_unicode_props(&mut self, n: u16) {
        let v: &mut [u16; 2] = bytemuck::cast_mut(&mut self.var2);
        v[0] = n;
    }

    pub(crate) fn init_unicode_props(&mut self, scratch_flags: &mut BufferScratchFlags) {
        let u = self.as_char();
        let gc = u.general_category();
        let mut props = gc.to_rb() as u16;

        if u as u32 >= 0x80 {
            *scratch_flags |= BufferScratchFlags::HAS_NON_ASCII;

            if u.is_default_ignorable() {
                props |= UnicodeProps::IGNORABLE.bits;
                *scratch_flags |= BufferScratchFlags::HAS_DEFAULT_IGNORABLES;

                match u as u32 {
                    0x200C => props |= UnicodeProps::CF_ZWNJ.bits,
                    0x200D => props |= UnicodeProps::CF_ZWJ.bits,

                    // Mongolian Free Variation Selectors need to be remembered
                    // because although we need to hide them like default-ignorables,
                    // they need to non-ignorable during shaping.  This is similar to
                    // what we do for joiners in Indic-like shapers, but since the
                    // FVSes are GC=Mn, we have use a separate bit to remember them.
                    // Fixes:
                    // https://github.com/harfbuzz/harfbuzz/issues/234
                    0x180B..=0x180D => props |= UnicodeProps::HIDDEN.bits,

                    // TAG characters need similar treatment. Fixes:
                    // https://github.com/harfbuzz/harfbuzz/issues/463
                    0xE0020..=0xE007F => props |= UnicodeProps::HIDDEN.bits,

                    // COMBINING GRAPHEME JOINER should not be skipped; at least some times.
                    // https://github.com/harfbuzz/harfbuzz/issues/554
                    0x034F => {
                        props |= UnicodeProps::HIDDEN.bits;
                        *scratch_flags |= BufferScratchFlags::HAS_CGJ;
                    }

                    _ => {}
                }
            }

            if gc.is_mark() {
                props |= UnicodeProps::CONTINUATION.bits;
                props |= (u.modified_combining_class() as u16) << 8;
            }
        }

        self.set_unicode_props(props);
    }

    #[inline]
    pub(crate) fn general_category(&self) -> GeneralCategory {
        let n = self.unicode_props() & UnicodeProps::GENERAL_CATEGORY.bits;
        GeneralCategory::from_rb(n as u32)
    }

    #[inline]
    pub(crate) fn set_general_category(&mut self, gc: GeneralCategory) {
        let gc = gc.to_rb();
        let n = (gc as u16) | (self.unicode_props() & (0xFF & !UnicodeProps::GENERAL_CATEGORY.bits));
        self.set_unicode_props(n);
    }

    #[inline]
    pub(crate) fn space_fallback(&self) -> Option<Space> {
        if self.general_category() == GeneralCategory::SpaceSeparator {
            let n = (self.unicode_props() >> 8) as u8;
            Some(n)
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn set_space_fallback(&mut self, space: Space) {
        if self.general_category() == GeneralCategory::SpaceSeparator {
            let n = ((space as u16) << 8) | (self.unicode_props() & 0xFF);
            self.set_unicode_props(n);
        }
    }

    #[inline]
    pub(crate) fn is_unicode_mark(&self) -> bool {
        self.general_category().is_mark()
    }

    #[inline]
    pub(crate) fn is_zwnj(&self) -> bool {
        self.general_category() == GeneralCategory::Format
            && (self.unicode_props() & UnicodeProps::CF_ZWNJ.bits != 0)
    }

    #[inline]
    pub(crate) fn is_zwj(&self) -> bool {
        self.general_category() == GeneralCategory::Format
            && (self.unicode_props() & UnicodeProps::CF_ZWJ.bits != 0)
    }

    #[inline]
    pub(crate) fn modified_combining_class(&self) -> u8 {
        if self.is_unicode_mark() {
            (self.unicode_props() >> 8) as u8
        } else {
            0
        }
    }

    #[inline]
    pub(crate) fn set_modified_combining_class(&mut self, mcc: u8) {
        if self.is_unicode_mark() {
            let n = ((mcc as u16) << 8) | (self.unicode_props() & 0xFF);
            self.set_unicode_props(n);
        }
    }

    #[inline]
    pub(crate) fn is_hidden(&self) -> bool {
        self.unicode_props() & UnicodeProps::HIDDEN.bits != 0
    }

    #[inline]
    pub(crate) fn unhide(&mut self) {
        let mut n = self.unicode_props();
        n &= !UnicodeProps::HIDDEN.bits;
        self.set_unicode_props(n);
    }

    #[inline]
    pub(crate) fn set_continuation(&mut self) {
        let mut n = self.unicode_props();
        n |= UnicodeProps::CONTINUATION.bits;
        self.set_unicode_props(n);
    }

    #[inline]
    pub(crate) fn reset_continuation(&mut self) {
        let mut n = self.unicode_props();
        n &= !UnicodeProps::CONTINUATION.bits;
        self.set_unicode_props(n);
    }

    #[inline]
    pub(crate) fn is_continuation(&self) -> bool {
        self.unicode_props() & UnicodeProps::CONTINUATION.bits != 0
    }

    #[inline]
    pub(crate) fn is_default_ignorable(&self) -> bool {
        let n = self.unicode_props() & UnicodeProps::IGNORABLE.bits;
        n != 0 && !self.is_ligated()
    }

    // Var allocation: lig_props (aka lig_id / lig_comp)
    // Used during the GSUB/GPOS processing to track ligatures
    //
    // When a ligature is formed:
    //
    //   - The ligature glyph and any marks in between all the same newly allocated
    //     lig_id,
    //   - The ligature glyph will get lig_num_comps set to the number of components
    //   - The marks get lig_comp > 0, reflecting which component of the ligature
    //     they were applied to.
    //   - This is used in GPOS to attach marks to the right component of a ligature
    //     in MarkLigPos,
    //   - Note that when marks are ligated together, much of the above is skipped
    //     and the current lig_id reused.
    //
    // When a multiple-substitution is done:
    //
    //   - All resulting glyphs will have lig_id = 0,
    //   - The resulting glyphs will have lig_comp = 0, 1, 2, ... respectively.
    //   - This is used in GPOS to attach marks to the first component of a
    //     multiple substitution in MarkBasePos.
    //
    // The numbers are also used in GPOS to do mark-to-mark positioning only
    // to marks that belong to the same component of the same ligature.

    #[inline]
    pub(crate) fn lig_props(&self) -> u8 {
        let v: &[u8; 4] = bytemuck::cast_ref(&self.var1);
        v[2]
    }

    #[inline]
    pub(crate) fn set_lig_props(&mut self, n: u8) {
        let v: &mut [u8; 4] = bytemuck::cast_mut(&mut self.var1);
        v[2] = n;
    }

    pub(crate) fn set_lig_props_for_ligature(&mut self, lig_id: u8, lig_num_comps: u8) {
        self.set_lig_props((lig_id << 5) | IS_LIG_BASE | (lig_num_comps & 0x0F));
    }

    pub(crate) fn set_lig_props_for_mark(&mut self, lig_id: u8, lig_comp: u8) {
        self.set_lig_props((lig_id << 5) | (lig_comp & 0x0F));
    }

    pub(crate) fn set_lig_props_for_component(&mut self, lig_comp: u8) {
        self.set_lig_props_for_mark(0, lig_comp)
    }

    #[inline]
    pub(crate) fn lig_id(&self) -> u8 {
        self.lig_props() >> 5
    }

    #[inline]
    pub(crate) fn is_ligated_internal(&self) -> bool {
        self.lig_props() & IS_LIG_BASE != 0
    }

    #[inline]
    pub(crate) fn lig_comp(&self) -> u8 {
        if self.is_ligated_internal() {
            0
        } else {
            self.lig_props() & 0x0F
        }
    }

    #[inline]
    pub(crate) fn lig_num_comps(&self) -> u8 {
        if self.glyph_props() & GlyphPropsFlags::LIGATURE.bits != 0 && self.is_ligated_internal() {
            self.lig_props() & 0x0F
        } else {
            1
        }
    }

    // Var allocation: glyph_props
    // Used during the GSUB/GPOS processing to store GDEF glyph properties

    #[inline]
    pub(crate) fn glyph_props(&self) -> u16 {
        let v: &[u16; 2] = bytemuck::cast_ref(&self.var1);
        v[0]
    }

    #[inline]
    pub(crate) fn set_glyph_props(&mut self, n: u16) {
        let v: &mut [u16; 2] = bytemuck::cast_mut(&mut self.var1);
        v[0] = n;
    }

    #[inline]
    pub(crate) fn is_base_glyph(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::BASE_GLYPH.bits != 0
    }

    #[inline]
    pub(crate) fn is_ligature(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::LIGATURE.bits != 0
    }

    #[inline]
    pub(crate) fn is_mark(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::MARK.bits != 0
    }

    #[inline]
    pub(crate) fn is_substituted(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::SUBSTITUTED.bits != 0
    }

    #[inline]
    pub(crate) fn is_ligated(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::LIGATED.bits != 0
    }

    #[inline]
    pub(crate) fn is_multiplied(&self) -> bool {
        self.glyph_props() & GlyphPropsFlags::MULTIPLIED.bits != 0
    }

    #[inline]
    pub(crate) fn is_ligated_and_didnt_multiply(&self) -> bool {
        self.is_ligated() && !self.is_multiplied()
    }

    #[inline]
    pub(crate) fn clear_ligated_and_multiplied(&mut self) {
        let mut n = self.glyph_props();
        n &= !(GlyphPropsFlags::LIGATED | GlyphPropsFlags::MULTIPLIED).bits;
        self.set_glyph_props(n);
    }

    #[inline]
    pub(crate) fn clear_substituted(&mut self) {
        let mut n = self.glyph_props();
        n &= !GlyphPropsFlags::SUBSTITUTED.bits;
        self.set_glyph_props(n);
    }

    // Var allocation: syllable
    // Used during the GSUB/GPOS processing to store shaping boundaries

    #[inline]
    pub(crate) fn syllable(&self) -> u8 {
        let v: &[u8; 4] = bytemuck::cast_ref(&self.var1);
        v[3]
    }

    #[inline]
    pub(crate) fn set_syllable(&mut self, n: u8) {
        let v: &mut [u8; 4] = bytemuck::cast_mut(&mut self.var1);
        v[3] = n;
    }

    // Var allocation: glyph_index
    // Used during the normalization process to store glyph indices

    #[inline]
    pub(crate) fn glyph_index(&mut self) -> u32 {
        self.var1
    }

    #[inline]
    pub(crate) fn set_glyph_index(&mut self, n: u32) {
        self.var1 = n;
    }
}


/// A cluster level.
#[allow(missing_docs)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BufferClusterLevel {
    MonotoneGraphemes,
    MonotoneCharacters,
    Characters,
}

impl Default for BufferClusterLevel {
    #[inline]
    fn default() -> Self {
        BufferClusterLevel::MonotoneGraphemes
    }
}


pub struct Buffer {
    // Information about how the text in the buffer should be treated.
    pub flags: BufferFlags,
    pub cluster_level: BufferClusterLevel,
    pub invisible: Option<GlyphId>,
    pub scratch_flags: BufferScratchFlags,
    // Maximum allowed len.
    pub max_len: usize,
    /// Maximum allowed operations.
    pub max_ops: i32,

    // Buffer contents.
    pub direction: Direction,
    pub script: Option<Script>,
    pub language: Option<Language>,

    /// Allocations successful.
    pub successful: bool,
    /// Whether we have an output buffer going on.
    have_output: bool,
    pub have_separate_output: bool,
    /// Whether we have positions
    pub have_positions: bool,

    pub idx: usize,
    pub len: usize,
    pub out_len: usize,

    pub info: Vec<GlyphInfo>,
    pub pos: Vec<GlyphPosition>,

    serial: u32,

    // Text before / after the main buffer contents.
    // Always in Unicode, and ordered outward.
    // Index 0 is for "pre-context", 1 for "post-context".
    pub context: [[char; CONTEXT_LENGTH]; 2],
    pub context_len: [usize; 2],
}

impl Buffer {
    pub const MAX_LEN_FACTOR: usize = 32;
    pub const MAX_LEN_MIN: usize = 8192;
    // Shaping more than a billion chars? Let us know!
    pub const MAX_LEN_DEFAULT: usize = 0x3FFFFFFF;

    pub const MAX_OPS_FACTOR: i32 = 64;
    pub const MAX_OPS_MIN: i32 = 1024;
    // Shaping more than a billion operations? Let us know!
    pub const MAX_OPS_DEFAULT: i32 = 0x1FFFFFFF;

    /// Creates a new `Buffer`.
    pub fn new() -> Self {
        Buffer {
            flags: BufferFlags::empty(),
            cluster_level: BufferClusterLevel::default(),
            invisible: None,
            scratch_flags: BufferScratchFlags::default(),
            max_len: Self::MAX_LEN_DEFAULT,
            max_ops: Self::MAX_OPS_DEFAULT,
            direction: Direction::Invalid,
            script: None,
            language: None,
            successful: true,
            have_output: false,
            have_positions: false,
            idx: 0,
            len: 0,
            out_len: 0,
            info: Vec::new(),
            pos: Vec::new(),
            have_separate_output: false,
            serial: 0,
            context: [['\0', '\0', '\0', '\0', '\0'], ['\0', '\0', '\0', '\0', '\0']],
            context_len: [0, 0],
        }
    }

    #[inline]
    pub fn info_slice(&self) -> &[GlyphInfo] {
        &self.info[..self.len]
    }

    #[inline]
    pub fn info_slice_mut(&mut self) -> &mut [GlyphInfo] {
        &mut self.info[..self.len]
    }

    #[inline]
    pub fn out_info(&self) -> &[GlyphInfo] {
        if self.have_separate_output {
            bytemuck::cast_slice(self.pos.as_slice())
        } else {
            &self.info
        }
    }

    #[inline]
    pub fn out_info_mut(&mut self) -> &mut [GlyphInfo] {
        if self.have_separate_output {
            bytemuck::cast_slice_mut(self.pos.as_mut_slice())
        } else {
            &mut self.info
        }
    }

    #[inline]
    fn set_out_info(&mut self, i: usize, info: GlyphInfo) {
        self.out_info_mut()[i] = info;
    }

    #[inline]
    pub fn cur(&self, i: usize) -> &GlyphInfo {
        &self.info[self.idx + i]
    }

    #[inline]
    pub fn cur_mut(&mut self, i: usize) -> &mut GlyphInfo {
        let idx = self.idx + i;
        &mut self.info[idx]
    }

    #[inline]
    pub fn cur_pos_mut(&mut self) -> &mut GlyphPosition {
        let i = self.idx;
        &mut self.pos[i]
    }

    #[inline]
    pub fn prev(&self) -> &GlyphInfo {
        let idx = self.out_len.saturating_sub(1);
        &self.out_info()[idx]
    }

    #[inline]
    pub fn prev_mut(&mut self) -> &mut GlyphInfo {
        let idx = self.out_len.saturating_sub(1);
        &mut self.out_info_mut()[idx]
    }

    fn clear(&mut self) {
        self.direction = Direction::Invalid;
        self.script = None;
        self.language = None;
        self.scratch_flags = BufferScratchFlags::default();

        self.successful = true;
        self.have_output = false;
        self.have_positions = false;

        self.idx = 0;
        self.info.clear();
        self.pos.clear();
        self.len = 0;
        self.out_len = 0;
        self.have_separate_output = false;

        self.serial = 0;

        self.context = [['\0', '\0', '\0', '\0', '\0'], ['\0', '\0', '\0', '\0', '\0']];
        self.context_len = [0, 0];
    }

    #[inline]
    pub fn backtrack_len(&self) -> usize {
        if self.have_output { self.out_len } else { self.idx }
    }

    #[inline]
    pub fn lookahead_len(&self) -> usize {
        self.len - self.idx
    }

    #[inline]
    fn next_serial(&mut self) -> u32 {
        self.serial += 1;
        self.serial
    }

    fn add(&mut self, codepoint: u32, cluster: u32) {
        self.ensure(self.len + 1);

        let i = self.len;
        self.info[i] = GlyphInfo {
            glyph_id: codepoint,
            mask: 0,
            cluster,
            var1: 0,
            var2: 0,
        };

        self.len += 1;
    }

    #[inline]
    pub fn reverse(&mut self) {
        if self.is_empty() {
            return;
        }

        self.reverse_range(0, self.len);
    }

    pub fn reverse_range(&mut self, start: usize, end: usize) {
        if end - start < 2 {
            return;
        }

        self.info[start..end].reverse();
        if self.have_positions {
            self.pos[start..end].reverse();
        }
    }

    #[inline]
    fn reset_clusters(&mut self) {
        for (i, info) in self.info.iter_mut().enumerate() {
            info.cluster = i as u32;
        }
    }

    pub fn guess_segment_properties(&mut self) {
        if self.script.is_none() {
            for info in &self.info {
                match info.as_char().script() {
                      crate::script::COMMON
                    | crate::script::INHERITED
                    | crate::script::UNKNOWN => {}
                    s => {
                        self.script = Some(s);
                        break;
                    }
                }
            }
        }

        if self.direction == Direction::Invalid {
            if let Some(script) = self.script {
                self.direction = Direction::from_script(script).unwrap_or_default();
            }

            if self.direction == Direction::Invalid {
                self.direction = Direction::LeftToRight;
            }
        }

        // TODO: language must be set
    }

    pub fn swap_buffers(&mut self) {
        if !self.successful {
            return;
        }

        assert!(self.have_output);
        self.have_output = false;

        if self.have_separate_output {
            // Swap info and pos buffers.
            let info: Vec<GlyphPosition> = bytemuck::cast_vec(core::mem::take(&mut self.info));
            let pos: Vec<GlyphInfo> = bytemuck::cast_vec(core::mem::take(&mut self.pos));
            self.pos = info;
            self.info = pos;
        }

        core::mem::swap(&mut self.len, &mut self.out_len);

        self.idx = 0;
    }

    pub fn remove_output(&mut self) {
        self.have_output = false;
        self.have_positions = false;

        self.out_len = 0;
        self.have_separate_output = false;
    }

    pub fn clear_output(&mut self) {
        self.have_output = true;
        self.have_positions = false;

        self.out_len = 0;
        self.have_separate_output = false;
    }

    pub fn clear_positions(&mut self) {
        self.have_output = false;
        self.have_positions = true;

        self.out_len = 0;
        self.have_separate_output = false;

        for pos in &mut self.pos {
            *pos = GlyphPosition::default();
        }
    }

    pub fn replace_glyphs(&mut self, num_in: usize, num_out: usize, glyph_data: &[u32]) {
        if !self.make_room_for(num_in, num_out) {
            return;
        }

        assert!(self.idx + num_in <= self.len);

        self.merge_clusters(self.idx, self.idx + num_in);

        let orig_info = self.info[self.idx];
        for i in 0..num_out {
            let ii = self.out_len + i;
            self.set_out_info(ii, orig_info);
            self.out_info_mut()[ii].glyph_id = glyph_data[i];
        }

        self.idx += num_in;
        self.out_len += num_out;
    }

    pub fn replace_glyph(&mut self, glyph_index: u32) {
        if self.have_separate_output || self.out_len != self.idx {
            if !self.make_room_for(1, 1) {
                return;
            }

            self.set_out_info(self.out_len, self.info[self.idx]);
        }

        let out_len = self.out_len;
        self.out_info_mut()[out_len].glyph_id = glyph_index;

        self.idx += 1;
        self.out_len += 1;
    }

    pub fn output_glyph(&mut self, glyph_index: u32) {
        if !self.make_room_for(0, 1) {
            return;
        }

        if self.idx == self.len && self.out_len == 0 {
            return;
        }

        let out_len = self.out_len;
        if self.idx < self.len {
            self.set_out_info(out_len, self.info[self.idx]);
        } else {
            let info = self.out_info()[out_len - 1];
            self.set_out_info(out_len, info);
        }

        self.out_info_mut()[out_len].glyph_id = glyph_index;

        self.out_len += 1;
    }

    pub fn output_info(&mut self, glyph_info: GlyphInfo) {
        if !self.make_room_for(0, 1) {
            return;
        }

        self.set_out_info(self.out_len, glyph_info);
        self.out_len += 1;
    }

    pub fn output_char(&mut self, unichar: u32, glyph: u32) {
        self.cur_mut(0).set_glyph_index(glyph);
        // This is very confusing indeed.
        self.output_glyph(unichar);
        let mut flags = self.scratch_flags;
        self.prev_mut().init_unicode_props(&mut flags);
        self.scratch_flags = flags;
    }

    /// Copies glyph at idx to output but doesn't advance idx.
    pub fn copy_glyph(&mut self) {
        if !self.make_room_for(0, 1) {
            return;
        }

        self.set_out_info(self.out_len, self.info[self.idx]);
        self.out_len += 1;
    }

    /// Copies glyph at idx to output and advance idx.
    ///
    /// If there's no output, just advance idx.
    pub fn next_glyph(&mut self) {
        if self.have_output {
            if self.have_separate_output || self.out_len != self.idx {
                if !self.make_room_for(1, 1) {
                    return;
                }

                self.set_out_info(self.out_len, self.info[self.idx]);
            }

            self.out_len += 1;
        }

        self.idx += 1;
    }

    /// Copies n glyphs at idx to output and advance idx.
    ///
    /// If there's no output, just advance idx.
    pub fn next_glyphs(&mut self, n: usize) {
        if self.have_output {
            if self.have_separate_output || self.out_len != self.idx {
                if !self.make_room_for(n, n) {
                    return;
                }

                for i in 0..n {
                    self.set_out_info(self.out_len + i, self.info[self.idx + i]);
                }
            }

            self.out_len += n;
        }

        self.idx += n;
    }

    pub fn next_char(&mut self, glyph: u32) {
        self.cur_mut(0).set_glyph_index(glyph);
        self.next_glyph();
    }

    /// Advance idx without copying to output.
    pub fn skip_glyph(&mut self) {
        self.idx += 1;
    }

    pub fn reset_masks(&mut self, mask: Mask) {
        for info in &mut self.info[..self.len] {
            info.mask = mask;
        }
    }

    pub fn set_masks(
        &mut self,
        mut value: Mask,
        mask: Mask,
        cluster_start: u32,
        cluster_end: u32,
    ) {
        let not_mask = !mask;
        value &= mask;

        if mask == 0 {
            return;
        }

        if cluster_start == 0 && cluster_end == core::u32::MAX {
            for info in &mut self.info[..self.len] {
                info.mask = (info.mask & not_mask) | value;
            }

            return;
        }

        for info in &mut self.info[..self.len] {
            if cluster_start <= info.cluster && info.cluster < cluster_end {
                info.mask = (info.mask & not_mask) | value;
            }
        }
    }

    pub fn merge_clusters(&mut self, start: usize, end: usize) {
        if end - start < 2 {
            return;
        }

        self.merge_clusters_impl(start, end)
    }

    fn merge_clusters_impl(&mut self, mut start: usize, mut end: usize) {
        if self.cluster_level == BufferClusterLevel::Characters {
            self.unsafe_to_break(start, end);
            return;
        }

        let mut cluster = self.info[start].cluster;

        for i in start+1..end {
            cluster = core::cmp::min(cluster, self.info[i].cluster);
        }

        // Extend end
        while end < self.len && self.info[end - 1].cluster == self.info[end].cluster {
            end += 1;
        }

        // Extend start
        while end < start && self.info[start - 1].cluster == self.info[start].cluster {
            start -= 1;
        }

        // If we hit the start of buffer, continue in out-buffer.
        if self.idx == start {
            let mut i = self.out_len;
            while i != 0 && self.out_info()[i - 1].cluster == self.info[start].cluster {
                Self::set_cluster(&mut self.out_info_mut()[i - 1], cluster, 0);
                i -= 1;
            }
        }

        for i in start..end {
            Self::set_cluster(&mut self.info[i], cluster, 0);
        }
    }

    pub fn merge_out_clusters(&mut self, mut start: usize, mut end: usize) {
        if self.cluster_level == BufferClusterLevel::Characters {
            return;
        }

        if end - start < 2 {
            return;
        }

        let mut cluster = self.out_info()[start].cluster;

        for i in start+1..end {
            cluster = core::cmp::min(cluster, self.out_info()[i].cluster);
        }

        // Extend start
        while start != 0 && self.out_info()[start - 1].cluster == self.out_info()[start].cluster {
            start -= 1;
        }

        // Extend end
        while end < self.out_len && self.out_info()[end - 1].cluster == self.out_info()[end].cluster {
            end += 1;
        }

        // If we hit the start of buffer, continue in out-buffer.
        if end == self.out_len {
            let mut i = self.idx;
            while i < self.len && self.info[i].cluster == self.out_info()[end - 1].cluster {
                Self::set_cluster(&mut self.info[i], cluster, 0);
                i += 1;
            }
        }

        for i in start..end {
            Self::set_cluster(&mut self.out_info_mut()[i], cluster, 0);
        }
    }

    /// Merge clusters for deleting current glyph, and skip it.
    pub fn delete_glyph(&mut self) {
        let cluster = self.info[self.idx].cluster;

        if self.idx + 1 < self.len && cluster == self.info[self.idx + 1].cluster {
            // Cluster survives; do nothing.
            self.skip_glyph();
            return;
        }

        if self.out_len != 0 {
            // Merge cluster backward.
            if cluster < self.out_info()[self.out_len - 1].cluster {
                let mask = self.info[self.idx].mask;
                let old_cluster = self.out_info()[self.out_len - 1].cluster;

                let mut i = self.out_len;
                while i != 0 && self.out_info()[i - 1].cluster == old_cluster {
                    Self::set_cluster(&mut self.out_info_mut()[i - 1], cluster, mask);
                    i -= 1;
                }
            }

            self.skip_glyph();
            return;
        }

        if self.idx + 1 < self.len {
            // Merge cluster forward.
            self.merge_clusters(self.idx, self.idx + 2);
        }

        self.skip_glyph();
    }

    pub fn delete_glyphs_inplace(&mut self, filter: impl Fn(&GlyphInfo) -> bool) {
        // Merge clusters and delete filtered glyphs.
        // NOTE! We can't use out-buffer as we have positioning data.
        let mut j = 0;

        for i in 0..self.len {
            if filter(&self.info[i]) {
                // Merge clusters.
                // Same logic as delete_glyph(), but for in-place removal

                let cluster = self.info[i].cluster;
                if i + 1 < self.len && cluster == self.info[i + 1].cluster {
                    // Cluster survives; do nothing.
                    continue;
                }

                if j != 0 {
                    // Merge cluster backward.
                    if cluster < self.info[j - 1].cluster {
                        let mask = self.info[i].mask;
                        let old_cluster = self.info[j - 1].cluster;

                        let mut k = j;
                        while k > 0 && self.info[k - 1].cluster == old_cluster {
                            Self::set_cluster(&mut self.info[k - 1], cluster, mask);
                            k -= 1;
                        }
                    }
                    continue;
                }

                if i + 1 < self.len {
                    // Merge cluster forward.
                    self.merge_clusters(i, i + 2);
                }

                continue;
            }

            if j != i {
                self.info[j] = self.info[i];
                self.pos[j] = self.pos[i];
            }

            j += 1;
        }

        self.len = j;
    }

    pub fn unsafe_to_break(&mut self, start: usize, end: usize) {
        if end - start < 2 {
            return;
        }

        self.unsafe_to_break_impl(start, end);
    }

    fn unsafe_to_break_impl(&mut self, start: usize, end: usize) {
        let mut cluster = core::u32::MAX;
        cluster = Self::_unsafe_to_break_find_min_cluster(&self.info, start, end, cluster);
        let unsafe_to_break = Self::_unsafe_to_break_set_mask(&mut self.info, start, end, cluster);
        if unsafe_to_break {
            self.scratch_flags |= BufferScratchFlags::HAS_UNSAFE_TO_BREAK;
        }
    }

    pub fn unsafe_to_break_from_outbuffer(&mut self, start: usize, end: usize) {
        if !self.have_output {
            self.unsafe_to_break_impl(start, end);
            return;
        }

        assert!(start <= self.out_len);
        assert!(self.idx <= end);

        let mut cluster = core::u32::MAX;
        cluster = Self::_unsafe_to_break_find_min_cluster(self.out_info(), start, self.out_len, cluster);
        cluster = Self::_unsafe_to_break_find_min_cluster(&self.info, self.idx, end, cluster);
        let idx = self.idx;
        let out_len = self.out_len;
        let unsafe_to_break1 = Self::_unsafe_to_break_set_mask(self.out_info_mut(), start, out_len, cluster);
        let unsafe_to_break2 = Self::_unsafe_to_break_set_mask(&mut self.info, idx, end, cluster);

        if unsafe_to_break1 || unsafe_to_break2 {
            self.scratch_flags |= BufferScratchFlags::HAS_UNSAFE_TO_BREAK;
        }
    }

    pub fn move_to(&mut self, i: usize) -> bool {
        if !self.have_output {
            assert!(i <= self.len);
            self.idx = i;
            return true;
        }

        if !self.successful {
            return false;
        }

        assert!(i <= self.out_len + (self.len - self.idx));

        if self.out_len < i {
            let count = i - self.out_len;
            if !self.make_room_for(count, count) {
                return false;
            }

            for j in 0..count {
                self.set_out_info(self.out_len + j, self.info[self.idx + j]);
            }

            self.idx += count;
            self.out_len += count;
        } else if self.out_len > i {
            // Tricky part: rewinding...
            let count = self.out_len - i;

            // This will blow in our face if memory allocation fails later
            // in this same lookup...
            //
            // We used to shift with extra 32 items, instead of the 0 below.
            // But that would leave empty slots in the buffer in case of allocation
            // failures.  Setting to zero for now to avoid other problems (see
            // comments in shift_forward().  This can cause O(N^2) behavior more
            // severely than adding 32 empty slots can...
            if self.idx < count {
                self.shift_forward(count);
            }

            assert!(self.idx >= count);

            self.idx -= count;
            self.out_len -= count;

            for j in 0..count {
                self.info[self.idx + j] = self.out_info()[self.out_len + j];
            }
        }

        true
    }

    pub fn ensure(&mut self, size: usize) -> bool {
        if size < self.len {
            return true;
        }

        if size > self.max_len {
            self.successful = false;
            return false;
        }

        self.info.resize(size, GlyphInfo::default());
        self.pos.resize(size, GlyphPosition::default());
        true
    }

    pub fn set_len(&mut self, len: usize) {
        self.ensure(len);
        self.len = len;
    }

    fn make_room_for(&mut self, num_in: usize, num_out: usize) -> bool {
        if !self.ensure(self.out_len + num_out) {
            return false;
        }

        if !self.have_separate_output && self.out_len + num_out > self.idx + num_in {
            assert!(self.have_output);

            self.have_separate_output = true;
            for i in 0..self.out_len {
                self.set_out_info(i, self.info[i]);
            }
        }

        true
    }

    fn shift_forward(&mut self, count: usize) {
        assert!(self.have_output);
        self.ensure(self.len + count);

        for i in 0..(self.len - self.idx) {
            self.info[self.idx + count + i] = self.info[self.idx + i];
        }

        if self.idx + count > self.len {
            for info in &mut self.info[self.len..self.idx+count] {
                *info = GlyphInfo::default();
            }
        }

        self.len += count;
        self.idx += count;
    }

    pub fn sort(&mut self, start: usize, end: usize, cmp: impl Fn(&GlyphInfo, &GlyphInfo) -> bool) {
        assert!(!self.have_positions);

        for i in start+1..end {
            let mut j = i;
            while j > start && cmp(&self.info[j - 1], &self.info[i]) {
                j -= 1;
            }

            if i == j {
                continue;
            }

            // Move item i to occupy place for item j, shift what's in between.
            self.merge_clusters(j, i + 1);

            {
                let t = self.info[i];
                for idx in (0..i-j).rev() {
                    self.info[idx + j + 1] = self.info[idx + j];
                }

                self.info[j] = t;
            }
        }
    }

    pub fn set_cluster(info: &mut GlyphInfo, cluster: u32, mask: Mask) {
        if info.cluster != cluster {
            if mask & glyph_flag::UNSAFE_TO_BREAK != 0 {
                info.mask |= glyph_flag::UNSAFE_TO_BREAK;
            } else {
                info.mask &= !glyph_flag::UNSAFE_TO_BREAK;
            }
        }

        info.cluster = cluster;
    }

    fn _unsafe_to_break_find_min_cluster(info: &[GlyphInfo], start: usize, end: usize, mut cluster: u32) -> u32 {
        for glyph_info in &info[start..end] {
            cluster = core::cmp::min(cluster, glyph_info.cluster);
        }

        cluster
    }

    fn _unsafe_to_break_set_mask(info: &mut [GlyphInfo], start: usize, end: usize, cluster: u32) -> bool {
        let mut unsafe_to_break = false;
        for glyph_info in &mut info[start..end] {
            if glyph_info.cluster != cluster {
                unsafe_to_break = true;
                glyph_info.mask |= glyph_flag::UNSAFE_TO_BREAK;
            }
        }

        unsafe_to_break
    }

    /// Checks that buffer contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push_str(&mut self, text: &str) {
        self.ensure(self.len + text.chars().count());

        for (i, c) in text.char_indices() {
            self.add(c as u32, i as u32);
        }
    }

    pub fn next_cluster(&self, mut start: usize) -> usize {
        if start >= self.len {
            return start;
        }

        let cluster = self.info[start].cluster;
        start += 1;
        while start < self.len && cluster == self.info[start].cluster {
            start += 1;
        }

        start
    }

    pub fn next_syllable(&self, mut start: usize) -> usize {
        if start >= self.len {
            return start;
        }

        let syllable = self.info[start].syllable();
        start += 1;
        while start < self.len && syllable == self.info[start].syllable() {
            start += 1;
        }

        start
    }

    pub fn next_grapheme(&self, mut start: usize) -> usize {
        if start >= self.len {
            return start;
        }

        start += 1;
        while start < self.len && self.info[start].is_continuation() {
            start += 1;
        }

        start
    }

    #[inline]
    pub fn allocate_lig_id(&mut self) -> u8 {
        let mut lig_id = self.next_serial() & 0x07;
        if lig_id == 0 {
            // In case of overflow.
            lig_id = self.next_serial() & 0x07;
        }
        lig_id as u8
    }
}

// TODO: to iter if possible

macro_rules! foreach_cluster {
    ($buffer:expr, $start:ident, $end:ident, $($body:tt)*) => {{
        let mut $start = 0;
        let mut $end = $buffer.next_cluster(0);
        while $start < $buffer.len {
            $($body)*;
            $start = $end;
            $end = $buffer.next_cluster($start);
        }
    }};
}

macro_rules! foreach_syllable {
    ($buffer:expr, $start:ident, $end:ident, $($body:tt)*) => {{
        let mut $start = 0;
        let mut $end = $buffer.next_syllable(0);
        while $start < $buffer.len {
            $($body)*;
            $start = $end;
            $end = $buffer.next_syllable($start);
        }
    }};
}

macro_rules! foreach_grapheme {
    ($buffer:expr, $start:ident, $end:ident, $($body:tt)*) => {{
        let mut $start = 0;
        let mut $end = $buffer.next_grapheme(0);
        while $start < $buffer.len {
            $($body)*;
            $start = $end;
            $end = $buffer.next_grapheme($start);
        }
    }};
}


bitflags::bitflags! {
    #[derive(Default)]
    pub struct UnicodeProps: u16 {
        const GENERAL_CATEGORY  = 0x001F;
        const IGNORABLE         = 0x0020;
        // MONGOLIAN FREE VARIATION SELECTOR 1..3, or TAG characters
        const HIDDEN            = 0x0040;
        const CONTINUATION      = 0x0080;

        // If GEN_CAT=FORMAT, top byte masks:
        const CF_ZWJ            = 0x0100;
        const CF_ZWNJ           = 0x0200;
    }
}


bitflags::bitflags! {
    #[derive(Default)]
    pub struct GlyphPropsFlags: u16 {
        // The following three match LookupFlags::Ignore* numbers.
        const BASE_GLYPH    = 0x02;
        const LIGATURE      = 0x04;
        const MARK          = 0x08;
        const CLASS_MASK    = Self::BASE_GLYPH.bits | Self::LIGATURE.bits | Self::MARK.bits;

        // The following are used internally; not derived from GDEF.
        const SUBSTITUTED   = 0x10;
        const LIGATED       = 0x20;
        const MULTIPLIED    = 0x40;

        const PRESERVE      = Self::SUBSTITUTED.bits | Self::LIGATED.bits | Self::MULTIPLIED.bits;
    }
}


bitflags::bitflags! {
    #[derive(Default)]
    pub struct BufferFlags: u32 {
        const BEGINNING_OF_TEXT             = 1 << 1;
        const END_OF_TEXT                   = 1 << 2;
        const PRESERVE_DEFAULT_IGNORABLES   = 1 << 3;
        const REMOVE_DEFAULT_IGNORABLES     = 1 << 4;
        const DO_NOT_INSERT_DOTTED_CIRCLE   = 1 << 5;
    }
}


bitflags::bitflags! {
    #[derive(Default)]
    pub struct BufferScratchFlags: u32 {
        const HAS_NON_ASCII             = 0x00000001;
        const HAS_DEFAULT_IGNORABLES    = 0x00000002;
        const HAS_SPACE_FALLBACK        = 0x00000004;
        const HAS_GPOS_ATTACHMENT       = 0x00000008;
        const HAS_UNSAFE_TO_BREAK       = 0x00000010;
        const HAS_CGJ                   = 0x00000020;

        // Reserved for complex shapers' internal use.
        const COMPLEX0                  = 0x01000000;
        const COMPLEX1                  = 0x02000000;
        const COMPLEX2                  = 0x04000000;
        const COMPLEX3                  = 0x08000000;
    }
}


bitflags::bitflags! {
    /// Flags used for serialization with a `BufferSerializer`.
    #[derive(Default)]
    pub struct SerializeFlags: u8 {
        /// Do not serialize glyph cluster.
        const NO_CLUSTERS       = 0b00000001;
        /// Do not serialize glyph position information.
        const NO_POSITIONS      = 0b00000010;
        /// Do no serialize glyph name.
        const NO_GLYPH_NAMES    = 0b00000100;
        /// Serialize glyph extents.
        const GLYPH_EXTENTS     = 0b00001000;
        /// Serialize glyph flags.
        const GLYPH_FLAGS       = 0b00010000;
        /// Do not serialize glyph advances, glyph offsets will reflect absolute
        /// glyph positions.
        const NO_ADVANCES       = 0b00100000;
    }
}


/// A buffer that contains an input string ready for shaping.
pub struct UnicodeBuffer(pub(crate) Buffer);

impl UnicodeBuffer {
    /// Create a new `UnicodeBuffer`.
    #[inline]
    pub fn new() -> UnicodeBuffer {
        UnicodeBuffer(Buffer::new())
    }

    /// Returns the length of the data of the buffer.
    ///
    /// This corresponds to the number of unicode codepoints contained in the
    /// buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len
    }

    /// Returns `true` if the buffer contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Pushes a string to a buffer.
    #[inline]
    pub fn push_str(&mut self, str: &str) {
        self.0.push_str(str);
    }

    /// Appends a character to a buffer with the given cluster value.
    #[inline]
    pub fn add(&mut self, codepoint: char, cluster: u32) {
        self.0.add(codepoint as u32, cluster);
        self.0.context_len[1] = 0;
    }

    /// Set the text direction of the `Buffer`'s contents.
    #[inline]
    pub fn set_direction(&mut self, direction: Direction) {
        self.0.direction = direction;
    }

    /// Returns the `Buffer`'s text direction.
    #[inline]
    pub fn direction(&self) -> Direction {
        self.0.direction
    }

    /// Set the script from an ISO15924 tag.
    #[inline]
    pub fn set_script(&mut self, script: Script) {
        self.0.script = Some(script);
    }

    /// Get the ISO15924 script tag.
    pub fn script(&self) -> Script {
        self.0.script.unwrap_or(script::UNKNOWN)
    }

    /// Set the buffer language.
    #[inline]
    pub fn set_language(&mut self, lang: Language) {
        self.0.language = Some(lang);
    }

    /// Get the buffer language.
    #[inline]
    pub fn language(&self) -> Option<Language> {
        self.0.language.clone()
    }

    /// Guess the segment properties (direction, language, script) for the
    /// current buffer.
    #[inline]
    pub fn guess_segment_properties(&mut self) {
        self.0.guess_segment_properties()
    }

    /// Set the cluster level of the buffer.
    #[inline]
    pub fn set_cluster_level(&mut self, cluster_level: BufferClusterLevel) {
        self.0.cluster_level = cluster_level
    }

    /// Retrieve the cluster level of the buffer.
    #[inline]
    pub fn cluster_level(&self) -> BufferClusterLevel {
        self.0.cluster_level
    }

    /// Resets clusters.
    #[inline]
    pub fn reset_clusters(&mut self) {
        self.0.reset_clusters();
    }

    /// Clear the contents of the buffer.
    #[inline]
    pub fn clear(&mut self) {
        self.0.clear()
    }
}

impl core::fmt::Debug for UnicodeBuffer {
    fn fmt(&self, fmt: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        fmt.debug_struct("UnicodeBuffer")
            .field("direction", &self.direction())
            .field("language", &self.language())
            .field("script", &self.script())
            .field("cluster_level", &self.cluster_level())
            .finish()
    }
}

impl Default for UnicodeBuffer {
    fn default() -> UnicodeBuffer {
        UnicodeBuffer::new()
    }
}


/// A buffer that contains the results of the shaping process.
pub struct GlyphBuffer(pub(crate) Buffer);

impl GlyphBuffer {
    /// Returns the length of the data of the buffer.
    ///
    /// When called before shaping this is the number of unicode codepoints
    /// contained in the buffer. When called after shaping it returns the number
    /// of glyphs stored.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len
    }

    /// Returns `true` if the buffer contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the glyph infos.
    #[inline]
    pub fn glyph_infos(&self) -> &[GlyphInfo] {
        &self.0.info[0..self.0.len]
    }

    /// Get the glyph positions.
    #[inline]
    pub fn glyph_positions(&self) -> &[GlyphPosition] {
        &self.0.pos[0..self.0.len]
    }

    /// Clears the content of the glyph buffer and returns an empty
    /// `UnicodeBuffer` reusing the existing allocation.
    #[inline]
    pub fn clear(mut self) -> UnicodeBuffer {
        self.0.clear();
        UnicodeBuffer(self.0)
    }

    /// Converts the glyph buffer content into a string.
    pub fn serialize(&self, face: &Face, flags: SerializeFlags) -> String {
        self.serialize_impl(face, flags).unwrap_or_default()
    }

    fn serialize_impl(&self, face: &Face, flags: SerializeFlags) -> Result<String, core::fmt::Error> {
        use core::fmt::Write;

        let mut s = String::with_capacity(64);

        let info = self.glyph_infos();
        let pos = self.glyph_positions();
        let mut x = 0;
        let mut y = 0;
        for (info, pos) in info.iter().zip(pos) {
            if !flags.contains(SerializeFlags::NO_GLYPH_NAMES) {
                match face.glyph_name(info.as_glyph()) {
                    Some(name) => s.push_str(name),
                    None => write!(&mut s, "gid{}", info.glyph_id)?,
                }
            } else {
                write!(&mut s, "{}", info.glyph_id)?;
            }

            if !flags.contains(SerializeFlags::NO_CLUSTERS) {
                write!(&mut s, "={}", info.cluster)?;
            }

            if !flags.contains(SerializeFlags::NO_POSITIONS) {
                if x + pos.x_offset != 0 || y + pos.y_offset != 0 {
                    write!(&mut s, "@{},{}", x + pos.x_offset, y + pos.y_offset)?;
                }

                if !flags.contains(SerializeFlags::NO_ADVANCES) {
                    write!(&mut s, "+{}", pos.x_advance)?;
                    if pos.y_advance != 0 {
                        write!(&mut s, ",{}", pos.y_advance)?;
                    }
                }
            }

            if flags.contains(SerializeFlags::GLYPH_FLAGS) {
                if info.mask & glyph_flag::DEFINED != 0 {
                    write!(&mut s, "#{:X}", info.mask & glyph_flag::DEFINED)?;
                }
            }

            if flags.contains(SerializeFlags::GLYPH_EXTENTS) {
                let extents = face.glyph_extents(info.as_glyph()).unwrap_or_default();
                write!(&mut s, "<{},{},{},{}>", extents.x_bearing, extents.y_bearing, extents.width, extents.height)?;
            }

            if flags.contains(SerializeFlags::NO_ADVANCES) {
                x += pos.x_advance;
                y += pos.y_advance;
            }

            s.push('|');
        }

        // Remove last `|`.
        if !s.is_empty() {
            s.pop();
        }

        Ok(s)
    }
}

impl core::fmt::Debug for GlyphBuffer {
    fn fmt(&self, fmt: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        fmt.debug_struct("GlyphBuffer")
            .field("glyph_positions", &self.glyph_positions())
            .field("glyph_infos", &self.glyph_infos())
            .finish()
    }
}
