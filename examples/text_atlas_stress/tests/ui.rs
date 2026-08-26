//! Glyphs cached before a heavy frame must still draw after it.
//!
//! Phase 0 renders four rows of text; the screenshot is the reference. Then
//! `stress` opens four rows in a second font (fresh glyph outlines, so the
//! append-only glyph atlas grows past its first rows) while `burst` creates
//! and binds 48 new textures inside every draw for eight frames. The original
//! rows must come out pixel-identical — a single glyph dropped from the
//! atlas shows as a change in that row's ink — and the new rows must carry
//! their own ink.
use makepad_test::{run_with_config, Selector, TestApp, TestConfig, TestError};
use makepad_zune_core::{bytestream::ZCursor, result::DecodingResult};
use makepad_zune_png::PngDecoder;
use std::path::Path;

/// Rows are stacked from the window's top edge, `ROW_H` points each, full
/// width (`MAKEPAD_HEADLESS_DPI=1` makes points equal screenshot pixels):
/// rows 0-3 regular, 4-7 bold. A two-point inset keeps neighbours apart.
const ROW_H: usize = 60;
const WIDTH: usize = 900;

fn row_rect(row: usize) -> (usize, usize, usize, usize) {
    (0, row * ROW_H + 2, WIDTH, ROW_H - 4)
}

/// Lit pixels (any channel > 96) inside a rect of a screenshot.
fn ink(png: &Path, rect: (usize, usize, usize, usize)) -> usize {
    let bytes = std::fs::read(png).expect("screenshot bytes");
    let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
    let pixels = match decoder.decode().expect("png decode") {
        DecodingResult::U8(p) => p,
        _ => panic!("unexpected png sample type (expected 8-bit)"),
    };
    let (width, height) = decoder.dimensions().expect("png dimensions");
    let channels = pixels.len() / (width * height);
    let (x0, y0, w, h) = rect;
    let mut count = 0;
    for y in y0..(y0 + h).min(height) {
        for x in x0..(x0 + w).min(width) {
            let p = (y * width + x) * channels;
            if pixels[p..p + 3].iter().any(|&c| c > 96) {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn glyphs_survive_a_texture_burst_frame() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::glyphs_survive_a_texture_burst_frame",
    )
    .unwrap();
    config.env.insert("MAKEPAD_HEADLESS_DPI".to_string(), "1".to_string());
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.locator(Selector::id("row_3")).wait_visible();
        app.locator(Selector::id("status")).wait_text("phase 0 frame 3");
        let rows: Vec<_> = (0..4).map(row_rect).collect();
        let before = app.screenshot();
        let ink_before: Vec<usize> = rows.iter().map(|r| ink(&before, *r)).collect();
        for (i, n) in ink_before.iter().enumerate() {
            assert!(*n > 200, "row_{i} drew no text before the stress ({n} lit pixels)");
        }

        app.locator(Selector::id("stress")).wait_visible().click();
        app.locator(Selector::id("row_b3")).wait_visible();
        app.locator(Selector::id("status")).wait_text("phase 1 frame 8");
        let after = app.screenshot();
        let ink_after: Vec<usize> = rows.iter().map(|r| ink(&after, *r)).collect();
        assert_eq!(
            ink_after, ink_before,
            "text rows changed across the stress frame (a glyph left the atlas): {before:?} vs {after:?}"
        );
        for i in 0..4 {
            let bold = ink(&after, row_rect(4 + i));
            assert!(
                bold * 10 >= ink_before[i] * 7,
                "row_b{i} lost glyphs: {bold} lit pixels against {} in row_{i} ({after:?})",
                ink_before[i]
            );
        }
        Ok(())
    })
    .unwrap();
}
