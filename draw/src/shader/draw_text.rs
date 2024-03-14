use {
    crate::{
        makepad_platform::*,
        turtle::{Walk, Size, Align},
        font_atlas::{CxFontsAtlasTodo, CxFont, CxFontsAtlas, Font},
        draw_list_2d::ManyInstances,
        geometry::GeometryQuad2D,
        cx_2d::Cx2d
    },
};


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
            return self.sample_color(scale, self.tex_coord1.xy);
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

#[derive(Clone, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live(9.0)] pub font_size: f64,
    #[live(1.0)] pub brightness: f32,
    #[live(0.5)] pub curve: f32,
    #[live(1.4)] pub line_spacing: f64,
    #[live(1.1)] pub top_drop: f64,
    #[live(1.3)] pub height_factor: f64,
}

#[derive(Clone, Live, LiveHook)]
#[live_ignore]
pub enum TextWrap {
    #[pick] Ellipsis,
    Word,
    Line
}

struct WordIterator<'a> {
    char_iter: Option<std::str::CharIndices<'a >>,
    eval_width: f64,
    word_width: f64,
    word_start: usize,
    last_is_whitespace: bool,
    last_char: char,
    last_index: usize,
    font_size_total: f64,
}

struct WordIteratorItem {
    start: usize,
    end: usize,
    width: f64,
    with_newline: bool
}

impl<'a> WordIterator<'a> {
    fn new(char_iter: std::str::CharIndices<'a>, eval_width: f64, font_size_total: f64) -> Self {
        Self {
            eval_width,
            char_iter: Some(char_iter),
            last_is_whitespace: false,
            word_width: 0.0,
            word_start: 0,
            last_char: '\0',
            last_index: 0,
            font_size_total
        }
    }
    
    fn next_word(&mut self, font: &mut CxFont) -> Option<WordIteratorItem> {
        if let Some(char_iter) = &mut self.char_iter {
            while let Some((i, c)) = char_iter.next() {
                self.last_index = i;
                self.last_char = c;
                let ret = WordIteratorItem {
                    start: self.word_start,
                    end: i,
                    width: self.word_width,
                    with_newline: false
                };
                
                let adv = if let Some(glyph) = font.get_glyph(c) {
                    glyph.horizontal_metrics.advance_width * self.font_size_total
                }else {0.0};
                
                if c == '\r' {
                    continue;
                }
                if c == '\n' {
                    self.last_is_whitespace = false;
                    self.word_start = i + 1;
                    self.word_width = 0.0;
                    return Some(WordIteratorItem {with_newline: true, end: i, ..ret})
                }
                else if c.is_whitespace() { // we only return words where whitespace turns to word
                    self.last_is_whitespace = true;
                }
                else if self.last_is_whitespace {
                    self.last_is_whitespace = false;
                    self.word_start = i;
                    self.word_width = adv;
                    return Some(ret);
                }
                // this causes a character-based split if the word doesnt fit at all
                if self.word_width + adv >= self.eval_width {
                    self.word_start = i;
                    self.word_width = adv;
                    return Some(ret);
                }
                self.word_width += adv;
            }
            self.char_iter = None;
            
            let mut buffer = [0; 4];
            let char_bytes_len = self.last_char.encode_utf8(&mut buffer).len();
            
            return Some(WordIteratorItem {
                start: self.word_start,
                end: self.last_index + char_bytes_len,
                width: self.word_width,
                with_newline: false
            });
        }
        else {
            None
        }
    }
}
/*
#[derive(Debug, Clone, Copy, Live, LiveHook)]
pub enum Overflow {
    #[live] Cut,
    #[pick] Ellipsis,
    #[live] None
}*/

pub struct TextGeom {
    pub eval_width: f64,
    pub eval_height: f64,
    pub measured_width: f64,
    pub measured_height: f64,
    pub ellip_pt: Option<(usize, f64, usize)>
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawText {
    #[rust] pub many_instances: Option<ManyInstances>,
    
    #[live] pub geometry: GeometryQuad2D,
    #[live] pub text_style: TextStyle,
    #[live] pub wrap: TextWrap,
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
    
    fn begin_many_instances_internal(&mut self, cx: &mut Cx2d, fonts_atlas: &CxFontsAtlas) {
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
    
    pub fn update_draw_call_vars(&mut self, font_atlas: &CxFontsAtlas) {
        self.draw_vars.texture_slots[0] = Some(font_atlas.texture.clone());
        self.draw_vars.user_uniforms[0] = self.text_style.brightness;
        self.draw_vars.user_uniforms[1] = self.text_style.curve;
        let (sdf_radius, sdf_cutoff) = font_atlas.alloc.sdf.as_ref()
            .map_or((0.0, 0.0), |sdf| (sdf.params.radius, sdf.params.cutoff));
        self.draw_vars.user_uniforms[2] = sdf_radius;
        self.draw_vars.user_uniforms[3] = sdf_cutoff;
    }
    
    fn draw_inner(&mut self, cx: &mut Cx2d, pos: DVec2, chunk: &str, fonts_atlas: &mut CxFontsAtlas) {
        if !self.draw_vars.can_instance()
            || pos.x.is_nan()
            || pos.y.is_nan()
            || self.text_style.font.font_id.is_none() {
            return
        }
        //self.draw_clip = cx.turtle().draw_clip().into();
        //let in_many = self.many_instances.is_some();
        let font_id = self.text_style.font.font_id.unwrap();
        
        if fonts_atlas.fonts[font_id].is_none() {
            return
        }
        
        //cx.debug.rect_r(Rect{pos:dvec2(1.0,2.0), size:dvec2(200.0,300.0)});
        let mut walk_x = pos.x;
        if walk_x.is_infinite() || walk_x.is_nan() {
            return
        }
        //let mut char_offset = char_offset;
        if !self.many_instances.is_some() {
            self.begin_many_instances_internal(cx, fonts_atlas);
        }
        
        let cxfont = fonts_atlas.fonts[font_id].as_mut().unwrap();
        let dpi_factor = cx.current_dpi_factor();
        
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, self.text_style.font_size);
        
        let font = &mut cxfont.ttf_font;
        let owned_font_face = &cxfont.owned_font_face;
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];
        
        let mi = if let Some(mi) = &mut self.many_instances {mi} else {return};
        let zbias_step = 0.00001;
        let mut char_depth = self.draw_depth;
        
        let mut rustybuzz_buffer = rustybuzz::UnicodeBuffer::new();
        
        // This relies on the UBA ("Unicode Bidirectional Algorithm")
        // (see http://www.unicode.org/reports/tr9/#Basic_Display_Algorithm),
        // as implemented by `unicode_bidi`, to slice the text into substrings
        // that can be individually shaped, then assembled visually.
        let bidi_info = unicode_bidi::BidiInfo::new(chunk, None);
        
        // NOTE(eddyb) the caller of `draw_inner` has already processed the text,
        // such that `chunk` won't contain e.g. any `\n`.
        if bidi_info.paragraphs.len() == 1 {
            let runs_with_level_and_range = {
                let para = &bidi_info.paragraphs[0];
                // Split `chunk` into "runs" (that differ in their LTR/RTL "level").
                let (adjusted_levels, runs) = bidi_info.visual_runs(para, para.range.clone());
                runs.into_iter().map(move | run_range | (adjusted_levels[run_range.start], run_range))
            };
            
            for (run_level, run_range) in runs_with_level_and_range {
                // FIXME(eddyb) UBA/`unicode_bidi` only offers a LTR/RTL distinction,
                // even if `rustybuzz` has vertical `Direction`s as well.
                let (glyph_ids, new_rustybuzz_buffer) = cxfont
                    .shape_cache
                    .get_or_compute_glyph_ids(
                    (
                            if run_level.is_rtl() {
                                rustybuzz::Direction::RightToLeft
                            } else {
                                rustybuzz::Direction::LeftToRight
                            },
                            &bidi_info.text[run_range]
                        ),
                        rustybuzz_buffer,
                        owned_font_face
                    );
                rustybuzz_buffer = new_rustybuzz_buffer;
                for &glyph_id in glyph_ids {
                    let glyph = owned_font_face.with_ref(|face| font.get_glyph_by_id(face, glyph_id).unwrap());
                    
                    let advance = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                    
                    // HACK(eddyb) this is a different padding from the SDF padding,
                    // this allows the glyph rasterization to avoid touching the
                    // edges of the raster area, while the SDF padding exists for
                    // e.g. bilinear sampling to have excess texels to sample.
                    let pad_dpx = 2.0;
                    let w_dpx = ((glyph.bounds.p_max.x - glyph.bounds.p_min.x) * font_size_pixels).ceil() + pad_dpx * 2.0;
                    let h_dpx = ((glyph.bounds.p_max.y - glyph.bounds.p_min.y) * font_size_pixels).ceil() + pad_dpx * 2.0;
                    let (w_dpx, h_dpx) = if w_dpx <= pad_dpx * 2.0{(0.0,0.0)}else { (w_dpx, h_dpx) };
                                        
                    let tc = *atlas_page.atlas_glyphs.entry(glyph_id).or_insert_with(|| {
                        // see if we can fit it
                        // allocate slot
                        fonts_atlas.alloc.alloc_atlas_glyph(w_dpx, h_dpx, CxFontsAtlasTodo {
                            font_id,
                            atlas_page_id,
                            glyph_id,
                        })
                    });

                    let pad = pad_dpx * self.font_scale / dpi_factor;
                    let w = w_dpx * self.font_scale / dpi_factor;
                    let h = h_dpx * self.font_scale / dpi_factor;
                    
                    let delta_x = font_size_logical * self.font_scale * glyph.bounds.p_min.x - pad;
                    let delta_y = -(font_size_logical * self.font_scale * glyph.bounds.p_min.y - pad)
                        + self.text_style.font_size * self.font_scale * self.text_style.top_drop;
                    // give the callback a chance to do things
                    //et scaled_min_pos_x = walk_x + delta_x;
                    //let scaled_min_pos_y = pos.y - delta_y;
                    self.font_t1 = tc.t1;
                    self.font_t2 = tc.t2;
                    self.rect_pos = dvec2(walk_x + delta_x, pos.y + delta_y).into();
                    self.rect_size = dvec2(w, h).into();
                    self.char_depth = char_depth;
                    self.delta.x = delta_x as f32;
                    self.delta.y = delta_y as f32;
                    self.shader_font_size = self.text_style.font_size as f32;
                    self.advance = advance as f32; //char_offset as f32;
                    char_depth += zbias_step;
                    mi.instances.extend_from_slice(self.draw_vars.as_slice());
                    walk_x += advance;
                }
            }
        }
        
    }
    pub fn compute_geom(&self, cx: &Cx2d, walk: Walk, text: &str) -> Option<TextGeom> {
        self.compute_geom_inner(cx, walk, text, &mut *cx.fonts_atlas_rc.0.borrow_mut())
    }
    
    fn compute_geom_inner(&self, cx: &Cx2d, walk: Walk, text: &str, fonts_atlas: &mut CxFontsAtlas) -> Option<TextGeom> {
        // we include the align factor and the width/height
        let font_id = self.text_style.font.font_id.unwrap();
        
        if fonts_atlas.fonts[font_id].is_none() {
            return None
        }
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.units_per_em);
        let line_height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
        let eval_width = cx.turtle().eval_width(walk.width, walk.margin, cx.turtle().layout().flow);
        let eval_height = cx.turtle().eval_height(walk.height, walk.margin, cx.turtle().layout().flow);
        
        match if walk.width.is_fit() {&TextWrap::Line}else {&self.wrap} {
            TextWrap::Ellipsis => {
                let ellip_width = if let Some(glyph) = fonts_atlas.fonts[font_id].as_mut().unwrap().get_glyph('.') {
                    glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale
                }
                else {
                    0.0
                };
                
                let mut measured_width = 0.0;
                let mut ellip_pt = None;
                for (i, c) in text.chars().enumerate() {
                    
                    if measured_width + ellip_width * 3.0 < eval_width {
                        ellip_pt = Some((i, measured_width, 3));
                    }
                    if let Some(glyph) = fonts_atlas.fonts[font_id].as_mut().unwrap().get_glyph(c) {
                        let adv = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                        // ok so now what.
                        if measured_width + adv >= eval_width { // we have to drop back to ellip_pt
                            // if we don't have an ellip_pt, set it to 0
                            if ellip_pt.is_none() {
                                let dots = if ellip_width * 3.0 < eval_width {3}
                                else if ellip_width * 2.0 < eval_width {2}
                                else if ellip_width < eval_width {1}
                                else {0};
                                ellip_pt = Some((0, 0.0, dots));
                            }
                            return Some(TextGeom {
                                eval_width,
                                eval_height,
                                measured_width: ellip_pt.unwrap().1 + ellip_width,
                                measured_height: line_height,
                                ellip_pt
                            })
                        }
                        measured_width += adv;
                    }
                }
                
                Some(TextGeom {
                    eval_width,
                    eval_height,
                    measured_width,
                    measured_height: line_height,
                    ellip_pt: None
                })
            }
            TextWrap::Word => {
                let mut max_width = 0.0;
                let mut measured_width = 0.0;
                let mut measured_height = line_height;
                
                let mut iter = WordIterator::new(text.char_indices(), eval_width, font_size_logical * self.font_scale);
                while let Some(word) = iter.next_word(fonts_atlas.fonts[font_id].as_mut().unwrap()) {
                    if measured_width + word.width >= eval_width {
                        measured_height += line_height * self.text_style.line_spacing;
                        measured_width = word.width;
                    }
                    else {
                        measured_width += word.width;
                    }
                    if measured_width > max_width {max_width = measured_width}
                    if word.with_newline {
                        measured_height += line_height * self.text_style.line_spacing;
                        measured_width = 0.0;
                    }
                }
                
                Some(TextGeom {
                    eval_width,
                    eval_height,
                    measured_width: max_width,
                    measured_height,
                    ellip_pt: None
                })
            }
            TextWrap::Line => {
                let mut max_width = 0.0;
                let mut measured_width = 0.0;
                let mut measured_height = line_height;
                
                for c in text.chars() {
                    if c == '\n' {
                        measured_height += line_height * self.text_style.line_spacing;
                    }
                    if let Some(glyph) = fonts_atlas.fonts[font_id].as_mut().unwrap().get_glyph(c) {
                        let adv = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                        measured_width += adv;
                    }
                    if measured_width > max_width {
                        max_width = measured_width;
                    }
                }
                Some(TextGeom {
                    eval_width,
                    eval_height,
                    measured_width: max_width,
                    measured_height: measured_height,
                    ellip_pt: None
                })
            }
        }
    }
    pub fn draw_walk_word(&mut self, cx: &mut Cx2d, text: &str){
        self.draw_walk_word_with(cx, text, |_,_|{});
    }
    
    pub fn draw_walk_word_with<F>(&mut self, cx: &mut Cx2d, text: &str, mut cb:F) where F: FnMut(&mut Cx2d, Rect){
        
        // this walks the turtle per word
        if text.len() == 0 {
            return
        }        
        let font_id = if let Some(font_id) = self.text_style.font.font_id{font_id}else{
            //log!("Draw text without font");
            return
        };
        let fonts_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;
                
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.units_per_em);
        let line_drop = self.text_style.font_size * self.text_style.height_factor * self.font_scale * self.text_style.top_drop;
        
        // lets get the width of the current turtle
        // we need it for the next_word item to properly break off
        let padded_rect = cx.turtle().padded_rect();
        
        let mut iter = WordIterator::new(
            text.char_indices(),
            padded_rect.size.x,
            font_size_logical * self.font_scale, 
        );
        
        while let Some(word) = iter.next_word(fonts_atlas.fonts[font_id].as_mut().unwrap()) {
            let walk_rect = cx.walk_turtle(Walk {
                abs_pos: None,
                margin: Margin::default(),
                width: Size::Fixed(word.width),
                height: Size::Fixed(line_drop)
            });
            cb(cx, walk_rect);
            // make sure our iterator uses the xpos from the turtle
            self.draw_inner(cx, walk_rect.pos, &text[word.start..word.end], fonts_atlas);
        }
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) {
        if text.len() == 0 {
            return
        }        
        let font_id = if let Some(font_id) = self.text_style.font.font_id{font_id}else{
            //log!("Draw text without font");
            return
        };
        let fonts_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.units_per_em);
        let line_height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
                
        //let in_many = self.many_instances.is_some();
        // lets compute the geom

        //if !in_many {
        //    self.begin_many_instances_internal(cx, fonts_atlas);
        //}
        if let Some(geom) = self.compute_geom_inner(cx, walk, text, fonts_atlas) {
            let height = if walk.height.is_fit() {
                geom.measured_height
            } else {
                geom.eval_height
            };
            let y_align = (height - geom.measured_height) * align.y;
            
            match if walk.width.is_fit() {&TextWrap::Line}else {&self.wrap} {
                TextWrap::Ellipsis => {
                    // otherwise we should check the ellipsis
                    if let Some((ellip, at_x, dots)) = geom.ellip_pt {
                        // ok so how do we draw this
                        let rect = cx.walk_turtle(Walk {
                            abs_pos: walk.abs_pos,
                            margin: walk.margin,
                            width: Size::Fixed(geom.eval_width),
                            height: Size::Fixed(height)
                        });
                        
                        // Ensure the chunk before the ellipsis is aligned down to a char boundary
                        let chunk = text.get(0..ellip).unwrap_or_else(|| {
                            let mut new_ellip = ellip.saturating_sub(1);
                            while new_ellip > 0 {
                                if let Some(s) = text.get(0..new_ellip) {
                                    return s;
                                }
                                new_ellip -= 1;
                            }
                            ""
                        });
                        self.draw_inner(cx, rect.pos + dvec2(0.0, y_align), chunk, fonts_atlas);
                        self.draw_inner(cx, rect.pos + dvec2(at_x, y_align), &"..."[0..dots], fonts_atlas);
                    }
                    else { // we might have space to h-align
                        let rect = cx.walk_turtle(Walk {
                            abs_pos: walk.abs_pos,
                            margin: walk.margin,
                            width: Size::Fixed(geom.eval_width),
                            height: Size::Fixed(
                                if walk.height.is_fit() {
                                    geom.measured_height
                                } else {
                                    geom.eval_height
                                }
                            )
                        });
                        let x_align = (geom.eval_width - geom.measured_width) * align.x;
                        self.draw_inner(cx, rect.pos + dvec2(x_align, y_align), text, fonts_atlas);
                    }
                }
                TextWrap::Word => {
                    let rect = cx.walk_turtle(Walk {
                        abs_pos: walk.abs_pos,
                        margin: walk.margin,
                        width: Size::Fixed(geom.eval_width),
                        height: Size::Fixed(geom.measured_height)
                    });
                    let mut pos = dvec2(0.0, 0.0);
                    
                    let mut iter = WordIterator::new(text.char_indices(), geom.eval_width, font_size_logical * self.font_scale);
                    while let Some(word) = iter.next_word(fonts_atlas.fonts[font_id].as_mut().unwrap()) {
                        if pos.x + word.width >= geom.eval_width {
                            pos.y += line_height * self.text_style.line_spacing;
                            pos.x = 0.0;
                        }
                        self.draw_inner(cx, rect.pos + pos, &text[word.start..word.end], fonts_atlas);
                        pos.x += word.width;
                        
                        if word.with_newline {
                            pos.y += line_height * self.text_style.line_spacing;
                            pos.x = 0.0;
                        }
                    }
                }
                TextWrap::Line => {
                    // lets just output it and walk it
                    let rect = cx.walk_turtle(Walk {
                        abs_pos: walk.abs_pos,
                        margin: walk.margin,
                        width: Size::Fixed(geom.measured_width),
                        height: Size::Fixed(height)
                    });
                    // lets do our y alignment
                    let mut ypos = 0.0;
                    for line in text.split('\n') {
                        self.draw_inner(cx, rect.pos + dvec2(0.0, y_align + ypos), line, fonts_atlas);
                        ypos += line_height * self.text_style.line_spacing;
                    }
                    
                }
            }
        }
        
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }
    
    pub fn closest_offset(&self, cx: &Cx, pos: DVec2) -> Option<usize> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }

        let line_spacing = self.get_line_spacing();
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        let delta = area.get_read_ref(cx, live_id!(delta), ShaderTy::Vec2).unwrap();
        let advance = area.get_read_ref(cx, live_id!(advance), ShaderTy::Float).unwrap();

        let mut last_y = None;
        for i in 0..rect_pos.repeat {
            let index = rect_pos.stride * i;
            let x = rect_pos.buffer[index + 0] as f64 - delta.buffer[index + 0] as f64;
            let y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            if last_y.is_none() {last_y = Some(y)}
            let advance = advance.buffer[index + 0] as f64;
            if i > 0 && (y - last_y.unwrap()) > 0.001 && pos.y < last_y.unwrap() as f64 + line_spacing as f64 {
                return Some(i - 1)
            }
            if pos.x < x + advance * 0.5 && pos.y < y as f64 + line_spacing as f64 {
                return Some(i)
            }
            last_y = Some(y)
        }
        return Some(rect_pos.repeat);
        
    }
    
    pub fn get_selection_rects(&self, cx: &Cx, start: usize, end: usize, shift: DVec2, pad: DVec2) -> Vec<Rect> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return Vec::new();
        }
        
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
    
    pub fn get_cursor_pos(&self, cx: &Cx, pos: f32, index: usize) -> Option<DVec2> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }
        
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
