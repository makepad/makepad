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
            cx_stdin::{HostToStdin, StdinToHost},
        },
        pass::{CxPassParent, PassClearColor, CxPassColorTexture},
        cx_api::CxOsOp,
        cx::Cx,
        windows::Win32::Foundation::HANDLE,
    } 
};

impl Cx {
    
    pub (crate) fn stdin_send_draw_complete_and_flip(&mut self) {

        let _ = io::stdout().write_all(StdinToHost::DrawCompleteAndFlip(self.os.present_index).to_json().as_bytes());
        
        /*
        if let Some(swapchain) = self.os.swapchain.as_ref() {
            if self.os.present_index < swapchain.len() - 1 {
                self.os.present_index += 1;
            }
            else {
                self.os.present_index = 0;
            }    
        }*/

        //self.os.present_index = 1 - self.os.present_index;
    }

    pub (crate) fn stdin_handle_repaint(&mut self, d3d11_cx: &mut D3d11Cx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_) => {

                    // only render to swapchain if swapchain exists
                    if self.os.swapchain.is_some() {

                        // and if GPU is not already rendering something else
                        if !self.os.new_frame_being_rendered {

                            // render
                            let texture_id = self.os.swapchain.as_ref().unwrap()[self.os.present_index].texture_id();
                            self.draw_pass_to_texture(*pass_id, d3d11_cx, texture_id);

                            // start GPU event query
                            d3d11_cx.start_querying();

                            // and inform event_loop to go poll GPU readiness
                            self.os.new_frame_being_rendered = true;
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
        let _ = io::stdout().write_all(StdinToHost::ReadyToStart.to_json().as_bytes());

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

                            // first update the swapchain
                            if self.os.swapchain_handles[0].0 as u64 != ws.swapchain_handles[0] {

                                // we got a new texture handles
                                let textures = [Texture::new(self),Texture::new(self),];

                                let handle = HANDLE(ws.swapchain_handles[0] as isize);
                                let cxtexture = &mut self.textures[textures[0].texture_id()];
                                cxtexture.os.update_from_shared_handle(d3d11_cx,handle);
                                self.os.swapchain_handles[0] = handle;

                                let handle = HANDLE(ws.swapchain_handles[1] as isize);
                                let cxtexture = &mut self.textures[textures[1].texture_id()];
                                cxtexture.os.update_from_shared_handle(d3d11_cx,handle);
                                self.os.swapchain_handles[1] = handle;

                                self.os.swapchain = Some(textures);

                                // and reset present_index
                                self.os.present_index = 0;
                            }

                            // redraw the window if needed
                            if window_size != Some(ws) {
                                window_size = Some(ws);
                                self.redraw_all();
                                
                                let window = &mut self.windows[CxWindowPool::id_zero()];
                                window.window_geom = WindowGeom {
                                    dpi_factor: ws.dpi_factor,
                                    inner_size: dvec2(ws.width, ws.height),
                                    ..Default::default()
                                };

                                self.stdin_handle_platform_ops(d3d11_cx);
                            }
                        }
                        HostToStdin::Tick {frame: _, time,..} => if let Some(_ws) = window_size {

                            // poll the service for updates
                            // check signals
                            if Signal::check_and_clear_ui_signal(){
                                //self.handle_media_signals();
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
                                self.hlsl_compile_shaders(d3d11_cx);
                            }
                            
                            // repaint
                            self.stdin_handle_repaint(d3d11_cx);
                        }
                    }
                    Err(err) => { // we should output a log string
                        error!("Cant parse stdin-JSON {} {:?}", line, err);
                    }
                }
            }
            // we should poll our runloop
            self.stdin_handle_platform_ops(d3d11_cx);

            // check if GPU is ready to flip frames
            if self.os.new_frame_being_rendered {
                if d3d11_cx.is_gpu_done() {
                    self.stdin_send_draw_complete_and_flip();
                    self.os.new_frame_being_rendered = false;
                }
            }
        }
    }
        
    fn stdin_handle_platform_ops(&mut self, _metal_cx: &D3d11Cx) {
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
                    if self.os.swapchain.is_some() {
                        let texture_id = self.os.swapchain.as_ref().unwrap()[self.os.present_index].texture_id();
                        pass.color_textures = vec![CxPassColorTexture {
                            clear_color: PassClearColor::ClearWith(vec4(1.0,1.0,0.0,1.0)),
                            //clear_color: PassClearColor::ClearWith(pass.clear_color),
                            texture_id,
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
