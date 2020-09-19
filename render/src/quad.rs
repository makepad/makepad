use crate::cx::*;
#[derive(Clone)]
pub struct Quad {
    pub shader: Shader,
    pub z: f32,
    pub color: Color
}

impl Quad {
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            shader: live_shader!(cx, self::shader),
            z: 0.0,
            color: Color::parse_name("green").unwrap()
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live!(cx, r#"self::shader: Shader {

            use crate::shader_std::prelude::*;
            
            default_geometry: crate::shader_std::quad_2d;
            geometry geom: vec2;
            
            varying pos: vec2;
            
            instance x: float;
            instance y: float;
            instance w: float;
            instance h: float;
            instance z: float;
            instance color: vec4;
            
            //let dpi_dilate: float<Uniform>;
            fn scroll() -> vec2 {
                return draw_scroll.xy;
            }
            
            fn vertex() -> vec4 {
                let scr = scroll();
                let clipped: vec2 = clamp(
                    geom * vec2(w, h) + vec2(x, y) - scr,
                    draw_clip.xy,
                    draw_clip.zw
                );
                pos = (clipped + scr - vec2(x, y)) / vec2(w, h);
                // only pass the clipped position forward
                return camera_projection * (camera_view * (view_transform * vec4(clipped.x, clipped.y, z + draw_zbias, 1.)));
            }
            
            fn pixel() -> vec4 {
                return vec4(color.rgb * color.a, color.a);
            }
        }"#);
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) -> InstanceArea {
        let inst = self.draw_quad_rel(cx, Rect::default());
        let area = inst.clone().into();
        cx.begin_turtle(layout, area);
        inst
    }
    
    pub fn end_quad(&mut self, cx: &mut Cx, inst: InstanceArea) -> Area {
        let area = inst.into();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
    pub fn begin_quad_fill(&mut self, cx: &mut Cx) -> InstanceArea {
        let inst = self.draw_quad_rel(cx, Rect::default());
        inst
    }
    
    pub fn end_quad_fill(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area: Area = inst.clone().into();
        let pos = cx.get_turtle_origin();
        area.set_rect(cx, &Rect {x: pos.x, y: pos.y, w: cx.get_width_total(), h: cx.get_height_total()});
        area
    }
    
    pub fn draw_quad(&mut self, cx: &mut Cx, walk: Walk) -> InstanceArea {
        let geom = cx.walk_turtle(walk);
        let inst = self.draw_quad_abs(cx, geom);
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_quad_rel(&mut self, cx: &mut Cx, rect: Rect) -> InstanceArea {
        let pos = cx.get_turtle_origin();
        let inst = self.draw_quad_abs(cx, Rect {x: rect.x + pos.x, y: rect.y + pos.y, w: rect.w, h: rect.h});
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_quad_abs(&mut self, cx: &mut Cx, rect: Rect) -> InstanceArea {
        let inst = cx.new_instance(self.shader, None, 1);
        if inst.need_uniforms_now(cx) {
        }
        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,
            rect.y,
            rect.w,
            rect.h,
            self.z,
            /*color*/self.color.r,
            self.color.g,
            self.color.b,
            self.color.a
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
