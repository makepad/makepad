//! Pins headless render-to-texture: the `CachedView` in this example renders
//! its children into a child `DrawPass` and composites the resulting texture
//! back with a quad. When the headless rasterizer skips child passes, the
//! composite samples an empty texture and the whole window comes out one flat
//! colour — which is exactly what this test refuses to accept.

use makepad_test::{makepad_test, Selector, TestApp};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

/// Solid colours the four quadrants are painted with, in draw order.
const QUADRANTS: [(&str, [u8; 3]); 4] = [
    ("top_left/red", [0xff, 0x00, 0x00]),
    ("top_right/green", [0x00, 0xcc, 0x00]),
    ("bottom_left/blue", [0x00, 0x00, 0xff]),
    ("bottom_right/yellow", [0xff, 0xff, 0x00]),
];

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
        // Normalize to RGBA so the samplers below can index by 4.
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

    /// Pixels matching `color` within `tolerance`: count plus the centre of
    /// their bounding box, in 0..1 image-relative coordinates.
    fn find(&self, color: [u8; 3], tolerance: i32) -> (usize, f64, f64) {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let mut count = 0usize;
        for y in 0..self.height {
            for x in 0..self.width {
                let p = (y * self.width + x) * 4;
                let close = (0..3).all(|c| {
                    (self.rgba[p + c] as i32 - color[c] as i32).abs() <= tolerance
                });
                if close {
                    count += 1;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        if count == 0 {
            return (0, -1.0, -1.0);
        }
        (
            count,
            (min_x + max_x) as f64 * 0.5 / self.width as f64,
            (min_y + max_y) as f64 * 0.5 / self.height as f64,
        )
    }

    fn distinct_colors(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        for p in self.rgba.chunks_exact(4) {
            seen.insert([p[0] >> 3, p[1] >> 3, p[2] >> 3]);
        }
        seen.len()
    }
}

#[makepad_test]
fn child_pass_composites_into_the_window(app: TestApp) {
    let pane = app
        .locator(Selector::id("cached_pane"))
        .wait_visible()
        .snapshot();
    assert!(
        pane.width > 0 && pane.height > 0,
        "cached_pane has no rect: {pane:?}"
    );

    let path = app.screenshot();
    println!("[render_to_texture] grab: {}", path.display());
    let image = Image::read(&path);

    // A skipped child pass leaves the composite quad sampling an empty
    // texture, so the grab is one flat colour end to end.
    assert!(
        image.distinct_colors() > 1,
        "grab {} is a single flat colour — the child pass never rendered",
        path.display()
    );

    // Every quadrant must be present, and land in its own corner: that pins
    // both that the child pass ran and that the render target's U/V axes match
    // the GPU backends (top-left origin, no V flip).
    let min_share = (image.width * image.height) / 10;
    for (index, (name, color)) in QUADRANTS.iter().enumerate() {
        let (count, cx, cy) = image.find(*color, 8);
        assert!(
            count >= min_share,
            "quadrant {name} covers {count} px of {} — expected at least {min_share}; \
             child-pass composite is missing or wrong",
            image.width * image.height
        );
        let want_right = index % 2 == 1;
        let want_bottom = index >= 2;
        assert_eq!(
            cx > 0.5,
            want_right,
            "quadrant {name} centred at x={cx:.3} — horizontal flip in the render target"
        );
        assert_eq!(
            cy > 0.5,
            want_bottom,
            "quadrant {name} centred at y={cy:.3} — vertical flip in the render target"
        );
    }
}
