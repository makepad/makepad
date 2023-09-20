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
        os::cx_stdin::{HostToStdin, StdinToHost, Swapchain},
        pass::{CxPassParent, PassClearColor, CxPassColorTexture},
        cx_api::CxOsOp,
        cx::Cx,
        gl_sys,
    } 
};

impl Cx {
    
    pub (crate) fn stdin_handle_repaint(
        &mut self,
        swapchain: Option<&Swapchain<Texture>>,
        present_index: &mut usize,
    ) {
        self.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    if let Some(swapchain) = swapchain {
                        // HACK(eddyb) retry loop in case we have broken images.
                        for _ in 0..swapchain.presentable_images.len() {
                            let current_image = &swapchain.presentable_images[*present_index];
                            *present_index = (*present_index + 1) % swapchain.presentable_images.len();

                            if self.textures[current_image.image.texture_id()].os.gl_texture.is_none() {
                                // FIXME(eddyb) ask the server for a new swapchain.
                                continue;
                            }

                            // render to swapchain
                            self.draw_pass_to_texture(*pass_id, current_image.image.texture_id());

                            // wait for GPU to finish rendering
                            unsafe { gl_sys::Finish(); }

                            // inform host that frame is ready
                            let _ = io::stdout().write_all(StdinToHost::DrawCompleteAndFlip(current_image.id).to_json().as_bytes());

                            break;
                        }
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_magic_texture(*pass_id);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id);
                }
            }
        }
    }
    
    pub fn stdin_event_loop(&mut self) {
        // HACK(eddyb) there's no easy way (AFAICT) to make `stdin` non-blocking,
        // and we want to be able to "see ahead" JSON messages, at the very least
        // for catching up the client after a spam of `WindowSize`s from the host.
        let swapchain_get_key32 = |swapchain: &Swapchain<_>| {
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

        let mut skip_outdated_swapchains_always = false;
        let mut skip_outdated_swapchains_batch = false;
        match std::env::var("MAKEPAD_WARP").ok().as_ref().map_or("", |s| &s[..]) {
            "always" => skip_outdated_swapchains_always = true,
            "batch" => skip_outdated_swapchains_batch = true,
            "" => {}
            s => error!("unknown `MAKEPAD_WARP={s}` value (only `always` and `batch` are allowed)"),
        }

        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut swapchain = None;
        let mut present_index = 0;

        self.call_event_handler(&Event::Construct);

        let mut messages = std::collections::VecDeque::new();
        let mut resizes_skipped = 0u32;
        loop {
            let mut latest_swapchain_key32_in_batch = !0;
            loop {
                match json_msg_rx.try_recv() {
                    Ok(msg) => {
                        if let HostToStdin::WindowSize(ws) = &msg {
                            latest_swapchain_key32_in_batch = swapchain_get_key32(&ws.swapchain);
                        }
                        messages.push_back(msg);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) if messages.is_empty() => return,
                    Err(_) => break,
                }
            }

            for msg in messages.drain(..) {
                match msg {
                    HostToStdin::ReloadFile{file, contents}=>{
                        // alright lets reload this file in our DSL system
                        let _ = self.live_file_change_sender.send(vec![LiveFileChange{
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
                            dvec2(e.x,e.y),
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
                        let key32 = swapchain_get_key32(&ws.swapchain);
                        let outdated_swapchain =
                            skip_outdated_swapchains_always
                                && key32 != latest_swapchain_key32.load(std::sync::atomic::Ordering::Acquire)
                            || skip_outdated_swapchains_batch
                                && key32 != latest_swapchain_key32_in_batch;
                        if outdated_swapchain  {
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
                        resizes_skipped = 0;

                        self.redraw_all();

                        let new_swapchain = ws.swapchain.images_map(|_, dma_buf_image| {
                            // HACK(eddyb) we get the host process's ID, and
                            // a file descriptor *in that process* - normally
                            // it'd need to be passed to this process via some
                            // objcap mechanism (e.g. UNIX domain sockets),
                            // but all we have is JSON, and even using procfs
                            // (i.e. opening `/proc/$REMOTE_PID/fd/$REMOTE_FD`)
                            // doesn't play well with non-file file descriptors,
                            // so for now this requires `pidfd` (Linux 5.6+),
                            // but could be reworked to use UNIX domain sockets
                            // (with `SCM_RIGHTS` messages) instead, long-term.
                            use crate::os::linux::dma_buf::pid_fd;

                            let dma_buf_image = dma_buf_image.planes_map(|plane| plane.fd_map(|fd| {
                                // FIXME(eddyb) reuse `PidFd`s as the host PID
                                // will never change (unless it can do client
                                // handover, or there's "worker processes").
                                pid_fd::PidFd::from_remote_pid(fd.remote_pid as pid_fd::pid_t)
                                    .and_then(|pid_fd| pid_fd.clone_remote_fd(fd.remote_fd))
                            }));

                            // FIXME(eddyb) is this necessary or could they be reused?
                            let new_texture = Texture::new(self);

                            if let Err(err) = &dma_buf_image.planes.dma_buf_fd {
                                error!("failed to pidfd_getfd the DMA-BUF fd: {err:?}");
                                if err.kind() == io::ErrorKind::Unsupported {
                                    error!("pidfd_getfd syscall requires at least Linux 5.6")
                                }
                            } else {
                                let dma_buf_image = dma_buf_image.planes_map(|plane| plane.fd_map(Result::unwrap));

                                // update texture
                                self.textures[new_texture.texture_id()]
                                    .os.update_from_shared_dma_buf_image(
                                        self.os.opengl_cx.as_ref().unwrap(),
                                        &ws.swapchain,
                                        &dma_buf_image,
                                    );
                            }

                            new_texture
                        });
                        let swapchain = swapchain.insert(new_swapchain);

                        // reset present_index
                        present_index = 0;

                        self.stdin_handle_platform_ops(Some(swapchain), present_index);
                    }

                    HostToStdin::Tick {frame: _, time, buffer_id: _} => if swapchain.is_some() {

                        // poll the service for updates
                        // check signals
                        if Signal::check_and_clear_ui_signal(){
                            self.handle_media_signals();
                            self.call_event_handler(&Event::Signal);
                        }
                        if self.handle_live_edit(){
                            self.call_event_handler(&Event::LiveEdit);
                            self.redraw_all();
                        }
                        self.handle_networking_events();
                        
                        // alright a tick.
                        // we should now run all the stuff.
                        if self.new_next_frames.len() != 0 {
                            self.call_next_frame_event(time);
                        }
                        
                        if self.need_redrawing() {
                            self.call_draw_event();
                            self.opengl_compile_shaders();
                        }
                        
                        // we need to make this shared texture handle into a true metal one
                        self.stdin_handle_repaint(swapchain.as_ref(), &mut present_index);
                    }
                }
            }

            // we should poll our runloop
            self.stdin_handle_platform_ops(swapchain.as_ref(), present_index);
        }
    }
    
    
    fn stdin_handle_platform_ops(
        &mut self,
        swapchain: Option<&Swapchain<Texture>>,
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
                            clear_color: PassClearColor::ClearWith(vec4(1.0,1.0,0.0,1.0)),
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
