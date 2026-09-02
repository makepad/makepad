//! The VJ regression in miniature: a clipped, page-sized score is followed by
//! a textured sibling in the same pass, then resized and redrawn in-process.

use makepad_test::{run_with_config, Selector, TestApp, TestConfig, WidgetSnapshot};
use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;
use std::path::{Path, PathBuf};

struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn read(path: &Path) -> Self {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let mut decoder = PngDecoder::new(ZCursor::new(&bytes));
        let pixels = decoder.decode_raw().expect("decode headless screenshot");
        let (width, height) = decoder.dimensions().expect("screenshot dimensions");
        let components = decoder
            .colorspace()
            .expect("screenshot color space")
            .num_components();
        assert!(components >= 3);
        let mut rgba = vec![0; width * height * 4];
        for index in 0..width * height {
            let source = index * components;
            rgba[index * 4..index * 4 + 3]
                .copy_from_slice(&pixels[source..source + 3]);
            rgba[index * 4 + 3] = if components == 4 { pixels[source + 3] } else { 255 };
        }
        Self { width, height, rgba }
    }

    fn pixels_in<'a>(&'a self, rect: &WidgetSnapshot) -> impl Iterator<Item = &'a [u8]> {
        let x0 = (rect.x + 2).max(0) as usize;
        let y0 = (rect.y + 2).max(0) as usize;
        let x1 = (rect.x + rect.width - 2).max(0) as usize;
        let y1 = (rect.y + rect.height - 2).max(0) as usize;
        (y0..y1.min(self.height)).flat_map(move |y| {
            (x0..x1.min(self.width)).map(move |x| {
                let offset = (y * self.width + x) * 4;
                &self.rgba[offset..offset + 4]
            })
        })
    }
}

fn assert_frame(app: &TestApp, expected_size: (i64, i64)) -> PathBuf {
    let score = app.locator(Selector::id("score_host")).wait_visible().snapshot();
    assert_eq!((score.width, score.height), expected_size);
    let sibling = app
        .locator(Selector::id("texture_host"))
        .wait_visible()
        .snapshot();
    let path = app.screenshot();
    let image = Image::read(&path);

    let green = image
        .pixels_in(&sibling)
        .filter(|pixel| pixel[1] > 220 && pixel[0] < 24 && pixel[2] < 24)
        .count();
    let sibling_pixels = (sibling.width * sibling.height) as usize;
    assert!(
        green * 2 > sibling_pixels,
        "textured sibling after ScoreView was depth-occluded: {green}/{sibling_pixels} green pixels in {}",
        path.display()
    );

    let dark_score_ink = image
        .pixels_in(&score)
        .filter(|pixel| pixel[0] < 96 && pixel[1] < 96 && pixel[2] < 96)
        .count();
    assert!(
        dark_score_ink > 100,
        "ScoreView did not exercise its glyph/vector paths (only {dark_score_ink} dark pixels) in {}",
        path.display()
    );
    path
}

#[test]
fn score_view_keeps_textured_siblings_at_two_sizes() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/embed_app");
    let mut config = TestConfig::new(
        &fixture,
        "makepad-score-view-embed-test",
        "embedded::score_view_keeps_textured_siblings_at_two_sizes",
    )
    .expect("headless fixture config");
    config
        .env
        .insert("MAKEPAD_HEADLESS_DPI".to_string(), "1".to_string());

    run_with_config(config, |app: TestApp| {
        app.locator(Selector::id("status")).wait_text("1318x181");
        let first = assert_frame(&app, (1318, 181));
        app.locator(Selector::id("resize")).click();
        app.locator(Selector::id("status")).wait_text("600x160");
        let second = assert_frame(&app, (600, 160));
        assert_ne!(first, second, "two frames should produce distinct captures");
    })
    .expect("headless ScoreView regression");
}
