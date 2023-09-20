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
            instance started: 0.0
            fn pixel(self) -> vec4 {
                //return vec4(self.max_iter / 1000.0,0.0,0.0,1.0);
                let fb = sample2d_rt(self.tex, self.pos)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return mix(#2,mix(fb, #4, self.recompiling * 0.4),self.started);
            }
        }
        animator: {
            started = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {started: 0.0}}
                }
                on = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {started: 1.0}}
                }
            }
            recompiling = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {draw_app: {recompiling: 0.0}}
                }
                on = {
                    from: {all: Forward {duration: 0.05}}
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
    #[rust] last_size: (u32, u32),
    #[rust] tick: NextFrame,
    #[rust] timer: Timer,
    #[rust(100usize)] redraw_countdown: usize,
    #[rust] time: f64,
    #[rust] frame: u64,
    #[rust] started: bool
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
            &StdinToHost::DrawCompleteAndFlip(drawn_presentable_id) => {
                if let Some(v) = manager.active.builds.values_mut().find(|v| v.run_view_id == run_view_id) {
                    for swapchain in &v.swapchain_history{
                        if let Some(swapchain) = swapchain {
                            if let Some(pi) = swapchain.presentable_images.iter().find(|pi| pi.id == drawn_presentable_id) {
                                if !self.started{
                                    self.started = true;
                                    self.animator_play(cx, id!(started.on));
                                }
                                self.redraw_countdown = 20;
                                v.last_presented_id.set(Some(pi.id));
                                self.draw_app.set_texture(0, &pi.image);
                            }
                        }
                    }
                    //}
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

        if let Some(v) = manager.active.builds.values_mut().find(|v| v.run_view_id == run_view_id) {
            let new_size = (
                ((rect.size.x * dpi_factor) as u32).max(1),
                ((rect.size.y * dpi_factor) as u32).max(1),
            );
            if new_size != self.last_size {
                self.last_size = new_size;
                self.redraw_countdown = 20;


                // lets make a new swapchain
                let mut swapchain = Swapchain::new(0, 0).images_map(|_, ()| Texture::new(cx));
                // Resize the swapchain and prepare its images for sharing.
                (swapchain.width, swapchain.height) = new_size;
                let shared_swapchain = swapchain.images_as_ref().images_map(|id, texture| {
                    texture.set_desc(cx, TextureDesc {
                        format: TextureFormat::SharedBGRA(id),
                        width: Some(swapchain.width as usize),
                        height: Some(swapchain.height as usize),
                    });
                    cx.get_shared_presentable_image_os_handle(&texture)
                });
                for i in 0..v.swapchain_history.len()-1{
                    v.swapchain_history[i] = v.swapchain_history[i+1].take();
                }
                v.swapchain_history[v.swapchain_history.len()-1] = Some(swapchain);

                manager.send_host_to_stdin(run_view_id, HostToStdin::WindowSize(StdinWindowSize {
                    dpi_factor: dpi_factor,
                    swapchain: shared_swapchain,
                }));
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
