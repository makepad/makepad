use crate::{
    cx::Cx,
    cx_api::{CxOsApi, CxOsOp},
    draw_pass::{CxDrawPassColorTexture, CxDrawPassParent, DrawPassClearColor},
    event::Event,
    event::{WindowGeom, WindowGeomChangeEvent},
    makepad_math::*,
    makepad_micro_serde::*,
    os::{
        metal::{DrawPassMode, MetalCx},
        shared_framebuf::{PollTimer, PresentableDraw, PresentableImageId, SWAPCHAIN_IMAGE_COUNT},
    },
    studio::{AppToStudio, GCSample, StudioToApp, StudioToAppVec},
    texture::{Texture, TextureFormat},
    thread::SignalToUI,
    web_socket::WebSocketMessage,
    window::CxWindowPool,
};

/// Local swapchain for client-side texture management
struct LocalSwapchain {
    alloc_width: u32,
    alloc_height: u32,
    presentable_images: [LocalPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

struct LocalPresentableImage {
    id: PresentableImageId,
    iosurface_id: u32,
    texture: Option<Texture>,
}

pub(crate) struct StdinWindow {
    swapchain: Option<LocalSwapchain>,
}

impl StdinWindow {
    fn new() -> Self {
        Self { swapchain: None }
    }
}

impl Cx {
    fn stdin_send_to_host(msg: AppToStudio) {
        Cx::send_studio_message(msg);
    }

    pub(crate) fn stdin_send_draw_complete(presentable_draw: PresentableDraw) {
        Self::stdin_send_to_host(AppToStudio::DrawCompleteAndFlip(presentable_draw));
    }

    pub(crate) fn stdin_handle_repaint(
        &mut self,
        metal_cx: &mut MetalCx,
        stdin_windows: &mut [StdinWindow],
        time: f32,
    ) {
        //self.demo_time_repaint = false;
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for &draw_pass_id in &passes_todo {
            self.passes[draw_pass_id].set_time(time as f32);
            match self.passes[draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(swapchain) = &mut stdin_windows[window_id.id()].swapchain {
                        let [current_image] = &swapchain.presentable_images;
                        if let Some(texture) = &current_image.texture {
                            let window = &mut self.windows[window_id];
                            let pass = &mut self.passes[window.main_pass_id.unwrap()];
                            pass.color_textures = vec![CxDrawPassColorTexture {
                                clear_color: DrawPassClearColor::ClearWith(pass.clear_color),
                                texture: texture.clone(),
                            }];

                            let kind_id = window.kind_id;
                            let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
                            let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();

                            let future_presentable_draw = PresentableDraw {
                                target_id: current_image.id,
                                window_id: window_id.id(),
                                width: (pass_rect.size.x * dpi_factor) as u32,
                                height: (pass_rect.size.y * dpi_factor) as u32,
                            };
                            // render to swapchain
                            self.draw_pass(
                                draw_pass_id,
                                metal_cx,
                                DrawPassMode::StdinMain(future_presentable_draw, kind_id),
                            );

                            // and then wait for GPU, which calls stdin_send_draw_complete when its done
                        }
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    self.draw_pass(draw_pass_id, metal_cx, DrawPassMode::StdinTexture);
                }
                CxDrawPassParent::None => {
                    self.draw_pass(draw_pass_id, metal_cx, DrawPassMode::StdinTexture);
                }
            }
        }
    }

    pub fn stdin_event_loop(&mut self, metal_cx: &mut MetalCx) {
        Self::stdin_send_to_host(AppToStudio::ReadyToStart);

        let mut stdin_windows: Vec<StdinWindow> = Vec::new();
        self.call_event_handler(&Event::Startup);

        loop {
            if !Self::has_studio_web_socket() {
                crate::error!("--stdin-loop mode requires a studio websocket");
                break;
            }
            let incoming = match self.recv_studio_websocket_message() {
                Some(incoming) => incoming,
                None => break,
            };

            match incoming {
                WebSocketMessage::Binary(data) => match StudioToAppVec::deserialize_bin(&data) {
                    Ok(msgs) => {
                        for msg in msgs.0 {
                            if self.stdin_handle_host_to_stdin(msg, metal_cx, &mut stdin_windows) {
                                return;
                            }
                        }
                        self.handle_actions();
                    }
                    Err(err) => {
                        crate::error!(
                            "Cant parse studio websocket binary payload in --stdin-loop: {:?}",
                            err
                        );
                    }
                },
                WebSocketMessage::String(text) => {
                    if let Ok(msg) = StudioToApp::deserialize_json(&text) {
                        if self.stdin_handle_host_to_stdin(msg, metal_cx, &mut stdin_windows) {
                            return;
                        }
                    } else if !text.trim().is_empty() {
                        crate::warning!(
                            "Ignoring unexpected studio websocket text: {}",
                            text.trim()
                        );
                    }
                }
                WebSocketMessage::Error(err) => {
                    crate::error!("Studio websocket error in --stdin-loop: {}", err);
                    break;
                }
                WebSocketMessage::Closed => break,
                WebSocketMessage::Opened => {}
            }
        }
    }

    fn stdin_handle_host_to_stdin(
        &mut self,
        msg: StudioToApp,
        metal_cx: &mut MetalCx,
        stdin_windows: &mut Vec<StdinWindow>,
    ) -> bool {
        match msg {
            // Mouse events: resolve window_id from coordinates (stdin mode
            // supports multiple virtual windows).
            StudioToApp::MouseDown(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::MouseMove(ref e) => {
                let (window_id, pos) = if let Some((_, window_id)) = self.fingers.first_mouse_button
                {
                    (window_id, self.windows[window_id].window_geom.position)
                } else {
                    self.windows.window_id_contains(dvec2(e.x, e.y))
                };
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::TweakRay(e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                let dpi_factor = self.windows[window_id].window_geom.dpi_factor.max(1.0);
                let tweak_ray = e.into_event(window_id, pos, dpi_factor);
                self.call_event_handler(&Event::TweakRay(tweak_ray));
            }
            StudioToApp::MouseUp(ref e) => {
                let (window_id, pos) = if let Some((_, window_id)) = self.fingers.first_mouse_button
                {
                    (window_id, self.windows[window_id].window_geom.position)
                } else {
                    self.windows.window_id_contains(dvec2(e.x, e.y))
                };
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            StudioToApp::Scroll(ref e) => {
                let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                return self.dispatch_studio_msg(msg, window_id, pos);
            }
            // Stdin-specific: window geometry and swapchain management.
            StudioToApp::WindowGeomChange {
                dpi_factor,
                left: _left,
                top: _top,
                width,
                height,
                window_id,
            } => {
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
                    if re.old_geom.dpi_factor != re.new_geom.dpi_factor
                        || re.old_geom.inner_size != re.new_geom.inner_size
                    {
                        if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                            self.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                    self.call_event_handler(&Event::WindowGeomChange(re));
                }
            }
            StudioToApp::Swapchain(new_swapchain) => {
                stdin_windows[new_swapchain.window_id].swapchain = Some(LocalSwapchain {
                    alloc_width: new_swapchain.alloc_width,
                    alloc_height: new_swapchain.alloc_height,
                    presentable_images: new_swapchain.presentable_images.map(|pi| {
                        LocalPresentableImage {
                            id: pi.id,
                            iosurface_id: pi.iosurface_id,
                            texture: None,
                        }
                    }),
                });
                self.redraw_all();
                self.stdin_handle_platform_ops(metal_cx, stdin_windows);
            }
            StudioToApp::Tick => {
                for stdin_window in stdin_windows.iter_mut() {
                    if let Some(swapchain) = stdin_window.swapchain.as_mut() {
                        let [presentable_image] = &mut swapchain.presentable_images;
                        if presentable_image.texture.is_none()
                            && presentable_image.iosurface_id != 0
                        {
                            let format = TextureFormat::SharedBGRAu8 {
                                id: presentable_image.id,
                                width: swapchain.alloc_width as usize,
                                height: swapchain.alloc_height as usize,
                                initial: true,
                            };
                            let texture = Texture::new_with_format(self, format);
                            if self.textures[texture.texture_id()]
                                .update_from_shared_handle(metal_cx, presentable_image.iosurface_id)
                            {
                                presentable_image.texture = Some(texture);
                            }
                        }
                    }
                }
                if SignalToUI::check_and_clear_ui_signal() {
                    self.handle_media_signals();
                    self.handle_script_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }
                let events = self.os.stdin_timers.get_dispatch();
                for event in events {
                    self.handle_script_timer(&event);
                    self.call_event_handler(&Event::Timer(event));
                }

                self.handle_networking_events();
                self.handle_gamepad_events();
                self.stdin_handle_platform_ops(metal_cx, stdin_windows);

                let time_now = self.os.stdin_timers.time_now();
                if !self.new_next_frames.is_empty() {
                    self.call_next_frame_event(time_now);
                }
                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    self.mtl_compile_shaders(metal_cx);
                }
                self.stdin_handle_repaint(
                    metal_cx,
                    stdin_windows,
                    self.os.stdin_timers.time_now() as f32,
                );

                let gc_start = self.seconds_since_app_start();
                let mut gc_heap_live = None;
                self.with_vm(|vm| {
                    if vm.heap().needs_gc() {
                        vm.gc();
                        gc_heap_live = Some(vm.heap().gc_live_len() as u64);
                    }
                });
                if let Some(heap_live) = gc_heap_live {
                    let gc_end = self.seconds_since_app_start();
                    Cx::send_studio_message(AppToStudio::GCSample(GCSample {
                        start: gc_start,
                        end: gc_end,
                        heap_live,
                    }));
                }

                if !self.new_next_frames.is_empty()
                    || self.need_redrawing()
                    || !self.os.stdin_timers.timers.is_empty()
                {
                    Self::stdin_send_to_host(AppToStudio::RequestAnimationFrame);
                }
            }
            // All other variants (Key*, Text*, Screenshot, WidgetTreeDump,
            // Kill, KeepAlive, LiveChange, None) handled by shared dispatch.
            other => {
                return self.dispatch_studio_msg(other, CxWindowPool::id_zero(), dvec2(0.0, 0.0));
            }
        }
        false
    }

    fn stdin_handle_platform_ops(
        &mut self,
        _metal_cx: &MetalCx,
        stdin_windows: &mut Vec<StdinWindow>,
    ) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    while window_id.id() >= stdin_windows.len() {
                        stdin_windows.push(StdinWindow::new());
                    }
                    let window = &mut self.windows[window_id];
                    window.is_created = true;
                    Self::stdin_send_to_host(AppToStudio::CreateWindow {
                        window_id: window_id.id(),
                        kind_id: window.kind_id,
                    });
                }
                CxOsOp::SetCursor(cursor) => {
                    Self::stdin_send_to_host(AppToStudio::SetCursor(cursor));
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
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::CopyToClipboard(content) => {
                    Self::stdin_send_to_host(AppToStudio::SetClipboard(content));
                }
                _ => (), /*
                         CxOsOp::CloseWindow(_window_id) => {},
                         CxOsOp::MinimizeWindow(_window_id) => {},
                         CxOsOp::MaximizeWindow(_window_id) => {},
                         CxOsOp::RestoreWindow(_window_id) => {},
                         CxOsOp::FullscreenWindow(_window_id) => {},
                         CxOsOp::NormalizeWindow(_window_id) => {}
                         CxOsOp::SetTopmost(_window_id, _is_topmost) => {}
                         CxOsOp::XrStartPresenting(_) => {},
                         CxOsOp::XrStopPresenting(_) => {},
                         CxOsOp::ShowTextIME(_area, _pos, _config) => {},
                         CxOsOp::HideTextIME => {},
                         CxOsOp::SetCursor(_cursor) => {},
                         CxOsOp::StartTimer {timer_id, interval, repeats} => {},
                         CxOsOp::StopTimer(timer_id) => {},
                         CxOsOp::StartDragging(dragged_item) => {}
                         CxOsOp::UpdateMenu(menu) => {}*/
            }
        }
    }
}
