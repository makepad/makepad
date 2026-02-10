use {
    crate::{
        cx::Cx,
        cx_api::CxOsOp,
        draw_pass::{CxDrawPassColorTexture, CxDrawPassParent, DrawPassClearColor},
        event::Event,
        event::{TextClipboardEvent, WindowGeom, WindowGeomChangeEvent},
        makepad_live_id::*,
        makepad_math::*,
        makepad_micro_serde::*,
        os::{
            cx_stdin::{
                HostToStdin, PollTimer, PresentableDraw, PresentableImageId, StdinToHost,
                SWAPCHAIN_IMAGE_COUNT,
            },
            metal::{DrawPassMode, MetalCx},
        },
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        window::CxWindowPool,
    },
    std::{cell::RefCell, io, io::prelude::*, io::BufReader, rc::Rc},
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
    pub(crate) fn stdin_send_draw_complete(presentable_draw: PresentableDraw) {
        let _ = io::stdout().write_all(
            StdinToHost::DrawCompleteAndFlip(presentable_draw)
                .to_json()
                .as_bytes(),
        );
        let _ = io::stdout().flush();
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
                    self.draw_pass(draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
                CxDrawPassParent::None => {
                    self.draw_pass(draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }

    pub fn stdin_event_loop(&mut self, metal_cx: &mut MetalCx) {
        let (json_msg_tx, json_msg_rx) = std::sync::mpsc::channel();
        {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(std::io::stdin().lock());
                let mut line = String::new();
                loop {
                    line.clear();
                    if let Ok(0) | Err(_) = reader.read_line(&mut line) {
                        break;
                    }

                    // alright lets put the line in a json parser
                    match HostToStdin::deserialize_json(&line) {
                        Ok(msg) => {
                            if json_msg_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            // we should output a log string
                            crate::error!("Cant parse stdin-JSON {} {:?}", line, err)
                        }
                    }
                }
            });
        }

        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut stdin_windows: Vec<StdinWindow> = Vec::new();

        self.call_event_handler(&Event::Startup);

        // lets create 2 windows

        while let Ok(msg) = json_msg_rx.recv() {
            match msg {
                /* HostToStdin::ReloadFile {file, contents} => {
                    // alright lets reload this file in our DSL system
                    let _ = self.live_file_change_sender.send(vec![LiveFileChange{
                        file_name: file,
                        content: contents
                    }]);
                }*/
                HostToStdin::KeyDown(e) => {
                    self.call_event_handler(&Event::KeyDown(e));
                }
                HostToStdin::KeyUp(e) => {
                    self.call_event_handler(&Event::KeyUp(e));
                }
                HostToStdin::TextInput(e) => {
                    self.call_event_handler(&Event::TextInput(e));
                }
                HostToStdin::TextCopy => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                        response: response.clone(),
                    }));
                    let text = response.borrow().clone();
                    if let Some(text) = text {
                        let _ = io::stdout()
                            .write_all(StdinToHost::SetClipboard(text).to_json().as_bytes());
                        let _ = io::stdout().flush();
                    }
                }
                HostToStdin::TextCut => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCut(TextClipboardEvent {
                        response: response.clone(),
                    }));
                    let text = response.borrow().clone();
                    if let Some(text) = text {
                        let _ = io::stdout()
                            .write_all(StdinToHost::SetClipboard(text).to_json().as_bytes());
                        let _ = io::stdout().flush();
                    }
                }
                HostToStdin::MouseDown(e) => {
                    self.fingers.process_tap_count(dvec2(e.x, e.y), e.time);
                    // store the window_id we mousedowned on
                    let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                    let mouse_down_event = e.into_event(window_id, pos);
                    self.fingers.mouse_down(mouse_down_event.button, window_id);
                    self.call_event_handler(&Event::MouseDown(mouse_down_event));
                }
                HostToStdin::MouseMove(e) => {
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
                HostToStdin::MouseUp(e) => {
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
                HostToStdin::Scroll(e) => {
                    let (window_id, pos) = self.windows.window_id_contains(dvec2(e.x, e.y));
                    self.call_event_handler(&Event::Scroll(e.into_event(window_id, pos)));
                }
                HostToStdin::WindowGeomChange {
                    dpi_factor,
                    left,
                    top,
                    width,
                    height,
                    window_id,
                } => {
                    let window_id = CxWindowPool::from_usize(window_id);

                    if self.windows.is_valid(window_id) {
                        let old_geom = self.windows[window_id].window_geom.clone();
                        let new_geom = WindowGeom {
                            position: dvec2(left, top),
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
                HostToStdin::Swapchain(new_swapchain) => {
                    // Convert SharedSwapchain to LocalSwapchain
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
                    self.stdin_handle_platform_ops(metal_cx, &mut stdin_windows);
                }
                HostToStdin::Tick => {
                    for stdin_window in &mut stdin_windows {
                        if let Some(swapchain) = stdin_window.swapchain.as_mut() {
                            let [presentable_image] = &mut swapchain.presentable_images;
                            // Create texture from IOSurface via global ID lookup
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
                                if self.textures[texture.texture_id()].update_from_shared_handle(
                                    metal_cx,
                                    presentable_image.iosurface_id,
                                ) {
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
                    /*if self.handle_live_edit() {
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }*/

                    self.handle_networking_events();
                    self.handle_gamepad_events();
                    self.stdin_handle_platform_ops(metal_cx, &mut stdin_windows);

                    // we should now run all the stuff.
                    let time_now = self.os.stdin_timers.time_now();
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(time_now);
                    }

                    if self.need_redrawing() {
                        self.call_draw_event(time_now);
                        self.mtl_compile_shaders(metal_cx);
                    }
                    self.stdin_handle_repaint(
                        metal_cx,
                        &mut stdin_windows,
                        self.os.stdin_timers.time_now() as f32,
                    );

                    // Run garbage collection if needed - safe moment after paint
                    self.with_vm(|vm| {
                        if vm.heap().needs_gc() {
                            vm.gc();
                        }
                    });

                    // If we have pending animations or timers, request another frame from the host
                    if self.new_next_frames.len() != 0
                        || self.need_redrawing()
                        || !self.os.stdin_timers.timers.is_empty()
                    {
                        let _ = io::stdout()
                            .write_all(StdinToHost::RequestAnimationFrame.to_json().as_bytes());
                    }
                }
            }
        }
        // we should poll our runloop
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
                    // we should call to the host to make a window with this id
                    let _ = io::stdout().write_all(
                        StdinToHost::CreateWindow {
                            window_id: window_id.id(),
                            kind_id: window.kind_id,
                        }
                        .to_json()
                        .as_bytes(),
                    );
                }
                CxOsOp::SetCursor(cursor) => {
                    let _ =
                        io::stdout().write_all(StdinToHost::SetCursor(cursor).to_json().as_bytes());
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
                    self.os.http_requests.make_http_request(
                        request_id,
                        request,
                        self.os.network_response.sender.clone(),
                    );
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    self.os.http_requests.cancel_http_request(request_id);
                }
                CxOsOp::CopyToClipboard(content) => {
                    let _ = io::stdout()
                        .write_all(StdinToHost::SetClipboard(content).to_json().as_bytes());
                    let _ = io::stdout().flush();
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
                         CxOsOp::ShowTextIME(_area, _pos) => {},
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
