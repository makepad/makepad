use crate::{
    app::{AppData},
    makepad_widgets::*,
    makepad_platform::os::cx_stdin::*,
    build_manager::build_manager::BuildManager,
};

live_design!{
    import makepad_draw::shader::std::*;
    
    RunView = {{RunView}} {
        draw_app: {
            texture tex: texture2d
            instance recompiling: 0.0
            instance started: 0.0
            instance tex_scale: vec2(0.0, 0.0),
            instance tex_size: vec2(0.0, 0.0),
            fn pixel(self) -> vec4 {
                //return sample2d_rt(self.tex, self.pos * self.tex_scale);
                let tp1 = sample2d_rt(self.tex, vec2(0.5/self.tex_size.x,0.5/self.tex_size.y))
                let tp2 = sample2d_rt(self.tex, vec2(1.5/self.tex_size.x,0.5/self.tex_size.y));
                let tp = vec2(tp1.r*65280.0 + tp1.b*255.0,tp2.r*65280.0 + tp2.b*255.0);
                // ok so we should be having the same size in self.pos
                let counter = (self.rect_size * self.dpi_factor) / tp;
                let tex_scale = tp / self.tex_size;
                let fb = sample2d_rt(self.tex, self.pos * tex_scale * counter)
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
 

#[derive(Live, Widget)]
pub struct RunView {
    #[walk] walk: Walk,
    #[animator] animator: Animator,
    #[redraw] #[live] draw_app: DrawQuad,
    //#[live] frame_delta: f64,
    #[rust] last_rect: Rect,
    #[rust(100usize)] redraw_countdown: usize,
   // #[rust] time: f64,
   // #[rust] frame: u64,
    #[rust] started: bool,
    #[rust] pub build_id: LiveId,
    #[rust] pub window_id: usize,
    #[rust(WindowKindId::Main)] pub kind_id: WindowKindId,
}

impl LiveHook for RunView {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        //self.tick = cx.new_next_frame(); //start_interval(self.frame_delta);
        //self.time = 0.0;
        self.draw_app.set_texture(0, &cx.null_texture());
    }
    
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc{..} = apply.from{
            self.last_rect = Default::default();
            self.animator_cut(cx, id!(started.on));
        }
    }
}

impl RunView {
    /*
    pub fn run_tick(&mut self, cx: &mut Cx, manager: &mut BuildManager) {
        self.frame += 1;
        
        manager.send_host_to_stdin(self.build_id, HostToStdin::PollSwapChain{window_id: self.window_id});
        
        if self.window_id == 0{
            manager.send_host_to_stdin(self.build_id, HostToStdin::Tick);
        }
            
        if self.redraw_countdown>0 {
            self.redraw_countdown -= 1;
            self.redraw(cx);
            self.tick = cx.new_next_frame();
        }
        else {
            self.timer = cx.start_timeout(0.008);
        }
    }*/
    /*
    pub fn pump_event_loop(&mut self, cx: &mut Cx, event: &Event, run_view_id: LiveId, manager: &mut BuildManager) {
        let run_view_id = run_view_id.sub(self.window_id as u64);
        if let Some(_) = self.timer.is_event(event) {
            self.run_tick(cx, run_view_id, manager)
        }
        if let Some(_) = self.tick.is_event(event) {
            self.run_tick(cx, run_view_id, manager)
        }
    }*/
    
    pub fn draw_complete_and_flip(&mut self, cx: &mut Cx, presentable_draw: &PresentableDraw, manager: &mut BuildManager){
        let window_id = self.window_id;
        if let Some(v) = manager.active.builds.get_mut(&self.build_id){
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
                self.redraw(cx);
                Some(())
            };
                    
            if try_present_through(&v.swapchain_mut(window_id)).is_some() {
                // The client is now drawing to the current swapchain,
                // we can discard any previous one we were stashing.
                *v.last_swapchain_with_completed_draws_mut(window_id) = None;
            } else {
                // New draws to a previous swapchain are fine, just means
                // the client hasn't yet drawn on the current swapchain,
                // what lets us accept draws is their target `Texture`s.
                try_present_through(&v.last_swapchain_with_completed_draws_mut(window_id));
            }
        }
    }
    
    pub fn ready_to_start(&mut self, cx: &mut Cx){
        self.animator_play(cx, id!(recompiling.off));
        // cause a resize event to fire
        self.last_rect = Default::default();
        self.redraw(cx);
    }
    
    pub fn recompile_started(&mut self, cx: &mut Cx) {
        self.animator_play(cx, id!(recompiling.on));
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_app.redraw(cx);
    }
    
    
    pub fn resend_framebuffer(&mut self, _cx: &mut Cx) {
        self.last_rect = Default::default();
    }
    
    pub fn draw_run_view(&mut self, cx: &mut Cx2d, run_view_id: LiveId, manager: &mut BuildManager, walk:Walk) {
        // alright so here we draw em texturezs
        // pick a texture off the buildstate
        let dpi_factor = cx.current_dpi_factor();
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        // lets pixelsnap rect in position and size
        if self.redraw_countdown > 0{
            self.redraw_countdown -= 1;
            self.redraw(cx);
        }
        if self.last_rect != rect{
            manager.send_host_to_stdin(run_view_id, HostToStdin::WindowGeomChange {
                window_id: self.window_id,
                dpi_factor,
                left: rect.pos.x,
                top: rect.pos.y,
                width: rect.size.x,
                height: rect.size.y,
            });
        }
        if self.last_rect.size != rect.size {

            let min_width = ((rect.size.x * dpi_factor).ceil() as u32).max(1);
            let min_height = ((rect.size.y * dpi_factor).ceil() as u32).max(1);
            
            let active_build_needs_new_swapchain = manager.active.builds
                .get_mut(&run_view_id)
                .filter(|v| {
                    v.aux_chan_host_endpoint.is_some()
                })
                .filter(|v| {
                    v.swapchain(self.window_id).map(|swapchain| {
                        min_width > swapchain.alloc_width || min_height > swapchain.alloc_height
                    }).unwrap_or(true)
                });
                                
            if let Some(v) = active_build_needs_new_swapchain {
                
                // HACK(eddyb) there is no check that there were any draws on
                // the current swapchain, but the absence of an older swapchain
                // (i.e. `last_swapchain_with_completed_draws`) implies either
                // zero draws so far, or a draw to the current one discarded it.
                if v.last_swapchain_with_completed_draws(self.window_id).is_none() {
                    let chain = v.swapchain_mut(self.window_id).take();
                    *v.last_swapchain_with_completed_draws_mut(self.window_id) = chain;
                }

                // `Texture`s can be reused, but all `PresentableImageId`s must
                // be regenerated, to tell apart swapchains when e.g. resizing
                // constantly, so textures keep getting created and replaced.
                if let Some(swapchain) = v.swapchain_mut(self.window_id) {
                    for pi in &mut swapchain.presentable_images {
                        pi.id = cx_stdin::PresentableImageId::alloc();
                    }
                }

                // Update the swapchain allocated size, rounding it up to
                // reduce the need for further swapchain recreation.
                let alloc_width = min_width.max(64).next_power_of_two();
                let alloc_height = min_height.max(64).next_power_of_two();
                
                let swapchain = v.swapchain_mut(self.window_id).get_or_insert_with(|| {
                    Swapchain::new(self.window_id, alloc_width, alloc_height).images_map(|pi| {
                        // Prepare a version of the swapchain for cross-process sharing.
                        Texture::new_with_format(cx, TextureFormat::SharedBGRAu8 {
                            id: pi.id,
                            width: alloc_width as usize,
                            height: alloc_height as usize,
                            initial: true,
                        })
                    })
                });
                
                let shared_swapchain = swapchain.images_as_ref().images_map(|pi| {
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
        self.last_rect = rect;
        self.draw_app.draw_abs(cx, rect);
        // lets store the area 
        if let Some(ab) = manager.active.builds.get_mut(&run_view_id){
            ab.app_area.insert(self.window_id, self.draw_app.area());
        }
        
    }
}

impl Widget for RunView {

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let run_view_id = scope.path.last().sub(self.window_id as u64);
        let manager = &mut scope.data.get_mut::<AppData>().unwrap().build_manager;
        self.draw_run_view(cx, run_view_id, manager, walk);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let run_view_id = scope.path.last().sub(self.window_id as u64);
        let manager = &scope.data.get::<AppData>().unwrap().build_manager;
        
        self.animator_handle_event(cx, event);
        // lets send mouse events
        match event.hits(cx, self.draw_app.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.draw_app.area());
            }
            Hit::TextInput(e) => {
                manager.send_host_to_stdin(run_view_id, HostToStdin::TextInput(e));
            }
            Hit::KeyDown(e) => {
                manager.send_host_to_stdin(run_view_id, HostToStdin::KeyDown(e));
            }
            Hit::KeyUp(e) => {
                manager.send_host_to_stdin(run_view_id, HostToStdin::KeyUp(e));
            }
            _ => ()
        }

    }
    
}
