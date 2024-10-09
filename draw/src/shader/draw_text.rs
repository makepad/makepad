use {
    crate::{
        cx_2d::Cx2d, draw_list_2d::ManyInstances, font_atlas::{self, CxFontAtlas, CxFontsAtlasTodo, CxShapeCache, Font}, geometry::GeometryQuad2D, makepad_platform::*, turtle::{Align, Flow, Size, Walk}
    },
    makepad_rustybuzz::Direction,
    unicode_segmentation::UnicodeSegmentation,
};

const ZBIAS_STEP: f32 = 0.00001;

live_design!{
    
    DrawText = {{DrawText}} {
        //debug: true;
        color: #fff
        
       // uniform brightness: float
       // uniform curve: float
        //uniform sdf_radius: float
        //uniform sdf_cutoff: float
        
        texture tex: texture2d
        
        varying tex_coord1: vec2
        varying tex_coord2: vec2
        varying tex_coord3: vec2
        varying clipped: vec2
        varying pos: vec2
        
        fn vertex(self) -> vec4 {
            let min_pos = vec2(self.rect_pos.x, self.rect_pos.y)
            let max_pos = vec2(self.rect_pos.x + self.rect_size.x, self.rect_pos.y - self.rect_size.y)
            
            self.clipped = clamp(
                mix(min_pos, max_pos, self.geom_pos),
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            
            let normalized: vec2 = (self.clipped - min_pos) / vec2(self.rect_size.x, -self.rect_size.y)
            
            self.tex_coord1 = mix(
                vec2(self.font_t1.x, 1.0-self.font_t1.y),
                vec2(self.font_t2.x, 1.0-self.font_t2.y),
                normalized.xy
            )
            self.pos = normalized;
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
        fn blend_color(self, incol:vec4)->vec4{
            return incol
        }
        
        fn get_brightness(self)->float{
            return 1.0;
        }
        
        fn sample_color(self, scale:float, pos:vec2)->vec4{
            let brightness = self.get_brightness();
            let sdf_radius = 8.0;
            let sdf_cutoff = 0.25;
            let s = sample2d(self.tex, pos).x;
            let curve = 0.5; 
            //if (self.sdf_radius != 0.0) {
            // HACK(eddyb) harcoded atlas size (see asserts below).
            let texel_coords = pos.xy * 4096.0;
            s = clamp((s - (1.0 - sdf_cutoff)) * sdf_radius / scale + 0.5, 0.0, 1.0);
            //} else {
            //    s = pow(s, curve);
            //}
            let col = self.get_color(); 
            return self.blend_color(vec4(s * col.rgb * brightness * col.a, s * col.a));
        }
        
        fn pixel(self) -> vec4 {
            let texel_coords = self.tex_coord1.xy;
            let dxt = length(dFdx(texel_coords));
            let dyt = length(dFdy(texel_coords));
            let scale = (dxt + dyt) * 4096.0 *0.5;
            return self.sample_color(scale, self.tex_coord1.xy);// + vec4(1.0, 0.0, 0.0, 0.0);
            // ok lets take our delta in the x direction
            /*
            //4x AA
            */
            /*
            let x1 = self.sample_color(scale, self.tex_coord1.xy);
            let x2 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * 0.5,0.0));
            let x3 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt* 0.5,dyt* 0.5));
            let x4 =  self.sample_color(scale, self.tex_coord1.xy+vec2(0.0,dyt* 0.5));
            return (x1+x2+x3+x4)/4;
            */
            /*
            let d = 0.333;
            let x1 = self.sample_color(scale, self.tex_coord1.xy);
            let x2 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * -d,0.0));
            let x3 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt* d,0.0));
            let x4 = self.sample_color(scale, self.tex_coord1.xy+vec2(0.0, dyt * -d));
            let x5 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * -d,dyt * -d));
            let x6 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt* d,dyt * -d));
            let x7 = self.sample_color(scale, self.tex_coord1.xy+vec2(0.0, dyt * d));
            let x8 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * -d,dyt * d));
            let x9 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt* d,dyt * d));
            return (x1+x2+x3+x4+x5+x6+x7+x8+x9)/9;
            */
            //16x AA
            /*
            let d = 0.25;
            let d2 = 0.5; 
            let d3 = 0.75; 
            let x1 = self.sample_color(scale, self.tex_coord1.xy);
            let x2 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d,0.0));
            let x3 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d2,0.0));
            let x4 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d3,0.0));
                        
            let x5 = self.sample_color(scale, self.tex_coord1.xy+vec2(0.0,dyt *d));
            let x6 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d,dyt *d));
            let x7 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d2,dyt *d));
            let x8 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d2,dyt *d));
                        
            let x9 = self.sample_color(scale, self.tex_coord1.xy+vec2(0.0,dyt *d2));
            let x10 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d,dyt *d2));
            let x11 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d2,dyt *d2));
            let x12 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d3,dyt *d2));           
            
            let x13 = self.sample_color(scale, self.tex_coord1.xy+vec2(0.0,dyt *d3));
            let x14 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d,dyt *d3));
            let x15 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d2,dyt *d3));
            let x16 =  self.sample_color(scale, self.tex_coord1.xy+vec2(dxt * d3,dyt *d3));            
            return (x1+x2+x3+x4+x5+x6+x7+x8+x9+x10+x11+x12+x13+x14+x15+x16)/16 ;*/
        }
    }
}

// HACK(eddyb) shader expects hardcoded atlas size (see `fn pixel` above).
const _: () = assert!(crate::font_atlas::ATLAS_WIDTH == 4096);
const _: () = assert!(crate::font_atlas::ATLAS_HEIGHT == 4096);

#[derive(Debug, Clone, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live()] pub font2: Font,
    #[live(9.0)] pub font_size: f64,
    //#[live(1.0)] pub brightness: f32,
    //#[live(0.5)] pub curve: f32,
    #[live(0.88)] pub line_scale: f64,
    #[live(1.4)] pub line_spacing: f64,
    //#[live(1.1)] pub top_drop: f64,
    #[live(1.3)] pub height_factor: f64,
}

#[derive(Clone, Live, LiveHook, PartialEq)]
#[live_ignore]
pub enum TextWrap {
    #[pick] Ellipsis,
    Word,
    Line
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawText {
    #[rust] pub many_instances: Option<ManyInstances>,
    
    #[live] pub geometry: GeometryQuad2D,
    #[live] pub text_style: TextStyle,
    #[live] pub wrap: TextWrap,
    
    #[live] pub ignore_newlines: bool,
    #[live] pub combine_spaces: bool,
    
    #[live(1.0)] pub font_scale: f64,
    #[live(1.0)] pub draw_depth: f32,
    
    #[deref] pub draw_vars: DrawVars,
    // these values are all generated
    #[live] pub color: Vec4,
    #[calc] pub font_t1: Vec2,
    #[calc] pub font_t2: Vec2,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[calc] pub draw_clip: Vec4,
    #[calc] pub char_depth: f32,
}

impl LiveHook for DrawText {
    fn before_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawText {
    
    pub fn draw(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos, val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos + cx.turtle().origin(), val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos, val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        let fonts_atlas_rc = cx.fonts_atlas_rc.clone();
        let fonts_atlas = fonts_atlas_rc.0.borrow();
        self.begin_many_instances_internal(cx, &*fonts_atlas);
    }
    
    fn begin_many_instances_internal(&mut self, cx: &mut Cx2d, fonts_atlas: &CxFontAtlas) {
        self.update_draw_call_vars(fonts_atlas);
        let mi = cx.begin_many_aligned_instances(&self.draw_vars);
        self.many_instances = mi;
    }
    
    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
    
    pub fn new_draw_call(&self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }
    
    pub fn update_draw_call_vars(&mut self, font_atlas: &CxFontAtlas) {
        self.draw_vars.texture_slots[0] = Some(font_atlas.texture_sdf.clone());
        // self.draw_vars.user_uniforms[0] = self.text_style.brightness;
        // self.draw_vars.user_uniforms[1] = self.text_style.curve;
        //let (sdf_radius, sdf_cutoff) = font_atlas.alloc.sdf.as_ref()
        //    .map_or((0.0, 0.0), |sdf| (sdf.params.radius, sdf.params.cutoff));
        //self.draw_vars.user_uniforms[0] = sdf_radius;
        //self.draw_vars.user_uniforms[1] = sdf_cutoff;
    }
    
    pub fn get_line_spacing(&self) -> f64 {
        self.text_style.font_size * self.font_scale * self.text_style.line_spacing
    }
    
    pub fn get_font_size(&self) -> f64 {
        self.text_style.font_size
    }
    
    pub fn get_monospace_base(&self, cx: &Cx2d) -> DVec2 {
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return DVec2::default();
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        if font_atlas.fonts[font_id].is_none() {
            return DVec2::default();
        }

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        let font = font_atlas.fonts[font_id].as_mut().unwrap();
        let slot = font.owned_font_face.with_ref( | face | face.glyph_index('!').map_or(0, | id | id.0 as usize));
        let glyph = font.get_glyph_by_id(slot).unwrap();
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        DVec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.ttf_font.units_per_em)),
            y: line_spacing / font_size,
        }
    }
}

impl DrawText {
    pub fn line_height(&self, cx: &Cx2d) -> f64 {
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return 0.0;
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };
        
        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas_ref = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas_ref;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;

        line_height
    }

    pub fn line_spacing(&self, cx: &Cx2d) -> f64 {
        self.line_height(cx) * self.text_style.line_spacing
    }

    pub fn selected_rects(
        &self,
        cx: &mut Cx2d, 
        walk: Walk,
        _align: Align,
        width: f64,
        text: &str,
        start: IndexAffinity,
        end: IndexAffinity,
    ) -> Vec<Rect> {
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return Vec::new();
        }

        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return Vec::new();
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas_ref = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas_ref;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache_ref = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache_ref;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        let fixed_width = if !walk.width.is_fit() {
            Some(width)
        } else {
            None
        };

        let wrap_width = if self.wrap == TextWrap::Word {
            fixed_width
        } else {
            None
        };

        let mut rects = Vec::new();
        let mut prev_newline_index = 0;
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |position, index, event, _font_atlas| {
                match event {
                    LayoutEvent::Newline { .. } => {
                        if start.index < index && end.index >= prev_newline_index {
                            rects.push(Rect {
                                pos: dvec2(0.0, position.y),
                                size: dvec2(position.x, line_height),
                            })
                        }
                        prev_newline_index = index;
                    }
                    _ => {}
                }
                false
            }
        );
        if end.index >= prev_newline_index {
            rects.push(Rect {
                pos: dvec2(0.0, position.y),
                size: dvec2(position.x, line_height),
            });
        }
        
        drop(shape_cache_ref);
        drop(font_atlas_ref);
        
        if let Some(rect) = rects.last_mut() {
            let end_position = self.index_affinity_to_position(
                cx,
                walk,
                _align,
                width,
                text,
                end,
            );

            rect.size.x = end_position.x;
        }

        if let Some(rect) = rects.first_mut() {
            let start_position = self.index_affinity_to_position(
                cx,
                walk,
                _align,
                width,
                text,
                start,
            );

            rect.pos.x = start_position.x;
            rect.size.x -= start_position.x;
        }

        rects
    }

    pub fn position_to_index_affinity(
        &self,
        cx: &mut Cx2d,
        walk: Walk,
        _align: Align,
        width: f64,
        text: &str,
        target_position: DVec2,
    ) -> IndexAffinity {
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return IndexAffinity::new(text.len(), Affinity::After);
        }

        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return IndexAffinity::new(text.len(), Affinity::After);
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas_ref = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas_ref;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache_ref = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache_ref;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        let fixed_width = if !walk.width.is_fit() {
            Some(width)
        } else {
            None
        };

        let wrap_width = if self.wrap == TextWrap::Word {
            fixed_width
        } else {
            None
        };

        let mut closest = IndexAffinity::new(text.len(), Affinity::After);
        let line_height = compute_line_height(font_ids, font_size, font_atlas);
        let mut prev_glyph_end = 0;
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |position, start, event, font_atlas| {
                match event {
                    LayoutEvent::Chunk {
                        string,
                        glyph_infos,
                        ..
                    } => {
                        let mut position = position;
                        let mut iter = glyph_infos.iter().peekable();
                        while let Some(glyph_info) = iter.next() {
                            let glyph_start = start + glyph_info.cluster;
                            let glyph_end = start + iter.peek().map_or(string.len(), |glyph_info| glyph_info.cluster);
                            let glyph_width = compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas);
                            let grapheme_count = text[glyph_start..glyph_end].graphemes(true).count();
                            let width_per_grapheme = glyph_width / (grapheme_count + 1) as f64;
                            for index in 0..grapheme_count {
                                let next_position_x = position.x + width_per_grapheme;
                                if target_position.x < next_position_x && target_position.y < position.y + line_spacing {
                                    closest = IndexAffinity::new(
                                        glyph_start + text[glyph_start..glyph_end].grapheme_indices(true).nth(index).unwrap().0,
                                        Affinity::After,
                                    );
                                    return true;
                                }
                                position.x = next_position_x;
                            }
                            position.x += width_per_grapheme;
                            prev_glyph_end = glyph_end;
                        }
                    }
                    LayoutEvent::Newline { is_soft } => {
                        if target_position.y < position.y + line_height {
                            closest = IndexAffinity::new(
                                prev_glyph_end,
                                if is_soft {
                                    Affinity::Before
                                } else {
                                    Affinity::After
                                }
                            );
                            return true;
                        }
                        prev_glyph_end += 1;
                    }
                }
                false
            }
        );

        closest
    }

    pub fn index_affinity_to_position(
        &self,
        cx: &mut Cx2d,
        walk: Walk,
        _align: Align,
        width: f64,
        text: &str,
        target: IndexAffinity,
    ) -> DVec2 {
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return DVec2::new();
        }

        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return DVec2::new();
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas_ref = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas_ref;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache_ref = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache_ref;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        let fixed_width = if !walk.width.is_fit() {
            Some(width)
        } else {
            None
        };

        let wrap_width = if self.wrap == TextWrap::Word {
            fixed_width
        } else {
            None
        };

        let mut closest_position = None;
        let mut prev_glyph_end = 0;
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |position, start, event, font_atlas| {
                match event {
                    LayoutEvent::Chunk {
                        string,
                        glyph_infos,
                        ..
                    } => {
                        let mut position = position;
                        let mut iter = glyph_infos.iter().peekable();
                        while let Some(glyph_info) = iter.next() {
                            let glyph_start = start + glyph_info.cluster;
                            let glyph_end = start + iter.peek().map_or(string.len(), |glyph_info| glyph_info.cluster);
                            let glyph_width = compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas);
                            let grapheme_count = text[glyph_start..glyph_end].graphemes(true).count();
                            let glyph_width_per_grapheme = glyph_width / grapheme_count as f64;
                            for (grapheme_start, grapheme) in text[glyph_start..glyph_end].grapheme_indices(true) {
                                if target.index < glyph_start + grapheme_start + grapheme.len() {
                                    closest_position = Some(position);
                                    return true;
                                }
                                position.x += glyph_width_per_grapheme;
                            }
                            prev_glyph_end = glyph_end;
                        }
                    }
                    LayoutEvent::Newline { is_soft } => {
                        if target.index == prev_glyph_end && (!is_soft || target.affinity == Affinity::Before) {
                            closest_position = Some(position);
                            return true;
                        }
                        prev_glyph_end += 1;
                    }
                }
                false
            }
        );

        closest_position.unwrap_or(position)
    }

    fn draw_inner(&mut self, cx: &mut Cx2d, position: DVec2, line: &str, font_atlas: &mut CxFontAtlas) {
        self.char_depth = self.draw_depth;
        
        // If the line is empty, there is nothing to draw.
        if line.is_empty() {
            return;
        }

        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return;
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache_ref = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache_ref;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;
        
        let origin = position;
        let mut position = DVec2::new();
        layout_line(
            &mut position,
            line,
            0,
            line.len(),
            font_ids,
            font_size,
            line_spacing,
            None,
            font_atlas,
            shape_cache,
            |position, _, event, font_atlas| {
                if let LayoutEvent::Chunk {
                    glyph_infos,
                    ..
                } = event {
                    self.draw_glyphs(
                        cx,
                        origin + position,
                        font_size,
                        &glyph_infos, 
                        font_atlas
                    );
                }
                false
            }
        );
    }

    /// Draws the given text with the turtle, using the given walk and alignment.
    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        _align: Align,
        text: &str,
    ) {
        self.char_depth = self.draw_depth;
        
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return;
        }
        
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return;
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };
        
        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        // Compute the fixed width of the bounding box, if it has one.
        let fixed_width = if !walk.width.is_fit() {
            Some(cx.turtle().eval_width(walk.width, walk.margin, cx.turtle().layout().flow))
        } else {
            None
        };

        // Compute the fixed height of the bounding box, if it has one.
        let fixed_height = if !walk.height.is_fit() {
            Some(cx.turtle().eval_width(walk.width, walk.margin, cx.turtle().layout().flow))
        } else {
            None
        };

        // If word wrapping is enabled, set the wrap width to the fixed width of the bounding box.
        let wrap_width = if self.wrap == TextWrap::Word {
            fixed_width
        } else {
            None
        };

        // Lay out the text to compute the width and height of the bounding box.
        let mut max_width = 0.0;
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |position, _, event, _| {
                if let LayoutEvent::Chunk {
                    width,
                    ..
                } = event {
                    max_width = width.max(position.x + width);
                }
                false
            }
        );
        let mut width = max_width;
        let mut height = position.y + line_height;

        // If the bounding box has a fixed width, it overrides the computed width.
        if let Some(fixed_width) = fixed_width {
            width = fixed_width;
        }

        // If the bounding box has a fixed height, it overrides the computed height.
        if let Some(fixed_height) = fixed_height {
            height = fixed_height;
        }

        // Walk the turtle with the bounding box to obtain the draw rectangle.
        let rect = cx.walk_turtle(Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: Size::Fixed(width),
            height: Size::Fixed(height),
        });

        // cx.cx.debug.rect(rect, vec4(1.0, 0.0, 0.0, 1.0));
        
        // Lay out the text again to draw the glyphs in the draw rectangle.
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |position, _, event, font_atlas| {
                if let LayoutEvent::Chunk {
                    glyph_infos,
                    ..
                    //string,
                    //..
                } = event {
                    self.draw_glyphs(
                        cx,
                        rect.pos + position,
                        font_size,
                        glyph_infos,
                        font_atlas,
                    );
                }
                false
            }
        );

        // Unlock the instance buffer.
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }

    pub fn draw_walk_resumable(
        &mut self,
        cx: &mut Cx2d,
        text: &str,
    ) {
        self.draw_walk_resumable_with(cx, text, |_, _| {});
    }

    pub fn draw_walk_resumable_with(
        &mut self,
        cx: &mut Cx2d,
        text: &str,
        mut f: impl FnMut(&mut Cx2d, Rect)
    ) {
        self.char_depth = self.draw_depth;
        
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return
        }
        
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        let font_size = self.text_style.font_size * self.font_scale;
        let line_height = compute_line_height(font_ids, font_size, font_atlas) * self.text_style.line_scale;
        let line_spacing = line_height * self.text_style.line_spacing;

        let fixed_width = if cx.turtle().padded_rect().size.x.is_nan() {
            None
        } else {
            Some(cx.turtle().padded_rect().size.x)
        };

        let wrap_width = if cx.turtle().layout().flow == Flow::RightWrap {
            fixed_width
        } else {
            None
        };

        let mut prev_rect_slot: Option<Rect> = None;
        let mut position = DVec2::new();
        layout_text(
            &mut position,
            text,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            |_, _, event, font_atlas| {
                match event {
                    LayoutEvent::Chunk {
                        width,
                        glyph_infos,
                        ..
                    } => {
                        cx.set_turtle_wrap_spacing(line_spacing - line_height);
                        let rect = cx.walk_turtle(Walk {
                            abs_pos: None,
                            margin: Margin::default(),
                            width: Size::Fixed(width),
                            height: Size::Fixed(line_height)
                        });

                        self.draw_glyphs(
                            cx,
                            rect.pos,
                            font_size,
                            &glyph_infos,
                            font_atlas
                        );

                        if let Some(prev_rect) = &mut prev_rect_slot {
                            if prev_rect.pos.y == rect.pos.y {
                                prev_rect.size.x += rect.size.x;
                            } else {
                                f(cx, *prev_rect);
                                prev_rect_slot = Some(rect);
                            }
                        } else {
                            prev_rect_slot = Some(rect);
                        }
                    }
                    LayoutEvent::Newline { is_soft, .. }  => {
                        if !is_soft {
                            cx.turtle_new_line();
                        }
                    }
                }
                false
            }
        );
        if let Some(prev_rect) = prev_rect_slot {
            f(cx, prev_rect);
        }

        // Unlock the instance buffer.
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }

    /// Draws a sequence of glyphs, defined by the given list of glyph infos, at the given position.
    fn draw_glyphs(
        &mut self,
        cx: &mut Cx2d,
        position: DVec2,
        font_size: f64,
        glyph_infos: &[font_atlas::GlyphInfo],
        font_atlas: &mut CxFontAtlas,
    ) {
        // If the position is invalid, there is nothing to draw.
        if position.x.is_infinite() || position.x.is_nan() {
            return;
        }

        // If the list of glyph infos is empty, there is nothing to draw.
        if glyph_infos.is_empty() {
            return;
        }

        // If the shader failed to compile, there is nothing to draw.
        if !self.draw_vars.can_instance() {
            return;
        }

        // Lock the instance buffer.
        if !self.many_instances.is_some() {
            self.begin_many_instances_internal(cx, font_atlas);
        }
        let Some(mi) = &mut self.many_instances else {
            return;
        };
        
        // Get the device pixel ratio.
        let device_pixel_ratio = cx.current_dpi_factor();

        // Compute the glyph padding.
        let glyph_padding_dpx = 2.0;
        let glyph_padding_lpx = glyph_padding_dpx / device_pixel_ratio;

        
        let mut position = position;
        for glyph_info in glyph_infos {
            let font = font_atlas.fonts[glyph_info.font_id].as_mut().unwrap();
            let units_per_em = font.ttf_font.units_per_em;
            let ascender = units_to_lpxs(font.ttf_font.ascender, units_per_em, font_size) * self.text_style.line_scale;
            
            // Use the glyph id to get the glyph from the font.
            let glyph = font.owned_font_face.with_ref(|face| {
                font.ttf_font.get_glyph_by_id(face, glyph_info.glyph_id as usize).unwrap()
            });

            // Compute the position of the glyph.
            let glyph_position = dvec2(
                units_to_lpxs(glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_min.y, units_per_em, font_size),
            );
            
            // Compute the size of the bounding box of the glyph in logical pixels.
            let glyph_size_lpx = dvec2(
                units_to_lpxs(glyph.bounds.p_max.x - glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_max.y - glyph.bounds.p_min.y, units_per_em, font_size),
            );

            // Compute the size of the bounding box of the glyph in device pixels.
            let glyph_size_dpx = glyph_size_lpx * device_pixel_ratio;

            // Compute the padded size of the bounding box of the glyph in device pixels.
            let mut padded_glyph_size_dpx = glyph_size_dpx;
            if padded_glyph_size_dpx.x != 0.0 {
                padded_glyph_size_dpx.x = padded_glyph_size_dpx.x.ceil() + glyph_padding_dpx * 2.0;
            }
            if padded_glyph_size_dpx.y != 0.0 {
                padded_glyph_size_dpx.y = padded_glyph_size_dpx.y.ceil() + glyph_padding_dpx * 2.0;
            }

            // Compute the padded size of the bounding box of the glyph in logical pixels.
            let padded_glyph_size_lpx = padded_glyph_size_dpx / device_pixel_ratio;
            
            // Compute the left side bearing.
            let left_side_bearing = units_to_lpxs(glyph.horizontal_metrics.left_side_bearing, units_per_em, font_size);

            // Use the font size in device pixels to get the atlas page id from the font.
            let atlas_page_id = font.get_atlas_page_id(units_to_lpxs(1.0, units_per_em, font_size / self.font_scale) * device_pixel_ratio);

            // Use the atlas page id to get the atlas page from the font.
            let atlas_page = &mut font.atlas_pages[atlas_page_id];

            // Use the padded glyph size in device pixels to get the atlas glyph from the atlas page.
            let atlas_glyph = *atlas_page.atlas_glyphs.entry(glyph_info.glyph_id as usize).or_insert_with(|| {
                font_atlas
                    .alloc
                    .alloc_atlas_glyph(
                        padded_glyph_size_dpx.x / self.font_scale,
                        padded_glyph_size_dpx.y / self.font_scale,
                        CxFontsAtlasTodo {
                            font_id: glyph_info.font_id,
                            atlas_page_id,
                            glyph_id: glyph_info.glyph_id as usize,
                        }
                    )
            });

            // Compute the distance from the current position to the draw rectangle.
            let delta = dvec2(
                left_side_bearing - glyph_padding_lpx,
                ascender - glyph_position.y + glyph_padding_lpx
            );

            // Compute the advance width.
            let advance_width = compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, self.text_style.font_size, font_atlas);
            
            // Emit the instance data.
            self.font_t1 = atlas_glyph.t1;
            self.font_t2 = atlas_glyph.t2;
            self.rect_pos = (position + delta).into();
            self.rect_size = padded_glyph_size_lpx.into();
            mi.instances.extend_from_slice(self.draw_vars.as_slice());

            self.char_depth += ZBIAS_STEP;
            
            // Advance to the next position.
            position.x += advance_width;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IndexAffinity {
    pub index: usize,
    pub affinity: Affinity,
}

impl IndexAffinity {
    pub fn new(index: usize, affinity: Affinity) -> Self {
        IndexAffinity { index, affinity }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Affinity {
    Before,
    After,
}

impl Default for Affinity {
    fn default() -> Self {
        Affinity::Before
    }
}

fn layout_text(
    position: &mut DVec2,
    text: &str,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    wrap_width: Option<f64>,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, usize, LayoutEvent, &mut CxFontAtlas) -> bool,
) -> bool {
    for (index, line) in lines(text).enumerate() {
        let line_start = line.as_ptr() as usize - text.as_ptr() as usize;
        let line_end = line_start + line.len();
        if index > 0 {
            if f(*position, line_start, LayoutEvent::Newline { is_soft: false }, font_atlas) {
                return true;
            }
            position.x = 0.0;
            position.y += line_spacing;
        }
        if layout_line(
            position,
            text,
            line_start,
            line_end,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas,
            shape_cache,
            &mut f,
        ) {
            return true;
        }
    }
    false
}

fn layout_line(
    position: &mut DVec2,
    text: &str,
    line_start: usize,
    line_end: usize,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    wrap_width: Option<f64>,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, usize, LayoutEvent, &mut CxFontAtlas) -> bool,
) -> bool {
    let line = &text[line_start..line_end];
    for (index, word) in words(line).enumerate() {
        let word_start = word.as_ptr() as usize - text.as_ptr() as usize;
        let word_end = word_start + word.len();
        if layout_word(
            position,
            index == 0,
            text,
            word_start,
            word_end,
            font_ids,
            font_size,
            line_spacing,
            wrap_width,
            font_atlas, 
            shape_cache,
            &mut f,
        ) {
            return true;
        }
    }
    false
}

fn layout_word(
    position: &mut DVec2,
    is_first: bool,
    text: &str,
    word_start: usize,
    word_end: usize,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    wrap_width: Option<f64>,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, usize, LayoutEvent, &mut CxFontAtlas) -> bool,
) -> bool {
    let word = &text[word_start..word_end];
    let glyph_infos = shape(word, font_ids, font_atlas, shape_cache);
    let width: f64 = glyph_infos.iter().map(|glyph_info| {
        compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas)
    }).sum();
    if wrap_width.map_or(false, |wrap_width| position.x + width > wrap_width) && !is_first {
        if f(*position, word_start, LayoutEvent::Newline { is_soft: true }, font_atlas) {
            return true;
        }
        position.x = 0.0;
        position.y += line_spacing;
    }
    if wrap_width.map_or(false, |wrap_width| position.x + width > wrap_width) {
        for (index, grapheme) in graphemes(word).enumerate() {
            let grapheme_start = grapheme.as_ptr() as usize - text.as_ptr() as usize;
            let grapheme_end = grapheme_start + grapheme.len();
            if layout_grapheme(
                position,
                index == 0,
                text,
                grapheme_start,
                grapheme_end,
                font_ids,
                font_size,
                line_spacing,
                wrap_width,
                font_atlas,
                shape_cache,
                &mut f,
            ) {
                return true;
            }
        }
    } else {
        if f(*position, word_start, LayoutEvent::Chunk {
            width,
            string: word,
            glyph_infos
        }, font_atlas) {
            return true;
        }
        position.x += width;
    }
    false
}

fn layout_grapheme(
    position: &mut DVec2,
    is_first: bool,
    text: &str,
    grapheme_start: usize,
    grapheme_end: usize,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    wrap_width: Option<f64>,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache, 
    mut f: impl FnMut(DVec2, usize, LayoutEvent, &mut CxFontAtlas) -> bool,
) -> bool {
    let grapheme = &text[grapheme_start..grapheme_end];
    let glyph_infos = shape(grapheme, font_ids, font_atlas, shape_cache);
    let width: f64 = glyph_infos.iter().map(|glyph_info| {
        compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas)
    }).sum();
    if wrap_width.map_or(false, |wrap_width| position.x + width > wrap_width) && !is_first {
        if f(*position, grapheme_start, LayoutEvent::Newline { is_soft: true }, font_atlas) {
            return true;
        }
        position.x = 0.0;
        position.y += line_spacing;
    }
    if f(*position, grapheme_start, LayoutEvent::Chunk {
        width,
        string: grapheme,
        glyph_infos
    }, font_atlas) {
        return true;
    }
    position.x += width;
    false
}

enum LayoutEvent<'a> {
    Chunk {
        width: f64,
        string: &'a str,
        glyph_infos: &'a [font_atlas::GlyphInfo],
    },
    Newline {
        is_soft: bool
    }
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    split_at_indices(line, break_opportunities(line))
}

fn graphemes(word: &str) -> impl Iterator<Item = &str> {
    use unicode_segmentation::UnicodeSegmentation;

    word.graphemes(true)
}

fn break_opportunities(line: &str) -> impl Iterator<Item = usize> + '_ {
    unicode_linebreak::linebreaks(line).map(|(index, _)| index).chain(if line.is_empty() {
        Some(0)
    } else {
        None
    })
}

fn split_at_indices(
    string: &str,
    indices: impl IntoIterator<Item = usize>
) -> impl Iterator<Item = &str> {
    indices.into_iter().scan(0, |start, end| {
        let substring = &string[*start..end];
        *start = end;
        Some(substring)
    })
}

fn compute_line_height(
    font_ids: &[usize],
    font_size: f64,
    font_atlas: &CxFontAtlas,
) -> f64 {
    font_ids.iter().map(|&font_id| {
        let font = font_atlas.fonts[font_id].as_ref().unwrap();
        let units_per_em = font.ttf_font.units_per_em;
        let line_height = font.ttf_font.ascender - font.ttf_font.descender;
        units_to_lpxs(line_height, units_per_em, font_size)
    }).reduce(|a, b| a.max(b)).unwrap()
}

fn compute_glyph_width(
    font_id: usize,
    glyph_id: usize,
    font_size: f64,
    font_atlas: &mut CxFontAtlas,
) -> f64 {
    let font = font_atlas.fonts[font_id].as_mut().unwrap();
    let units_per_em = font.ttf_font.units_per_em;
    let glyph_width = font.owned_font_face.with_ref(|face| {
        let glyph = font.ttf_font.get_glyph_by_id(face, glyph_id as usize).unwrap();
        glyph.horizontal_metrics.advance_width
    });
    units_to_lpxs(glyph_width, units_per_em, font_size)
}

fn units_to_lpxs(units: f64, units_per_em: f64, font_size: f64) -> f64 {
    const LPXS_PER_IN: f64 = 96.0;
    const PTS_PER_IN: f64 = 72.0;

    let ems = units / units_per_em;
    let pts = ems * font_size;
    let ins = pts / PTS_PER_IN;
    let lpxs = ins * LPXS_PER_IN;
    lpxs
}

fn shape<'a>(
    string: &str,
    font_ids: &[usize],
    font_atlas: &CxFontAtlas,
    shape_cache: &'a mut CxShapeCache,
) -> &'a [font_atlas::GlyphInfo] {
    shape_cache.shape(
        Direction::LeftToRight,
        string,
        font_ids,
        font_atlas
    )
}