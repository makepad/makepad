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
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawRect {
    #[deref] draw_super: DrawQuad,
    #[live] color: Vec4,
}


#[derive(Live, LiveHook, LiveRegister)]
pub struct DebugView {
    #[live] draw_list: DrawList2d,
    #[live] rect: DrawRect,
    #[live] label: DrawText
}

impl DebugView {
    pub fn handle_event(&mut self, cx: &mut Cx, _event: &Event) {
        if cx.debug.has_data() {
            self.draw_list.redraw(cx);
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if cx.debug.has_data() {
            self.draw_list.redraw(cx);
            self.draw_list.begin_always(cx);
        }
        else{
            if !self.draw_list.begin(cx, Walk::default()).is_redrawing() {
                return
            }
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
        
        let debug = cx.debug.clone();
        let areas = debug.take_areas();
        for (area, tl, br, color) in areas {
            self.rect.color = color;
            let rect = area.rect(cx);
            let rect = Rect{pos: rect.pos - tl, size: rect.size + tl + br};
            self.rect.draw_abs(cx, rect);
        }
        
        self.draw_list.end(cx);
        
    }
}

