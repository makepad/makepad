use {
    std::rc::Rc,
    crate::{
        makepad_platform::*,
        icon_atlas::{CxIconAtlas, CxIconArgs},
        shader::draw_quad::DrawQuad,
        cx_2d::Cx2d,
        turtle::{Walk, Size}
    },
};


live_design!{
    
    DrawIcon = {{DrawIcon}} {
        color: #fff
        
        uniform u_brightness: float
        uniform u_curve: float
        
        texture tex: texture2d
        
        varying tex_coord1: vec2
        varying clipped: vec2
        
        fn vertex(self) -> vec4 {
            let ret = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            
            self.tex_coord1 = mix(
                self.icon_t1.xy,
                self.icon_t2.xy,
                self.pos.xy
            )
            
            return ret
        }
        
        fn get_color(self) -> vec4 {
            return self.color;
        }
        
        fn pixel(self) -> vec4 {
            let dx = dFdx(vec2(self.tex_coord1.x * 2048.0, 0.)).x;
            let dp = 1.0 / 2048.0;
            
            // basic hardcoded mipmapping so it stops 'swimming' in VR
            // mipmaps are stored in red/green/blue channel
            let s = sample2d_rt(self.tex, self.tex_coord1.xy).x;
            //return mix(#f00,#0f0,s);
            s = pow(s, self.u_curve);
            let col = self.get_color(); //color!(white);//get_color();
            return vec4(s * col.rgb * self.u_brightness * col.a, s * col.a);
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawIcon {
    #[live(1.0)] pub brightness: f32,
    #[live(0.6)] pub curve: f32,
    #[live(1.0)] pub draw_depth: f32,
    #[live] pub path: Rc<String>,
    #[live] pub translate: DVec2,
    #[live(1.0)] pub scale: f64,
    #[live()] pub draw_super: DrawQuad,
    
    #[live] pub color: Vec4,
    #[calc] pub icon_t1: Vec2,
    #[calc] pub icon_t2: Vec2,
}

impl DrawIcon {
    
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        let icon_atlas_rc = cx.icon_atlas_rc.clone();
        let icon_atlas = icon_atlas_rc.0.borrow();
        let mi = cx.begin_many_aligned_instances(&self.draw_vars);
        self.update_draw_call_vars(&*icon_atlas);
        self.many_instances = mi;
    }
    
    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) {
        // allocate our path on the icon atlas
        // lets allocate/fetch our path on the icon atlas
        let icon_atlas_rc = cx.icon_atlas_rc.clone();
        let mut icon_atlas = icon_atlas_rc.0.borrow_mut();
        let icon_atlas = &mut*icon_atlas;
        
        if let Some((path_hash, bounds)) = icon_atlas.get_icon_bounds(&self.path) {
            if walk.width.is_fit() {
                walk.width = Size::Fixed(bounds.size.x * self.scale);
            }
            if walk.height.is_fit() {
                walk.height = Size::Fixed(bounds.size.y * self.scale);
            }
            let rect = cx.walk_turtle(walk);
            
            let dpi_factor = cx.current_dpi_factor();
            
            let subpixel = dvec2(
                rect.pos.x as f64 - (rect.pos.x as f64 * dpi_factor).floor() / dpi_factor,
                rect.pos.y as f64 - (rect.pos.y as f64 * dpi_factor).floor() / dpi_factor
            );
            
            let slot = icon_atlas.get_icon_slot(CxIconArgs {
                size: rect.size,
                scale: self.scale,
                translate: self.translate - rect.pos + subpixel
            }, path_hash);
            
            self.rect_pos = rect.pos.into();
            self.rect_size = rect.size.into();
            self.icon_t1 = slot.t1;
            self.icon_t2 = slot.t2;
            
            if let Some(mi) = &mut self.draw_super.many_instances {
                mi.instances.extend_from_slice(self.draw_super.draw_vars.as_slice());
            }
            else if self.draw_vars.can_instance() {
                self.update_draw_call_vars(icon_atlas);
                let new_area = cx.add_aligned_instance(&self.draw_vars);
                self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
            }
            
            // ok now what
            // we have a bounds rect we can use to set up a translate/scale
            // what kind of walks do we have
            // Fixed, Fit and Fill
            // only 'Fit' we can now look up
            // so how do we compute 'fit'.
        }
        
        //alright we have an icon atlas. lets look up our subpixel + size + path hash
        
    }
    
    pub fn update_draw_call_vars(&mut self, atlas: &CxIconAtlas) {
        self.draw_vars.texture_slots[0] = Some(atlas.texture_id);
        self.draw_vars.user_uniforms[0] = self.brightness;
        self.draw_vars.user_uniforms[1] = self.curve;
    }
    
}
