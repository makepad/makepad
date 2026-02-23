use super::raster::encode_png_rgba;
use crate::{
    cx::Cx,
    cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
    event::{Event, TextClipboardEvent, WindowGeom, WindowGeomChangeEvent},
    makepad_live_id::*,
    makepad_math::dvec2,
    makepad_micro_serde::*,
    os::shared_framebuf::{PollTimer, PresentableDraw, PresentableImageId},
    studio::{AppToStudio, ScreenshotResponse, StudioToApp},
    thread::SignalToUI,
    window::CxWindowPool,
};
use std::{
    cell::RefCell,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

#[derive(Default)]
struct HeadlessWindowState {
    created: bool,
    width: u32,
    height: u32,
    dpi_factor: f64,
    frame_id: u64,
    presentable_id: Option<PresentableImageId>,
}

impl HeadlessWindowState {
    fn ensure_size_defaults(&mut self) {
        if self.width <= 1 {
            self.width = 1280;
        }
        if self.height <= 1 {
            self.height = 720;
        }
        if self.dpi_factor <= 0.0 {
            self.dpi_factor = 1.0;
        }
    }
}

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());

        if crate::app_main::should_run_stdin_loop_from_env() {
            cx.borrow_mut().in_makepad_studio = true;
            cx.borrow_mut().stdin_event_loop();
        } else {
            cx.borrow_mut().headless_single_frame();
        }
    }

    fn headless_single_frame(&mut self) {
        let mut windows = Vec::new();
        self.call_event_handler(&Event::Startup);
        self.headless_handle_platform_ops(&mut windows, false);
        if windows.is_empty() {
            windows.push(HeadlessWindowState {
                created: true,
                width: 1280,
                height: 720,
                dpi_factor: 1.0,
                frame_id: 0,
                presentable_id: None,
            });
        }
        let time_now = self.seconds_since_app_start();
        if !self.new_next_frames.is_empty() {
            self.call_next_frame_event(time_now);
        }
        if self.need_redrawing() {
            self.call_draw_event(time_now);
            self.headless_compile_shaders();
            self.headless_emit_frames(&mut windows, false, time_now);
        }
    }

    pub fn stdin_event_loop(&mut self) {
        let (json_msg_tx, json_msg_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(std::io::stdin().lock());
            let mut line = String::new();
            loop {
                line.clear();
                if let Ok(0) | Err(_) = reader.read_line(&mut line) {
                    break;
                }
                match StudioToApp::deserialize_json(&line) {
                    Ok(msg) => {
                        if json_msg_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        crate::error!("Cant parse stdin-JSON {} {:?}", line, err);
                    }
                }
            }
        });

        write_stdout_msg(&AppToStudio::ReadyToStart);
        self.call_event_handler(&Event::Startup);

        let mut windows = Vec::<HeadlessWindowState>::new();
        let mut running = true;

        while running {
            let msg = match json_msg_rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            };
            match msg {
                StudioToApp::KeyDown(e) => self.call_event_handler(&Event::KeyDown(e)),
                StudioToApp::KeyUp(e) => self.call_event_handler(&Event::KeyUp(e)),
                StudioToApp::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
                StudioToApp::TextCopy => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                        response: response.clone(),
                    }));
                    let text = response.borrow().clone();
                    if let Some(text) = text {
                        write_stdout_msg(&AppToStudio::SetClipboard(text));
                    }
                }
                StudioToApp::TextCut => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCut(TextClipboardEvent {
                        response: response.clone(),
                    }));
                    let text = response.borrow().clone();
                    if let Some(text) = text {
                        write_stdout_msg(&AppToStudio::SetClipboard(text));
                    }
                }
                StudioToApp::MouseDown(e) => {
                    self.fingers.process_tap_count(dvec2(e.x, e.y), e.time);
                    let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                    let mouse_down_event = e.into_event(window_id, pos);
                    self.fingers.mouse_down(mouse_down_event.button, window_id);
                    self.call_event_handler(&Event::MouseDown(mouse_down_event));
                }
                StudioToApp::MouseMove(e) => {
                    let (window_id, pos) =
                        if let Some((_, window_id)) = self.fingers.first_mouse_button {
                            (window_id, self.windows[window_id].window_geom.position)
                        } else {
                            self.windows.window_id_contains(dvec2(e.x, e.y))
                        };
                    self.call_event_handler(&Event::MouseMove(e.into_event(window_id, pos)));
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                    self.fingers.switch_captures();
                }
                StudioToApp::TweakRay(e) => {
                    let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                    let dpi_factor = self.windows[window_id].window_geom.dpi_factor.max(1.0);
                    let tweak_ray = e.into_event(window_id, pos, dpi_factor);
                    self.call_event_handler(&Event::TweakRay(tweak_ray));
                }
                StudioToApp::MouseUp(e) => {
                    let (window_id, pos) =
                        if let Some((_, window_id)) = self.fingers.first_mouse_button {
                            (window_id, self.windows[window_id].window_geom.position)
                        } else {
                            self.windows.window_id_contains(dvec2(e.x, e.y))
                        };
                    let mouse_up_event = e.into_event(window_id, pos);
                    let button = mouse_up_event.button;
                    self.call_event_handler(&Event::MouseUp(mouse_up_event));
                    self.fingers.mouse_up(button);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }
                StudioToApp::Scroll(e) => {
                    let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                    self.call_event_handler(&Event::Scroll(e.into_event(window_id, pos)));
                }
                StudioToApp::WindowGeomChange {
                    dpi_factor,
                    left: _left,
                    top: _top,
                    width,
                    height,
                    window_id,
                } => {
                    while windows.len() <= window_id {
                        windows.push(Default::default());
                    }
                    windows[window_id].created = true;
                    windows[window_id].dpi_factor = dpi_factor;
                    windows[window_id].width = width.max(1.0) as u32;
                    windows[window_id].height = height.max(1.0) as u32;
                    windows[window_id].ensure_size_defaults();

                    let window_id = CxWindowPool::from_usize(window_id);
                    if self.windows.is_valid(window_id) {
                        let old_geom = self.windows[window_id].window_geom.clone();
                        let new_geom = WindowGeom {
                            position: dvec2(0.0, 0.0),
                            dpi_factor,
                            inner_size: dvec2(width, height),
                            ..Default::default()
                        };
                        self.windows[window_id].window_geom = new_geom.clone();
                        let re = WindowGeomChangeEvent {
                            window_id,
                            new_geom,
                            old_geom,
                        };
                        self.call_event_handler(&Event::WindowGeomChange(re));
                    }
                    self.redraw_all();
                }
                StudioToApp::Swapchain(shared_swapchain) => {
                    let window_id = shared_swapchain.window_id;
                    while windows.len() <= window_id {
                        windows.push(Default::default());
                    }
                    let state = &mut windows[window_id];
                    state.created = true;
                    state.width = shared_swapchain.alloc_width.max(1);
                    state.height = shared_swapchain.alloc_height.max(1);
                    state.presentable_id =
                        shared_swapchain.presentable_images.first().map(|pi| pi.id);
                    state.ensure_size_defaults();
                    self.redraw_all();
                }
                StudioToApp::Tick => {
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                    }

                    let timer_events = self.os.stdin_timers.get_dispatch();
                    for event in timer_events {
                        self.handle_script_timer(&event);
                        self.call_event_handler(&Event::Timer(event));
                    }

                    running = self.headless_handle_platform_ops(&mut windows, true);
                    if !running {
                        break;
                    }

                    let time_now = self.os.stdin_timers.time_now();
                    if !self.new_next_frames.is_empty() {
                        self.call_next_frame_event(time_now);
                    }

                    let mut rendered = false;
                    if self.need_redrawing() && !self.screenshot_requests.is_empty() {
                        self.call_draw_event(time_now);
                        self.headless_compile_shaders();
                        rendered = self.headless_emit_frames(&mut windows, true, time_now);
                    }

                    if rendered
                        || !self.os.stdin_timers.timers.is_empty()
                        || !self.new_next_frames.is_empty()
                    {
                        write_stdout_msg(&AppToStudio::RequestAnimationFrame);
                    }
                }
            }
        }
    }

    fn headless_emit_frames(
        &mut self,
        windows: &mut [HeadlessWindowState],
        send_protocol: bool,
        time_now: f64,
    ) -> bool {
        let output_dir = self.headless_output_dir();
        let mut rendered_any = false;

        // Render all passes using the real draw tree + JIT shaders
        let framebuffers = self.headless_render_all_passes(time_now);

        for (window_id, fb) in framebuffers {
            // Skip if we don't have a window state for this window
            if window_id >= windows.len() {
                continue;
            }
            let state = &mut windows[window_id];
            if !state.created {
                state.created = true;
                state.ensure_size_defaults();
            }

            let width = fb.width as u32;
            let height = fb.height as u32;

            let request_ids = if send_protocol {
                self.take_studio_screenshot_request_ids(0)
            } else {
                Vec::new()
            };
            if send_protocol && request_ids.is_empty() {
                continue;
            }

            let rgba = fb.to_rgba8();
            let png = match encode_png_rgba(width, height, &rgba) {
                Ok(png) => png,
                Err(err) => {
                    crate::error!(
                        "headless png encode failed for window {} frame {}: {}",
                        window_id,
                        state.frame_id,
                        err
                    );
                    continue;
                }
            };

            let png_path = output_dir.join(format!(
                "window_{window_id}_frame_{:06}.png",
                state.frame_id
            ));
            if let Err(err) = std::fs::write(&png_path, png) {
                crate::error!(
                    "headless frame write failed for `{}`: {}",
                    png_path.display(),
                    err
                );
                continue;
            }

            if send_protocol {
                write_stdout_msg(&AppToStudio::Screenshot(ScreenshotResponse {
                    request_ids,
                    png,
                    width,
                    height,
                }));
                let target_id = if let Some(id) = state.presentable_id {
                    id
                } else {
                    let id = PresentableImageId::alloc();
                    state.presentable_id = Some(id);
                    id
                };
                write_stdout_msg(&AppToStudio::DrawCompleteAndFlip(PresentableDraw {
                    window_id,
                    target_id,
                    width,
                    height,
                }));
            } else {
                crate::log!(
                    "headless frame written: {} ({}x{})",
                    png_path.display(),
                    width,
                    height
                );
            }

            state.frame_id += 1;
            rendered_any = true;
        }

        rendered_any
    }

    fn headless_output_dir(&mut self) -> PathBuf {
        if let Some(path) = &self.os.frame_dir {
            return path.clone();
        }
        let path = std::env::var("MAKEPAD_HEADLESS_OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        if let Err(err) = std::fs::create_dir_all(&path) {
            crate::error!(
                "failed to create headless frame output dir `{}`: {}",
                path.display(),
                err
            );
        }
        self.os.frame_dir = Some(path.clone());
        path
    }

    fn headless_handle_platform_ops(
        &mut self,
        windows: &mut Vec<HeadlessWindowState>,
        send_protocol: bool,
    ) -> bool {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    while window_id.id() >= windows.len() {
                        windows.push(Default::default());
                    }

                    // Headless: use 1920x1080 at 2x DPI for high-quality output
                    let inner_size = dvec2(1920.0, 1080.0);
                    let dpi_factor = 2.0;

                    let state = &mut windows[window_id.id()];
                    state.created = true;
                    state.dpi_factor = dpi_factor;
                    state.width = inner_size.x.max(1.0) as u32;
                    state.height = inner_size.y.max(1.0) as u32;

                    let window = &mut self.windows[window_id];
                    window.is_created = true;
                    window.window_geom.inner_size = inner_size;
                    window.window_geom.dpi_factor = dpi_factor;
                    if send_protocol {
                        write_stdout_msg(&AppToStudio::CreateWindow {
                            window_id: window_id.id(),
                            kind_id: window.kind_id,
                        });
                    }
                    self.redraw_all();
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    if self.windows.is_valid(window_id) {
                        self.windows[window_id].window_geom.inner_size = size;
                    }
                    while window_id.id() >= windows.len() {
                        windows.push(Default::default());
                    }
                    windows[window_id.id()].created = true;
                    windows[window_id.id()].width = size.x.max(1.0) as u32;
                    windows[window_id.id()].height = size.y.max(1.0) as u32;
                    windows[window_id.id()].ensure_size_defaults();
                    self.redraw_all();
                }
                CxOsOp::SetCursor(cursor) => {
                    if send_protocol {
                        write_stdout_msg(&AppToStudio::SetCursor(cursor));
                    }
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    self.os
                        .stdin_timers
                        .timers
                        .insert(timer_id, PollTimer::new(interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.stdin_timers.timers.remove(&timer_id);
                }
                CxOsOp::CopyToClipboard(content) => {
                    if send_protocol {
                        write_stdout_msg(&AppToStudio::SetClipboard(content));
                    }
                }
                CxOsOp::Quit => {
                    return false;
                }
                _ => {}
            }
        }
        true
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR") {
            self.package_root = Some(item.to_string());
        }
        self.native_load_dependencies();
    }

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(f);
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time.unwrap_or_else(Instant::now))
            .as_secs_f64()
    }

    fn open_url(&mut self, _url: &str, _in_place: OpenUrlInPlace) {
        crate::warning!("open_url is ignored in headless mode");
    }
}

fn write_stdout_msg(msg: &AppToStudio) {
    let _ = io::stdout().write_all(msg.to_json().as_bytes());
    let _ = io::stdout().flush();
}
