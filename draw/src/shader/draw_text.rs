use {
    crate::{
        makepad_platform::*,
        turtle::{Walk, Size, Flow, Align},
        font_atlas::{CxFontsAtlasTodo, CxFont, CxFontsAtlas, Font},
        view::ManyInstances,
        geometry::GeometryQuad2D,
        cx_2d::Cx2d
    },
};


live_design!{
    
    DrawText = {{DrawText}} {
        //debug: true;
        text_style: {
            font: {
                path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")
            }
        }
        
        color: #fff
        
        uniform brightness: float
        uniform curve: float
        
        texture tex: texture2d
        
        varying tex_coord1: vec2
        varying tex_coord2: vec2
        varying tex_coord3: vec2
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
                self.font_t1.xy,
                self.font_t2.xy,
                normalized.xy
            )
            
            self.tex_coord2 = mix(
                self.font_t1.xy,
                self.font_t1.xy + (self.font_t2.xy - self.font_t1.xy) * 0.75,
                normalized.xy
            )
            
            self.tex_coord3 = mix(
                self.font_t1.xy,
                self.font_t1.xy + (self.font_t2.xy - self.font_t1.xy) * 0.6,
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
            let s = 1.0;
            
            if dx > 7.0 {
                s = 0.7;
            }
            else if dx > 2.75 {
                s = (
                    sample2d_rt(self.tex, self.tex_coord3.xy + vec2(0., 0.)).z
                        + sample2d_rt(self.tex, self.tex_coord3.xy + vec2(dp, 0.)).z
                        + sample2d_rt(self.tex, self.tex_coord3.xy + vec2(0., dp)).z
                        + sample2d_rt(self.tex, self.tex_coord3.xy + vec2(dp, dp)).z
                ) * 0.25;
            }
            else if dx > 1.75 {
                s = sample2d_rt(self.tex, self.tex_coord3.xy).z;
            }
            else if dx > 1.3 {
                s = sample2d_rt(self.tex, self.tex_coord2.xy).y;
            }
            else {
                s = sample2d_rt(self.tex, self.tex_coord1.xy).x;
            }
            
            s = pow(s, self.curve);
            let col = self.get_color(); //color!(white);//get_color();
            return vec4(s * col.rgb * self.brightness * col.a, s * col.a);
        }
    }
}

#[derive(Clone, Live, LiveHook)]
#[live_ignore]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live(9.0)] pub font_size: f64,
    #[live(1.0)] pub brightness: f32,
    #[live(0.6)] pub curve: f32,
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
            last_index: 0,
            font_size_total
        }
    }
    fn next_word(&mut self, font: &CxFont) -> Option<WordIteratorItem> {
        if let Some(char_iter) = &mut self.char_iter {
            while let Some((i, c)) = char_iter.next() {
                self.last_index = i;
                let ret = WordIteratorItem {
                    start: self.word_start,
                    end: i,
                    width: self.word_width,
                    with_newline: false
                };
                
                let adv = if let Some(glyph) = font.ttf_font.get_glyph(c) {
                    glyph.horizontal_metrics.advance_width * self.font_size_total
                }else {0.0};
                
                if c == '\r' {
                    continue;
                }
                if c == '\n' {
                    self.last_is_whitespace = false;
                    self.word_start = i;
                    self.word_width = 0.0;
                    return Some(WordIteratorItem {with_newline: true, end:i, ..ret})
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
            return Some(WordIteratorItem {
                start: self.word_start,
                end: self.last_index + 1,
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

#[derive(Live)]
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
    #[calc] pub font_size: f32,
    #[calc] pub advance: f32,
}

impl LiveHook for DrawText {
    fn before_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.before_apply_init_shader(cx, apply_from, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply_from, index, nodes, &self.geometry);
    }
}

impl DrawText {
    
    pub fn draw(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos, val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos + cx.turtle().origin(), val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos, val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
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
        self.draw_vars.texture_slots[0] = Some(font_atlas.texture_id);
        self.draw_vars.user_uniforms[0] = self.text_style.brightness;
        self.draw_vars.user_uniforms[1] = self.text_style.curve;
    }
    
    pub fn draw_inner(&mut self, cx: &mut Cx2d, pos: DVec2, chunk: &str, fonts_atlas: &mut CxFontsAtlas) {
        if !self.draw_vars.can_instance()
            || pos.x.is_nan()
            || pos.y.is_nan()
            || self.text_style.font.font_id.is_none() {
            return
        }
        //self.draw_clip = cx.turtle().draw_clip().into();
        let in_many = self.many_instances.is_some();
        let font_id = self.text_style.font.font_id.unwrap();

        if fonts_atlas.fonts[font_id].is_none() {
            return
        }
        
        if !in_many {
            self.begin_many_instances_internal(cx, fonts_atlas);
        }
        
        //cx.debug.rect_r(Rect{pos:dvec2(1.0,2.0), size:dvec2(200.0,300.0)});
        let mut walk_x = pos.x;
        if walk_x.is_infinite() || walk_x.is_nan() {
            return
        }
        //let mut char_offset = char_offset;
        
        let cxfont = fonts_atlas.fonts[font_id].as_mut().unwrap();
        let dpi_factor = cx.current_dpi_factor();
        
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, self.text_style.font_size);
        
        let font = &mut cxfont.ttf_font;
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];
        
        let mi = if let Some(mi) = &mut self.many_instances {mi} else {return};
        let zbias_step = 0.00001;
        let mut char_depth = self.draw_depth;
        for wc in chunk.chars() {
            
            let unicode = wc as usize;
            let glyph_id = font.char_code_to_glyph_index_map[unicode];
            
            let glyph = &font.glyphs[glyph_id];
            
            let advance = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
            
            // snap width/height to pixel granularity
            let w = ((glyph.bounds.p_max.x - glyph.bounds.p_min.x) * font_size_pixels).ceil() + 1.0;
            let h = ((glyph.bounds.p_max.y - glyph.bounds.p_min.y) * font_size_pixels).ceil() + 1.0;
            
            // this one needs pixel snapping
            let min_pos_x = walk_x + font_size_logical * glyph.bounds.p_min.x;
            let min_pos_y = pos.y - font_size_logical * glyph.bounds.p_min.y + self.text_style.font_size * self.text_style.top_drop;
            
            // compute subpixel shift
            let subpixel_x_fract = min_pos_x - (min_pos_x * dpi_factor).floor() / dpi_factor;
            let subpixel_y_fract = min_pos_y - (min_pos_y * dpi_factor).floor() / dpi_factor;
            // scale and snap it
            // only use a subpixel id for small fonts
            let subpixel_id = if self.text_style.font_size>32.0 {
                0
            }
            else { // subtle 64 index subpixel id
                ((subpixel_y_fract * dpi_factor * 7.0) as usize) << 3 |
                (subpixel_x_fract * dpi_factor * 7.0) as usize
            };
            
            let tc = if let Some(tc) = &atlas_page.atlas_glyphs[glyph_id][subpixel_id] {
                //println!("{} {} {} {}", tc.tx1,tc.tx2,tc.ty1,tc.ty2);
                tc
            }
            else {
                // see if we can fit it
                // allocate slot
                fonts_atlas.alloc.todo.push(CxFontsAtlasTodo {
                    subpixel_x_fract,
                    subpixel_y_fract,
                    font_id,
                    atlas_page_id,
                    glyph_id,
                    subpixel_id
                });
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id] = Some(
                    fonts_atlas.alloc.alloc_atlas_glyph(w, h)
                );
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id].as_ref().unwrap()
            };
            
            let delta_x = font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let delta_y = -font_size_logical * self.font_scale * glyph.bounds.p_min.y + self.text_style.font_size * self.font_scale * self.text_style.top_drop - subpixel_y_fract;
            // give the callback a chance to do things
            //et scaled_min_pos_x = walk_x + delta_x;
            //let scaled_min_pos_y = pos.y - delta_y;
            self.font_t1 = tc.t1;
            self.font_t2 = tc.t2;
            self.rect_pos = dvec2(walk_x + delta_x, pos.y + delta_y).into();
            self.rect_size = dvec2(w * self.font_scale / dpi_factor, h * self.font_scale / dpi_factor).into();
            self.char_depth = char_depth;
            self.delta.x = delta_x as f32;
            self.delta.y = delta_y as f32;
            self.font_size = self.text_style.font_size as f32;
            self.advance = advance as f32; //char_offset as f32;
            char_depth += zbias_step;
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
            walk_x += advance;
        }
        
        if !in_many {
            self.end_many_instances(cx)
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
        let eval_height = cx.turtle().eval_height(walk.height, walk.margin,  cx.turtle().layout().flow);
        
        match if walk.width.is_fit() {&TextWrap::Line}else{ &self.wrap} {
            TextWrap::Ellipsis => {
                let ellip_width = if let Some(glyph) = fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph('.') {
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
                    if let Some(glyph) = fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph(c) {
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
                while let Some(word) = iter.next_word(fonts_atlas.fonts[font_id].as_ref().unwrap()) {
                    if measured_width + word.width >= eval_width {
                        measured_height += line_height;
                        measured_width = word.width;
                    }
                    else {
                        measured_width += word.width;
                    }
                    if measured_width > max_width {max_width = measured_width}
                    if word.with_newline {
                        measured_height += line_height;
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
                    if c == '\n'{
                        measured_height += line_height;
                    }
                    if let Some(glyph) = fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph(c) {
                        let adv = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                        measured_width += adv;
                    }
                    if measured_width > max_width{
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
    
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) {
        let font_id = self.text_style.font.font_id.unwrap();
        let fonts_atlas_rc = cx.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;
        // lets compute the geom
        if text.len() == 0 {
            return
        }
        if let Some(geom) = self.compute_geom_inner(cx, walk, text, fonts_atlas) {
            let height = if walk.height.is_fit() {
                geom.measured_height
            } else {
                geom.eval_height
            };
            let y_align = (height - geom.measured_height) * align.y;
            
            match if walk.width.is_fit() {&TextWrap::Line}else{&self.wrap} {
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
                        
                        self.draw_inner(cx, rect.pos + dvec2(0.0, y_align), &text[0..ellip], fonts_atlas);
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
                    let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font.units_per_em);
                    let line_height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
                    
                    let rect = cx.walk_turtle(Walk {
                        abs_pos: walk.abs_pos,
                        margin: walk.margin,
                        width: Size::Fixed(geom.eval_width),
                        height: Size::Fixed(geom.measured_height)
                    });
                    let mut pos = dvec2(0.0, 0.0);
                    
                    let mut iter = WordIterator::new(text.char_indices(), geom.eval_width, font_size_logical * self.font_scale);
                    while let Some(word) = iter.next_word(fonts_atlas.fonts[font_id].as_ref().unwrap()) {
                        if pos.x + word.width >= geom.eval_width {
                            pos.y += line_height;
                            pos.x = 0.0;
                        }
                        self.draw_inner(cx, rect.pos + pos, &text[word.start..word.end], fonts_atlas);
                        pos.x += word.width;
                        
                        if word.with_newline {
                            pos.y += line_height;
                            pos.x = 0.0;
                        }
                    }
                }
                TextWrap::Line => {
                    let line_height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
                    // lets just output it and walk it
                    let rect = cx.walk_turtle(Walk {
                        abs_pos: walk.abs_pos,
                        margin: walk.margin,
                        width: Size::Fixed(geom.measured_width),
                        height: Size::Fixed(height)
                    });
                    // lets do our y alignment
                    let mut ypos = 0.0;
                    for line in text.split('\n'){
                        self.draw_inner(cx, rect.pos + dvec2(0.0, y_align + ypos), line, fonts_atlas);
                        ypos += line_height;
                    }
                    
                }
            }
        }
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn closest_offset(&self, cx: &Cx, pos: DVec2) -> Option<usize> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }
        //let debug = cx.debug.clone();
        //let scroll_pos = area.get_scroll_pos(cx);
        //let pos = Vec2 {x: pos.x + scroll_pos.x, y: pos.y + scroll_pos.y};
        
        let rect_pos = area.get_read_ref(cx, live_id!(rect_pos), ShaderTy::Vec2).unwrap();
        let delta = area.get_read_ref(cx, live_id!(delta), ShaderTy::Vec2).unwrap();
        let advance = area.get_read_ref(cx, live_id!(advance), ShaderTy::Float).unwrap();
        //let font_size = area.get_read_ref(cx, live_id!(font_size), ShaderTy::Float).unwrap();
        
        //let line_spacing = self.text_style.line_spacing;
        
        // TODO add multiline support
        for i in 0..rect_pos.repeat {
            //let index = rect_pos.stride * i;
            //let fs = font_size.buffer[index];
            let index = rect_pos.stride * i;
            let x = rect_pos.buffer[index + 0] as f64 - delta.buffer[index + 0] as f64;
            //let y = rect_pos.buffer[index + 1] - delta.buffer[index + 1];
            let advance = advance.buffer[index + 0] as f64;
            if pos.x < x + advance * 0.5 {
                return Some(i)
            }
        }
        return Some(rect_pos.repeat);
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
    
    pub fn get_monospace_base(&self, cx: &Cx2d) -> DVec2 {
        let fonts_atlas = cx.fonts_atlas_rc.0.borrow_mut();
        if self.text_style.font.font_id.is_none() {
            return DVec2::default();
        }
        let font_id = self.text_style.font.font_id.unwrap();
        if fonts_atlas.fonts[font_id].is_none() {
            return DVec2::default();
        }
        let font = &fonts_atlas.fonts[font_id].as_ref().unwrap().ttf_font;
        let slot = font.char_code_to_glyph_index_map[33];
        let glyph = &font.glyphs[slot];
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        DVec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.units_per_em)),
            y: self.text_style.line_spacing
        }
    }
}