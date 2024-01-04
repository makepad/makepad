use {
    std::{
        io,
        io::prelude::*,
        io::BufReader,
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        makepad_micro_serde::*,
        makepad_live_compiler::LiveFileChange,
        event::Event,
        window::CxWindowPool,
        event::WindowGeom,
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        os::cx_stdin::{aux_chan, HostToStdin, PresentableDraw, StdinToHost, Swapchain, PollTimer},
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
        for &pass_id in &passes_todo {
            match self.passes[pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    // only render to swapchain if swapchain exists
                    if let Some(swapchain) = swapchain {
                        let current_image = &swapchain.presentable_images[*present_index];
                        *present_index = (*present_index + 1) % swapchain.presentable_images.len();

                        // render to swapchain
                        self.draw_pass_to_texture(pass_id, &current_image.image);

                        // wait for GPU to finish rendering
                        unsafe { gl_sys::Finish(); }

                        let dpi_factor = self.passes[pass_id].dpi_factor.unwrap();
                        let pass_rect = self.get_pass_rect(pass_id, dpi_factor).unwrap();
                        let presentable_draw = PresentableDraw {
                            target_id: current_image.id,
                            width: (pass_rect.size.x * dpi_factor) as u32,
                            height: (pass_rect.size.y * dpi_factor) as u32,
                        };

                        // inform host that frame is ready
                        let _ = io::stdout().write_all(StdinToHost::DrawCompleteAndFlip(presentable_draw).to_json().as_bytes());
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_magic_texture(pass_id);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(pass_id);
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
            });
        }

        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

        let mut swapchain = None;
        let mut present_index = 0;

        self.call_event_handler(&Event::Startup);

        while let Ok(msg) = json_msg_rx.recv(){
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
                HostToStdin::TextInput(e) => {
                    self.call_event_handler(&Event::TextInput(e));
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
                HostToStdin::WindowGeomChange { dpi_factor, inner_width, inner_height } => {
                    self.windows[CxWindowPool::id_zero()].window_geom = WindowGeom {
                        dpi_factor,
                        inner_size: dvec2(inner_width, inner_height),
                        ..Default::default()
                    };
                    self.redraw_all();
                }
                HostToStdin::Swapchain(new_swapchain) => {
                    let new_swapchain = new_swapchain.images_map(|pi| {
                        let new_texture = Texture::new(self);
                        match pi.recv_fds_from_aux_chan(&aux_chan_client_endpoint) {
                            Ok(pi) => {
                                // update texture
                                let desc = TextureFormat::SharedBGRAu8{
                                    id: pi.id,
                                    width: new_swapchain.alloc_width as usize,
                                    height: new_swapchain.alloc_height as usize,
                                };
                                new_texture = Texture::new_with_format(self, desc);
                                self.textures[new_texture.texture_id()]
                                .update_from_shared_dma_buf_image(
                                    self.os.opengl_cx.as_ref().unwrap(),
                                    &pi.image,
                                );
                            }
                            Err(err) => {
                                crate::error!("failed to receive new swapchain on auxiliary channel: {err:?}");
                            }
                        }
                        new_texture
                    });
                    let swapchain = swapchain.insert(new_swapchain);

                    // reset present_index
                    present_index = 0;

                    self.redraw_all();
                    self.stdin_handle_platform_ops(Some(swapchain), present_index);
                }

                HostToStdin::Tick {frame: _, time, buffer_id: _} => if swapchain.is_some() {

                    // poll the service for updates
                    // check signals
                    if SignalToUI::check_and_clear_ui_signal(){
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    for event in self.os.stdin_timers.get_dispatch() {
                        self.call_event_handler(&event);
                    }                    
                    if self.handle_live_edit(){
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
                        self.opengl_compile_shaders();
                    }

                    self.stdin_handle_repaint(swapchain.as_ref(), &mut present_index);
                }
            }
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
                            texture: swapchain.presentable_images[present_index].image.clone(),
                        }];
                    }
                },
                CxOsOp::SetCursor(cursor) => {
                    let _ = io::stdout().write_all(StdinToHost::SetCursor(cursor).to_json().as_bytes());
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    self.os.stdin_timers.timers.insert(timer_id, PollTimer::new(interval, repeats));
                },
                CxOsOp::StopTimer(timer_id) => {
                    self.os.stdin_timers.timers.remove(&timer_id);
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
