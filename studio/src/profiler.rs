
use {
    crate::{
        app::AppData,
        makepad_widgets::*,
    },
    std::{
        fmt::Write,
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
        draw_bg: {
            fn pixel(self)->vec4{
                return #3
            }
        }
        draw_line: {
            fn pixel(self)->vec4{
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.rect(
                    1.,
                    1.,
                    self.rect_size.x - 2.0 ,
                    self.rect_size.y - 2.0
                )
                sdf.fill_keep(#6)
                return sdf.result
            }
        }
        draw_item:{
            fn pixel(self)->vec4{
                return self.color
            }
        }
        draw_time:{ 
            color: #f,
            text_style: <THEME_FONT_LABEL>{}
        }
        draw_label:{
            color: #0,
            text_style: <THEME_FONT_LABEL>{}
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
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_line: DrawQuad,
    #[live] draw_item: DrawColor,
    #[live] draw_label: DrawText,
    #[live] draw_time: DrawText,
    #[rust(TimeRange{start:0.0, end: 1.0})] time_range: TimeRange, 
    #[rust] time_drag: Option<TimeRange>,
    #[rust] tmp_label: String,
}

impl ProfilerEventChart{
    fn draw_block(&mut self, cx: &mut Cx2d, rect:&Rect, sample_start:f64, sample_end: f64, label:&str){
        let scale = rect.size.x / self.time_range.len();
        let xpos = rect.pos.x + (sample_start - self.time_range.start) * scale;
        let xsize = ((sample_end - sample_start) * scale).max(2.0);
        
        let pos = dvec2(xpos, rect.pos.y+20.0);
        let size = dvec2(xsize, 20.0);
        let rect = Rect{pos,size};
                
        self.draw_item.draw_abs(cx, rect);
        self.tmp_label.clear();
        if sample_end - sample_start > 0.001{
            write!(&mut self.tmp_label, "{} {:.2} ms", label, (sample_end-sample_start)*1000.0).unwrap();                        
        }
        else{
            write!(&mut self.tmp_label, "{} {:.0} µs", label, (sample_end-sample_start)*1000_000.0).unwrap();
        }
        
        // if xsize > 10.0 lets draw a clipped piece of text 
        if xsize > 10.0{
            cx.begin_turtle(Walk::abs_rect(rect), Layout::default());
            self.draw_label.draw_abs(cx, pos+dvec2(2.0,4.0), &self.tmp_label);
            cx.end_turtle();
        }
    }
}

impl Widget for ProfilerEventChart {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        self.draw_bg.begin(cx, walk, Layout::default());
        let bm = &scope.data.get::<AppData>().build_manager;
        let mut label = String::new();
        
        let rect = cx.turtle().rect(); 
        if let Some(pss) = bm.profile.values().next(){
            let scale = rect.size.x / self.time_range.len();
                            
            let mut step_size = 0.008;
            while self.time_range.len() / step_size > rect.size.x / 80.0{
                step_size *= 2.0;
            }
                            
            while self.time_range.len() / step_size < rect.size.x / 80.0{
                step_size /= 2.0;
            }
                        
            let mut iter = (self.time_range.start / step_size).floor() * step_size - self.time_range.start;
            while iter < self.time_range.len(){
                let xpos =  iter * scale;
                let pos = dvec2(xpos,0.0)+rect.pos;
                self.draw_line.draw_abs(cx, Rect{pos, size:dvec2(3.0, rect.size.y)});
                label.clear();
                write!(&mut label, "{:.3}s", (iter+self.time_range.start)).unwrap();       
                self.draw_time.draw_abs(cx, pos+dvec2(2.0,2.0), &label);
                iter += step_size; 
            }
            
            if let Some(first) = pss.event.iter().position(|v| v.end > self.time_range.start){
                // lets draw the time lines and time text
                for i in first..pss.event.len(){
                    let sample = &pss.event[i];
                    if sample.start > self.time_range.end{
                        break;
                    }
                    let color = LiveId(0).bytes_append(&sample.event_u32.to_be_bytes()).0 as u32 | 0xff000000;
                    self.draw_item.color = Vec4::from_u32(color);
                    self.draw_block(cx, &rect, sample.start, sample.end, Event::name_from_u32(sample.event_u32));
                }
            }
            
            self.draw_item.color = Vec4::from_u32(0x7f7f7fff);
            if let Some(first) = pss.gpu.iter().position(|v| v.end > self.time_range.start){
                // lets draw the time lines and time text
                for i in first..pss.gpu.len(){
                    let sample = &pss.gpu[i];
                    if sample.start > self.time_range.end{
                        break;
                    }
                    self.draw_block(cx, &Rect{
                        pos:rect.pos + dvec2(0.0,25.0),
                        size:rect.size
                    }, sample.start, sample.end, "GPU");
                }
            }
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }
        
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                // ok so we get multiple finger downs
                cx.set_key_focus(self.draw_bg.area());
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
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerScroll(e)=>{
               if e.device.is_mouse(){
                    let zoom = (1.03).powf(e.scroll.y / 150.0);
                   let scale = self.time_range.len() / e.rect.size.x;
                   let time = scale * (e.abs.x - e.rect.pos.x) + self.time_range.start;
                   self.time_range = TimeRange{
                       start: (self.time_range.start - time) * zoom + time,
                       end: (self.time_range.end - time) * zoom + time,
                   }
               }
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
