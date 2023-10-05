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
            instance tex_scale: vec2(0.0, 0.0),
            instance tex_size: vec2(0.0, 0.0),
            fn pixel(self) -> vec4 {
                //return sample2d_rt(self.tex, self.pos * self.tex_scale);
                let tp1 = sample2d_rt(self.tex, vec2(0.0,0.0))
                let tp2 = sample2d_rt(self.tex, vec2(1.0/self.tex_size.x,0.0));
                let tp = vec2(tp1.r*65280.0 + tp1.b*255.0,tp2.r*65280.0 + tp2.b*255.0);

                let tex_scale = tp / self.tex_size;
                let fb = sample2d_rt(self.tex, self.pos * tex_scale)
                if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                    return #2
                }
                return mix(fb, #4, self.recompiling * 0.4);
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
    #[rust] last_size: DVec2,
    #[rust] tick: NextFrame,
    #[rust] timer: Timer,
    #[rust(100usize)] redraw_countdown: usize,
    #[rust] time: f64,
    #[rust] frame: u64,
    #[rust] started: bool,
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
    
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc{..} = from{
            self.last_size = dvec2(0.0,0.0);
            self.animator_cut(cx, id!(started.on));
        }
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
            StdinToHost::DrawCompleteAndFlip(presentable_draw) => {
                if let Some(v) = manager.active.builds.values_mut().find(|v| v.run_view_id == run_view_id) {
                    // Only allow presenting images in the current host swapchain
                    // (or the previous one, before any draws on the current one),
                    // and look them up by their unique IDs, to avoid rendering
                    // different textures than the ones the client just drew to.
                    let mut try_present_through = |swapchain: &Option<Swapchain<Texture>>| {
                        let swapchain = swapchain.as_ref()?;
                        let drawn = swapchain.get_image(presentable_draw.target_id)?;

                        self.draw_app.set_texture(0, &drawn.image);
                        self.draw_app.draw_vars.set_var_instance(cx, id!(tex_scale), &[
                            (presentable_draw.width as f32) / (swapchain.alloc_width as f32),
                            (presentable_draw.height as f32) / (swapchain.alloc_height as f32),
                        ]);
                        self.draw_app.draw_vars.set_var_instance(cx, id!(tex_size), &[
                            (swapchain.alloc_width as f32),
                            (swapchain.alloc_height as f32),
                        ]);

                        if !self.started {
                            self.started = true;
                            self.animator_play(cx, id!(started.on));
                        }
                        self.redraw_countdown = 20;
                       
                        Some(())
                    };

                    if try_present_through(&v.swapchain).is_some() {
                        // The client is now drawing to the current swapchain,
                        // we can discard any previous one we were stashing.
                        v.last_swapchain_with_completed_draws = None;
                    } else {
                        // New draws to a previous swapchain are fine, just means
                        // the client hasn't yet drawn on the current swapchain,
                        // what lets us accept draws is their target `Texture`s.
                        try_present_through(&v.last_swapchain_with_completed_draws);
                    }
                }
            }
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_app.redraw(cx);
    }
    
    
    pub fn resend_framebuffer(&mut self, _cx: &mut Cx) {
        self.last_size = dvec2(0.0,0.0);
    }
    
    
    pub fn draw(&mut self, cx: &mut Cx2d, run_view_id: LiveId, manager: &mut BuildManager) {
        
        // alright so here we draw em texturezs
        // pick a texture off the buildstate
        let dpi_factor = cx.current_dpi_factor();
        let walk = if let Some(walk) = self.draw_state.get() {walk}else {panic!()};
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        // lets pixelsnap rect in position and size

        if self.last_size != rect.size {
            self.last_size = rect.size;
            self.redraw_countdown = 20;
            // FIXME(eddyb) there's no type or naming scheme that tells apart
            // DPI-scaled and non-DPI-scaled values (other than float-vs-int).
            let DVec2 { x: inner_width, y: inner_height } = self.last_size;

            // Try to only send the new geometry information to the client
            // most of the time, letting it draw on its existing swapchain,
            // and only replace the swapchain when a larger one is needed.
            manager.send_host_to_stdin(run_view_id, HostToStdin::WindowGeomChange {
                dpi_factor,
                inner_width,
                inner_height,
            });

            let min_width = ((inner_width * dpi_factor).ceil() as u32).max(1);
            let min_height = ((inner_height * dpi_factor).ceil() as u32).max(1);
            let active_build_needs_new_swapchain = manager.active.builds.values_mut()
                .find(|v| v.run_view_id == run_view_id)
                .filter(|v| v.aux_chan_host_endpoint.is_some())
                .filter(|v| {
                    v.swapchain.as_ref().map(|swapchain| {
                        min_width > swapchain.alloc_width || min_height > swapchain.alloc_height
                    }).unwrap_or(true)
                });
            if let Some(v) = active_build_needs_new_swapchain {
                // HACK(eddyb) there is no check that there were any draws on
                // the current swapchain, but the absence of an older swapchain
                // (i.e. `last_swapchain_with_completed_draws`) implies either
                // zero draws so far, or a draw to the current one discarded it.
                if v.last_swapchain_with_completed_draws.is_none() {
                    v.last_swapchain_with_completed_draws = v.swapchain.take();
                }

                // `Texture`s can be reused, but all `PresentableImageId`s must
                // be regenerated, to tell apart swapchains when e.g. resizing
                // constantly, so textures keep getting created and replaced.
                if let Some(swapchain) = &mut v.swapchain {
                    for pi in &mut swapchain.presentable_images {
                        pi.id = cx_stdin::PresentableImageId::alloc();
                    }
                }
                let swapchain = v.swapchain.get_or_insert_with(|| {
                    Swapchain::new(0, 0).images_map(|_| Texture::new(cx))
                });

                // Update the swapchain allocated size, rounding it up to
                // reduce the need for further swapchain recreation.
                swapchain.alloc_width = min_width.max(64).next_power_of_two();
                swapchain.alloc_height = min_height.max(64).next_power_of_two();

                // Prepare a version of the swapchain for cross-process sharing.
                let shared_swapchain = swapchain.images_as_ref().images_map(|pi| {
                    pi.image.set_desc(cx, TextureDesc {
                        format: TextureFormat::SharedBGRA(pi.id),
                        width: Some(swapchain.alloc_width as usize),
                        height: Some(swapchain.alloc_height as usize),
                    });
                    cx.share_texture_for_presentable_image(&pi.image)
                });

                let shared_swapchain =  {
                    // FIMXE(eddyb) this could be platform-agnostic if the serializer
                    // could drive the out-of-band UNIX domain socket messaging itself.
                    #[cfg(target_os = "linux")] {
                        shared_swapchain.images_map(|pi| {
                            pi.send_fds_to_aux_chan(v.aux_chan_host_endpoint.as_ref().unwrap())
                                .map(|pi| pi.image)
                        })
                    }
                    #[cfg(not(target_os = "linux"))] {
                        shared_swapchain.images_map(|pi| std::io::Result::Ok(pi.image))
                    }
                };

                let mut any_errors = false;
                for pi in &shared_swapchain.presentable_images {
                    if let Err(err) = &pi.image {
                        // FIXME(eddyb) is this recoverable or should the whole
                        // client be restarted? desyncs can get really bad...
                        error!("failed to send swapchain image to client: {:?}", err);
                        any_errors = true;
                    }
                }

                if !any_errors {
                    // Inform the client about the new swapchain it *should* use
                    // (using older swapchains isn't an error, but the draw calls
                    // will either be noops, or write to orphaned GPU memory ranges).
                    manager.send_host_to_stdin(run_view_id, HostToStdin::Swapchain(
                        shared_swapchain.images_map(|pi| pi.image.unwrap()),
                    ));
                }
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
