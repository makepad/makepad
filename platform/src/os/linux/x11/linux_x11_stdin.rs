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
        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut reader = BufReader::new(std::io::stdin());

        let mut swapchain = None;
        let mut present_index = 0;

        self.call_event_handler(&Event::Construct);
        
        loop {
            let mut line = String::new();
            if let Ok(len) = reader.read_line(&mut line) {
                if len == 0 {
                    break
                }
                // alright lets put the line in a json parser
                let parsed: Result<HostToStdin, DeJsonErr> = DeJson::deserialize_json(&line);
                
                match parsed {
                    Ok(msg) => match msg {
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

                            let window = &mut self.windows[CxWindowPool::id_zero()];
                            window.window_geom = WindowGeom {
                                dpi_factor: ws.dpi_factor,
                                inner_size: dvec2(swapchain.width as f64, swapchain.height as f64) / ws.dpi_factor,
                                ..Default::default()
                            };

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
                    Err(err) => { // we should output a log string
                        error!("Cant parse stdin-JSON {} {:?}", line, err);
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
