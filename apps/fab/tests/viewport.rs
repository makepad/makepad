//! Lane B (L2) regression: **a viewport's composite stays inside its own walk
//! rect**, and its rect is never empty.
//!
//! The bug this pins, exactly: the composite quad was drawn before any
//! `DrawList` had been begun inside the offscreen composite pass. `begin_pass`
//! clears the pass's `main_draw_list_id`, and whatever calls `begin_always`
//! first becomes it — so anything drawn before that lands in the *enclosing*
//! draw list, which belongs to the **window**. The quad was therefore painted
//! into the window at pass-local `(0,0)`, i.e. the window origin, on top of the
//! top bar and the tool column; and the composite pass, having no draw list of
//! its own, rendered nothing, so the viewport's real rect showed an empty
//! (black) composite target. One cause, both symptoms.
//!
//! The two assertions below are the two symptoms, inverted:
//!
//! 1. the top-bar band at the top of the window is still chrome — no viewport
//!    composite has been painted over it;
//! 2. every visible viewport's own rect is not one flat colour — the composite
//!    pass did render, and the blit landed there.
//!
//! Headless (`MAKEPAD=headless`) runs on the CPU rasterizer at dpi 1, so no GPU
//! is touched and screenshot pixels are layout points.

use makepad_test::{makepad_test, Selector, TestApp};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

/// How far two chrome samples may drift and still count as the same paint.
/// The composite's own background gradient alone is 20 levels wide, and the
/// lit image is far further off than that.
const CHROME_TOLERANCE: i32 = 6;

struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn read(path: &std::path::Path) -> Image {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|err| panic!("cannot read grab {}: {err}", path.display()));
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        let pixels = decoder
            .decode_raw()
            .unwrap_or_else(|err| panic!("cannot decode grab {}: {err:?}", path.display()));
        let (width, height) = decoder.dimensions().expect("grab has no dimensions");
        let components = decoder
            .colorspace()
            .expect("grab has no colorspace")
            .num_components();
        assert!(
            components >= 3,
            "grab is not a colour image ({components} components)"
        );
        let mut rgba = vec![0u8; width * height * 4];
        for i in 0..width * height {
            let src = i * components;
            rgba[i * 4] = pixels[src];
            rgba[i * 4 + 1] = pixels[src + 1];
            rgba[i * 4 + 2] = pixels[src + 2];
            rgba[i * 4 + 3] = if components == 4 { pixels[src + 3] } else { 255 };
        }
        Image {
            width,
            height,
            rgba,
        }
    }

    fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let p = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4;
        [self.rgba[p], self.rgba[p + 1], self.rgba[p + 2]]
    }

    /// The most common colour in a row span — for a chrome bar that is its
    /// background, with the text and icons outvoted.
    fn row_mode(&self, y: usize, x0: usize, x1: usize) -> [u8; 3] {
        let x1 = x1.min(self.width);
        assert!(x0 < x1, "empty row span {x0}..{x1} at y={y}");
        let mut tally: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
        for x in x0..x1 {
            *tally.entry(self.pixel(x, y)).or_default() += 1;
        }
        tally
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(c, _)| c)
            .unwrap()
    }

    /// Distinct colours (5 bits per channel) inside a rect.
    fn distinct_colors_in(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
        let mut seen = std::collections::HashSet::new();
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                let p = self.pixel(x, y);
                seen.insert([p[0] >> 3, p[1] >> 3, p[2] >> 3]);
            }
        }
        seen.len()
    }

    fn luma(&self, x: usize, y: usize) -> i32 {
        let p = self.pixel(x, y);
        (p[0] as i32 * 54 + p[1] as i32 * 183 + p[2] as i32 * 19) / 256
    }

    /// Mean |Δluma| of 8-pixel-aligned block boundaries over interior
    /// adjacent pixels in the same span. ≈1 on a smooth face; ≫1 when the
    /// composite is an un-upsampled 8×8 AO/cavity grid.
    fn blockiness_8(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
        let x1 = x1.min(self.width);
        let y1 = y1.min(self.height);
        if x1 <= x0 + 16 || y1 <= y0 + 16 {
            return 0.0;
        }
        let mut boundary = 0i64;
        let mut interior = 0i64;
        let mut n = 0i64;
        for y in y0..y1 {
            let mut x = x0 + 8;
            while x < x1 {
                boundary += (self.luma(x, y) - self.luma(x - 1, y)).abs() as i64;
                interior += (self.luma(x - 4, y) - self.luma(x - 5, y)).abs() as i64;
                n += 1;
                x += 8;
            }
        }
        for x in x0..x1 {
            let mut y = y0 + 8;
            while y < y1 {
                boundary += (self.luma(x, y) - self.luma(x, y - 1)).abs() as i64;
                interior += (self.luma(x, y - 4) - self.luma(x, y - 5)).abs() as i64;
                n += 1;
                y += 8;
            }
        }
        if n == 0 || interior == 0 {
            return 0.0;
        }
        boundary as f32 / interior as f32
    }

    /// Variance of luma in a rect. Used to pick a flat lit face.
    fn luma_stats(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> (f32, f32) {
        let x1 = x1.min(self.width);
        let y1 = y1.min(self.height);
        if x1 <= x0 || y1 <= y0 {
            return (0.0, 0.0);
        }
        let mut sum = 0i64;
        let mut n = 0i64;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += self.luma(x, y) as i64;
                n += 1;
            }
        }
        if n == 0 {
            return (0.0, 0.0);
        }
        let mean = sum as f32 / n as f32;
        let mut var = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = self.luma(x, y) as f32 - mean;
                var += d * d;
            }
        }
        (mean, var / n as f32)
    }
}

#[makepad_test]
fn viewport_composites_stay_inside_their_walk_rects(app: TestApp) {
    // `main` is the shell's root view: its rect is the window's content area,
    // in the same layout points every widget rect below uses.
    let shell = app.locator(Selector::id("main")).wait_visible().snapshot();
    assert!(
        shell.width > 0 && shell.height > 0,
        "the shell root has no rect: {shell:?}"
    );

    let viewports = Selector::id("viewport");
    let count = app.locator(viewports.clone()).wait_visible().count();
    assert!(
        count > 0,
        "no visible FabViewport in the shell — the layout changed, fix the selector"
    );
    let rects: Vec<_> = (0..count)
        .map(|i| app.locator(viewports.clone().nth(i)).snapshot())
        .collect();

    let path = app.screenshot();
    println!("[fab] grab: {}", path.display());
    let image = Image::read(&path);
    // dpi 1 headless, but derive the factor anyway so a retina visible-mode
    // run (MAKEPAD_TEST_VISIBLE=1) measures the same thing.
    let scale = image.width as f64 / shell.width as f64;
    let to_px = |v: i64| ((v as f64) * scale).round().max(0.0) as usize;

    // ---- 1. the chrome above the viewports is still chrome -----------------
    // A misplaced composite is an opaque quad at pass-local (0,0) — the window
    // origin — sized like its viewport, so it covers the top bar from x=0 out
    // to the viewport's width and stops. Chrome further right is out of its
    // reach. Comparing the two halves of the same top-bar row therefore needs
    // no colour constant at all: the bar is one paint, and if the left half
    // stops matching the right half, something is being painted over it.
    let top = rects.iter().map(|r| to_px(r.y)).min().unwrap_or(0);
    assert!(
        top >= 4,
        "the first viewport starts at y={top}px — there is no chrome band to test"
    );
    let covered = rects.iter().map(|r| to_px(r.x) + to_px(r.width)).max().unwrap_or(0);
    let untouched = covered + (image.width - covered) / 2;
    assert!(
        untouched + 8 < image.width,
        "no chrome to the right of the viewports to compare against"
    );
    for y in [2usize, 4, 6] {
        let over = image.row_mode(y, 0, (image.width / 4).max(8));
        let clear = image.row_mode(y, untouched, image.width);
        let drift = (0..3)
            .map(|c| (over[c] as i32 - clear[c] as i32).abs())
            .max()
            .unwrap();
        assert!(
            drift <= CHROME_TOLERANCE,
            "top-bar row y={y} reads {over:?} above the viewports but {clear:?} where no \
             viewport can reach — a viewport composite is being painted at the window origin \
             (grab {})",
            path.display()
        );
    }

    // ---- 2. every viewport painted something into its own rect -------------
    for (i, r) in rects.iter().enumerate() {
        let (x0, y0) = (to_px(r.x), to_px(r.y));
        let (x1, y1) = (x0 + to_px(r.width), y0 + to_px(r.height));
        assert!(
            x1 > x0 + 8 && y1 > y0 + 8,
            "viewport {i} has a degenerate rect {r:?}"
        );
        // Inset past the 1 px area border and any overlay chrome at the edge.
        let colors = distinct_inset(&image, x0, y0, x1, y1);
        assert!(
            colors > 1,
            "viewport {i} rect {r:?} is one flat colour — its composite pass \
             rendered nothing into its own target (grab {})",
            path.display()
        );
    }

    // ---- 3. Solid (left) composite is not an 8×8 AO/cavity grid ------------
    // The leftmost viewport is the realtime Solid pane. On a flat lit face
    // the 8-pixel-aligned block-boundary jump must not dwarf the interior
    // 1 px jump — that was the un-upsampled cavity/SSAO (or the 256-wide
    // element LUT sampled as if it were spatial).
    let left = rects
        .iter()
        .min_by_key(|r| r.x)
        .expect("no viewport rect");
    let (x0, y0) = (to_px(left.x), to_px(left.y));
    let (x1, y1) = (x0 + to_px(left.width), y0 + to_px(left.height));
    let inset_x = ((x1 - x0) / 6).max(12);
    let inset_y = ((y1 - y0) / 6).max(12);
    let ix0 = x0 + inset_x;
    let iy0 = y0 + inset_y;
    let ix1 = x1.saturating_sub(inset_x);
    let iy1 = y1.saturating_sub(inset_y);
    let patch = 48usize;
    let mut best: Option<(f32, usize, usize)> = None;
    if ix1 > ix0 + patch && iy1 > iy0 + patch {
        let mut y = iy0;
        while y + patch <= iy1 {
            let mut x = ix0;
            while x + patch <= ix1 {
                let (mean, var) = image.luma_stats(x, y, x + patch, y + patch);
                if mean > 28.0 && mean < 230.0 {
                    match best {
                        Some((v, _, _)) if var >= v => {}
                        _ => best = Some((var, x, y)),
                    }
                }
                x += 16;
            }
            y += 16;
        }
    }
    if let Some((var, x, y)) = best {
        let ratio = image.blockiness_8(x, y, x + patch, y + patch);
        assert!(
            ratio < 2.2,
            "solid viewport has 8×8 blockiness {ratio:.2} (patch variance {var:.1}) \
             at ({x},{y}) — cavity/SSAO is being composited without a full-res \
             (or bilateral) upsample (grab {})",
            path.display()
        );
    }
}

fn distinct_inset(image: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> usize {
    let inset = 6usize;
    image.distinct_colors_in(
        x0 + inset,
        y0 + inset,
        x1.saturating_sub(inset),
        y1.saturating_sub(inset),
    )
}
