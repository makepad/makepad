
use {
    crate::{
        app::{AppData},
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    ProfilerEventChart = {{ProfilerEventChart}}{
        height: Fill,
        width: Fill
        bg: {
            fn pixel(self)->vec4{
                return #3
            }
        }
        item:{
            fn pixel(self)->vec4{
                return self.color
            }
        }
    }
    
    Profiler = {{Profiler}}{
        height: Fill,
        width: Fill
        <ProfilerEventChart>{
        }
    }
}

#[derive(Clone)]
struct TimeRange{
    start:f64,
    end: f64
}

impl TimeRange{
    fn len(&self)->f64{self.end - self.start}
    fn shifted(&self, shift:f64)->Self{Self{start:self.start+shift, end:self.end+shift}}
}

#[derive(Live, LiveHook, Widget)]
struct ProfilerEventChart{
    #[walk] walk:Walk,
    #[redraw] #[live] bg: DrawQuad,
    #[live] item: DrawColor,
    #[rust(TimeRange{start:0.0, end: 1.0})] time_range: TimeRange, 
    #[rust] time_drag: Option<TimeRange>
}

impl Widget for ProfilerEventChart {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        self.bg.begin(cx, walk, Layout::default());
        // alright lets draw some event blocks
        // lets assume nonoverlapping events
        let bm = &scope.data.get::<AppData>().build_manager;
        // alright so. first we scan to the first visible item
        // lets get the first profile ssample store
        // lets get our current rect
        let rect = cx.turtle().rect(); 
        if let Some(pss) = bm.profile.values().next(){
            if let Some(first) = pss.events.iter().position(|v| v.end > self.time_range.start){
                for i in first..pss.events.len(){
                    let sample = &pss.events[i];
                    if sample.start > self.time_range.end{
                        break;
                    }
                    // alright lets draw it.
                    let scale = rect.size.x / self.time_range.len();
                    let xpos = rect.pos.x + (sample.start - self.time_range.start) * scale;
                    let xsize = ((sample.end - sample.start) * scale).max(1.0);
                    
                    // if our rect is bigger than 10px we draw a clipped label
                    
                    let color = LiveId(0).bytes_append(&sample.event_u32.to_be_bytes()).0 as u32 | 0xff000000;
                    
                    self.item.color = Vec4::from_u32(color);
                    self.item.draw_abs(cx, Rect{
                        pos: dvec2(xpos, rect.pos.y+10.0),
                        size: dvec2(xsize, 20.0)
                    })
                }
            }
        }
        self.bg.end(cx);
        DrawStep::done()
    }
        
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        match event.hits(cx, self.bg.area()) {
            Hit::FingerDown(_fe) => {
                // ok so we get multiple finger downs
                cx.set_key_focus(self.bg.area());
                self.time_drag = Some(self.time_range.clone());
            },
            Hit::FingerMove(fe) => {
                if let Some(start) = &self.time_drag{
                    // ok so how much did we move?
                    let moved = fe.abs_start.x - fe.abs.x;
                    // scale this thing to the time window
                    let scale = self.time_range.len() / fe.rect.size.x;
                    let shift_time = moved * scale;
                    self.time_range = start.shifted(shift_time);
                    self.bg.redraw(cx);
                }
                // lets fix up scrollbar
            }
            // lets fix up mouse wheel zoom
            Hit::FingerScroll(e)=>{
                
            }
            Hit::FingerUp(_) => {
            }
            _ => ()
        }
    }
}

#[derive(Live, LiveHook, Widget)]
struct Profiler{
    #[deref] view:View,
}

impl Widget for Profiler {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        self.view.draw_walk_all(cx, scope, walk);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
    }
}
