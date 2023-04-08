use {
    crate::{
        makepad_platform::*,
        icon_atlas::{CxIconAtlas},
        view::ManyInstances,
        geometry::GeometryQuad2D,
        cx_2d::Cx2d
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
            let min_pos = vec2(self.rect_pos.x, self.rect_pos.y)
            let max_pos = vec2(self.rect_pos.x + self.rect_size.x, self.rect_pos.y - self.rect_size.y)
            
            self.clipped = clamp(
                mix(min_pos, max_pos, self.geom_pos),
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            
            let normalized: vec2 = (self.clipped - min_pos) / vec2(self.rect_size.x, -self.rect_size.y)
            //rect = vec4(min_pos.x, min_pos.y, max_pos.x, max_pos.y) - draw_scroll.xyxy;
            
            self.tex_coord1 = mix(
                self.icon_t1.xy,
                self.icon_t2.xy,
                normalized.xy
            )
            
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                self.clipped.x,
                self.clipped.y,
                self.char_depth + self.draw_zbias,
                1.
            )))
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
            
            s = pow(s, self.u_curve);
            let col = self.get_color(); //color!(white);//get_color();
            return vec4(s * col.rgb * self.u_brightness * col.a, s * col.a);
        }
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawIcon {
    #[rust] pub many_instances: Option<ManyInstances>,
    #[live] pub geometry: GeometryQuad2D,
    
    #[live(1.0)] pub brightness: f32,
    #[live(0.6)] pub curve: f32,
    #[live(1.0)] pub draw_depth: f32,
    
    #[live] pub path: String,
    
    #[calc] pub draw_vars: DrawVars,
    // these values are all generated
    #[live] pub color: Vec4,
    #[calc] pub icon_t1: Vec2,
    #[calc] pub icon_t2: Vec2,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[calc] pub draw_clip: Vec4,
    #[calc] pub char_depth: f32,
    #[calc] pub delta: Vec2,
    #[calc] pub font_size: f32,
    #[calc] pub advance: f32,
}

impl DrawIcon {
    
    pub fn update_abs(&mut self, cx: &mut Cx, rect: Rect) {
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw_vars.update_rect(cx, rect);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect, path: Option<&str>) {
        self.draw_clip = cx.turtle().draw_clip().into();
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx, path);
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, rect: Rect, path: Option<&str>) {
        let rect = rect.translate(cx.turtle().origin());
        self.draw_clip = cx.turtle().draw_clip().into();
        self.rect_pos = rect.pos.into();
        self.rect_size = rect.size.into();
        self.draw(cx, path);
    }
    
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
    
    pub fn draw(&mut self, cx: &mut Cx2d, path: Option<&str>) {
        // allocate our path on the icon atlas
        let path = if let Some(path) = path {path} else {&self.path};
        
        // lets allocate/fetch our path on the icon atlas
        let icon_atlas_rc = cx.icon_atlas_rc.clone();
        let mut icon_atlas = icon_atlas_rc.0.borrow_mut();
        let icon_atlas = &mut*icon_atlas;
        let dpi_factor = cx.current_dpi_factor() as f32;
        //alright we have an icon atlas. lets look up our subpixel + size + path hash
        let subpixel_fract = vec2(
            self.rect_pos.x - (self.rect_pos.x * dpi_factor).floor() / dpi_factor,
            self.rect_pos.y - (self.rect_pos.y * dpi_factor).floor() / dpi_factor
        );
        
        let tc = icon_atlas.get_icon_pos(subpixel_fract, self.rect_size, path);
        self.icon_t1 = tc.t1;
        self.icon_t2 = tc.t2;
        
        if let Some(mi) = &mut self.many_instances {
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        else if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn update_draw_call_vars(&mut self, atlas: &CxIconAtlas) {
        self.draw_vars.texture_slots[0] = Some(atlas.texture_id);
        self.draw_vars.user_uniforms[0] = self.brightness;
        self.draw_vars.user_uniforms[1] = self.curve;
    }
    
}
