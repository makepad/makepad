use crate::makepad_draw_2d::*;


live_register!{
    import makepad_draw_2d::shader::std::*;
    
    DrawRect: {{DrawRect}} {
        fn pixel(self) -> vec4 {
            return vec4(self.color.xyz * self.color.w, self.color.w)
        }
    }
    
    DebugView: {{DebugView}} {
        label: {
            text_style:{
                font_size: 6
            },
            color: #a
        }
        view:{}
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRect {
    draw_super: DrawQuad,
    color: Vec4,
}


#[derive(Live, LiveHook)]
pub struct DebugView {
    view: View,
    rect: DrawRect,
    label: DrawText
}

impl DebugView {
    pub fn handle_event(&mut self, cx: &mut Cx, _event: &Event) {
        if cx.debug.has_data(){
            self.view.redraw(cx);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if !self.view.begin(cx, Walk::default(), Layout::default()).is_redrawing(){
            return
        }
        let debug = cx.debug.clone();
        let rects = debug.take_rects();
        for (rect, color) in rects {
            self.rect.color = color;
            self.rect.draw_abs(cx, rect);
        }
        
        let points = debug.take_points();
        let point_size = vec2(1.0, 1.0);
        for (point, color) in points {
            self.rect.color = color;
            let rect = Rect {pos: point - 0.5 * point_size, size: point_size};
            self.rect.draw_abs(cx, rect);
        }
        
        let labels = debug.take_labels();
        let point_size = vec2(1.0, 1.0);
        for (point, color, label) in labels {
            self.rect.color = color;
            let rect = Rect {pos: point - 0.5 * point_size, size: point_size};
            self.rect.draw_abs(cx, rect);
            self.label.draw_abs(cx, point, &label);
        }
        
        self.view.end(cx);
        
    }
}

