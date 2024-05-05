use crate::Face;
use crate::buffer::{Buffer, BufferScratchFlags, GlyphInfo};
use crate::complex::MAX_COMBINING_MARKS;
use crate::plan::ShapePlan;
use crate::unicode::{CharExt, GeneralCategory};

// HIGHLEVEL DESIGN:
//
// This file exports one main function: normalize().
//
// This function closely reflects the Unicode Normalization Algorithm,
// yet it's different.
//
// Each shaper specifies whether it prefers decomposed (NFD) or composed (NFC).
// The logic however tries to use whatever the font can support.
//
// In general what happens is that: each grapheme is decomposed in a chain
// of 1:2 decompositions, marks reordered, and then recomposed if desired,
// so far it's like Unicode Normalization.  However, the decomposition and
// recomposition only happens if the font supports the resulting characters.
//
// The goals are:
//
//   - Try to render all canonically equivalent strings similarly.  To really
//     achieve this we have to always do the full decomposition and then
//     selectively recompose from there.  It's kinda too expensive though, so
//     we skip some cases.  For example, if composed is desired, we simply
//     don't touch 1-character clusters that are supported by the font, even
//     though their NFC may be different.
//
//   - When a font has a precomposed character for a sequence but the 'ccmp'
//     feature in the font is not adequate, use the precomposed character
//     which typically has better mark positioning.
//
//   - When a font does not support a combining mark, but supports it precomposed
//     with previous base, use that.  This needs the itemizer to have this
//     knowledge too.  We need to provide assistance to the itemizer.
//
//   - When a font does not support a character but supports its canonical
//     decomposition, well, use the decomposition.
//
//   - The complex shapers can customize the compose and decompose functions to
//     offload some of their requirements to the normalizer.  For example, the
//     Indic shaper may want to disallow recomposing of two matras.

pub struct ShapeNormalizeContext<'a> {
    pub plan: &'a ShapePlan,
    pub buffer: &'a mut Buffer,
    pub face: &'a Face<'a>,
    pub decompose: fn(&ShapeNormalizeContext, char) -> Option<(char, char)>,
    pub compose: fn(&ShapeNormalizeContext, char, char) -> Option<char>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeNormalizationMode {
    #[allow(dead_code)]
    Decomposed,
    /// Never composes base-to-base.
    ComposedDiacritics,
    /// Always fully decomposes and then recompose back.
    ComposedDiacriticsNoShortCircuit,
    Auto,
}

impl Default for ShapeNormalizationMode {
    fn default() -> Self {
        Self::Auto
    }
}

pub fn normalize(plan: &ShapePlan, face: &Face, buffer: &mut Buffer) {
    if buffer.is_empty() {
        return;
    }

    let mut mode = plan.shaper.normalization_mode;
    if mode == Some(ShapeNormalizationMode::Auto) {
        // https://github.com/harfbuzz/harfbuzz/issues/653#issuecomment-423905920
        // if plan.has_gpos_mark() {
        //     mode = ShapeNormalizationMode::Decomposed;
        // }
        mode = Some(ShapeNormalizationMode::ComposedDiacritics);
    }

    let decompose = plan.shaper.decompose.unwrap_or(|_, ab| crate::unicode::decompose(ab));
    let compose = plan.shaper.compose.unwrap_or(|_, a, b| crate::unicode::compose(a, b));
    let mut ctx = ShapeNormalizeContext { plan, buffer, face, decompose, compose };
    let mut buffer = &mut ctx.buffer;

    let always_short_circuit = mode.is_none();
    let might_short_circuit = always_short_circuit || !matches!(
        mode,
        Some(ShapeNormalizationMode::Decomposed) |
        Some(ShapeNormalizationMode::ComposedDiacriticsNoShortCircuit)
    );

    // We do a fairly straightforward yet custom normalization process in three
    // separate rounds: decompose, reorder, recompose (if desired).  Currently
    // this makes two buffer swaps.  We can make it faster by moving the last
    // two rounds into the inner loop for the first round, but it's more readable
    // this way.

    // First round, decompose
    let mut all_simple = true;
    {
        let count = buffer.len;
        buffer.idx = 0;
        buffer.clear_output();
        loop {
            let mut end = buffer.idx + 1;
            while end < count && !buffer.info[end].is_unicode_mark() {
                end += 1;
            }

            if end < count {
                // Leave one base for the marks to cluster with.
                end -= 1;
            }

            // From idx to end are simple clusters.
            if might_short_circuit {
                let len = end - buffer.idx;
                let mut done = 0;
                while done < len {
                    let cur = buffer.cur_mut(done);
                    cur.set_glyph_index(match face.glyph_index(cur.glyph_id) {
                        Some(glyph_id) => u32::from(glyph_id.0),
                        None => break,
                    });
                    done += 1;
                }
                buffer.next_glyphs(done);
            }

            while buffer.idx < end && buffer.successful {
                decompose_current_character(&mut ctx, might_short_circuit);
                buffer = &mut ctx.buffer;
            }

            if buffer.idx == count || !buffer.successful {
                break;
            }

            all_simple = false;

            // Find all the marks now.
            end = buffer.idx + 1;
            while end < count && buffer.info[end].is_unicode_mark() {
                end += 1;
            }

            // idx to end is one non-simple cluster.
            decompose_multi_char_cluster(&mut ctx, end, always_short_circuit);
            buffer = &mut ctx.buffer;

            if buffer.idx >= count || !buffer.successful {
                break;
            }
        }

        buffer.swap_buffers();
    }

    // Second round, reorder (inplace)
    if !all_simple {
        let count = buffer.len;
        let mut i = 0;
        while i < count {
            if buffer.info[i].modified_combining_class() == 0 {
                i += 1;
                continue;
            }

            let mut end = i + 1;
            while end < count && buffer.info[end].modified_combining_class() != 0 {
                end += 1;
            }

            // We are going to do a O(n^2).  Only do this if the sequence is short.
            if end - i <= MAX_COMBINING_MARKS {
                buffer.sort(i, end, |a, b| a.modified_combining_class() > b.modified_combining_class());

                if let Some(reorder_marks) = ctx.plan.shaper.reorder_marks {
                    reorder_marks(ctx.plan, buffer, i, end);
                }
            }

            i = end + 1;
        }
    }
    if buffer.scratch_flags.contains(BufferScratchFlags::HAS_CGJ) {
        // For all CGJ, check if it prevented any reordering at all.
        // If it did NOT, then make it skippable.
        // https://github.com/harfbuzz/harfbuzz/issues/554
        for i in 1..buffer.len.saturating_sub(1) {
            if buffer.info[i].glyph_id == 0x034F /* CGJ */ {
                let last = buffer.info[i - 1].modified_combining_class();
                let next = buffer.info[i + 1].modified_combining_class();
                if next == 0 || last <= next {
                    buffer.info[i].unhide();
                }
            }
        }
    }

    // Third round, recompose
    if !all_simple && matches!(
        mode,
        Some(ShapeNormalizationMode::ComposedDiacritics) |
        Some(ShapeNormalizationMode::ComposedDiacriticsNoShortCircuit)
    ) {
        // As noted in the comment earlier, we don't try to combine
        // ccc=0 chars with their previous Starter.

        let count = buffer.len;
        let mut starter = 0;
        buffer.clear_output();
        buffer.next_glyph();
        while buffer.idx < count && buffer.successful {
            // We don't try to compose a non-mark character with it's preceding starter.
            // This is both an optimization to avoid trying to compose every two neighboring
            // glyphs in most scripts AND a desired feature for Hangul.  Apparently Hangul
            // fonts are not designed to mix-and-match pre-composed syllables and Jamo.
            let cur = buffer.cur(0);
            if cur.is_unicode_mark() &&
                // If there's anything between the starter and this char, they should have CCC
                // smaller than this character's.
                (starter == buffer.out_len - 1
                    || buffer.prev().modified_combining_class() < cur.modified_combining_class())
            {
                let a = buffer.out_info()[starter].as_char();
                let b = cur.as_char();
                if let Some(composed) = (ctx.compose)(&ctx, a, b) {
                    if let Some(glyph_id) = face.glyph_index(u32::from(composed)) {
                        // Copy to out-buffer.
                        buffer = &mut ctx.buffer;
                        buffer.next_glyph();
                        if !buffer.successful {
                            return;
                        }

                        // Merge and remove the second composable.
                        buffer.merge_out_clusters(starter, buffer.out_len);
                        buffer.out_len -= 1;

                        // Modify starter and carry on.
                        let mut flags = buffer.scratch_flags;
                        let info = &mut buffer.out_info_mut()[starter];
                        info.glyph_id = u32::from(composed);
                        info.set_glyph_index(u32::from(glyph_id.0));
                        info.init_unicode_props(&mut flags);
                        buffer.scratch_flags = flags;

                        continue;
                    }
                }
            }

            // Blocked, or doesn't compose.
            buffer = &mut ctx.buffer;
            buffer.next_glyph();

            if buffer.prev().modified_combining_class() == 0 {
                starter = buffer.out_len - 1;
            }
        }

        buffer.swap_buffers();
    }
}

fn decompose_multi_char_cluster(ctx: &mut ShapeNormalizeContext, end: usize, short_circuit: bool) {
    let mut i = ctx.buffer.idx;
    while i < end && ctx.buffer.successful {
        if ctx.buffer.info[i].as_char().is_variation_selector() {
            handle_variation_selector_cluster(ctx, end, short_circuit);
            return;
        }
        i += 1;
    }

    while ctx.buffer.idx < end && ctx.buffer.successful {
        decompose_current_character(ctx, short_circuit);
    }
}

fn handle_variation_selector_cluster(ctx: &mut ShapeNormalizeContext, end: usize, _: bool) {
    let face = ctx.face;
    let set_glyph = |info: &mut GlyphInfo| {
        if let Some(glyph_id) = face.glyph_index(info.glyph_id) {
            info.set_glyph_index(u32::from(glyph_id.0));
        }
    };

    // TODO: Currently if there's a variation-selector we give-up, it's just too hard.
    let buffer = &mut ctx.buffer;
    while buffer.idx < end - 1 && buffer.successful {
        if buffer.cur(1).as_char().is_variation_selector() {
            if let Some(glyph_id) = face.glyph_variation_index(
                buffer.cur(0).as_char(),
                buffer.cur(1).as_char(),
            ) {
                buffer.cur_mut(0).set_glyph_index(u32::from(glyph_id.0));
                let unicode = buffer.cur(0).glyph_id;
                buffer.replace_glyphs(2, 1, &[unicode]);
            } else {
                // Just pass on the two characters separately, let GSUB do its magic.
                set_glyph(buffer.cur_mut(0));
                buffer.next_glyph();
                set_glyph(buffer.cur_mut(0));
                buffer.next_glyph();
            }

            // Skip any further variation selectors.
            while buffer.idx < end && buffer.cur(0).as_char().is_variation_selector() {
                set_glyph(buffer.cur_mut(0));
                buffer.next_glyph();
            }
        } else {
            set_glyph(buffer.cur_mut(0));
            buffer.next_glyph();
        }
    }

    if ctx.buffer.idx < end {
        set_glyph(ctx.buffer.cur_mut(0));
        ctx.buffer.next_glyph();
    }
}

fn decompose_current_character(ctx: &mut ShapeNormalizeContext, shortest: bool) {
    let u = ctx.buffer.cur(0).as_char();
    let glyph = ctx.face.glyph_index(u32::from(u));

    if !shortest || glyph.is_none() {
        if decompose(ctx, shortest, u) > 0 {
            ctx.buffer.skip_glyph();
            return;
        }
    }

    if let Some(glyph) = glyph {
        ctx.buffer.next_char(u32::from(glyph.0));
        return;
    }

    // Handle space characters.
    if ctx.buffer.cur(0).general_category() == GeneralCategory::SpaceSeparator {
        if let Some(space_type) = u.space_fallback() {
            if let Some(space_glyph) = ctx.face.glyph_index(u32::from(' ')) {
                ctx.buffer.cur_mut(0).set_space_fallback(space_type);
                ctx.buffer.next_char(u32::from(space_glyph.0));
                ctx.buffer.scratch_flags |= BufferScratchFlags::HAS_SPACE_FALLBACK;
                return;
            }
        }
    }

    // U+2011 is the only sensible character that is a no-break version of another character
    // and not a space.  The space ones are handled already.  Handle this lone one.
    if u == '\u{2011}' {
        if let Some(other_glyph) = ctx.face.glyph_index(0x2010) {
            ctx.buffer.next_char(u32::from(other_glyph.0));
            return;
        }
    }

    // Insert a .notdef glyph if decomposition failed.
    ctx.buffer.next_char(0);
}

/// Returns 0 if didn't decompose, number of resulting characters otherwise.
fn decompose(ctx: &mut ShapeNormalizeContext, shortest: bool, ab: char) -> u32 {
    let (a, b) = match (ctx.decompose)(ctx, ab) {
        Some(decomposed) => decomposed,
        _ => return 0,
    };

    let a_glyph = ctx.face.glyph_index(u32::from(a));
    let b_glyph = if b != '\0' {
        match ctx.face.glyph_index(u32::from(b)) {
            Some(glyph_id) => Some(glyph_id),
            None => return 0,
        }
    } else {
        None
    };

    if !shortest || a_glyph.is_none() {
        let ret = decompose(ctx, shortest, a);
        if ret != 0 {
            if let Some(b_glyph) = b_glyph {
                ctx.buffer.output_char(u32::from(b), u32::from(b_glyph.0));
                return ret + 1;
            }
            return ret;
        }
    }

    if let Some(a_glyph) = a_glyph {
        // Output a and b.
        ctx.buffer.output_char(u32::from(a), u32::from(a_glyph.0));
        if let Some(b_glyph) = b_glyph {
            ctx.buffer.output_char(u32::from(b), u32::from(b_glyph.0));
            return 2;
        }
        return 1;
    }

    0
}
