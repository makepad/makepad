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
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            alpha: 1.0,
            min_x:0.0,
            min_y:0.0,
            max_x:1.0,
            max_y:1.0,
            shader: cx.add_shader(Self::def_blit_shader(), "Blit"),
            do_scroll:false,
        }
    }
    
    fn geom()->Vec2Id{uid!()}
    pub fn x()->FloatId{uid!()}
    pub fn y()->FloatId{uid!()}
    pub fn w()->FloatId{uid!()}
    pub fn h()->FloatId{uid!()}
    pub fn min_x()->FloatId{uid!()}
    pub fn min_y()->FloatId{uid!()}
    pub fn max_x()->FloatId{uid!()}
    pub fn max_y()->FloatId{uid!()}
    pub fn z()->FloatId{uid!()}
    pub fn color()->ColorId{uid!()}
    pub fn alpha()->FloatId{uid!()}
    pub fn texturez()->Texture2dId{uid!()}
    pub fn def_blit_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sb = ShaderGen::new();
        
        sb.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sb.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sb.compose(shader!{"
            
            geometry geom: Self::geom();
            instance x: Self::x();
            instance y: Self::y();
            instance w: Self::w();
            instance h: Self::h();
            instance min_x: Self::min_x();
            instance min_y: Self::min_y();
            instance max_x: Self::max_x();
            instance max_y: Self::max_y();
            varying tc: vec2;
            uniform alpha: Self::alpha();
            
            texture texturez:Self::texturez();
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
                tc = mix(vec2(min_x,min_y), vec2(max_x,max_y), pos);
                v_pixel = clipped;
                // only pass the clipped position forward
                return camera_projection * vec4(clipped.x, clipped.y, 0., 1.);
            }
            
            fn pixel() -> vec4 {
                return vec4(sample2d(texturez, tc.xy).rgb, alpha);
            }
            
        "})
    }
    
    
    pub fn begin_blit(&mut self, cx: &mut Cx, texture:&Texture, layout: Layout) -> InstanceArea {
        let inst = self.draw_blit(cx, texture, Rect::default());
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
    
    pub fn draw_blit_walk(&mut self, cx: &mut Cx, texture:&Texture, walk:Walk) -> InstanceArea {
        let geom = cx.walk_turtle(walk);
        let inst = self.draw_blit_abs(cx, texture, geom);
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit(&mut self, cx: &mut Cx, texture:&Texture, rect: Rect) -> InstanceArea {
        let pos = cx.get_turtle_origin();
        let inst = self.draw_blit_abs(cx, texture, Rect {x: rect.x + pos.x, y: rect.y + pos.y, w: rect.w, h: rect.h});
        cx.align_instance(inst);
        inst
    }
    
    pub fn draw_blit_abs(&mut self, cx: &mut Cx, texture:&Texture, rect: Rect) -> InstanceArea {
        let inst = cx.new_instance_draw_call(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
            inst.push_uniform_float(cx, if self.do_scroll {1.0}else {0.0});
            inst.push_uniform_float(cx, self.alpha);
            inst.push_uniform_texture_2d(cx, texture);
        }
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
