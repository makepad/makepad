use crate::makepad_draw::*;


live_design!{
    import makepad_draw::shader::std::*;
    
    DrawRect = {{DrawRect}} {
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.rect(0., 0., self.rect_size.x, self.rect_size.y);
            sdf.stroke(self.color, 1.0);
            return sdf.result;
            //return vec4(self.color.xyz * self.color.w, self.color.w)
        }
        draw_depth: 20.0
    }
    
    DebugView = {{DebugView}} {
        label: {
            text_style: {
                font_size: 6
            },
            color: #a
        }
        view: {}
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawRect {
    #[live] draw_super: DrawQuad,
    #[live] color: Vec4,
}


#[derive(Live, LiveHook)]
pub struct DebugView {
    #[live] view: View,
    #[live] rect: DrawRect,
    #[live] label: DrawText
}

impl DebugView {
    pub fn handle_event(&mut self, cx: &mut Cx, _event: &Event) {
        if cx.debug.has_data() {
            self.view.redraw(cx);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if !self.view.begin(cx, Walk::default()).is_redrawing() {
            return
        }
        let debug = cx.debug.clone();
        let rects = debug.take_rects();
        for (rect, color) in rects {
            self.rect.color = color;
            self.rect.draw_abs(cx, rect);
        }
        
        let points = debug.take_points();
        let point_size = dvec2(1.0, 1.0);
        for (point, color) in points {
            self.rect.color = color;
            let rect = Rect {pos: point - 0.5 * point_size, size: point_size};
            self.rect.draw_abs(cx, rect);
        }
        
        let labels = debug.take_labels();
        let point_size = dvec2(1.0, 1.0);
        for (point, color, label) in labels {
            self.rect.color = color;
            let rect = Rect {pos: point - 0.5 * point_size, size: point_size};
            self.rect.draw_abs(cx, rect);
            self.label.draw_abs(cx, point, &label);
        }
        
        self.view.end(cx);
        
    }
}

