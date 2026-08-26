//! Ink-centre of boxed chrome labels (workspace tabs, header menus).
//!
//! Same contract as `examples/text_center/tests/ui.rs`: the cap-height band of
//! a one-line label sits within 0.5 logical px of the widget box centre.
//! Specimens are live product controls (`Layout` workspace tab, `View` header
//! menu), measured from a headless grab — dpi 1, no GPU.

use makepad_test::{makepad_test, Selector, TestApp, WidgetSnapshot};
use std::path::Path;

const TOLERANCE_LPX: f64 = 0.5;

struct Frame {
    width: usize,
    height: usize,
    lum: Vec<f64>,
}

impl Frame {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.lum[y * self.width + x]
    }

    fn decode(path: &Path) -> Frame {
        use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
        use makepad_zune_png::PngDecoder;

        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        let mut dec = PngDecoder::new(ZCursor::new(&bytes));
        dec.decode_headers().expect("png headers");
        let (width, height) = dec.dimensions().expect("png dimensions");
        let components = dec.colorspace().expect("png colorspace").num_components();
        let decoded = dec.decode().expect("png decode");
        let src = decoded.u8().expect("png 8-bit");
        let lum = (0..width * height)
            .map(|i| src[i * components] as f64)
            .collect();
        Frame {
            width,
            height,
            lum,
        }
    }
}

fn band_edges(
    frame: &Frame,
    x: usize,
    y0: usize,
    y1: usize,
    inside: f64,
    outside: f64,
) -> (f64, f64) {
    let span = inside - outside;
    assert!(span.abs() > 8.0, "no contrast between band and surround");
    let cov = |y: usize| ((frame.at(x, y) - outside) / span).clamp(0.0, 1.0);
    let mid = (y0..=y1)
        .max_by(|a, b| cov(*a).partial_cmp(&cov(*b)).unwrap())
        .expect("non-empty window");
    assert!(cov(mid) > 0.5, "no band on column {x} between {y0} and {y1}");
    let mut top = mid as f64;
    for y in y0..mid {
        top -= cov(y);
    }
    let mut bottom = mid as f64;
    for y in mid..=y1 {
        bottom += cov(y);
    }
    (top, bottom)
}

fn fill_value(frame: &Frame, r: &DevRect) -> f64 {
    let mut hist = [0usize; 256];
    let y0 = (r.y0 + 2).min(r.y1);
    let y1 = r.y1.saturating_sub(2).max(y0);
    let x0 = (r.x0 + 2).min(r.x1);
    let x1 = r.x1.saturating_sub(2).max(x0);
    for y in y0..y1 {
        for x in x0..x1 {
            hist[frame.at(x, y) as usize] += 1;
        }
    }
    hist.iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(v, _)| v as f64)
        .unwrap_or(0.0)
}

#[derive(Clone, Copy, Debug)]
struct DevRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

struct Measurement {
    box_center: f64,
    ink_center: f64,
    ink_height: f64,
    box_source: &'static str,
}

impl Measurement {
    fn delta_lpx(&self, dpi: f64) -> f64 {
        (self.ink_center - self.box_center) / dpi
    }
}

fn measure(frame: &Frame, snap: &WidgetSnapshot, dpi: f64) -> Measurement {
    let rect = DevRect {
        x0: (snap.x as f64 * dpi).round() as usize,
        y0: (snap.y as f64 * dpi).round() as usize,
        x1: ((snap.x + snap.width) as f64 * dpi).round() as usize,
        y1: ((snap.y + snap.height) as f64 * dpi).round() as usize,
    };
    assert!(
        rect.x1 <= frame.width && rect.y1 <= frame.height && rect.x1 > rect.x0 + 8,
        "widget {} rect {rect:?} is not on the frame",
        snap.id
    );
    let fill = fill_value(frame, &rect);
    let rim = (2.0 * dpi).ceil() as usize;
    let corner = (2.0 * dpi).ceil() as usize;
    let columns = (rect.x0 + corner)..(rect.x1.saturating_sub(corner).max(rect.x0 + corner + 1));
    let span = columns.len().max(1);
    let lit = |y: usize| {
        columns
            .clone()
            .filter(|x| (frame.at(*x, y) - fill).abs() > 40.0)
            .count()
    };
    let text_rows: Vec<usize> = ((rect.y0 + rim)..(rect.y1.saturating_sub(rim).max(rect.y0 + rim + 1)))
        .filter(|y| {
            let n = lit(*y);
            n > 0 && n * 2 < span
        })
        .collect();
    assert!(
        !text_rows.is_empty(),
        "widget {} has no legible ink over fill {fill}",
        snap.id
    );
    let window = (text_rows[0].saturating_sub(2))..=(text_rows[text_rows.len() - 1] + 2).min(frame.height - 1);
    let mass = |x: usize| {
        window
            .clone()
            .map(|y| (frame.at(x, y) - fill).abs())
            .sum::<f64>()
    };
    let stem = columns
        .clone()
        .max_by(|a, b| mass(*a).partial_cmp(&mass(*b)).unwrap())
        .expect("non-empty rect");
    let peak = window.clone().map(|y| frame.at(stem, y)).fold(fill, |acc, v| {
        if (v - fill).abs() > (acc - fill).abs() {
            v
        } else {
            acc
        }
    });
    assert!(
        (peak - fill).abs() > 40.0,
        "widget {} has no legible ink (peak {peak}, fill {fill})",
        snap.id
    );
    let (ink_top, ink_bottom) =
        band_edges(frame, stem, *window.start(), *window.end(), peak, fill);
    let box_center = (snap.y as f64 + snap.height as f64 / 2.0) * dpi;
    Measurement {
        box_center,
        ink_center: (ink_top + ink_bottom) / 2.0,
        ink_height: ink_bottom - ink_top,
        box_source: "widget rect",
    }
}

fn snapshot_of<'a>(widgets: &'a [WidgetSnapshot], id: &str) -> &'a WidgetSnapshot {
    widgets
        .iter()
        .find(|w| w.id == id && w.visible)
        .unwrap_or_else(|| panic!("no visible widget with id `{id}`"))
}

#[makepad_test]
fn chrome_labels_are_ink_centered(app: TestApp) {
    app.locator(Selector::id("ws_quad")).wait_visible();
    let widgets = app.widget_snapshot();
    let shot = app.screenshot();
    let frame = Frame::decode(&shot);
    let widest = widgets.iter().map(|w| w.width).max().unwrap_or(1);
    let dpi = (frame.width as f64 / widest as f64).round().max(1.0);
    println!(
        "[fab-chrome] frame {:?} {}x{} dpi {dpi}",
        shot, frame.width, frame.height
    );

    let mut worst: Option<(String, f64)> = None;
    let mut check = |name: &str, snap: &WidgetSnapshot| {
        let m = measure(&frame, snap, dpi);
        let delta = m.delta_lpx(dpi);
        println!(
            "[fab-chrome] {name:16} box_c {:8.3} ({}) ink_c {:8.3} ink_h {:6.3} -> {delta:+.3} lpx",
            m.box_center, m.box_source, m.ink_center, m.ink_height
        );
        let worse = match &worst {
            None => true,
            Some((_, w)) => delta.abs() > w.abs(),
        };
        if worse {
            worst = Some((name.to_string(), delta));
        }
    };

    check("ws_quad", snapshot_of(&widgets, "ws_quad"));
    check("ws_render", snapshot_of(&widgets, "ws_render"));
    if let Some(snap) = widgets.iter().find(|w| w.id == "menu_view" && w.visible) {
        check("menu_view", snap);
    }

    let (name, delta) = worst.expect("measured nothing");
    assert!(
        delta.abs() <= TOLERANCE_LPX,
        "[fab-chrome] `{name}`: label ink sits {delta:+.3} logical px off the box center \
         (tolerance {TOLERANCE_LPX}); see {shot:?}"
    );
}
