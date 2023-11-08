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
           // let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            // first we darw a line from min to max
            // then we draw a box from open to close
            //sdf.move_to(0.0,0.0);
            //sdf.line_to(self.rect_size.x, self.rect_size.y);
            //sdf.stroke(#f00,1);
            //return sdf.result

            return #ff0
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
    #[calc] start_y: f32,
    #[calc] end_y: f32
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
    #[live(10.0)] line_width: f64,
    #[rust(dvec2(10.,10.))] line_start: DVec2,
    #[rust(dvec2(100.,40.))] line_end: DVec2,
    #[rust(dvec2(100.,40.))] line_dragstart: DVec2,
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
        let linerect = self.line_end - self.line_start;
        let hw = self.line_width / 2.;
        if (self.line_start.y - self.line_end.y).abs().floor() == 0.0 || (self.line_start.x - self.line_end.x).abs().floor() == 0.0 {
            
            let r = Rect{
                pos: dvec2(min(self.line_start.x, self.line_end.x) -hw ,min(self.line_start.y, self.line_end.y) -hw ),
                size: dvec2(linerect.x.abs() + self.line_width, linerect.y.abs() + self.line_width)
            };
            self.draw_ls.draw_abs(cx, r);

            return
        }
        if linerect.x.abs() > linerect.y.abs() // more horizontal than vertical
        {
            let abslinerect = dvec2(linerect.x.abs(), linerect.y.abs());
            
            let numblocks = (abslinerect.x / hw).ceil();
            let blockwidth = abslinerect.x / (numblocks as f64);
            let normalizedir = linerect.normalize();
            let step = dvec2(blockwidth, normalizedir.y * blockwidth);
            let blockheight  = normalizedir.y * blockwidth * 2. + self.line_width;
            for i in 0..numblocks.ceil() as i32{
                let r = Rect{
                    pos: self.line_start + step* (i as f64),
                    size: dvec2(blockwidth,blockheight)
                };
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
