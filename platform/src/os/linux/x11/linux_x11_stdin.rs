use {
    crate::{
        cx::Cx,
        cx_api::CxOsOp,
        draw_pass::{CxDrawPassColorTexture, CxDrawPassParent, DrawPassClearColor},
        event::{Event, TextClipboardEvent, WindowGeom},
        gl_sys,
        makepad_live_id::*,
        makepad_math::*,
        makepad_micro_serde::*,
        os::cx_stdin::{
            aux_chan, HostPresentableImage, HostSwapchain, HostToStdin,
            LinuxSharedSoftwareBuffer, PollTimer, PresentableDraw, StdinToHost,
        },
        texture::{Texture, TextureFormat, TextureSize},
        thread::SignalToUI,
        window::CxWindowPool,
        CxOsApi,
    },
    std::{
        cell::RefCell,
        io::{self, prelude::*, BufReader},
        rc::Rc,
    },
};

#[derive(Default)]
pub(crate) struct StdinWindow {
    swapchain: Option<HostSwapchain>,
    present_index: usize,
    readback_framebuffer: Option<u32>,
}

impl Cx {
    pub(crate) fn stdin_handle_repaint(&mut self, windows: &mut Vec<StdinWindow>) {
        self.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;

        let time_now = self.os.stdin_timers.time_now();
        for &draw_pass_id in &passes_todo {
            self.passes[draw_pass_id].set_time(time_now as f32);
            match self.passes[draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    // only render to swapchain if swapchain exists
                    let window = &mut windows[window_id.id()];
                    if let Some(swapchain) = &mut window.swapchain {
                        let current_index = window.present_index;
                        window.present_index =
                            (window.present_index + 1) % swapchain.presentable_images.len();
                        let current_image = &mut swapchain.presentable_images[current_index];

                        // render to swapchain
                        self.draw_pass_to_texture(draw_pass_id, Some(&current_image.texture));

                        // wait for GPU to finish rendering
                        unsafe {
                            (self.os.gl().glFinish)();
                        }

                        let dpi_factor = self.passes[draw_pass_id].dpi_factor.unwrap();
                        let pass_rect = self.get_pass_rect(draw_pass_id, dpi_factor).unwrap();
                        let presentable_draw = PresentableDraw {
                            window_id: window_id.id(),
                            target_id: current_image.id,
                            width: (pass_rect.size.x * dpi_factor) as u32,
                            height: (pass_rect.size.y * dpi_factor) as u32,
                        };

                        if let Some(software_buffer) = current_image.software_buffer.as_mut() {
                            software_buffer.as_bytes_mut().fill(0);
                            unsafe {
                                let gl = self.os.gl();

                                while (gl.glGetError)() != 0 {}

                                if window.readback_framebuffer.is_none() {
                                    let mut framebuffer = std::mem::MaybeUninit::uninit();
                                    (gl.glGenFramebuffers)(1, framebuffer.as_mut_ptr());
                                    window.readback_framebuffer = Some(framebuffer.assume_init());
                                }
                                let readback_framebuffer = window.readback_framebuffer.unwrap();
                                let gl_texture = match self.textures[current_image.texture.texture_id()]
                                    .os
                                    .gl_texture
                                {
                                    Some(texture) => texture,
                                    None => continue,
                                };

                                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, readback_framebuffer);
                                (gl.glFramebufferTexture2D)(
                                    gl_sys::FRAMEBUFFER,
                                    gl_sys::COLOR_ATTACHMENT0,
                                    gl_sys::TEXTURE_2D,
                                    gl_texture,
                                    0,
                                );
                                (gl.glPixelStorei)(gl_sys::PACK_ALIGNMENT, 1);
                                (gl.glPixelStorei)(gl_sys::PACK_ROW_LENGTH, 0);
                                (gl.glPixelStorei)(gl_sys::PACK_SKIP_PIXELS, 0);
                                (gl.glPixelStorei)(gl_sys::PACK_SKIP_ROWS, 0);
                                (gl.glReadPixels)(
                                    0,
                                    0,
                                    swapchain.alloc_width as i32,
                                    swapchain.alloc_height as i32,
                                    gl_sys::RGBA,
                                    gl_sys::UNSIGNED_BYTE,
                                    software_buffer.as_mut_ptr(),
                                );
                                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);

                                let gl_error = (gl.glGetError)();
                                if gl_error != 0 {
                                    crate::error!(
                                        "software fallback readback glReadPixels error={}",
                                        gl_error
                                    );
                                }
                            }

                            // Keep RunView size pixels in-band, matching other backends.
                            let encode_size_pixel = |size: u32| {
                                [
                                    ((size >> 8) & 0xff) as u8,
                                    0,
                                    (size & 0xff) as u8,
                                    0xff,
                                ]
                            };
                            if let Ok(stride) = usize::try_from(software_buffer.stride) {
                                let width_px = encode_size_pixel(presentable_draw.width);
                                let height_px = encode_size_pixel(presentable_draw.height);
                                let bytes = software_buffer.as_bytes_mut();
                                if stride >= 8 && bytes.len() >= 8 {
                                    bytes[0..4].copy_from_slice(&width_px);
                                    bytes[4..8].copy_from_slice(&height_px);

                                    if swapchain.alloc_height > 1 {
                                        let last_row =
                                            (swapchain.alloc_height as usize - 1).saturating_mul(stride);
                                        if last_row + 8 <= bytes.len() {
                                            bytes[last_row..last_row + 4].copy_from_slice(&width_px);
                                            bytes[last_row + 4..last_row + 8]
                                                .copy_from_slice(&height_px);
                                        }
                                    }
                                }
                            }
                        }

                        // inform host that frame is ready
                        let _ = io::stdout().write_all(
                            StdinToHost::DrawCompleteAndFlip(presentable_draw)
                                .to_json()
                                .as_bytes(),
                        );
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(draw_pass_id, None);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_texture(draw_pass_id, None);
                }
            }
        }
    }

    pub fn stdin_event_loop(&mut self) {
        let aux_chan_client_endpoint =
            aux_chan::InheritableClientEndpoint::from_process_args_in_client()
                .and_then(|chan| chan.into_uninheritable())
                .expect("failed to acquire auxiliary channel");

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
                println!("Terminating STDIN reader loop")
            });
        }

        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut stdin_windows: Vec<StdinWindow> = Vec::new();

        self.call_event_handler(&Event::Startup);

        while let Ok(msg) = json_msg_rx.recv() {
            match msg {
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
                    self.call_event_handler(&Event::Scroll(e.into_event(window_id, pos)))
                }
                HostToStdin::WindowGeomChange {
                    dpi_factor,
                    left,
                    top,
                    width,
                    height,
                    window_id,
                } => {
                    self.windows[CxWindowPool::from_usize(window_id)].window_geom = WindowGeom {
                        dpi_factor,
                        position: dvec2(left, top),
                        inner_size: dvec2(width, height),
                        ..Default::default()
                    };
                    self.redraw_all();
                }
                HostToStdin::Swapchain(new_swapchain) => {
                    let window_id = new_swapchain.window_id;
                    let alloc_width = new_swapchain.alloc_width;
                    let alloc_height = new_swapchain.alloc_height;
                    let shared_images = new_swapchain.presentable_images;
                    let presentable_images = std::array::from_fn(|i| {
                        let shared_pi = shared_images[i];
                        let mut texture = Texture::new(self);
                        let mut software_buffer = None;
                        match shared_pi.recv_fds_from_aux_chan(&aux_chan_client_endpoint) {
                            Ok(pi) => {
                                if pi.image.is_software_fallback() {
                                    texture = Texture::new_with_format(
                                        self,
                                        TextureFormat::RenderBGRAu8 {
                                            size: TextureSize::Fixed {
                                                width: alloc_width as usize,
                                                height: alloc_height as usize,
                                            },
                                            initial: true,
                                        },
                                    );
                                    let stride = pi.image.plane.stride;
                                    let maybe_len = usize::try_from(alloc_height)
                                        .ok()
                                        .and_then(|height| {
                                            usize::try_from(stride)
                                                .ok()
                                                .and_then(|stride| stride.checked_mul(height))
                                        });
                                    match maybe_len {
                                        Some(len) => match LinuxSharedSoftwareBuffer::from_fd(
                                            pi.image.plane.dma_buf_fd,
                                            len,
                                            stride,
                                        ) {
                                            Ok(buffer) => software_buffer = Some(buffer),
                                            Err(err) => {
                                                crate::error!(
                                                    "failed to map software fallback swapchain image: {err:?}"
                                                );
                                            }
                                        },
                                        None => {
                                            crate::error!(
                                                "software fallback swapchain size overflow ({alloc_width}x{alloc_height}, stride={stride})"
                                            );
                                        }
                                    }
                                } else {
                                    let desc = TextureFormat::SharedBGRAu8 {
                                        id: pi.id,
                                        width: alloc_width as usize,
                                        height: alloc_height as usize,
                                        initial: true,
                                    };
                                    texture = Texture::new_with_format(self, desc);
                                    self.textures[texture.texture_id()]
                                        .update_from_shared_dma_buf_image(
                                            self.os.gl(),
                                            self.os.opengl_cx.as_ref().unwrap(),
                                            &pi.image,
                                        );
                                }
                            }
                            Err(err) => {
                                crate::error!(
                                    "failed to receive new swapchain on auxiliary channel: {err:?}"
                                );
                            }
                        }
                        HostPresentableImage {
                            id: shared_pi.id,
                            texture,
                            software_buffer,
                        }
                    });
                    let new_swapchain = HostSwapchain {
                        window_id,
                        alloc_width,
                        alloc_height,
                        presentable_images,
                    };
                    let stdin_window = &mut stdin_windows[window_id];
                    stdin_window.swapchain = Some(new_swapchain);

                    // reset present_index
                    stdin_window.present_index = 0;

                    // lets set up our render pass target
                    let window = &mut self.windows[CxWindowPool::from_usize(window_id)];
                    let pass = &mut self.passes[window.main_pass_id.unwrap()];
                    if let Some(swapchain) = &stdin_window.swapchain {
                        pass.color_textures = vec![CxDrawPassColorTexture {
                            clear_color: DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 0.0, 1.0)),
                            //clear_color: DrawPassClearColor::ClearWith(pass.clear_color),
                            texture: swapchain.presentable_images[stdin_window.present_index]
                                .texture
                                .clone(),
                        }];
                    }

                    self.redraw_all();
                    self.stdin_handle_platform_ops(&mut stdin_windows);
                }

                HostToStdin::Tick => {
                    // poll the service for updates
                    // check signals
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

                    if self.handle_live_edit() {
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();

                    // we should poll our runloop
                    self.stdin_handle_platform_ops(&mut stdin_windows);

                    // alright a tick.
                    // we should now run all the stuff.
                    let time_now = self.seconds_since_app_start();
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(time_now);
                    }

                    if self.need_redrawing() {
                        self.call_draw_event(time_now);
                        self.opengl_compile_shaders();
                    }

                    self.stdin_handle_repaint(&mut stdin_windows);
                }
            }
        }
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
                    use crate::os::linux::http::LinuxHttpSocket;
                    LinuxHttpSocket::open(
                        request_id,
                        request,
                        self.os.network_response.sender.clone(),
                    );
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    use crate::os::linux::http::LinuxHttpSocket;
                    LinuxHttpSocket::cancel(request_id);
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
