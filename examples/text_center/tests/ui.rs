//! The vertical text-centering contract for boxed labels, measured in pixels.
//!
//! A widget that paints a box and puts one line of text in it (button, tab,
//! dropdown, a label centered in a view) must center the *ink* of that line —
//! the cap-height band, from the flat top of a capital `H` down to the
//! baseline — on the box's center line. Centering the font's line box instead
//! (ascender over baseline over descender) leaves the ink sitting high,
//! because a text font's ascender reaches further above the cap line than its
//! descender reaches below the baseline.
//!
//! Every specimen on the page is labelled `H` for that reason: `H` has a flat
//! cap top and sits flat on the baseline, so its ink band *is* the cap band,
//! and its stem gives a column of pixels whose coverage integrates to a
//! sub-pixel edge position.

use makepad_test::{makepad_test, run_with_config, Selector, TestApp, TestConfig, WidgetSnapshot};
use std::path::Path;

/// How far the ink center may sit from the box center, in logical pixels.
const TOLERANCE_LPX: f64 = 0.5;

// ---------------------------------------------------------------- image ----

struct Frame {
    width: usize,
    height: usize,
    /// Luminance-ish: these specimens are gray, so the red channel is enough.
    lum: Vec<f64>,
}

impl Frame {
    fn at(&self, x: usize, y: usize) -> f64 {
        self.lum[y * self.width + x]
    }

    fn decode(path: &Path) -> Frame {
        use makepad_zune_core::bytestream::ZCursor;
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

/// The sub-pixel top and bottom of a band along one column.
///
/// `inside`/`outside` are the two flat values the band sits between, so
/// `coverage = (value - outside) / (inside - outside)`. For a band with flat
/// horizontal edges, the coverage of the partially covered rows *is* the
/// fraction of those rows the band covers, so summing coverage outward from a
/// row that is definitely inside the band gives the edges directly. Whatever
/// spread the antialiasing adds is the same at both edges, so it cancels in
/// the center — which is the only number this suite asserts on.
///
/// The row to start from is the one with the most coverage, so that a band
/// sitting anywhere in the window is measured correctly — including a badly
/// off-center one, which is the whole point.
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
    assert!(
        top > y0 as f64 + 0.5 && bottom < y1 as f64 - 0.5,
        "band on column {x} runs into the edge of its window {y0}..{y1} \
         (measured {top:.2}..{bottom:.2}) — the wrong thing is being measured"
    );
    (top, bottom)
}

/// The most common pixel value strictly inside a rect: the widget's own fill.
fn fill_value(frame: &Frame, r: &DevRect) -> f64 {
    let mut hist = [0usize; 256];
    for y in (r.y0 + 3)..(r.y1 - 3) {
        for x in (r.x0 + 3)..(r.x1 - 3) {
            hist[frame.at(x, y) as usize] += 1;
        }
    }
    let (value, _) = hist
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .expect("non-empty rect");
    value as f64
}

#[derive(Clone, Copy, Debug)]
struct DevRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

// ------------------------------------------------------------ measuring ----

struct Measurement {
    /// Center of the painted box, in device pixels.
    box_center: f64,
    /// Center of the ink band, in device pixels.
    ink_center: f64,
    ink_height: f64,
    box_source: &'static str,
}

impl Measurement {
    fn delta_lpx(&self, dpi: f64) -> f64 {
        (self.ink_center - self.box_center) / dpi
    }
}

/// Measures where the label's ink sits inside the widget's box.
///
/// The ink comes out of the pixels either way. The box comes out of the
/// pixels too whenever the widget paints a flat face with a clean column
/// (every specimen but the dock tab, whose face is a gradient that bleeds
/// past its own rect to hide the seam with the dock body); otherwise it falls
/// back to the widget rect the app reports, which is rounded to whole logical
/// pixels.
fn measure(frame: &Frame, snap: &WidgetSnapshot, dpi: f64) -> Measurement {
    let rect = DevRect {
        x0: (snap.x as f64 * dpi).round() as usize,
        y0: (snap.y as f64 * dpi).round() as usize,
        x1: ((snap.x + snap.width) as f64 * dpi).round() as usize,
        y1: ((snap.y + snap.height) as f64 * dpi).round() as usize,
    };
    assert!(
        rect.x1 <= frame.width && rect.y1 <= frame.height && rect.x1 > rect.x0 + 12,
        "widget {} rect {rect:?} is not on the frame",
        snap.id
    );
    let fill = fill_value(frame, &rect);

    // The ink: the column that departs furthest from the fill, which for an
    // `H` is one of its stems, running from the cap line to the baseline. Ink
    // is lighter than its box under a dark theme and darker under a light one,
    // so everything here works on the distance from the fill, not on
    // brightness. The search stays clear of the box's own antialiased rim and
    // rounded corners, which depart from the fill too.
    let rim = (2.0 * dpi).ceil() as usize;
    let corner = (3.0 * dpi).ceil() as usize;
    let columns = (rect.x0 + corner)..(rect.x1 - corner);
    let span = columns.len();
    let lit = |y: usize| {
        columns
            .clone()
            .filter(|x| (frame.at(*x, y) - fill).abs() > 40.0)
            .count()
    };
    // Rows a glyph touches are lit across a few columns; rows a seam, border
    // or gradient step touches are lit clear across the widget. Only the
    // former are text, so the ink window is the run of rows between the first
    // and the last of them (a dock tab, whose face bleeds past its own rect to
    // hide the seam with the dock body, needs this).
    let text_rows: Vec<usize> = ((rect.y0 + rim)..(rect.y1 - rim))
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
    let window = (text_rows[0].saturating_sub(2))..=(text_rows[text_rows.len() - 1] + 2);
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
    let interior = (rect.y0 + rim)..(rect.y1 - rim);

    // The box: a column inside the widget that the label does not cross, read
    // from a few pixels above the rect to a few below it.
    let pad = 4;
    let clean = ((rect.x0 + corner)..(rect.x1 - corner)).find(|x| {
        interior
            .clone()
            .all(|y| (frame.at(*x, y) - fill).abs() <= 6.0)
    });
    let outside = clean.map(|x| frame.at(x, rect.y0.saturating_sub(pad)));
    let sampled = match (clean, outside) {
        (Some(x), Some(out))
            if (out - fill).abs() > 20.0
                && rect.y0 > pad
                && rect.y1 + pad < frame.height
                // The surround has to be flat on both sides, or the "edge"
                // being integrated is somebody else's gradient.
                && (frame.at(x, rect.y1 + pad) - out).abs() <= 6.0 =>
        {
            Some(band_edges(frame, x, rect.y0 - pad, rect.y1 + pad, fill, out))
        }
        _ => None,
    };
    let (box_center, box_source) = match sampled {
        Some((top, bottom)) => ((top + bottom) / 2.0, "pixels"),
        None => (
            (snap.y as f64 + snap.height as f64 / 2.0) * dpi,
            "widget rect",
        ),
    };

    Measurement {
        box_center,
        ink_center: (ink_top + ink_bottom) / 2.0,
        ink_height: ink_bottom - ink_top,
        box_source,
    }
}

fn dpi_of(frame: &Frame, widgets: &[WidgetSnapshot]) -> f64 {
    let widest = widgets.iter().map(|w| w.width).max().unwrap_or(0);
    assert!(widest > 0, "no widgets in snapshot");
    let dpi = frame.width as f64 / widest as f64;
    assert!(
        (dpi - dpi.round()).abs() < 0.02,
        "frame {}px over widest widget {widest}px is not a whole dpi factor",
        frame.width
    );
    dpi.round()
}

fn snapshot_of<'a>(widgets: &'a [WidgetSnapshot], id: &str) -> &'a WidgetSnapshot {
    widgets
        .iter()
        .find(|w| w.id == id && w.visible)
        .unwrap_or_else(|| panic!("no visible widget with id `{id}`"))
}

fn tab_snapshot<'a>(widgets: &'a [WidgetSnapshot], name: &str) -> &'a WidgetSnapshot {
    widgets
        .iter()
        .find(|w| w.widget_type == "DockTab" && w.text.as_deref() == Some(name) && w.visible)
        .unwrap_or_else(|| panic!("no visible DockTab named `{name}`"))
}

/// Every specimen on the page, measured and asserted in one pass.
fn assert_page_is_centered(app: TestApp, label: &str) {
    app.locator(Selector::id("btn_10")).wait_visible();
    let widgets = app.widget_snapshot();
    let shot = app.screenshot();
    let frame = Frame::decode(&shot);
    let dpi = dpi_of(&frame, &widgets);
    println!("[{label}] frame {shot:?} {}x{} dpi {dpi}", frame.width, frame.height);

    let by_id = [
        "btn_08", "btn_10", "btn_14", "btn_24", "btn_tall", "btn_bold", "btn_icon", "drop_10",
        "drop_14", "label_box",
    ];
    let mut worst: Option<(String, f64)> = None;
    let mut check = |name: String, snap: &WidgetSnapshot| {
        let m = measure(&frame, snap, dpi);
        let delta = m.delta_lpx(dpi);
        println!(
            "[{label}] {name:12} box_c {:8.3} ({}) ink_c {:8.3} ink_h {:6.3} -> {delta:+.3} lpx",
            m.box_center, m.box_source, m.ink_center, m.ink_height
        );
        let is_worse = match &worst {
            None => true,
            Some((_, w)) => delta.abs() > w.abs(),
        };
        if is_worse {
            worst = Some((name, delta));
        }
    };
    for id in by_id {
        check(id.to_string(), snapshot_of(&widgets, id));
    }
    check("tab(H)".to_string(), tab_snapshot(&widgets, "H"));

    let (name, delta) = worst.expect("measured nothing");
    assert!(
        delta.abs() <= TOLERANCE_LPX,
        "[{label}] `{name}`: label ink sits {delta:+.3} logical px off the box center \
         (tolerance {TOLERANCE_LPX}); see {shot:?}"
    );
}

#[makepad_test]
fn boxed_labels_are_ink_centered(app: TestApp) {
    assert_page_is_centered(app, "dark");
}

/// The same page under the light theme. The two desktop themes share every
/// number that decides vertical placement, so this is a guard against a
/// future theme growing its own padding or font size and quietly reopening
/// the bug on one side only.
#[test]
fn boxed_labels_are_ink_centered_light_theme() {
    let mut config = TestConfig::current_package(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_PKG_NAME"),
        "boxed_labels_are_ink_centered_light_theme",
    )
    .expect("test config");
    config
        .env
        .insert("MAKEPAD_TEXT_CENTER_THEME".to_string(), "light".to_string());
    run_with_config(config, |app| assert_page_is_centered(app, "light")).expect("light theme run");
}
