use crate::cx::*;
use crate::drawquad::DrawQuad;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawImage {
    #[default_shader(self::shader_bg)]
    pub texture: Texture2D,
    pub base: DrawQuad,
    pub p1: Vec2,
    pub p2: Vec2,
    pub alpha: f32
}

impl DrawImage{
    
    pub fn with_color(self, color:Vec4)->Self{
        Self{
            color:color,
            ..self
        }
    }
    
    pub fn style(cx:&mut Cx){
        self::DrawImage::register_draw_input(cx);
        live_body!(cx, r#"
            self::shader: Shader {
                use makepad_render::quad::shader::*;
                draw_input: self::DrawColor,
                fn pixel() -> vec4 {
                    return vec4(color.rgb*color.a, color.a)
                }
            }
        "#);
    }
}



use crate::cx::*;

#[derive(Clone)]
pub struct Blit {
    pub shader: Shader,
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub alpha: f32,
    pub do_scroll: bool
}

impl Blit {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            alpha: 1.0,
            min_x: 0.0,
            min_y: 0.0,
            max_x: 1.0,
            max_y: 1.0,
            shader: live_shader!(cx, self::shader),
            do_scroll: false,
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"self::shader: Shader {
            use crate::shader_std::prelude::*;

            default_geometry: crate::shader_std::quad_2d;
            geometry geom: vec2;
            
            instance x: float;
            instance y: float;
            instance w: float;
            instance h: float;
            instance min_x: float;
            instance min_y: float;
            instance max_x: float;
            instance max_y: float;
            
            uniform alpha: float;
            
            texture texturez: texture2D;
            
            varying tc: vec2;
            varying v_pixel: vec2;
            //let dpi_dilate: float<Uniform>;
            
            fn vertex() -> vec4 {
                // return vec4(geom.x-0.5, geom.y, 0., 1.);
                let shift: vec2 = -draw_scroll.xy;
                let clipped: vec2 = clamp(
                    geom * vec2(w, h) + vec2(x, y) + shift,
                    draw_clip.xy,
                    draw_clip.zw
                );
                let pos = (clipped - shift - vec2(x, y)) / vec2(w, h);
                tc = mix(vec2(min_x, min_y), vec2(max_x, max_y), pos);
                v_pixel = clipped;
                // only pass the clipped position forward
                return camera_projection * vec4(clipped.x, clipped.y, 0., 1.);
            }
            
            fn pixel() -> vec4 {
                return vec4(sample2d(texturez, tc.xy).rgb, alpha);
            }
        }"#);
    }
    
    pub fn begin_blit(&mut self, cx: &mut Cx, texture: Texture, layout: Layout) -> InstanceArea {
        let inst = self.draw_blit_rel(cx, texture, Rect::default());
        let area = inst.clone().into();
        cx.begin_turtle(layout, area);
        inst
    }
    
    pub fn end_blit(&mut self, cx: &mut Cx, inst: &InstanceArea) -> Area {
        let area = inst.clone().into();
        let rect = cx.end_turtle(area);
        area.set_rect(cx, &rect);
        area
    }
    
     pub fn begin_blit_fill(&mut self, cx: &mut Cx, texture: Texture) -> InstanceArea {
        let inst = self.draw_blit_rel(cx, texture, Rect::default());
        inst
    }
    
    pub fn end_blit_fill(&mut self, cx: &mut Cx, inst: InstanceArea) -> Area {
        let area: Area = inst.clone().into();
        let pos = cx.get_turtle_origin();
        area.set_rect(cx, &Rect {x: pos.x, y: pos.y, w: cx.get_width_total(), h: cx.get_height_total()});
        area
    }
    
    pub fn draw_blit(&mut self, cx: &mut Cx, texture: Texture, walk: Walk) -> InstanceArea {
        let geom = cx.walk_turtle(walk);
        let inst = self.draw_blit_abs(cx, texture, geom);
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit_rel(&mut self, cx: &mut Cx, texture: Texture, rect: Rect) -> InstanceArea {
        let pos = cx.get_turtle_origin();
        let inst = self.draw_blit_abs(cx, texture, Rect {x: rect.x + pos.x, y: rect.y + pos.y, w: rect.w, h: rect.h});
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit_abs(&mut self, cx: &mut Cx, texture: Texture, rect: Rect) -> InstanceArea {
        let inst = cx.new_instance_draw_call(self.shader, None, 1);
        
        inst.write_uniform_float(cx, live_item_id!(self::shader::alpha), self.alpha);
        inst.write_texture_2d(cx, live_item_id!(self::shader::texturez), texture);

        //println!("{:?} {}", area, cx.current_draw_list_id);
        let data = [
            /*x,y,w,h*/rect.x,
            rect.y,
            rect.w,
            rect.h,
            self.min_x,
            self.min_y,
            self.max_x,
            self.max_y
        ];
        inst.push_slice(cx, &data);
        inst
    }
}
