//! Kernel host components that read fonts (PDOOM-PARITY §9): `text.outline`
//! (glyph outlines as placed polylines) and `text.layout` (shaped glyph
//! records), registered into `makepad-script-compute`'s host registry.
//!
//! Fonts are resolved at load, never at call time: the host reads a font
//! through the document's resolver and [`register_font`]s it, and the
//! document passes the returned id to the kernel as a parameter. The text
//! is a slice of Unicode scalar values (a host-filled input buffer,
//! [`codepoints`]). Calls are pure (shaping and flattening are IEEE
//! arithmetic on the font's tables: tier X), write only their output slice,
//! report overflow as a result of -1 (nothing written past the slice), and
//! are charged per input: a call costs at most [`outline_cost`] /
//! [`layout_cost`] op-equivalents for its slice lengths.
//!
//! ```text
//! let cps = input(i32)                     // the text, one scalar per word
//! let out = output(f32, 1, 0, out)         // the contour stream
//! fn element(i) {
//!     let words = text.outline(cps, 0, 12, out, 0, 8192, font, 96.0, 0.25)
//! }
//! ```
//!
//! `text.outline(text, off, len, out, off, len, font, size, tolerance)`
//! writes contours, each `[points, glyph, x0, y0, x1, y1, ...]` (the
//! glyph's index in the shaped text), y up, baseline 0, and returns the
//! words written. `text.layout(text, off, len, out, off, len, font, size,
//! tracking)` writes one [`GLYPH_RECORD`]-word `TextGlyph` record per
//! glyph and returns the glyph count.

use crate::typo::font::OutlineFont;
use crate::typo::layout::{layout, TextStyle};
use crate::typo::outline::outlines;
use makepad_script_compute::host::{self, HostError, HostFn, HostSlice, SliceSig, Tier};
use makepad_script_compute::ir::Ty;
use makepad_script_compute::kernel::{FieldTy, Layout, LayoutField};
use std::sync::RwLock;

/// Most text scalars one call reads.
pub const MAX_TEXT: u32 = 4096;
/// Most words one call writes.
pub const MAX_OUT: u32 = 1 << 22;
/// Words per `text.layout` record: x, y, advance, ink (x0, y0, x1, y1),
/// char index, word (-1 for whitespace).
pub const GLYPH_RECORD: usize = 9;

static FONTS: RwLock<Vec<OutlineFont>> = RwLock::new(Vec::new());

/// Makes `font` callable from kernels; returns its id (the same font
/// registered twice gets its first id).
pub fn register_font(font: OutlineFont) -> u32 {
    let mut w = FONTS.write().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = w.iter().position(|f| *f == font) {
        return k as u32;
    }
    w.push(font);
    (w.len() - 1) as u32
}

fn font(id: u32) -> Result<OutlineFont, HostError> {
    FONTS.read().unwrap_or_else(|e| e.into_inner()).get(id as usize).cloned().ok_or_else(|| HostError(format!("font {id} is not registered")))
}

/// A text as the slice of scalars the host components read.
pub fn codepoints(text: &str) -> Vec<u32> {
    text.chars().map(|c| c as u32).collect()
}

fn text_of(s: &HostSlice) -> String {
    (0..s.len()).filter_map(|i| char::from_u32(s.get(i))).collect()
}

/// Shaping and outline decoding per scalar (with its glyph's curves), plus
/// one op per output word and a fixed overhead.
pub fn outline_cost(lens: &[u32]) -> u64 {
    let (n, m) = (lens.first().copied().unwrap_or(0) as u64, lens.get(1).copied().unwrap_or(0) as u64);
    6_000 * n + 2 * m + 2_000
}

pub fn layout_cost(lens: &[u32]) -> u64 {
    let (n, m) = (lens.first().copied().unwrap_or(0) as u64, lens.get(1).copied().unwrap_or(0) as u64);
    2_500 * n + m + 2_000
}

fn f32_arg(args: &[u32], k: usize) -> f32 {
    f32::from_bits(args[k])
}

fn call_outline(args: &[u32], s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    let font = font(args[0])?;
    let (size, tol) = (f32_arg(args, 1), f32_arg(args, 2));
    if !(size.is_finite() && size > 0.0 && tol.is_finite() && tol > 0.0) {
        return Err(HostError("text.outline: size and tolerance must be positive".into()));
    }
    // A tolerance under 1/1000 of the size buys nothing but points.
    let tol = tol.max(size * 1e-3);
    let text = text_of(&s[0]);
    let l = layout(&font, &text, &TextStyle { size, ..TextStyle::default() });
    let contours = outlines(&font, &l, tol);
    let need: usize = contours.iter().map(|c| 2 + 2 * c.points.len()).sum();
    let out = &s[1];
    if need > out.len() {
        rets[0] = (-1i32) as u32;
        return Ok(());
    }
    let mut at = 0;
    for c in &contours {
        out.set_f32(at, c.points.len() as f32);
        out.set_f32(at + 1, c.glyph as f32);
        at += 2;
        for p in &c.points {
            out.set_f32(at, p[0]);
            out.set_f32(at + 1, p[1]);
            at += 2;
        }
    }
    rets[0] = at as u32;
    Ok(())
}

fn call_layout(args: &[u32], s: &[HostSlice], rets: &mut [u32]) -> Result<(), HostError> {
    let font = font(args[0])?;
    let (size, tracking) = (f32_arg(args, 1), f32_arg(args, 2));
    if !(size.is_finite() && size > 0.0 && tracking.is_finite()) {
        return Err(HostError("text.layout: size must be positive and tracking finite".into()));
    }
    let text = text_of(&s[0]);
    let l = layout(&font, &text, &TextStyle { size, tracking, ..TextStyle::default() });
    let out = &s[1];
    if l.glyphs.len() * GLYPH_RECORD > out.len() {
        rets[0] = (-1i32) as u32;
        return Ok(());
    }
    for (i, g) in l.glyphs.iter().enumerate() {
        let r = [g.x, g.y, g.advance, g.ink[0], g.ink[1], g.ink[2], g.ink[3], g.char_index as f32, g.word.map_or(-1.0, |w| w as f32)];
        for (k, v) in r.iter().enumerate() {
            out.set_f32(i * GLYPH_RECORD + k, *v);
        }
    }
    rets[0] = l.glyphs.len() as u32;
    Ok(())
}

const SLICES: &[SliceSig] = &[SliceSig { max_words: MAX_TEXT, writable: false }, SliceSig { max_words: MAX_OUT, writable: true }];

/// Registers `text.outline` and `text.layout` (idempotent); call at
/// start-up, before kernels that call them compile.
pub fn register_host_components() -> Result<(), HostError> {
    host::register(HostFn {
        name: "text.outline",
        params: &[Ty::I32, Ty::F32, Ty::F32],
        slices: SLICES,
        rets: &[Ty::I32],
        cost: outline_cost,
        misses: 64,
        tier: Tier::X,
        call: call_outline,
        doc: "text.outline(text, off, len, out, off, len, font, size, tolerance) -> words: the glyph outlines of a text (Unicode scalars) as contours [points, glyph, x, y, ...] in size units, y up; -1 when `out` is too small",
    })?;
    host::register(HostFn {
        name: "text.layout",
        params: &[Ty::I32, Ty::F32, Ty::F32],
        slices: SLICES,
        rets: &[Ty::I32],
        cost: layout_cost,
        misses: 32,
        tier: Tier::X,
        call: call_layout,
        doc: "text.layout(text, off, len, out, off, len, font, size, tracking) -> glyphs: shaped, kerned TextGlyph records (x, y, advance, ink x0 y0 x1 y1, char, word); -1 when `out` is too small",
    })?;
    Ok(())
}

/// The `TextGlyph` record layout, for kernels that read `text.layout`'s
/// output by field.
pub fn layouts() -> Vec<Layout> {
    let f = |name: &str, ty, offset| LayoutField { name: name.into(), ty, offset };
    vec![Layout {
        name: "TextGlyph".into(),
        stride: GLYPH_RECORD as u32,
        fields: vec![f("pos", FieldTy::Vec2, 0), f("advance", FieldTy::F32, 2), f("ink", FieldTy::Vec4, 3), f("char", FieldTy::F32, 7), f("word", FieldTy::F32, 8)],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typo::font::test_font;
    use makepad_script_compute::kernel::compile_with;
    use makepad_script_compute::sched::{InlineExecutor, Job};
    use makepad_script_compute::Backend;

    #[test]
    fn kernels_read_outlines_and_layout() {
        register_host_components().unwrap();
        let id = register_font(test_font("IBMPlexSans-Text.ttf"));
        assert_eq!(register_font(test_font("IBMPlexSans-Text.ttf")), id + 1, "a second read of the bytes is another font");
        let text = codepoints("Oi");
        let src = format!(
            "let cps = input(i32)\nlet out = output(f32, 1, 0, out)\nlet glyphs = output(f32, 1, 0, glyphs)\nlet r = output(i32)\n\
             fn element(i) {{ if i == 0 {{ r[i] = text.outline(cps, 0, 2, out, 0, 4096, {id}, 100.0, 0.25) }} else {{ if i == 1 {{ r[i] = text.layout(cps, 0, 2, glyphs, 0, 18, {id}, 100.0, 0.0) }} else {{ r[i] = text.outline(cps, 0, 2, out, 0, 8, {id}, 100.0, 0.25) }} }} }}"
        );
        for backend in [Backend::Native, Backend::Interp] {
            let k = compile_with(&src, &[], backend).unwrap_or_else(|e| panic!("{e:?}"));
            let mut j = Job::new(k, 3);
            j.input("cps", text.iter().map(|&c| f32::from_bits(c)).collect::<Vec<_>>().into()).unwrap();
            j.output_u32("out", vec![0; 4096]).unwrap();
            j.output_u32("glyphs", vec![0; 18]).unwrap();
            j.output_u32("r", vec![0; 3]).unwrap();
            let st = j.run(&InlineExecutor, 1).unwrap().clone();
            assert!(!st.host_error);
            let r = j.out_u32("r").unwrap().to_vec();
            let words = r[0] as usize;
            assert!(words > 40, "{words} words of contours");
            assert_eq!(r[1], 2, "two glyphs");
            assert_eq!(r[2] as i32, -1, "an 8-word output overflows and says so");
            let out: Vec<f32> = j.out_u32("out").unwrap().iter().map(|w| f32::from_bits(*w)).collect();
            // The stream walks: [n, glyph, pts...] per contour; O has 2, i has 2.
            let (mut at, mut contours) = (0, Vec::new());
            while at < words {
                let n = out[at] as usize;
                contours.push(out[at + 1] as usize);
                at += 2 + 2 * n;
            }
            assert_eq!(at, words);
            assert_eq!(contours, vec![0, 0, 1, 1]);
            let g: Vec<f32> = j.out_u32("glyphs").unwrap().iter().map(|w| f32::from_bits(*w)).collect();
            assert_eq!((g[0], g[7], g[16]), (0.0, 0.0, 1.0));
            assert!(g[9] > 50.0, "i starts after O's advance: {}", g[9]);
        }
    }

    #[test]
    fn costs_grow_with_the_input_and_bad_fonts_fail() {
        assert!(outline_cost(&[100, 4096]) > outline_cost(&[10, 4096]));
        assert!(layout_cost(&[10, 90]) < outline_cost(&[10, 90]));
        register_host_components().unwrap();
        let src = "let cps = input(i32)\nlet out = output(f32, 1, 0, out)\nlet r = output(i32)\nfn element(i) { r[i] = text.outline(cps, 0, 1, out, 0, 64, 999999, 10.0, 0.5) }";
        let k = compile_with(src, &[], Backend::Interp).unwrap();
        let mut j = Job::new(k, 1);
        j.input("cps", vec![f32::from_bits('A' as u32)].into()).unwrap();
        j.output_u32("out", vec![0; 64]).unwrap();
        j.output_u32("r", vec![7]).unwrap();
        let st = j.run(&InlineExecutor, 1).unwrap().clone();
        assert!(st.host_error, "an unknown font sets the error word");
        assert_eq!(j.out_u32("r").unwrap()[0], 0);
    }
}
