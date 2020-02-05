use crate::cx::*;

#[derive(Clone)]
pub struct Quad {
    pub shader: Shader,
    pub z: f32,
    pub color: Color
}

impl Quad {
    pub fn proto_with_shader(cx: &mut Cx, shader: ShaderGen, name: &str) -> Self {
        Self {
            shader: cx.add_shader(shader, name),
            ..Self::new(cx)
        }
    }
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_quad_shader(), "Quad"),
            z: 0.0,
            color: color("green")
        }
    }
    
    pub fn instance_x() -> InstanceFloat {uid!()}
    pub fn instance_y() -> InstanceFloat {uid!()}
    pub fn instance_w() -> InstanceFloat {uid!()}
    pub fn instance_h() -> InstanceFloat {uid!()}
    pub fn instance_z() -> InstanceFloat {uid!()}
    pub fn instance_color() -> InstanceColor {uid!()}
    
    pub fn def_quad_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sg.compose(shader_ast!({
            
            let geom: vec2<Geometry>;
            let pos: vec2<Varying>;
            
            let x: Self::instance_x();
            let y: Self::instance_y();
            let w: Self::instance_w();
            let h: Self::instance_h();
            let z: Self::instance_z();
            let color: Self::instance_color();
            
            //let dpi_dilate: float<Uniform>;
            fn scroll() -> vec2{
                return draw_scroll.xy
            }
            
            fn vertex() -> vec4 {
                // return vec4(geom.x-0.5, geom.y, 0., 1.);
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
            
        }))
    }
    
    pub fn begin_quad(&mut self, cx: &mut Cx, layout: Layout) -> InstanceArea {
        let inst = self.draw_quad_rel(cx, Rect::default());
        let area = inst.clone().into();
        cx.begin_turtle(layout, area);
        inst
    }

    pub fn end_quad(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area = inst.clone().into();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
    pub fn begin_quad_fill(&mut self, cx: &mut Cx) -> InstanceArea {
        let inst = self.draw_quad_rel(cx, Rect::default());
        inst
    }

    pub fn end_quad_fill(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area:Area = inst.clone().into();
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
        let inst = cx.new_instance(&self.shader, 1);
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
