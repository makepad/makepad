use crate::{
    makepad_draw::*,
    makepad_widgets::*,
    makepad_platform::os::cx_stdin::*,
    build_manager::build_manager::BuildManager,
};

live_design!{
    import makepad_draw::shader::std::*;
    
    RunView = {{RunView}} {
        frame_delta: 0.008,
        draw_app: {
            texture tex: texture2d
            instance recompiling: 0.0
            fn pixel(self) -> vec4 {
                //return vec4(self.max_iter / 1000.0,0.0,0.0,1.0);
                let fb = sample2d_rt(self.tex, self.pos)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return mix(fb, #4, self.recompiling * 0.4);
            }
        }
        animator: {
            recompiling = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_app: {recompiling: 0.0}}
                }
                on = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_app: {recompiling: 1.0}}
                }
            }
        }
    }
}


#[derive(Live)]
pub struct RunView {
    #[walk] walk: Walk,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[animator] animator: Animator,
    #[live] draw_app: DrawQuad,
    #[live] frame_delta: f64,
    #[rust] last_size: (usize, usize),
    #[rust] tick: NextFrame,
    #[rust] timer: Timer,
    #[rust(100usize)] redraw_countdown: usize,
    #[rust] time: f64,
    #[rust] frame: u64
}


impl LiveHook for RunView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, RunView)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.tick = cx.new_next_frame(); //start_interval(self.frame_delta);
        self.time = 0.0;
        self.draw_app.set_texture(0, &cx.null_texture());
    }
}

impl RunView {
    
    pub fn run_tick(&mut self, cx: &mut Cx, time: f64, run_view_id: LiveId, manager: &mut BuildManager) {
        self.frame += 1;
        manager.send_host_to_stdin(run_view_id, HostToStdin::Tick {
            buffer_id: run_view_id.0,
            frame: 0,
            time: time
        });
        if self.redraw_countdown>0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
            self.tick = cx.new_next_frame();
        }
        else {
            self.timer = cx.start_timeout(0.008);
        }
    }
    
    pub fn pump_event_loop(&mut self, cx: &mut Cx, event: &Event, run_view_id: LiveId, manager: &mut BuildManager) {
        
        self.animator_handle_event(cx, event);
        if let Some(te) = self.timer.is_event(event) {
            self.run_tick(cx, te.time.unwrap_or(0.0), run_view_id, manager)
        }
        if let Some(te) = self.tick.is_event(event) {
            self.run_tick(cx, te.time, run_view_id, manager)
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, run_view_id: LiveId, manager: &mut BuildManager) {
        
        self.animator_handle_event(cx, event);
        
        // lets send mouse events
        match event.hits(cx, self.draw_app.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.draw_app.area());
            }
            Hit::KeyDown(e) => {
                manager.send_host_to_stdin(run_view_id, HostToStdin::KeyDown(e));
            }
            Hit::KeyUp(e) => {
                manager.send_host_to_stdin(run_view_id, HostToStdin::KeyUp(e));
            }
            _ => ()
        }
        let rect = self.draw_app.area().get_rect(cx);
        match event {
            Event::MouseDown(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(run_view_id, HostToStdin::MouseDown(StdinMouseDown {
                    time: e.time,
                    x: rel.x,
                    y: rel.y,
                    button: e.button,
                }));
            }
            Event::MouseMove(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(run_view_id, HostToStdin::MouseMove(StdinMouseMove {
                    time: e.time,
                    x: rel.x,
                    y: rel.y,
                }));
            }
            Event::MouseUp(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(run_view_id, HostToStdin::MouseUp(StdinMouseUp {
                    time: e.time,
                    button: e.button,
                    x: rel.x,
                    y: rel.y,
                }));
            }
            Event::Scroll(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(run_view_id, HostToStdin::Scroll(StdinScroll {
                    is_mouse: e.is_mouse,
                    time: e.time,
                    x: rel.x,
                    y: rel.y,
                    sx: e.scroll.x,
                    sy: e.scroll.y
                }));
            }
            _ => ()
        }
    }
    
    pub fn handle_stdin_to_host(&mut self, cx: &mut Cx, msg: &StdinToHost, run_view_id: LiveId, manager: &mut BuildManager) {
        match msg {
            
            StdinToHost::SetCursor(cursor) => {
                cx.set_cursor(*cursor)
            }
            StdinToHost::ReadyToStart => {
                self.animator_play(cx, id!(recompiling.off));
                // cause a resize event to fire
                self.last_size = Default::default();
                self.redraw(cx);
            }
            StdinToHost::DrawCompleteAndFlip(present_index) => {
                for v in manager.active.builds.values_mut() {
                    if v.run_view_id == run_view_id {
                        self.redraw_countdown = 20;
                        v.present_index.set(*present_index);
                        self.draw_app.set_texture(0, &v.swapchain[*present_index]);
                        v.mac_resize_id = 1 - present_index;
                    }
                }
            }
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_app.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, run_view_id: LiveId, manager: &mut BuildManager) {
        
        // alright so here we draw em texturezs
        // pick a texture off the buildstate
        let dpi_factor = cx.current_dpi_factor();
        let walk = if let Some(walk) = self.draw_state.get() {walk}else {panic!()};
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        // lets pixelsnap rect in position and size
        
        for v in manager.active.builds.values_mut() {
            if v.run_view_id == run_view_id {
                
                // update texture size and indicate new size to client if needed
                let new_size = ((rect.size.x * dpi_factor) as usize, (rect.size.y * dpi_factor) as usize);
                if new_size != self.last_size {
                    self.last_size = new_size;
                    self.redraw_countdown = 20;
                    // update descriptors for swapchain textures
                    let ids = [run_view_id.0 & 0x0000FFFFFFFFFFFF, run_view_id.0 | 0x0001000000000000];
                    let descs = [TextureDesc {
                        format: TextureFormat::SharedBGRA(ids[0]),
                        width: Some(new_size.0.max(1)),
                        height: Some(new_size.1.max(1)),
                    }, TextureDesc {
                        format: TextureFormat::SharedBGRA(ids[1]),
                        width: Some(new_size.0.max(1)),
                        height: Some(new_size.1.max(1)),
                    }];
                    
                    // make sure the actual shared texture resources exist, and get their handles                   
                    #[cfg(target_os = "windows")] {
                        v.swapchain[0].set_desc(cx, descs[0]);
                        v.swapchain[1].set_desc(cx, descs[1]);
                        
                        let mut handles = [0u64, 0u64];
                        
                        let d3d11_device = cx.cx.os.d3d11_device.replace(None).unwrap();
                        
                        let cxtexture = &mut cx.textures[v.swapchain[0].texture_id()];
                        cxtexture.os.update_shared_texture(&d3d11_device, new_size.0 as u32, new_size.1 as u32);
                        handles[0] = cxtexture.os.shared_handle.0 as u64;
                        
                        let cxtexture = &mut cx.textures[v.swapchain[1].texture_id()];
                        cxtexture.os.update_shared_texture(&d3d11_device, new_size.0 as u32, new_size.1 as u32);
                        handles[1] = cxtexture.os.shared_handle.0 as u64;
                        
                        cx.cx.os.d3d11_device.replace(Some(d3d11_device));
                        manager.send_host_to_stdin(run_view_id, HostToStdin::WindowSize(StdinWindowSize {
                            width: rect.size.x,
                            height: rect.size.y,
                            dpi_factor: dpi_factor,
                            swapchain_handles: handles,
                        }));
                    };
                    
                    #[cfg(target_os = "macos")] {
                        // macos alternates resizes with doublebuffering NOT frames
                        let id = v.mac_resize_id;
                        v.swapchain[id].set_desc(cx, descs[id]);
                        let metal_device = cx.cx.os.metal_device.replace(None).unwrap();
                        // ok so we have to alternate which buffer we resize
                        let cxtexture = &mut cx.textures[v.swapchain[id].texture_id()];
                        cxtexture.os.update_shared_texture(metal_device, &descs[id]);
                        
                        cx.cx.os.metal_device.replace(Some(metal_device));
                        // for macos, pass the hashmap indices to the client, so they can find the right texture in XPC
                        // send size update to client
                        let swapchain_front = v.mac_resize_id as u32;
                        manager.send_host_to_stdin(run_view_id, HostToStdin::WindowSize(StdinWindowSize {
                            width: rect.size.x,
                            height: rect.size.y,
                            dpi_factor: dpi_factor,
                            swapchain_handle: ids[id],
                            swapchain_front
                        }));
                    };
                    
                    #[cfg(target_os = "linux")]
                    {
                        v.swapchain[0].set_desc(cx, descs[0]);
                        v.swapchain[1].set_desc(cx, descs[1]);
                        
                        // HACK(eddyb) normally this would be triggered later,
                        // but we need it *before* `get_shared_texture_dma_buf_image`.
                        {
                            // FIXME(eddyb) there should probably be an unified EGL `OpenglCx`.
                            let cxtexture = &mut cx.cx.textures[v.swapchain[0].texture_id()];
                            #[cfg(not(any(linux_direct, target_os = "android")))]
                            cxtexture.os.update_shared_texture(cx.cx.os.opengl_cx.as_ref().unwrap(), &descs[0]);
                            
                            let cxtexture = &mut cx.cx.textures[v.swapchain[1].texture_id()];
                            #[cfg(not(any(linux_direct, target_os = "android")))]
                            cxtexture.os.update_shared_texture(cx.cx.os.opengl_cx.as_ref().unwrap(), &descs[1]);
                        }
                        
                        let handles = [
                            cx.get_shared_texture_dma_buf_image(&v.swapchain[0]),
                            cx.get_shared_texture_dma_buf_image(&v.swapchain[1]),
                        ];
                        
                        manager.send_host_to_stdin(run_view_id, HostToStdin::WindowSize(StdinWindowSize {
                            width: rect.size.x,
                            height: rect.size.y,
                            dpi_factor: dpi_factor,
                            swapchain_handles: handles,
                        }));
                    };
                    
                }
                
                // make sure it's going to present the right texture
                
                
                break
            }
        }
        
        self.draw_app.draw_abs(cx, rect);
    }
}

impl Widget for RunView {
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_app.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct RunViewRef(WidgetRef);

impl RunViewRef {
    
    pub fn recompile_started(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, id!(recompiling.on));
        }
    }
    
}
