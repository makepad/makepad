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
/// width (`MAKEPAD_TEST_DPI=1` makes points equal screenshot pixels):
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

/// Waits until the `status` label reports `phase` at frame `min_frame` or later.
/// The frame counter counts draws: any redraw (a font answer arriving, a resize)
/// adds to it, so the exact count is not a stable thing to wait for.
fn wait_status(app: &TestApp, phase: u32, min_frame: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        let status = app
            .widget_snapshot()
            .into_iter()
            .find(|w| w.id == "status" && w.visible)
            .and_then(|w| w.text);
        if let Some(text) = &status {
            let mut words = text.split_whitespace();
            let parsed = (words.next(), words.next(), words.next(), words.next());
            if let (Some("phase"), Some(p), Some("frame"), Some(f)) = parsed {
                if p.parse() == Ok(phase) && f.parse::<u64>().is_ok_and(|f| f >= min_frame) {
                    return;
                }
            }
        }
        last = status;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("status never reached phase {phase} frame >= {min_frame} (last seen {last:?})");
}

#[test]
fn glyphs_survive_a_texture_burst_frame() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::glyphs_survive_a_texture_burst_frame",
    )
    .unwrap();
    config.env.insert("MAKEPAD_TEST_DPI".to_string(), "1".to_string());
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.locator(Selector::id("row_3")).wait_visible();
        wait_status(&app, 0, 3);
        let rows: Vec<_> = (0..4).map(row_rect).collect();
        let before = app.screenshot();
        let ink_before: Vec<usize> = rows.iter().map(|r| ink(&before, *r)).collect();
        for (i, n) in ink_before.iter().enumerate() {
            assert!(*n > 200, "row_{i} drew no text before the stress ({n} lit pixels)");
        }

        app.locator(Selector::id("stress")).wait_visible().click();
        app.locator(Selector::id("row_b3")).wait_visible();
        wait_status(&app, 1, 8);
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

/// A run shaped once and kept (as a game HUD keeps its words) draws the same
/// ink as the same words laid out afresh: its glyphs are looked up again when
/// it is drawn, so ones that were still on their way from the HAL font service
/// when it was shaped are not left blank ('STRIKE' drew as 'STR  E' on Metal).
#[test]
fn a_kept_run_draws_every_glyph() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::a_kept_run_draws_every_glyph",
    )
    .unwrap();
    config.env.insert("MAKEPAD_GPUSIM_DPI".to_string(), "1".to_string());
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.locator(Selector::id("kept")).wait_visible();
        wait_status(&app, 0, 3);
        // `kept` sits under the four rows (the bold rows are hidden): its top
        // half draws the kept runs, its bottom half the same words afresh.
        let shot = app.screenshot();
        let bytes = std::fs::read(&shot).expect("screenshot bytes");
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        decoder.decode_headers().expect("png headers");
        let (width, _) = decoder.dimensions().expect("png dimensions");
        let dpi = (width as f64 / WIDTH as f64).round().max(1.0) as usize;
        let (x, y, w) = (0, 4 * ROW_H * dpi, WIDTH * dpi);
        let kept_ink = ink(&shot, (x, y, w, 58 * dpi));
        let fresh_ink = ink(&shot, (x, y + 60 * dpi, w, 58 * dpi));
        println!("kept {kept_ink} fresh {fresh_ink} lit pixels (dpi {dpi}) {shot:?}");
        assert!(fresh_ink > 200, "the freshly laid-out words drew no text ({fresh_ink}) {shot:?}");
        assert!(
            kept_ink * 100 >= fresh_ink * 85 && kept_ink * 100 <= fresh_ink * 115,
            "the kept run drew {kept_ink} lit pixels against {fresh_ink} for the same words laid out afresh: glyphs are missing ({shot:?})"
        );
        Ok(())
    })
    .unwrap();
}

/// No frame shows a glyph blank, and a draw list that is not redrawn keeps its
/// words. `churn` animates 760 frames of glyphs it already holds (past the
/// 600-frame idle sweep, nothing asks for glyphs so nothing redraws the `still`
/// row's own list), then 240 frames of fresh glyph/size pairs into a 1024x1024 host
/// atlas (evicting and reusing slots). Every sampled frame: each churn cell has ink, and
/// the still row is pixel-identical to how it looked before the churn.
#[test]
fn no_frame_shows_a_blank_glyph_and_a_retained_list_keeps_its_words() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::no_frame_shows_a_blank_glyph_and_a_retained_list_keeps_its_words",
    )
    .unwrap();
    config.env.insert("MAKEPAD_GPUSIM_DPI".to_string(), "1".to_string());
    config.env.insert("MAKEPAD_TEXT_ATLAS_SIZE".to_string(), "1024".to_string());
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.locator(Selector::id("still")).wait_visible();
        wait_status(&app, 0, 3);
        // rows 0-3, then kept (120), still (60), churn (70)
        let still_y = 4 * ROW_H + 120;
        let churn_y = still_y + 60;
        let pixels = |png: &Path| -> (usize, Vec<u8>) {
            let bytes = std::fs::read(png).expect("screenshot bytes");
            let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
            let pixels = match decoder.decode().expect("png decode") {
                DecodingResult::U8(p) => p,
                _ => panic!("8-bit png"),
            };
            let (width, height) = decoder.dimensions().expect("png dimensions");
            (pixels.len() / (width * height), pixels)
        };
        let region = |png: &Path, y0: usize, h: usize| -> Vec<u8> {
            let (channels, px) = pixels(png);
            px[y0 * WIDTH * channels..(y0 + h) * WIDTH * channels].to_vec()
        };
        let reference = app.screenshot();
        let still_before = region(&reference, still_y, 60);
        let still_ink = ink(&reference, (0, still_y, WIDTH, 60));
        assert!(still_ink > 200, "the still row drew no text ({still_ink})");

        app.locator(Selector::id("churn")).wait_visible().click();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let (mut sampled, mut blank_frames, mut still_changed) = (0, 0, 0);
        let blank_presented: u64;
        // No screenshots during the quiet frames: a screenshot asks for a full redraw,
        // which would redraw the still row. From the fresh frames on, every sample is one.
        loop {
            assert!(std::time::Instant::now() < deadline, "the churn did not finish");
            let status = app
                .widget_snapshot()
                .into_iter()
                .find(|w| w.id == "churn_status")
                .and_then(|w| w.text)
                .unwrap_or_default();
            if status == "churn fresh" {
                let shot = app.screenshot();
                sampled += 1;
                let cells: Vec<usize> = (0..8)
                    .map(|k| ink(&shot, (8 + k * 64, churn_y, 56, 66)))
                    .collect();
                if cells.iter().any(|n| *n == 0) {
                    blank_frames += 1;
                    println!("BLANK CELL at {status}: {cells:?} {shot:?}");
                }
                // a glyph gone shows as many pixels changed; the GPU's last bit is not one
                let now = region(&shot, still_y, 60);
                let changed = now.iter().zip(&still_before).filter(|(a, b)| a.abs_diff(**b) > 48).count();
                if changed > 8 {
                    still_changed += 1;
                    println!("STILL ROW CHANGED at {status}: ink {} vs {still_ink} {shot:?}", ink(&shot, (0, still_y, WIDTH, 60)));
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Some(n) = status.strip_prefix("churn done blank_frames=") {
                blank_presented = n.parse().expect("count");
                break;
            }
        }
        println!("sampled {sampled} churn frames: {blank_frames} with a blank cell, {still_changed} with the still row changed; the app presented {blank_presented} frames with a blank glyph");
        assert_eq!(blank_presented, 0, "the app presented frames with a glyph drawn blank");
        assert!(sampled >= 10, "too few churn frames sampled ({sampled})");
        assert_eq!(blank_frames, 0, "frames showed a blank glyph");
        assert_eq!(still_changed, 0, "the retained still row changed");
        Ok(())
    })
    .unwrap();
}

/// A draw list recorded once and painted from its kept instances ever after (never redrawn,
/// not even on a full redraw, as a render target's passes are) keeps its words while the churn
/// fills a small curve store with fresh glyphs in three faces: its glyphs stay in use and are
/// touched at every sweep, so the store takes back unused glyphs first. Before (149, the Relax
/// deck title): they were let go as unused, their curves given to another glyph ("Re ax").
#[test]
fn a_kept_draw_list_keeps_its_words() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "ui::a_kept_draw_list_keeps_its_words",
    )
    .unwrap();
    config.env.insert("MAKEPAD_GPUSIM_DPI".to_string(), "1".to_string());
    // a 24-row curve store (and a 384 atlas): the churn's glyphs overflow it
    let atlas = std::env::var("KEPT_LIST_ATLAS").unwrap_or_else(|_| "384".into());
    config.env.insert("MAKEPAD_TEXT_ATLAS_SIZE".to_string(), atlas);
    run_with_config(config, |app: TestApp| -> Result<(), TestError> {
        app.locator(Selector::id("still")).wait_visible();
        wait_status(&app, 0, 3);
        // rows 0-3, kept (120), still (60), churn (70), then the kept list (60)
        let list_y = 4 * ROW_H + 120 + 60 + 70;
        let rows = |png: &Path| -> Vec<u8> {
            let bytes = std::fs::read(png).expect("screenshot bytes");
            let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
            let pixels = match decoder.decode().expect("png decode") {
                DecodingResult::U8(p) => p,
                _ => panic!("8-bit png"),
            };
            let (width, height) = decoder.dimensions().expect("png dimensions");
            let channels = pixels.len() / (width * height);
            pixels[list_y * WIDTH * channels..(list_y + 60) * WIDTH * channels].to_vec()
        };
        let reference = app.screenshot();
        let before = rows(&reference);
        let list_ink = ink(&reference, (0, list_y, WIDTH, 60));
        assert!(list_ink > 200, "the kept list drew no text ({list_ink}) {reference:?}");
        app.locator(Selector::id("churn")).wait_visible().click();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let (mut sampled, mut changed) = (0, 0);
        loop {
            assert!(std::time::Instant::now() < deadline, "the churn did not finish");
            let status = app
                .widget_snapshot()
                .into_iter()
                .find(|w| w.id == "churn_status")
                .and_then(|w| w.text)
                .unwrap_or_default();
            if status == "churn fresh" || status.starts_with("churn done") {
                let shot = app.screenshot();
                sampled += 1;
                let now = rows(&shot);
                let diff = now.iter().zip(&before).filter(|(a, b)| a.abs_diff(**b) > 48).count();
                if diff > 8 {
                    changed += 1;
                    println!("KEPT LIST CHANGED at {status}: {diff} values, ink {} vs {list_ink} {shot:?}", ink(&shot, (0, list_y, WIDTH, 60)));
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if status.starts_with("churn done") {
                break;
            }
        }
        println!("sampled {sampled} frames: the kept list changed in {changed}");
        assert!(sampled >= 5, "too few frames sampled ({sampled})");
        assert_eq!(changed, 0, "the kept draw list lost or changed its words");
        Ok(())
    })
    .unwrap();
}
