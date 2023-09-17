use {
    std::{
        io,
        io::prelude::*,
        io::{BufReader},
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        makepad_error_log::*,
        makepad_micro_serde::*,
        event::Event,
        window::CxWindowPool,
        event::WindowGeom,
        texture::Texture,
        live_traits::LiveNew,
        thread::Signal,
        os::{
            cx_stdin::{HostToStdin, StdinToHost},
        },
        pass::{CxPassParent, PassClearColor, CxPassColorTexture},
        cx_api::{CxOsOp},
        cx::{Cx},
        gl_sys,
    } 
};

impl Cx {
    
    pub (crate) fn stdin_handle_repaint(&mut self, fb_texture: &Texture) {
        self.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    self.draw_pass_to_texture(*pass_id, fb_texture);
                    unsafe { gl_sys::Finish(); }
                    let _ = io::stdout().write_all(StdinToHost::DrawComplete.to_json().as_bytes());
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
        let fb_texture = Texture::new(self);

        let mut reader = BufReader::new(std::io::stdin());
        let mut window_size = None;
        
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
                        HostToStdin::ReloadFile{file:_, contents:_}=>{
                            // alright lets reload this file in our DSL system
                            
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
                            if window_size != Some(ws) {
                                let [dma_buf_image] = ws.swapchain_handles;

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
                                let dma_buf_image = dma_buf_image.planes_ref_map(|plane| plane.fd_map(|fd| {
                                    // FIXME(eddyb) reuse `PidFd`s as the host PID
                                    // will never change (unless it can do client
                                    // handover, or there's "worker processes").
                                    pid_fd::PidFd::from_remote_pid(fd.remote_pid as pid_fd::pid_t)
                                        .and_then(|pid_fd| pid_fd.clone_remote_fd(fd.remote_fd))
                                }));
                                if let Err(err) = &dma_buf_image.planes[0].dma_buf_fd {
                                    error!("failed to pidfd_getfd the DMA-BUF fd: {err:?}");
                                    if err.kind() == io::ErrorKind::Unsupported {
                                        error!("pidfd_getfd syscall requires at least Linux 5.6")
                                    }
                                } else {
                                    let dma_buf_image = dma_buf_image.planes_map(|plane| plane.fd_map(Result::unwrap));

                                    // update texture
                                    let cxtexture = &mut self.textures[fb_texture.texture_id()];
                                    cxtexture.os.update_from_shared_dma_buf_image(
                                        self.os.opengl_cx.as_ref().unwrap(),
                                        &dma_buf_image,
                                    );
                                }

                                window_size = Some(ws);
                                self.redraw_all();

                                let window = &mut self.windows[CxWindowPool::id_zero()];
                                window.window_geom = WindowGeom {
                                    dpi_factor: ws.dpi_factor,
                                    inner_size: dvec2(ws.width, ws.height),
                                    ..Default::default()
                                };
                                self.stdin_handle_platform_ops(&fb_texture);
                            }
                        }
                        HostToStdin::Tick {frame: _, time, buffer_id: _} => if let Some(_ws) = window_size {
                            // poll the service for updates
                            // check signals
                            if Signal::check_and_clear_ui_signal(){
                                self.handle_media_signals();
                                self.call_event_handler(&Event::Signal);
                            }
                            if self.was_live_edit(){
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
                            self.stdin_handle_repaint(&fb_texture);
                        }
                        _ => {}
                    }
                    Err(err) => { // we should output a log string
                        error!("Cant parse stdin-JSON {} {:?}", line, err);
                    }
                }
            }
            // we should poll our runloop
            self.stdin_handle_platform_ops(&fb_texture);
        }
    }
    
    
    fn stdin_handle_platform_ops(&mut self, main_texture: &Texture) {
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
                    pass.color_textures = vec![CxPassColorTexture {
                        clear_color: PassClearColor::ClearWith(vec4(1.0,1.0,0.0,1.0)),
                        //clear_color: PassClearColor::ClearWith(pass.clear_color),
                        texture_id: main_texture.texture_id()
                    }];
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

/// Linux pidfd (file-descriptor-based API for "process handles") wrapper.
///
/// `PidFd` is used here specifically for its ability to clone any file descriptors
/// from a remote process, similar to opening `/proc/$REMOTE_PID/fd/$REMOTE_FD`,
/// but without all the caveats and failure modes around special file descriptors.
//
// FIXME(eddyb) `std::os::linux::process::PidFd` should be used/wrapped instead,
// but for now it's still unstable, and it also lacks `pidfd_getfd` functionality,
// as its main purpose appears to be creating a pidfd from `std::process::Command`.
#[allow(non_camel_case_types, non_upper_case_globals)]
mod pid_fd {
    use std::{
        ffi::{c_int, c_long, c_uint},
        io,
        os::{self, fd::{AsRawFd as _, FromRawFd as _}},
    };

    pub(super) type pid_t = c_int;

    extern "C" { fn syscall(num: c_long, ...) -> c_long; }
    const SYS_pidfd_open: c_long = 434;
    const SYS_pidfd_getfd: c_long = 438;

    pub(super) struct PidFd(os::fd::OwnedFd);
    impl PidFd {
        pub fn from_remote_pid(remote_pid: pid_t) -> Result<PidFd, io::Error> {
            unsafe {
                let flags: c_uint = 0;
                let pid_fd = syscall(SYS_pidfd_open, remote_pid, flags);
                if pid_fd == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(PidFd(os::fd::OwnedFd::from_raw_fd(pid_fd as os::fd::RawFd)))
                }
            }
        }
        pub fn clone_remote_fd(&self, remote_fd: os::fd::RawFd) -> Result<os::fd::OwnedFd, io::Error> {
            unsafe {
                let flags: c_uint = 0;
                let cloned_fd = syscall(SYS_pidfd_getfd, self.0.as_raw_fd(), remote_fd, flags);
                if cloned_fd == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(os::fd::OwnedFd::from_raw_fd(cloned_fd as os::fd::RawFd))
                }
            }
        }
    }
}
