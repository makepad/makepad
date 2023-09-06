use crate::{
    makepad_draw::*,
    makepad_widgets::*,
    makepad_platform::os::cx_stdin::*,
    build_manager::{
        build_manager::BuildManager,
        build_protocol::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawApp = {{DrawApp}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
            //return vec4(self.max_iter / 1000.0,0.0,0.0,1.0);
            let fb = sample2d_rt(self.tex, self.pos)
            if fb.r == 1.0 && fb.g == 0.0 && fb.b == 1.0 {
                return #4
            }
            return fb;
        }
    }
    
    
    RunView = {{RunView}} {
        frame_delta: 0.008
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawApp {
    #[deref] draw_super: DrawQuad,
}

#[derive(Live)]
pub struct RunView {
    #[walk] walk: Walk,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[live] draw_bg: DrawApp,
    #[animator] animator: Animator,
    #[live] frame_delta: f64,
    #[rust] last_size: (usize, usize),
    #[rust] tick: Timer,
    #[rust] time: f64,
    #[rust] frame: u64
}


impl LiveHook for RunView {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, RunView)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.tick = cx.start_interval(self.frame_delta);
        self.time = 0.0;
    }
}

impl RunView {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, manager: &mut BuildManager) {

        self.animator_handle_event(cx, event);
        if self.tick.is_event(event) {
            self.time += self.frame_delta;
            self.frame += 1;
            
            // ugly hack but whatever for now.
            #[cfg(target_os = "windows")]
            for client in &manager.clients {
                for process in client.processes.values() {
                    let handle = cx.get_shared_handle(&process.texture);

                    let marshalled_handle = handle.0 as u64;  // hack: unsure if HANDLE is supported in microserde yet, so convert to u64
                    manager.send_host_to_stdin(None, HostToStdin::Dx11SharedHandle(marshalled_handle));
                }
            }
            
            // what shall we do, a timer? or do we do a next-frame
            manager.send_host_to_stdin(None, HostToStdin::Tick {
                frame: self.frame,
                time: self.time
            })
        }
        // lets send mouse events
        let rect = self.draw_bg.area().get_rect(cx);
        match event {
            Event::MouseDown(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(None, HostToStdin::MouseDown(StdinMouseDown {
                    time: e.time,
                    x: rel.x,
                    y: rel.y,
                    button: e.button,
                }));
            }
            Event::MouseMove(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(None, HostToStdin::MouseMove(StdinMouseMove {
                    time: e.time,
                    x: rel.x,
                    y: rel.y,
                }));
            }
            Event::MouseUp(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(None, HostToStdin::MouseUp(StdinMouseUp {
                    time: e.time,
                    button: e.button,
                    x: rel.x,
                    y: rel.y,
                }));
            }
            Event::Scroll(e) => {
                let rel = e.abs - rect.pos;
                manager.send_host_to_stdin(None, HostToStdin::Scroll(StdinScroll {
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
    
    pub fn handle_stdin_to_host(&mut self, cx: &mut Cx, _cmd_id: BuildCmdId, msg: StdinToHost, _manager: &mut BuildManager) {
        match msg {
            StdinToHost::SetCursor(cursor) => {
                cx.set_cursor(cursor)
            }
            StdinToHost::ReadyToStart => {
                // cause a resize event to fire
                self.last_size = Default::default();
                self.redraw(cx);
            }
            StdinToHost::DrawComplete => {
                self.draw_bg.redraw(cx);
            }
        }
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.area().redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, manager: &BuildManager) {
        
        // alright so here we draw em texturezs
        // pick a texture off the buildstate
        let dpi_factor = cx.current_dpi_factor();
        let walk = if let Some(walk) = self.draw_state.get() {walk}else {panic!()};
        let rect = cx.walk_turtle(walk).dpi_snap(dpi_factor);
        // lets pixelsnap rect in position and size
        for client in &manager.clients {
            for process in client.processes.values() {
                
                let new_size = ((rect.size.x * dpi_factor) as usize, (rect.size.y * dpi_factor) as usize);
                if new_size != self.last_size {
                    self.last_size = new_size;

                    println!("host: run_view::draw: process.texture {}x{}",new_size.0,new_size.1);

                    process.texture.set_desc(cx, TextureDesc {
                        format: TextureFormat::SharedBGRA(0),
                        width: Some(new_size.0.max(1)),
                        height: Some(new_size.1.max(1)),
                    });

                    manager.send_host_to_stdin(Some(process.cmd_id), HostToStdin::WindowSize(StdinWindowSize {
                        width: rect.size.x,
                        height: rect.size.y,
                        dpi_factor: dpi_factor,
                    }));
                }
                
                self.draw_bg.set_texture(0, &process.texture);
                
                break
            }
        }
        self.draw_bg.draw_abs(cx, rect);
    }
}

impl Widget for RunView {
    fn walk(&self) -> Walk {
        self.walk
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx)
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

