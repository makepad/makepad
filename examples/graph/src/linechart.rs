use core::num;


use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
    DrawLineSegment = {{DrawLineSegment}} {
        fn pixel(self) -> vec4 {
            //return mix(#f00,#0f0, left+0.5);
            let pixelpos = self.pos * self.rect_size;
            let sdf = Sdf2d::viewport(pixelpos);
            // first we darw a line from min to max
            // then we draw a box from open to close

            
            sdf.move_to(0.0,0.0);
            sdf.line_to(self.rect_size.x, 0);
            sdf.line_to(self.rect_size.x, self.rect_size.y);
            sdf.line_to(0, self.rect_size.y);
            sdf.line_to(0, 0);
            sdf.stroke(#222,1);
            sdf.move_to(self.line_start.x , self.line_start.y);
            sdf.line_to(self.line_end.x , self.line_end.y);
            sdf.stroke(self.color,self.width);
            
            return sdf.result

            //return #ff0
        }
    }
    
    LineChart = {{LineChart}} {
        width: Fill,
        height: Fill
    }
}

#[derive(Live, LiveHook)]#[repr(C)]
struct DrawLineSegment {
    #[deref] draw_super: DrawQuad,
    #[calc] line_start: Vec2,
    #[calc] line_end: Vec2,
    #[calc] width: f32,
    #[calc] color: Vec4
  
}

enum DraggingSide {
    LineStartNOTDragging,
    LineStartIsDragging,
    LineEndIsDragging,
}

#[derive(Live)]
pub struct LineChart {
    #[walk] walk: Walk,
    #[live] draw_ls: DrawLineSegment,
    #[rust] area: Area,
    #[rust] _screen_view: Rect,
    #[rust] _data_view: Rect,
    #[live(15.0)] line_width: f64,
    #[rust(dvec2(10.,10.))] line_start: DVec2,
    #[rust(dvec2(1000.,240.))] line_end: DVec2,
    #[rust(dvec2(1000.,140.))] line_dragstart: DVec2,
    #[rust(DraggingSide::LineStartNOTDragging)] draggingside: DraggingSide
}

impl Widget for LineChart {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, WidgetAction)]
pub enum LineChartAction {
    None
}

impl LiveHook for LineChart {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, LineChart)
    }
    
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
    }
}

impl LineChart {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // lets draw a bunch of quads
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);

        let r = Rect{
            pos: dvec2(0.,0.),
            size: dvec2(2000.,2000.)
        };
        let mut actualstart =self.line_start;
        let mut actualend =self.line_end;
        self.draw_ls.line_start = (actualstart - r.pos).into_vec2();
        self.draw_ls.line_end = (actualend - r.pos).into_vec2();
        self.draw_ls.width = (self.line_width as f32)*1.3;
        self.draw_ls.color = vec4(0.8,0.0,0.0,1.0);


        
       
        self.draw_ls.draw_abs(cx, r);


        let linerect = self.line_end - self.line_start;
        let hw = self.line_width / 2.;
        if (self.line_start.y - self.line_end.y).abs().floor() == 0.0 || (self.line_start.x - self.line_end.x).abs().floor() == 0.0 {
            
            let r = Rect{
                pos: dvec2(min(self.line_start.x , self.line_end.x) -self.line_width ,min(self.line_start.y, self.line_end.y) -self.line_width ),
                size: dvec2(linerect.x.abs() + self.line_width*2., linerect.y.abs() + self.line_width*2.)
            };
            self.draw_ls.line_start = (self.line_start - r.pos).into_vec2();
            self.draw_ls.line_end = (self.line_end - r.pos).into_vec2();
            self.draw_ls.width = self.line_width as f32;
            self.draw_ls.color = vec4(1.,1.,0.2,1.0);
            
            self.draw_ls.draw_abs(cx, r);

            return
        }
       


        if linerect.x.abs() > linerect.y.abs() // more horizontal than vertical
        {
            let mut actualstart =self.line_start;
            let mut actualend =self.line_end;

            if actualend.x < actualstart.x
            {
                std::mem::swap(&mut actualstart, &mut actualend);
            }
            let delta = actualend - actualstart;

            let abslinerect = dvec2(delta.x.abs(), delta.y.abs());
            
            let numblocks = (abslinerect.x / self.line_width).ceil();
            let blockwidth = (abslinerect.x / (numblocks as f64));

            let normalizedelta = delta.normalize_to_x();
            
            let step = dvec2(blockwidth, normalizedelta.y*blockwidth);
            let blockheight  = normalizedelta.y.abs() * blockwidth * 2. + self.line_width;
            for i in 0..numblocks.ceil() as i32{
                let r = Rect{
                    pos:actualstart + dvec2(step.x * (i as f64),step.y * (i as f64) +self.line_width/2.),
                    size: dvec2(blockwidth,blockheight)
                
                };
                self.draw_ls.color = vec4(0.9,0.9,0.0,1.0);
       
                self.draw_ls.line_start = (actualstart - r.pos).into_vec2();
                self.draw_ls.line_end = (actualend - r.pos).into_vec2();
                self.draw_ls.width = self.line_width as f32;
                self.draw_ls.draw_abs(cx, r);
            }
        }
        else {
            for i in 0..10{
                let r = Rect{
                    pos: self.line_start + (self.line_end - self.line_start) * (i as f64)*0.1,
                    size: dvec2(10.0,10.0)
                };
                
                self.draw_ls.draw_abs(cx, r);
            }
        }

      


    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, LineChartAction),) {
        match event.hits(cx, self.area) {
            
            Hit::FingerDown(fe) => {
              let l1 = (fe.abs - self.line_start).lengthsquared();
              let l2 = (fe.abs - self.line_end).lengthsquared();
                if l2<l1 {
                    self.draggingside = DraggingSide::LineEndIsDragging;
                    self.line_dragstart = self.line_end;

                }
                else {
                    self.draggingside = DraggingSide::LineStartIsDragging;
                    self.line_dragstart = self.line_start;
                }

            },
            Hit::FingerUp(_fe) => {
                self.draggingside = DraggingSide::LineStartNOTDragging;
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.abs_start;
                log!("{:?}", rel);
                if let DraggingSide::LineStartIsDragging = self.draggingside 
                {

                    self.line_start = self.line_dragstart + rel;
                }
                if let DraggingSide::LineEndIsDragging = self.draggingside 
                {

                    self.line_end = self.line_dragstart + rel;
                    
                }
                self.area.redraw(cx);
            }
            _ => ()
        }
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct LineChartRef(WidgetRef);

impl LineChartRef {
}
