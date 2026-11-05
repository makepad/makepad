//! `MAKEPAD_TRACE=frames` prints the HAL loop's frame histograms. An app whose window paints every
//! frame runs the app loop until a print counted presents; the prints must have counted the beats
//! and the next frames too. The window is hidden (an offscreen window of the default backend:
//! Metal on macOS, Vulkan on Linux), so this runs on any box and puts nothing on a screen.
use makepad_platform::makepad_error_log::{self, LogLevel};
use makepad_platform::*;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The `[frames]` lines logged so far.
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn tap(message: &str, _: LogLevel) {
    if message.starts_with("[frames] ") {
        LINES.lock().unwrap().push(message.to_string());
    }
}

/// The most any print counted after `prefix` (`<name>: n=<count> ...`, `ticks/2s: host=<count>`).
fn most(lines: &[String], prefix: &str) -> u32 {
    lines.iter().filter_map(|l| l.split_once(prefix)?.1.split(' ').next()?.parse().ok()).max().unwrap_or(0)
}

fn main() {
    std::env::set_var("MAKEPAD_HIDE_WINDOWS", "1");
    makepad_error_log::set_trace_topics("frames");
    makepad_error_log::set_log_tap(Some(tap));
    let start = Instant::now();
    // the window and what it paints, and every frame asks for the next one and paints again
    let mut window: Option<(WindowHandle, DrawPass, DrawList)> = None;
    let mut cx = Cx::new(Box::new(move |cx, event| match event {
        Event::Startup => {
            let (handle, mut pass, list) = (WindowHandle::new(cx), DrawPass::new(cx), DrawList::new(cx));
            handle.set_pass(cx, &pass);
            pass.set_size(cx, dvec2(64.0, 64.0));
            pass.set_dpi_factor(cx, 1.0);
            cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
            window = Some((handle, pass, list));
            cx.new_next_frame();
        }
        Event::NextFrame(_) => {
            if most(&LINES.lock().unwrap(), "present gap: n=") > 0 || start.elapsed() > Duration::from_secs(30) {
                cx.quit();
                return;
            }
            if let Some((_, pass, _)) = &window {
                cx.passes[pass.draw_pass_id()].paint_dirty = true;
            }
            cx.new_next_frame();
        }
        _ => {}
    }));
    cx.init_cx_os();
    Cx::event_loop(std::rc::Rc::new(std::cell::RefCell::new(cx)));

    let lines = LINES.lock().unwrap().clone();
    let shown = lines.join("\n");
    assert!(most(&lines, "present gap: n=") > 0, "no print counted presents in 30 s:\n{shown}");
    assert!(most(&lines, "next_frame gap: n=") > 0, "no print counted next frames:\n{shown}");
    let beats = [" display=", " cadence=", " host="].map(|p| most(&lines, p));
    assert!(beats.iter().any(|&n| n > 0), "no print counted beats:\n{shown}");
    println!("{shown}");
}
