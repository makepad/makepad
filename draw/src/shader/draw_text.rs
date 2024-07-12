use {
    crate::{
        cx_2d::Cx2d, draw_list_2d::ManyInstances, font_atlas::{self, CxFontAtlas, CxFontsAtlasTodo, CxShapeCache, Font}, geometry::GeometryQuad2D, makepad_platform::*, turtle::{Align, Size, Walk}
    },
    makepad_rustybuzz::Direction,
    std::mem,
};

const ZBIAS_STEP: f32 = 0.00001;

live_design!{
    
    DrawText = {{DrawText}} {
        //debug: true;
        color: #fff
        
        uniform brightness: float
        uniform curve: float
        uniform sdf_radius: float
        uniform sdf_cutoff: float
        
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
        
        fn sample_color(self, scale:float, pos:vec2)->vec4{
            let s = sample2d(self.tex, pos).x;
            if (self.sdf_radius != 0.0) {
                // HACK(eddyb) harcoded atlas size (see asserts below).
                let texel_coords = pos.xy * 4096.0;
                s = clamp((s - (1.0 - self.sdf_cutoff)) * self.sdf_radius / scale + 0.5, 0.0, 1.0);
            } else {
                s = pow(s, self.curve);
            }
            let col = self.get_color(); 
            return self.blend_color(vec4(s * col.rgb * self.brightness * col.a, s * col.a));
        }
        
        fn pixel(self) -> vec4 {
            let texel_coords = self.tex_coord1.xy;
            let dxt = length(dFdx(texel_coords));
            let dyt = length(dFdy(texel_coords));
            let scale = (dxt + dyt) * 4096.0 *0.5;
            return self.sample_color(scale, self.tex_coord1.xy); // + vec4(1.0, 0.0, 0.0, 0.0);
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
    #[live(9.0)] pub font_size: f64,
    #[live(1.0)] pub brightness: f32,
    #[live(0.5)] pub curve: f32,
    #[live(0.88)] pub line_scale: f64,
    #[live(1.4)] pub line_spacing: f64,
    #[live(1.1)] pub top_drop: f64,
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
    #[calc] pub delta: Vec2,
    #[calc] pub shader_font_size: f32,
    #[calc] pub advance: f32,
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
        self.draw_vars.user_uniforms[0] = self.text_style.brightness;
        self.draw_vars.user_uniforms[1] = self.text_style.curve;
        let (sdf_radius, sdf_cutoff) = font_atlas.alloc.sdf.as_ref()
            .map_or((0.0, 0.0), |sdf| (sdf.params.radius, sdf.params.cutoff));
        self.draw_vars.user_uniforms[2] = sdf_radius;
        self.draw_vars.user_uniforms[3] = sdf_cutoff;
    }
    
    fn draw_inner(&mut self, cx: &mut Cx2d, position: DVec2, chunk: &str, font_atlas: &mut CxFontAtlas) {
        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        // Take the glyph info vector from the context.
        let mut glyph_infos = mem::take(&mut cx.glyph_infos);

        for glyph_info in shape_cache.shape(
            Direction::LeftToRight,
            chunk,
            &[self.text_style.font.font_id.unwrap()],
            font_atlas
        ) {
            glyph_infos.push(GlyphInfo {
                font_id: glyph_info.font_id,
                glyph_id: glyph_info.glyph_id,
                width: compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, self.text_style.font_size, font_atlas),
                start: 0,
                end: 0,
            });
        }

        // Draw the glyphs.
        self.draw_glyphs(cx, position, &glyph_infos, font_atlas);

        // Clear glyph info vector and put it back onto the context.
        glyph_infos.clear();
        cx.glyph_infos = glyph_infos;
    }

    pub fn closest_offset(&self, cx: &Cx, newline_indexes: Vec<usize>, pos: DVec2) -> Option<usize> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }

        let line_spacing = self.get_line_spacing();
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        let delta = area.get_read_ref(cx, live_id!(delta), ShaderTy::Vec2).unwrap();
        let advance = area.get_read_ref(cx, live_id!(advance), ShaderTy::Float).unwrap();

        let mut last_y = None;
        let mut newlines = 0;
        for i in 0..rect_pos.repeat {
            if newline_indexes.contains(&(i + newlines)) {
                newlines += 1;
            }

            let index = rect_pos.stride * i;
            let x = rect_pos.buffer[index + 0] as f64 - delta.buffer[index + 0] as f64;

            let y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            if last_y.is_none() {last_y = Some(y)}
            let advance = advance.buffer[index + 0] as f64;
            if i > 0 && (y - last_y.unwrap()) > 0.001 && pos.y < last_y.unwrap() as f64 + line_spacing as f64 {
                return Some(i - 1 + newlines)
            }
            if pos.x < x + advance * 0.5 && pos.y < y as f64 + line_spacing as f64 {
                return Some(i + newlines)
            }
            last_y = Some(y)
        }
        return Some(rect_pos.repeat + newlines);
        
    }
    
    pub fn get_selection_rects(&self, cx: &Cx, newline_indexes: Vec<usize>, start: usize, end: usize, shift: DVec2, pad: DVec2) -> Vec<Rect> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return Vec::new();
        }

        // Adjustments because of newlines characters (they are not in the buffers)
        let start_offset = newline_indexes.iter().filter(|&&i| i < start).count();
        let start = start - start_offset;
        let end_offset = newline_indexes.iter().filter(|&&i| i < end).count();
        let end = end - end_offset;
        
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        let delta = area.get_read_ref(cx, live_id!(delta), ShaderTy::Vec2).unwrap();
        let advance = area.get_read_ref(cx, live_id!(advance), ShaderTy::Float).unwrap();
        
        if rect_pos.repeat == 0 || start >= rect_pos.repeat{
            return Vec::new();
        }
        // alright now we go and walk from start to end and collect our selection rects
        
        let index = start * rect_pos.stride;
        let start_x = rect_pos.buffer[index + 0] - delta.buffer[index + 0]; // + advance.buffer[index + 0] * pos;
        let start_y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
        let line_spacing = self.get_line_spacing();
        let mut last_y = start_y;
        let mut min_x = start_x;
        let mut last_x = start_x;
        let mut last_advance = advance.buffer[index + 0];
        let mut out = Vec::new();
        for index in start..end {
            if index >= rect_pos.repeat{
                break;
            }
            let index = index * rect_pos.stride;
            let end_x = rect_pos.buffer[index + 0] - delta.buffer[index + 0];
            let end_y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            last_advance = advance.buffer[index + 0];
            if end_y > last_y { // emit rect
                out.push(Rect {
                    pos: dvec2(min_x as f64, last_y as f64) + shift,
                    size: dvec2((last_x - min_x + last_advance) as f64, line_spacing) + pad
                });
                min_x = end_x;
                last_y = end_y;
            }
            last_x = end_x;
        }
        out.push(Rect {
            pos: dvec2(min_x as f64, last_y as f64) + shift,
            size: dvec2((last_x - min_x + last_advance) as f64, line_spacing) + pad
        });
        out
    }
    
    pub fn get_char_count(&self, cx: &Cx) -> usize {
        let area = &self.draw_vars.area;
        if !area.is_valid(cx) {
            return 0
        }
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        rect_pos.repeat
    }
    
    pub fn get_cursor_pos(&self, cx: &Cx, newline_indexes: Vec<usize>, pos: f32, index: usize) -> Option<DVec2> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }
        // Adjustment because of newlines characters (they are not in the buffers)
        let index_offset = newline_indexes.iter().filter(|&&i| i < index).count();
        let (index, pos) = if newline_indexes.contains(&(index)){
            (index - index_offset - 1, pos + 1.0)
        } else {
            (index - index_offset, pos)
        };
        
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        let delta = area.get_read_ref(cx, live_id!(delta), ShaderTy::Vec2).unwrap();
        let advance = area.get_read_ref(cx, live_id!(advance), ShaderTy::Float).unwrap();
        
        if rect_pos.repeat == 0 {
            return None
        }
        if index >= rect_pos.repeat {
            // lets get the last one and advance
            let index = (rect_pos.repeat - 1) * rect_pos.stride;
            let x = rect_pos.buffer[index + 0] - delta.buffer[index + 0] + advance.buffer[index + 0];
            let y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            Some(dvec2(x as f64, y as f64))
        }
        else {
            let index = index * rect_pos.stride;
            let x = rect_pos.buffer[index + 0] - delta.buffer[index + 0] + advance.buffer[index + 0] * pos;
            let y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            Some(dvec2(x as f64, y as f64))
        }
    }
    
    pub fn get_line_spacing(&self) -> f64 {
        self.text_style.font_size * self.text_style.height_factor * self.font_scale * self.text_style.line_spacing
    }
    
    pub fn get_font_size(&self) -> f64 {
        self.text_style.font_size * self.font_scale
    }
    
    pub fn get_monospace_base(&self, cx: &Cx2d) -> DVec2 {
        let mut fonts_atlas = cx.fonts_atlas_rc.0.borrow_mut();
        if self.text_style.font.font_id.is_none() {
            return DVec2::default();
        }
        let font_id = self.text_style.font.font_id.unwrap();
        if fonts_atlas.fonts[font_id].is_none() {
            return DVec2::default();
        }
        let font = fonts_atlas.fonts[font_id].as_mut().unwrap();
        let slot = font.owned_font_face.with_ref( | face | face.glyph_index('!').map_or(0, | id | id.0 as usize));
        let glyph = font.get_glyph_by_id(slot).unwrap();
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        DVec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.ttf_font.units_per_em)),
            y: self.text_style.line_spacing
        }
    }
}

impl DrawText {
    pub fn pick_walk(
        &mut self,
        cx: &mut Cx,
        walk: Walk,
        _align: Align,
        text: &str,
        position: DVec2,
    ) -> Option<usize> {
        let draw_event = DrawEvent::default();
        let cx = &mut Cx2d::new(cx, &draw_event);

        // If the text is empty, there is nothing to pick.
        if text.is_empty() {
            return None;
        }

        // If the font did not load, there is nothing to pick.
        let Some(font_id) = self.text_style.font.font_id else {
            return None;
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        // Take the line, word, and glyph info vectors from the context.
        let mut line_infos = mem::take(&mut cx.line_infos);
        let mut word_infos = mem::take(&mut cx.word_infos);
        let mut glyph_infos = mem::take(&mut cx.glyph_infos);

        // Compute info for each line, word, and glyph in the text.
        compute_infos(
            text,
            &[font_id],
            self.text_style.font_size,
            &mut line_infos,
            &mut word_infos,
            &mut glyph_infos,
            font_atlas,
            shape_cache,
        );

        let wrap_width = None;

        // Walk over the words of the text.
        let mut index = None;
        let font_scale = self.font_scale;
        let line_scale = self.text_style.line_scale;
        let line_spacing = self.text_style.line_spacing;
        let mut prev_end = None;
        println!("Looking for {:?}", position);
        walk_words(
            wrap_width,
            font_scale,
            line_scale,
            line_spacing,
            &line_infos,
            &word_infos,
            &glyph_infos,
            |current, line_info, glyph_infos| {
                let height = line_info.height * font_scale * line_scale * line_spacing;
                let mut current = current;
                for glyph_info in glyph_infos {
                    let width = glyph_info.width * font_scale;
                    if let Some(prev_end) = prev_end {
                        if position.y < current.y {
                            index = Some(prev_end);
                            return true;
                        }
                    }
                    println!(" {:?} < {:?} && {:?} < {:?}?", position.x, current.x + width * 0.5, position.y, current.y + height);
                    if position.x < current.x + width * 0.5 && position.y < current.y + height {
                        index = Some(glyph_info.start);
                        return true;
                    }
                    prev_end = Some(glyph_info.end);
                    current.x += width;
                }
                false
            }
        );
        Some(text[..index.unwrap_or(text.len())].chars().count())
    }

    /// Draws the given text with the turtle, using the given walk and alignment.
    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        _align: Align,
        text: &str,
    ) {
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return;
        }
        
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return;
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        // Take the line, word, and glyph info vectors from the context.
        let mut line_infos = mem::take(&mut cx.line_infos);
        let mut word_infos = mem::take(&mut cx.word_infos);
        let mut glyph_infos = mem::take(&mut cx.glyph_infos);
        
        // Compute info for each line, word, and glyph in the text.
        compute_infos(
            text,
            &[font_id],
            self.text_style.font_size,
            &mut line_infos,
            &mut word_infos,
            &mut glyph_infos,
            font_atlas,
            shape_cache,
        );

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
        let wrap_width = if !walk.width.is_fit() && self.wrap == TextWrap::Word {
            fixed_width
        } else {
            None
        };

        // Walk over the words of the text to determine the actual size of the bounding box.
        let mut size = walk_words(
            wrap_width,
            self.font_scale,
            self.text_style.line_scale,
            self.text_style.line_spacing,
            &line_infos,
            &word_infos,
            &glyph_infos,
            |_, _, _| false
        );

        // If the bounding box has a fixed width, it overrides the actual width.
        if let Some(fixed_width) = fixed_width {
            size.x = fixed_width;
        }

        // If the bounding box has a fixed height, it overrides the actual height.
        if let Some(fixed_height) = fixed_height {
            size.y = fixed_height;
        }

        // Walk the turtle with the bounding box.
        let rect = cx.walk_turtle(Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: Size::Fixed(size.x),
            height: Size::Fixed(size.y),
        });

        // cx.cx.debug.rect(rect, vec4(1.0, 0.0, 0.0, 1.0));
        
        // Walk over the words of the text to draw the glyphs.
        walk_words(
            wrap_width,
            self.font_scale,
            self.text_style.line_scale,
            self.text_style.line_spacing,
            &line_infos,
            &word_infos,
            &glyph_infos,
            |position, _, glyph_infos| {
                self.draw_glyphs(
                    cx,
                    dvec2(rect.pos.x + position.x, rect.pos.y + position.y),
                    glyph_infos,
                    font_atlas
                );
                false
            }
        );

        // Unlock the instance buffer.
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }

        // Clear the line, word, and glyph info vectors and put them back onto the context.
        line_infos.clear();
        word_infos.clear();
        glyph_infos.clear();
        cx.line_infos = line_infos;
        cx.word_infos = word_infos;
        cx.glyph_infos = glyph_infos;
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
        // If the text is empty, there is nothing to draw.
        if text.is_empty() {
            return
        }
        
        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return
        };

        // Borrow the font atlas from the context.
        let font_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut font_atlas = font_atlas_rc.0.borrow_mut();
        let font_atlas = &mut *font_atlas;

        // Borrow the shape cache from the context.
        let shape_cache_rc = cx.shape_cache_rc.clone();
        let mut shape_cache = shape_cache_rc.0.borrow_mut();
        let shape_cache = &mut *shape_cache;

        // Take the line, word, and glyph info vectors from the context.
        let mut line_infos = mem::take(&mut cx.line_infos);
        let mut word_infos = mem::take(&mut cx.word_infos);
        let mut glyph_infos = mem::take(&mut cx.glyph_infos);

        // Compute info vectors for each line, word, and glyph in the text.
        compute_infos(
            text,
            &[font_id],
            self.text_style.font_size,
            &mut line_infos,
            &mut word_infos,
            &mut glyph_infos,
            font_atlas,
            shape_cache,
        );

        let mut prev_rect_slot: Option<Rect> = None;
        for line_info in &line_infos {
            for word_info in &word_infos[line_info.word_info_start..line_info.word_info_end] {
                let rect = cx.walk_turtle(Walk {
                    abs_pos: None,
                    margin: Margin::default(),
                    width: Size::Fixed(word_info.width),
                    height: Size::Fixed(line_info.height * self.text_style.top_drop)
                });

                if let Some(prev_rect) = &mut prev_rect_slot {
                    if prev_rect.pos.y == rect.pos.y {
                        prev_rect.size.x += rect.size.x;
                    } else {
                        f(cx, rect);
                        prev_rect_slot = Some(rect);
                    }
                } else {
                    prev_rect_slot = Some(rect);
                }

                self.draw_glyphs(
                    cx,
                    rect.pos,
                    &glyph_infos[word_info.glyph_info_start..word_info.glyph_info_end],
                    font_atlas
                );
            }
            cx.turtle_new_line();
        }
        if let Some(prev_rect) = prev_rect_slot {
            f(cx, prev_rect);
        }

        // Unlock the instance buffer.
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }

        // Clear the line, word, and glyph info vectors and put them back onto the context.
        line_infos.clear();
        word_infos.clear();
        glyph_infos.clear();
        cx.line_infos = line_infos;
        cx.word_infos = word_infos;
        cx.glyph_infos = glyph_infos;
    }

    /// Draws a sequence of glyphs, defined by the given list of glyph infos, at the given position.
    fn draw_glyphs(
        &mut self,
        cx: &mut Cx2d,
        position: DVec2,
        glyph_infos: &[GlyphInfo],
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
        
        let font_size = self.text_style.font_size;

        self.char_depth = self.draw_depth;
        let mut position = position;
        for glyph_info in glyph_infos {
            let font = font_atlas.fonts[glyph_info.font_id].as_mut().unwrap();
            let units_per_em = font.ttf_font.units_per_em;

            // Compute the ascender.
            let ascender = units_to_lpxs(font.ttf_font.ascender, units_per_em, font_size);
            
            // Use the glyph id to get the glyph from the font.
            let glyph = font.owned_font_face.with_ref(|face| {
                font.ttf_font.get_glyph_by_id(face, glyph_info.glyph_id as usize).unwrap()
            });

            // Compute the glyph position.
            let glyph_position = dvec2(
                units_to_lpxs(glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_min.y, units_per_em, font_size),
            );
            
            // Compute the glyph size in logical pixels.
            let glyph_size_lpx = dvec2(
                units_to_lpxs(glyph.bounds.p_max.x - glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_max.y - glyph.bounds.p_min.y, units_per_em, font_size),
            );

            // Compute the glyph size in device pixels.
            let glyph_size_dpx = glyph_size_lpx * device_pixel_ratio;

            // Compute the padded glyph size in device pixels.
            let mut padded_glyph_size_dpx = glyph_size_dpx;
            if padded_glyph_size_dpx.x != 0.0 {
                padded_glyph_size_dpx.x += glyph_padding_dpx * 2.0;
            }
            if padded_glyph_size_dpx.y != 0.0 {
                padded_glyph_size_dpx.y += glyph_padding_dpx * 2.0;
            }

            // Compute the padded glyph size in logical pixels.
            let padded_glyph_size_lpx = padded_glyph_size_dpx / device_pixel_ratio;
            
            // Compute the left side bearing.
            let left_side_bearing = units_to_lpxs(glyph.horizontal_metrics.left_side_bearing, units_per_em, font_size);
            
            // Use the font size in device pixels to get the atlas page id from the font.
            let atlas_page_id = font.get_atlas_page_id(units_to_lpxs(1.0, units_per_em, font_size) * device_pixel_ratio);

            // Use the atlas page id to get the atlas page from the font.
            let atlas_page = &mut font.atlas_pages[atlas_page_id];

            // Use the padded glyph size in device pixels to get the atlas glyph from the atlas page.
            let atlas_glyph = *atlas_page.atlas_glyphs.entry(glyph_info.glyph_id as usize).or_insert_with(|| {
                font_atlas
                    .alloc
                    .alloc_atlas_glyph(
                        padded_glyph_size_dpx.x,
                        padded_glyph_size_dpx.y,
                        CxFontsAtlasTodo {
                            font_id: glyph_info.font_id,
                            atlas_page_id,
                            glyph_id: glyph_info.glyph_id as usize,
                        }
                    )
            });

            // Compute the distance from the current position to the rect.
            let delta = dvec2(
                left_side_bearing * self.font_scale,
                (ascender - glyph_position.y) * self.font_scale,
            ) * self.font_scale - glyph_padding_lpx;
            
            // Emit the instance data.
            self.font_t1 = atlas_glyph.t1;
            self.font_t2 = atlas_glyph.t2;
            self.rect_pos = (position + delta).into();
            self.rect_size = (padded_glyph_size_lpx * self.font_scale).into();
            self.delta.x = delta.x as f32;
            self.delta.y = delta.y as f32;
            self.advance = (glyph_info.width * self.font_scale) as f32;
            mi.instances.extend_from_slice(self.draw_vars.as_slice());

            self.char_depth += ZBIAS_STEP;
            
            // Advance to the next position.
            position.x += glyph_info.width * self.font_scale;
        }
    }
}

// Info about a line in a text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineInfo {
    word_info_start: usize,
    word_info_end: usize,
    height: f64,
}

// Info about a word in a text.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WordInfo {
    glyph_info_start: usize,
    glyph_info_end: usize,
    width: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphInfo {
    font_id: usize,
    glyph_id: usize,
    width: f64,
    start: usize,
    end: usize,
}

/// Computes info vectors for each line, word, and glyph in the text.
fn compute_infos(
    text: &str,
    font_ids: &[usize],
    font_size: f64,
    line_infos: &mut Vec<LineInfo>,
    word_infos: &mut Vec<WordInfo>,
    glyph_infos: &mut Vec<GlyphInfo>,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
) {
    for (line_start, line_end) in line_ranges(text) {
        let line = &text[line_start..line_end];

        let word_info_start = word_infos.len();
        for (word_start, word_end) in word_ranges(line) {
            let word_start = line_start + word_start;
            let word_end = line_start + word_end;

            let glyph_info_start = glyph_infos.len();
            let mut iter = shape_cache.shape(
                Direction::LeftToRight,
                &text[word_start..word_end],
                font_ids,
                font_atlas
            ).iter().peekable();
            while let Some(glyph_info) = iter.next() {
                glyph_infos.push(GlyphInfo {
                    font_id: glyph_info.font_id,
                    glyph_id: glyph_info.glyph_id,
                    width: compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas),
                    start: glyph_info.cluster + word_start,
                    end: iter.peek().map_or(word_end, |glyph_info| glyph_info.cluster + word_start)
                });
            }

            word_infos.push(WordInfo {
                glyph_info_start,
                glyph_info_end: glyph_infos.len(),
                width: glyph_infos[glyph_info_start..].iter().map(|glyph_info| glyph_info.width).sum(),
            });
        }

        let font = &font_atlas.fonts[font_ids[0]].as_ref().unwrap();
        let units_per_em = font.ttf_font.units_per_em;
        line_infos.push(LineInfo {
            word_info_start,
            word_info_end: word_infos.len(),
            height: compute_line_height(font_ids[0], font_size, font_atlas)
        });
    }
}

// Returns an iterator over the range of each line in the given text.
fn line_ranges(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    text
        .lines()
        .scan(0, |start, line| {
            let end = *start + line.len();
            let range = (*start, end);
            *start = end + 1;
            Some(range)
        })
}

// Returns an iterator over the range of each word in the given line.
fn word_ranges(line: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    unicode_linebreak::linebreaks(line)
        .map(|(index, _)| index)
        .scan(0, |start, end| {
            let range = (*start, end);
            *start = end;
            Some(range)
        })
}

/// Walk the words in a text using the given line, word, and glyph info vectors.
///
/// This function also takes several 'fudge factors' which can be used to tweak the way the text is
/// layed out:
/// - font_scale: used to scale the width of every glyph and the height of every line.
/// - line_scale: used to scale the height of every line.
/// - line_spacing: used to scale the height of every line except the last.
fn walk_words(
    wrap_width: Option<f64>,
    font_scale: f64,
    line_scale: f64,
    line_spacing: f64,
    line_infos: &[LineInfo],
    word_infos: &[WordInfo],
    glyph_infos: &[GlyphInfo],
    mut f: impl FnMut(DVec2, &LineInfo, &[GlyphInfo]) -> bool,
) -> DVec2 {
    let mut width = 0.0;
    let mut position = DVec2::new();
    for (index, line_info) in line_infos.iter().enumerate() {
        for word_info in &word_infos[line_info.word_info_start..line_info.word_info_end] {
            if let Some(max_width) = wrap_width {
                if position.x + word_info.width * font_scale > max_width && position.x > 0.0 {
                    position.x = 0.0;
                    position.y += line_info.height * font_scale * line_scale * line_spacing;
                }
            }
            if f(position, line_info, &glyph_infos[word_info.glyph_info_start..word_info.glyph_info_end]) {
                return dvec2(width, position.y);
            }
            position.x += word_info.width * font_scale
        }
        width = width.max(position.x);
        position.x = 0.0;
        position.y += line_info.height * font_scale * line_scale * if index == line_infos.len() - 1 {
            1.0
        } else {
            line_spacing
        };
    }
    dvec2(width, position.y)
}

fn layout_text(
    position: &mut DVec2,
    text: &str,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    max_width: f64,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, f64, &[font_atlas::GlyphInfo]),
) {
    for line in lines(text) {
        layout_line(
            position,
            line,
            font_ids,
            font_size,
            line_spacing,
            max_width,
            font_atlas,
            shape_cache,
            &mut f,
        );
        position.x = 0.0;
        position.y += line_spacing;
    }
}

fn layout_line(
    position: &mut DVec2,
    line: &str,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    max_width: f64,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, f64, &[font_atlas::GlyphInfo]),
) {
    for (index, word) in words(line).enumerate() {
        layout_word(
            position,
            index == 0,
            word,
            font_ids,
            font_size,
            line_spacing,
            max_width,
            font_atlas, 
            shape_cache,
            &mut f,
        );
    }
}

fn layout_word(
    position: &mut DVec2,
    is_first: bool,
    word: &str,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    max_width: f64,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, f64, &[font_atlas::GlyphInfo]),
) {
    let glyph_infos = shape(word, font_ids, font_atlas, shape_cache);
    let word_width: f64 = glyph_infos.iter().map(|glyph_info| {
        compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas)
    }).sum();
    if position.x + word_width > max_width && !is_first {
        position.x = 0.0;
        position.y += line_spacing;
    }
    if position.x + word_width > max_width {
        for (index, grapheme) in graphemes(word).enumerate() {
            layout_grapheme(
                position,
                index == 0,
                grapheme,
                font_ids,
                font_size,
                line_spacing,
                max_width,
                font_atlas,
                shape_cache,
                &mut f,
            );
        }
    } else {
        f(*position, word_width, glyph_infos);
        position.x += word_width;
    }
}

fn layout_grapheme(
    position: &mut DVec2,
    is_first: bool,
    grapheme: &str,
    font_ids: &[usize],
    font_size: f64,
    line_spacing: f64,
    max_width: f64,
    font_atlas: &mut CxFontAtlas,
    shape_cache: &mut CxShapeCache,
    mut f: impl FnMut(DVec2, f64, &[font_atlas::GlyphInfo]),
) {
    let glyph_infos = shape(grapheme, font_ids, font_atlas, shape_cache);
    let grapheme_width: f64 = glyph_infos.iter().map(|glyph_info| {
        compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, font_size, font_atlas)
    }).sum();
    if position.x + grapheme_width > max_width && !is_first {
        position.x = 0.0;
        position.y += line_spacing;
    }
    f(*position, grapheme_width, glyph_infos);
    position.x += grapheme_width;
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
    unicode_linebreak::linebreaks(line).map(|(index, _)| index)
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
    font_id: usize,
    font_size: f64,
    font_atlas: &CxFontAtlas,
) -> f64 {
    let font = font_atlas.fonts[font_id].as_ref().unwrap();
    let units_per_em = font.ttf_font.units_per_em;
    let line_height = font.ttf_font.ascender - font.ttf_font.descender;
    units_to_lpxs(line_height, units_per_em, font_size)
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