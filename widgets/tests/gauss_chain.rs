//! Headless determinism check of the windows_blur example GaussStack.
//! Run with MAKEPAD=headless RUSTFLAGS='--cfg headless' cargo test --release
//! -p makepad-widgets --test gauss_chain -- --ignored --nocapture. Optional
//! MAKEPAD_HEADLESS_OUT_DIR keeps the two compared frames.

pub use makepad_widgets;
use makepad_widgets::*;
use std::{cell::RefCell, rc::Rc};

#[path = "../../examples/windows_blur/src/main.rs"]
mod windows_blur;

fn decode_png(path: &std::path::Path) -> ((usize, usize), Vec<u8>) {
    use makepad_zune_png::{makepad_zune_core::bytestream::ZCursor, PngDecoder};
    let bytes = std::fs::read(path).unwrap();
    let mut png = PngDecoder::new(ZCursor::new(bytes));
    let pixels = png.decode_raw().unwrap();
    (png.dimensions().unwrap(), pixels)
}

fn render_gauss_frame(out_dir: &std::path::Path) -> usize {
    use windows_blur::App;
    std::fs::create_dir_all(out_dir).unwrap();
    std::env::set_var("MAKEPAD_HEADLESS_OUT_DIR", out_dir);
    let mut handler = makepad_platform::_app_main_event_closure!(App);
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        handler(cx, event);
        if matches!(event, Event::Draw(_)) {
            // Warm up window glass registration, then record the capture and
            // consumer together before the headless backend submits the frame.
            cx.redraw_all();
            handler(cx, event);
        }
    }))));
    cx.borrow_mut().init_cx_os();
    makepad_platform::remote::start_if_requested();
    Cx::event_loop(cx.clone());
    let cx = cx.borrow();
    cx.passes
        .id_iter()
        .filter(|id| {
            let pass = &cx.passes[*id];
            (pass.debug_name.starts_with("gauss_mip_")
                || pass.debug_name.starts_with("gauss_smooth_mip_"))
                && pass.main_draw_list_id.is_some()
        })
        .count()
}

#[test]
#[ignore = "requires a MAKEPAD=headless release build"]
fn windows_blur_gauss_stack_render_matches_reference() {
    assert!(std::env::var("MAKEPAD")
        .unwrap_or_default()
        .contains("headless"));
    // Logical window is 1380x920; default headless DPI is 2. Pin to 1 so the
    // comparison is independent of a previously captured retina PNG.
    std::env::set_var("MAKEPAD_HEADLESS_DPI", "1");

    let keep = std::env::var_os("MAKEPAD_HEADLESS_OUT_DIR").map(std::path::PathBuf::from);
    let scratch = keep.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("makepad-gauss-chain-{}", std::process::id()))
    });
    let first_dir = scratch.join("instance_a");
    let second_dir = scratch.join("instance_b");

    let filters_a = render_gauss_frame(&first_dir);
    let filters_b = render_gauss_frame(&second_dir);
    assert_eq!(
        filters_a, 12,
        "the example must exercise all six downsample and six tent passes"
    );
    assert_eq!(filters_a, filters_b, "second instance must run the same GaussStack");

    let first = first_dir.join("window_0_frame_000000.png");
    let second = second_dir.join("window_0_frame_000000.png");
    assert!(first.is_file(), "missing frame {}", first.display());
    assert!(second.is_file(), "missing frame {}", second.display());

    let (size, pixels) = decode_png(&first);
    let (other_size, other_pixels) = decode_png(&second);
    assert_eq!(size, other_size);
    assert_eq!(pixels.len(), other_pixels.len());
    let components = pixels.len() / (size.0 * size.1);
    let changed = pixels
        .chunks_exact(components)
        .zip(other_pixels.chunks_exact(components))
        .filter(|(a, b)| a != b)
        .count();
    let fraction = changed as f64 / (size.0 * size.1) as f64;
    println!(
        "GaussStack pixel diff: {changed}/{} ({:.6}%)",
        size.0 * size.1,
        fraction * 100.0
    );
    assert!(
        fraction <= 0.005,
        "GaussStack changed more than 0.5% of pixels"
    );
}
