use {
    std::{
        io,
        io::prelude::*,
        io::BufReader,
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        makepad_error_log::*,
        makepad_micro_serde::*,
        makepad_live_compiler::LiveFileChange,
        event::Event,
        window::CxWindowPool,
        event::WindowGeom,
        texture::Texture,
        live_traits::LiveNew,
        thread::Signal,
        os::{
            d3d11::D3d11Cx,
            cx_stdin::{HostToStdin, StdinToHost, Swapchain},
        },
        pass::{CxPassParent, PassClearColor, CxPassColorTexture},
        cx_api::CxOsOp,
        cx::Cx,
        windows::Win32::Foundation::HANDLE,
    }
};

impl Cx {
    
    pub (crate) fn stdin_handle_repaint(
        &mut self,
        d3d11_cx: &mut D3d11Cx,
        swapchain: Option<&Swapchain<Texture >>,
        present_index: &mut usize,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    // only render to swapchain if swapchain exists
                    if let Some(swapchain) = swapchain {
                        
                        // and if GPU is not already rendering something else
                        if self.os.new_frame_being_rendered.is_none() {
                            let current_image = &swapchain.presentable_images[*present_index];
                            *present_index = (*present_index + 1) % swapchain.presentable_images.len();
                            
                            // render to swapchain
                            self.draw_pass_to_texture(*pass_id, d3d11_cx, current_image.image.texture_id());
                            
                            // start GPU event query
                            d3d11_cx.start_querying();
                            
                            // and inform event_loop to go poll GPU readiness
                            self.os.new_frame_being_rendered = Some(current_image.id);
                        }
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_magic_texture(*pass_id, d3d11_cx);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id, d3d11_cx);
                }
            }
        }
    }
    
    pub fn stdin_event_loop(&mut self, d3d11_cx: &mut D3d11Cx) {
        // HACK(eddyb) there's no easy way (AFAICT) to make `stdin` non-blocking,
        // and we want to be able to "see ahead" JSON messages, at the very least
        // for catching up the client after a spam of `WindowSize`s from the host.
        let swapchain_get_key32 = | swapchain: &Swapchain<_> | {
            swapchain.presentable_images[0].id.as_u64() as u32
        };
        let latest_swapchain_key32 = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(!0));
        let (json_msg_tx, json_msg_rx) = std::sync::mpsc::channel();
        {
            let latest_swapchain_key32 = latest_swapchain_key32.clone();
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
                            // Keep track of `WindowSize`s, because if we fall behind,
                            // we end up with a backlog of useless outdated swapchains,
                            // while not having any other way to determine they are.
                            if let HostToStdin::WindowSize(ws) = &msg {
                                latest_swapchain_key32.store(
                                    swapchain_get_key32(&ws.swapchain),
                                    std::sync::atomic::Ordering::Release,
                                );
                            }
                            if json_msg_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            // we should output a log string
                            error!("Cant parse stdin-JSON {} {:?}", line, err)
                        }
                    }
                }
            });
        }
        
        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());
        
        let mut swapchain = None;
        let mut present_index = 0;
        
        self.call_event_handler(&Event::Construct);
        
        while let Ok(msg) = json_msg_rx.recv() {
            match msg {
                HostToStdin::ReloadFile {file, contents} => {
                    // alright lets reload this file in our DSL system
                    let _ = self.live_file_change_sender.send(vec![LiveFileChange {
                        file_name: file,
                        content: contents
                    }]);
                }
                HostToStdin::KeyDown(e) => {
                    self.call_event_handler(&Event::KeyDown(e));
                }
                HostToStdin::KeyUp(e) => {
                    self.call_event_handler(&Event::KeyUp(e));
                }
                HostToStdin::MouseDown(e) => {
                    self.fingers.process_tap_count(
                        dvec2(e.x, e.y),
                        e.time
                    );
                    self.fingers.mouse_down(e.button);
                    
                    self.call_event_handler(&Event::MouseDown(e.into()));
                }
                HostToStdin::MouseMove(e) => {
                    self.call_event_handler(&Event::MouseMove(e.into()));
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                    self.fingers.switch_captures();
                }
                HostToStdin::MouseUp(e) => {
                    let button = e.button;
                    self.call_event_handler(&Event::MouseUp(e.into()));
                    self.fingers.mouse_up(button);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }
                HostToStdin::Scroll(e) => {
                    self.call_event_handler(&Event::Scroll(e.into()))
                }
                HostToStdin::WindowSize(ws) => {
                    // Always update the geometry, as latter input events might
                    // need it to be accurately handled, while output swapchains
                    // do not have the same semantic effect when they're missing.
                    let window = &mut self.windows[CxWindowPool::id_zero()];
                    window.window_geom = WindowGeom {
                        dpi_factor: ws.dpi_factor,
                        inner_size: dvec2(ws.swapchain.width as f64, ws.swapchain.height as f64) / ws.dpi_factor,
                        ..Default::default()
                    };
                    
                    // We could only have gotten to this point iff at some point
                    // `swapchain_get_key32(&ws.swapchain)` had been stored into
                    // `latest_swapchain_key32` - and if that's *not* anymore the
                    // current value, a newer `WindowSize` was therefore observed,
                    // which implies the host will ignore any attempt to present
                    // on this swapchain (as it has lost all track of its backing
                    // GPU memory etc.), and rendering on it at all is wasteful.
                    /*let key32 = swapchain_get_key32(&ws.swapchain);
                    let outdated_swapchain =
                    skip_outdated_swapchains_always
                        && key32 != latest_swapchain_key32.load(std::sync::atomic::Ordering::Acquire)
                        || skip_outdated_swapchains_batch
                        && key32 != latest_swapchain_key32_in_batch;
                    if outdated_swapchain {
                        swapchain = None;
                        resizes_skipped += 1;
                        let npot = resizes_skipped.next_power_of_two();
                        if
                        (3..16).contains(&resizes_skipped)
                            || resizes_skipped >= 16 &&
                        (resizes_skipped == npot || resizes_skipped == npot * 3 / 2)
                        {
                            error!("!!! {resizes_skipped} `WindowSize`s skipped in a row !!!");
                        }
                        continue;
                    }
                    resizes_skipped = 0;*/
                    
                    self.redraw_all();
                    
                    let new_swapchain = ws.swapchain.images_map( | _, handle | {
                        let handle = HANDLE(handle as isize);
                        
                        let texture = Texture::new(self);
                        self.textures[texture.texture_id()]
                            .os.update_from_shared_handle(d3d11_cx, handle);
                        texture
                    });
                    let swapchain = swapchain.insert(new_swapchain);
                    
                    // reset present_index
                    present_index = 0;
                    
                    self.stdin_handle_platform_ops(Some(swapchain), present_index);
                }
                HostToStdin::Tick {frame: _, time, ..} => if swapchain.is_some() {
                    
                    // poll the service for updates
                    // check signals
                    if Signal::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if self.handle_live_edit() {
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();
                    // we should poll our runloop
                    self.stdin_handle_platform_ops(swapchain.as_ref(), present_index);
                    
                    // alright a tick.
                    // we should now run all the stuff.
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(time);
                    }
                    
                    if self.need_redrawing() {
                        self.call_draw_event();
                        self.hlsl_compile_shaders(d3d11_cx);
                    }
                    
                    // repaint
                    self.stdin_handle_repaint(d3d11_cx, swapchain.as_ref(), &mut present_index);
                    
                    
                    // check if GPU is ready to flip frames
                    if let Some(rendered_image_id) = self.os.new_frame_being_rendered {
                        if d3d11_cx.is_gpu_done() {
                            let _ = io::stdout().write_all(StdinToHost::DrawCompleteAndFlip(rendered_image_id).to_json().as_bytes());
                            self.os.new_frame_being_rendered = None;
                        }
                    }
                    
                }
            }
        }
    }
    
    fn stdin_handle_platform_ops(
        &mut self,
        swapchain: Option<&Swapchain<Texture >>,
        present_index: usize,
    ) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    if window_id != CxWindowPool::id_zero() {
                        panic!("ONLY ONE WINDOW SUPPORTED");
                    }
                    let window = &mut self.windows[CxWindowPool::id_zero()];
                    window.is_created = true;
                    // lets set up our render pass target
                    let pass = &mut self.passes[window.main_pass_id.unwrap()];
                    if let Some(swapchain) = swapchain {
                        pass.color_textures = vec![CxPassColorTexture {
                            clear_color: PassClearColor::ClearWith(vec4(1.0, 1.0, 0.0, 1.0)),
                            //clear_color: PassClearColor::ClearWith(pass.clear_color),
                            texture_id: swapchain.presentable_images[present_index].image.texture_id(),
                        }];
                    }
                },
                CxOsOp::SetCursor(cursor) => {
                    let _ = io::stdout().write_all(StdinToHost::SetCursor(cursor).to_json().as_bytes());
                },
                _ => ()
                /*
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
