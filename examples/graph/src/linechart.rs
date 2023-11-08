
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
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            // first we darw a line from min to max
            // then we draw a box from open to close
            sdf.move_to(0.0,0.0);
            sdf.line_to(self.rect_size.x, self.rect_size.y);
            sdf.stroke(#f00,1);
            return sdf.result
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

#[derive(Live)]
pub struct LineChart {
    #[walk] walk: Walk,
    #[live] draw_ls: DrawLineSegment,
    #[rust] screen_view: Rect,
    #[rust] data_view: Rect
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
        self.draw_cs.redraw(cx)
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
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
    }
}

impl LineChart {
    pub fn process_buffer(&mut self, cx: &mut Cx) {
 
    }
}

impl LineChart {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // lets draw a bunch of quads
        
        for i in 0..10{
            self.draw_ls.draw_abs(cx);
        }
        self.draw_cs.draw_walk(cx, walk);
    }
    
    pub fn handle_event_with(&mut self, _cx: &mut Cx, _event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, LineChartAction),) {
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct LineChartRef(WidgetRef);

impl LineChartRef {
}
