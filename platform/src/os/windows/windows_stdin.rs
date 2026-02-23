use {
    crate::{
        cx::Cx,
        cx_api::CxOsOp,
        draw_pass::CxDrawPassParent,
        event::Event,
        event::WindowGeom,
        makepad_math::*,
        makepad_micro_serde::*,
        os::{
            d3d11::D3d11Cx,
            shared_framebuf::{PresentableDraw, PresentableImageId, SWAPCHAIN_IMAGE_COUNT},
            win32_app::Win32Time,
        },
        studio::{AppToStudio, GCSample, StudioToApp, StudioToAppVec},
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        web_socket::WebSocketMessage,
        window::CxWindowPool,
        windows::Win32::Foundation::HANDLE,
        CxOsApi,
    },
    std::ffi::c_void,
};

struct LocalPresentableImage {
    id: PresentableImageId,
    image: Texture,
}

struct LocalSwapchain {
    presentable_images: [LocalPresentableImage; SWAPCHAIN_IMAGE_COUNT],
}

#[derive(Default)]
pub(crate) struct StdinWindow {
    swapchain: Option<LocalSwapchain>,
    present_index: usize,
    new_frame_being_rendered: Option<PresentableDraw>,
}

impl Cx {
    fn stdin_send_to_host(msg: AppToStudio) {
        Cx::send_studio_message(msg);
    }

    pub(crate) fn stdin_handle_repaint(
        &mut self,
        d3d11_cx: &mut D3d11Cx,
        windows: &mut Vec<StdinWindow>,
        time: &Win32Time,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        let time_now = time.time_now();
        for &draw_pass_id in &passes_todo {
            self.passes[draw_pass_id].set_time(time_now as f32);
            match self.passes[draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    // only render to swapchain if swapchain exists
                    let window = &mut windows[window_id.id()];
                    if let Some(swapchain) = &window.swapchain {
                        // and if GPU is not already rendering something else
                        if window.new_frame_being_rendered.is_none() {
                            let current_image = &swapchain.presentable_images[window.present_index];

                            window.present_index =
                                (window.present_index + 1) % swapchain.presentable_images.len();

                            // render to swapchain
                            self.draw_pass_to_texture(
                                draw_pass_id,
                                d3d11_cx,
                                Some(current_image.image.texture_id()),
                            );

                            let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
                            let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
                            let future_presentable_draw = PresentableDraw {
                                window_id: window_id.id(),
                                target_id: current_image.id,
                                width: (pass_rect.size.x * dpi_factor) as u32,
                                height: (pass_rect.size.y * dpi_factor) as u32,
                            };

                            // start GPU event query
                            d3d11_cx.start_querying();

                            // and inform event_loop to go poll GPU readiness
                            window.new_frame_being_rendered = Some(future_presentable_draw);
                        }
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(draw_pass_id, d3d11_cx, None);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_texture(draw_pass_id, d3d11_cx, None);
                }
            }
        }
    }

    pub fn stdin_event_loop(&mut self, d3d11_cx: &mut D3d11Cx) {
        Self::stdin_send_to_host(AppToStudio::ReadyToStart);

        let mut stdin_windows: Vec<StdinWindow> = Vec::new();
        let time = Win32Time::new();
        self.call_event_handler(&Event::Startup);

        loop {
            let studio_web_socket = if let Some(studio_web_socket) = &mut self.studio_web_socket {
                studio_web_socket
            } else {
                crate::error!("--stdin-loop mode requires a studio websocket");
                break;
            };

            let incoming = match studio_web_socket.recv() {
                Ok(incoming) => incoming,
                Err(_) => break,
            };

            match incoming {
                WebSocketMessage::Binary(data) => match StudioToAppVec::deserialize_bin(&data) {
                    Ok(msgs) => {
                        for msg in msgs.0 {
                            if self.stdin_handle_host_to_stdin(
                                msg,
                                d3d11_cx,
                                &mut stdin_windows,
                                &time,
                            ) {
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
                        if self.stdin_handle_host_to_stdin(msg, d3d11_cx, &mut stdin_windows, &time)
                        {
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
        d3d11_cx: &mut D3d11Cx,
        stdin_windows: &mut Vec<StdinWindow>,
        time: &Win32Time,
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
                self.windows[CxWindowPool::from_usize(window_id)].window_geom = WindowGeom {
                    dpi_factor,
                    position: dvec2(0.0, 0.0),
                    inner_size: dvec2(width, height),
                    ..Default::default()
                };
                self.redraw_all();
            }
            StudioToApp::Swapchain(new_swapchain) => {
                let window_id = new_swapchain.window_id;
                let local_swapchain = LocalSwapchain {
                    presentable_images: new_swapchain.presentable_images.map(|pi| {
                        let handle = HANDLE(pi.handle as usize as *mut c_void);
                        let format = TextureFormat::SharedBGRAu8 {
                            id: pi.id,
                            width: new_swapchain.alloc_width as usize,
                            height: new_swapchain.alloc_height as usize,
                            initial: true,
                        };
                        let texture = Texture::new_with_format(self, format);
                        self.textures[texture.texture_id()]
                            .update_from_shared_handle(d3d11_cx, handle);
                        LocalPresentableImage {
                            id: pi.id,
                            image: texture,
                        }
                    }),
                };
                let stdin_window = &mut stdin_windows[window_id];
                stdin_window.swapchain = Some(local_swapchain);
                stdin_window.present_index = 0;

                self.redraw_all();
                self.stdin_handle_platform_ops(stdin_windows);
            }
            StudioToApp::Tick => {
                if SignalToUI::check_and_clear_ui_signal() {
                    self.handle_media_signals();
                    self.handle_script_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }

                self.handle_networking_events();
                self.stdin_handle_platform_ops(stdin_windows);

                let time_now = self.seconds_since_app_start();
                if !self.new_next_frames.is_empty() {
                    self.call_next_frame_event(time_now);
                }

                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    self.hlsl_compile_shaders(d3d11_cx);
                }

                self.stdin_handle_repaint(d3d11_cx, stdin_windows, time);

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

                let has_pending_draws = stdin_windows
                    .iter()
                    .any(|window| window.new_frame_being_rendered.is_some());
                if has_pending_draws && d3d11_cx.is_gpu_done() {
                    for window in stdin_windows.iter_mut() {
                        if let Some(presentable_draw) = window.new_frame_being_rendered.take() {
                            Self::stdin_send_to_host(AppToStudio::DrawCompleteAndFlip(
                                presentable_draw,
                            ));
                        }
                    }
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

    fn stdin_handle_platform_ops(&mut self, stdin_windows: &mut Vec<StdinWindow>) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    while window_id.id() >= stdin_windows.len() {
                        stdin_windows.push(StdinWindow::default());
                    }
                    //let stdin_window = &mut stdin_windows[window_id.id()];
                    let window = &mut self.windows[window_id];
                    window.is_created = true;
                    Self::stdin_send_to_host(AppToStudio::CreateWindow {
                        window_id: window_id.id(),
                        kind_id: window.kind_id,
                    });

                    // lets set up our render pass target
                    /* let pass = &mut self.passes[window.main_pass_id.unwrap()];
                    if let Some(swapchain) = swapchain {
                        pass.color_textures = vec![CxDrawPassColorTexture {
                            clear_color: DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 0.0, 1.0)),
                            //clear_color: DrawPassClearColor::ClearWith(pass.clear_color),
                            texture: swapchain.presentable_images[present_index].image.clone(),
                        }];
                    }*/
                }
                CxOsOp::SetCursor(cursor) => {
                    Self::stdin_send_to_host(AppToStudio::SetCursor(cursor));
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
