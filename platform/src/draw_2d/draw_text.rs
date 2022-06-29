use {
    crate::{
        makepad_derive_live::*,
        makepad_math::*,
        makepad_shader_compiler::{
            ShaderTy,
        },
        makepad_live_id::*,
        cx::Cx,
        draw_2d::cx_2d::Cx2d,
        live_traits::*,
        draw_2d::turtle::{Walk, Size, Flow, Align},
        font::{CxFontsAtlasTodo, Font},
        draw_2d::view::ManyInstances,
        draw_vars::DrawVars,
        shader::geometry_gen::GeometryQuad2D,
    },
};

live_register!{
    
    DrawText: {{DrawText}} {
        //debug: true;
        wrapping: Wrapping::None
        text_style: {
            font: {
                path: "resources/IBMPlexSans-Text.ttf"
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
        
        fn scroll(self) -> vec2 {
            return self.draw_scroll.xy
        }
        
        fn vertex(self) -> vec4 {
            let min_pos = vec2(self.rect_pos.x, self.rect_pos.y)
            let max_pos = vec2(self.rect_pos.x + self.rect_size.x, self.rect_pos.y - self.rect_size.y)
            
            self.clipped = clamp(
                mix(min_pos, max_pos, self.geom_pos) - self.draw_scroll.xy,
                self.draw_clip.xy,
                self.draw_clip.zw
            )
            
            let normalized: vec2 = (self.clipped - min_pos + self.draw_scroll.xy) / vec2(self.rect_size.x, -self.rect_size.y)
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
                    sample2d(self.tex, self.tex_coord3.xy + vec2(0., 0.)).z
                        + sample2d(self.tex, self.tex_coord3.xy + vec2(dp, 0.)).z
                        + sample2d(self.tex, self.tex_coord3.xy + vec2(0., dp)).z
                        + sample2d(self.tex, self.tex_coord3.xy + vec2(dp, dp)).z
                ) * 0.25;
            }
            else if dx > 1.75 {
                s = sample2d(self.tex, self.tex_coord3.xy).z;
            }
            else if dx > 1.3 {
                s = sample2d(self.tex, self.tex_coord2.xy).y;
            }
            else {
                s = sample2d(self.tex, self.tex_coord1.xy).x;
            }
            
            s = pow(s, self.curve);
            let col = self.get_color(); //color!(white);//get_color();
            return vec4(s * col.rgb * self.brightness * col.a, s * col.a);
        }
    }
}

#[derive(Clone, Live, LiveHook)]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live(9.0)] pub font_size: f32,
    #[live(1.0)] pub brightness: f32,
    #[live(0.6)] pub curve: f32,
    #[live(1.4)] pub line_spacing: f32,
    #[live(1.1)] pub top_drop: f32,
    #[live(1.3)] pub height_factor: f32,
}
/*
#[derive(Debug, Clone, Copy, Live, LiveHook)]
pub enum Overflow {
    #[live] Cut,
    #[pick] Ellipsis,
    #[live] None
}*/

pub struct TextGeom {
    pub eval_width: f32,
    pub eval_height: f32,
    pub measured_width: f32,
    pub measured_height: f32,
    pub ellip_pt: Option<(usize, f32, usize)>
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawText {
    #[rust] pub many_instances: Option<ManyInstances>,
    
    #[live] pub geometry: GeometryQuad2D,
    #[live] pub text_style: TextStyle,
    
    #[live(1.0)] pub font_scale: f32,
    #[live(1.0)] pub draw_depth: f32,
    
    #[calc] pub draw_vars: DrawVars,
    // these values are all generated
    #[live] pub color: Vec4,
    #[calc] pub font_t1: Vec2,
    #[calc] pub font_t2: Vec2,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[calc] pub char_depth: f32,
    #[calc] pub base: Vec2,
    #[calc] pub font_size: f32,
    #[calc] pub char_offset: f32,
}

impl DrawText {
    
    pub fn draw(&mut self, cx: &mut Cx2d, pos: Vec2, val: &str) {
        self.draw_inner(cx, pos, 0, val);
    }
    
    pub fn draw_rel(&mut self, cx: &mut Cx2d, pos: Vec2, val: &str) {
        self.draw_inner(cx, pos + cx.turtle().origin(), 0, val);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2, val: &str) {
        self.draw_inner(cx, pos, 0, val);
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        self.update_draw_call_vars(cx);
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
    
    pub fn update_draw_call_vars(&mut self, cx: &mut Cx) {
        self.draw_vars.texture_slots[0] = Some(cx.fonts_atlas.texture_id);
        self.draw_vars.user_uniforms[0] = self.text_style.brightness;
        self.draw_vars.user_uniforms[1] = self.text_style.curve;
    }
    
    pub fn draw_inner_fix_later_when_editor_rep_is_not_vec_of_char(&mut self, cx: &mut Cx2d, pos: Vec2, char_offset: usize, chunk: &[char]) {
        if !self.draw_vars.can_instance()
            || pos.x.is_nan()
            || pos.y.is_nan()
            || self.text_style.font.font_id.is_none() {
            return
        }
        
        let in_many = self.many_instances.is_some();
        let font_id = self.text_style.font.font_id.unwrap();
        
        if cx.fonts[font_id].is_none() {
            return
        }
        
        if !in_many {
            self.begin_many_instances(cx);
        }
        
        let mut walk_x = pos.x;
        let mut char_offset = char_offset;
        
        let cxfont = cx.cx.fonts[font_id].as_mut().unwrap();
        let dpi_factor = cx.current_dpi_factor;
        
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, self.text_style.font_size);
        
        let font = &mut cxfont.ttf_font;
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];
        
        let mi = if let Some(mi) = &mut self.many_instances {mi} else {return};
        let zbias_step = 0.00001;
        let mut char_depth = self.draw_depth;
        for wc in chunk {
            
            let unicode = *wc as usize;
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
            let scaled_min_pos_x = walk_x + font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let scaled_min_pos_y = pos.y - font_size_logical * self.font_scale * glyph.bounds.p_min.y + self.text_style.font_size * self.font_scale * self.text_style.top_drop - subpixel_y_fract;
            
            // only use a subpixel id for small fonts
            let subpixel_id = if self.text_style.font_size>32.0 {
                0
            }
            else { // subtle 64 index subpixel id
                ((subpixel_y_fract * 7.0) as usize) << 3 |
                (subpixel_x_fract * 7.0) as usize
            };
            
            let tc = if let Some(tc) = &atlas_page.atlas_glyphs[glyph_id][subpixel_id] {
                //println!("{} {} {} {}", tc.tx1,tc.tx2,tc.ty1,tc.ty2);
                tc
            }
            else {
                // see if we can fit it
                // allocate slot
                cx.cx.fonts_atlas.atlas_todo.push(CxFontsAtlasTodo {
                    subpixel_x_fract,
                    subpixel_y_fract,
                    font_id,
                    atlas_page_id,
                    glyph_id,
                    subpixel_id
                });
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id] = Some(
                    cx.cx.fonts_atlas.alloc_atlas_glyph(w, h)
                );
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id].as_ref().unwrap()
            };
            
            // give the callback a chance to do things
            self.font_t1.x = tc.tx1;
            self.font_t1.y = tc.ty1;
            self.font_t2.x = tc.tx2;
            self.font_t2.y = tc.ty2;
            self.rect_pos = vec2(scaled_min_pos_x, scaled_min_pos_y);
            self.rect_size = vec2(w * self.font_scale / dpi_factor, h * self.font_scale / dpi_factor);
            self.char_depth = char_depth;
            self.base.x = walk_x;
            self.base.y = pos.y;
            self.font_size = self.text_style.font_size;
            self.char_offset = char_offset as f32;
            char_depth += zbias_step;
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
            walk_x += advance;
            char_offset += 1;
        }
        
        if !in_many {
            self.end_many_instances(cx)
        }
    }
    
    pub fn draw_inner(&mut self, cx: &mut Cx2d, pos: Vec2, char_offset: usize, chunk: &str) {
        if !self.draw_vars.can_instance()
            || pos.x.is_nan()
            || pos.y.is_nan()
            || self.text_style.font.font_id.is_none() {
            return
        }
        
        let in_many = self.many_instances.is_some();
        let font_id = self.text_style.font.font_id.unwrap();
        
        if cx.fonts[font_id].is_none() {
            return
        }
        
        if !in_many {
            self.begin_many_instances(cx);
        }
        
        let mut walk_x = pos.x;
        let mut char_offset = char_offset;
        
        let cxfont = cx.cx.fonts[font_id].as_mut().unwrap();
        let dpi_factor = cx.current_dpi_factor;
        
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
            let scaled_min_pos_x = walk_x + font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let scaled_min_pos_y = pos.y - font_size_logical * self.font_scale * glyph.bounds.p_min.y + self.text_style.font_size * self.font_scale * self.text_style.top_drop - subpixel_y_fract;
            
            // only use a subpixel id for small fonts
            let subpixel_id = if self.text_style.font_size>32.0 {
                0
            }
            else { // subtle 64 index subpixel id
                ((subpixel_y_fract * 7.0) as usize) << 3 |
                (subpixel_x_fract * 7.0) as usize
            };
            
            let tc = if let Some(tc) = &atlas_page.atlas_glyphs[glyph_id][subpixel_id] {
                //println!("{} {} {} {}", tc.tx1,tc.tx2,tc.ty1,tc.ty2);
                tc
            }
            else {
                // see if we can fit it
                // allocate slot
                cx.cx.fonts_atlas.atlas_todo.push(CxFontsAtlasTodo {
                    subpixel_x_fract,
                    subpixel_y_fract,
                    font_id,
                    atlas_page_id,
                    glyph_id,
                    subpixel_id
                });
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id] = Some(
                    cx.cx.fonts_atlas.alloc_atlas_glyph(w, h)
                );
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id].as_ref().unwrap()
            };
            
            // give the callback a chance to do things
            self.font_t1.x = tc.tx1;
            self.font_t1.y = tc.ty1;
            self.font_t2.x = tc.tx2;
            self.font_t2.y = tc.ty2;
            self.rect_pos = vec2(scaled_min_pos_x, scaled_min_pos_y);
            self.rect_size = vec2(w * self.font_scale / dpi_factor, h * self.font_scale / dpi_factor);
            self.char_depth = char_depth;
            self.base.x = walk_x;
            self.base.y = pos.y;
            self.font_size = self.text_style.font_size;
            self.char_offset = char_offset as f32;
            char_depth += zbias_step;
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
            walk_x += advance;
            char_offset += 1;
        }
        
        if !in_many {
            self.end_many_instances(cx)
        }
    }
    
    pub fn compute_geom(&self, cx: &Cx2d, walk: Walk, text: &str) -> Option<TextGeom> {
        // we include the align factor and the width/height
        let font_id = self.text_style.font.font_id.unwrap();
        
        if cx.fonts[font_id].is_none() {
            return None
        }
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * cx.fonts[font_id].as_ref().unwrap().ttf_font.units_per_em);
        let measured_height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
        let eval_width = cx.turtle().eval_width(walk.width, walk.margin, Flow::Right);
        let eval_height = cx.turtle().eval_height(walk.height, walk.margin, Flow::Right);
        
        // if we have a fit width, we simply fit
        // if we have a fixed width, we can apply align + ellipsis
        if walk.width.is_fit() {
            let mut measured_width = 0.0;
            for c in text.chars() {
                if let Some(glyph) = cx.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph(c) {
                    let adv = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                    measured_width += adv;
                }
            }
            Some(TextGeom {
                eval_width,
                eval_height,
                measured_width,
                measured_height,
                ellip_pt: None
            })
        }
        else {
            
            let ellip_width = if let Some(glyph) = cx.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph('.') {
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
                if let Some(glyph) = cx.fonts[font_id].as_ref().unwrap().ttf_font.get_glyph(c) {
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
                            measured_height,
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
                measured_height,
                ellip_pt: None
            })
        }
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) {
        
        // lets compute the geom
        if text.len() == 0{
            return
        }
        if let Some(geom) = self.compute_geom(cx, walk, text) {
            let height = if walk.height.is_fit() {
                geom.measured_height
            } else {
                geom.eval_height
            };
            let y_align = (height - geom.measured_height) * align.y;

            if walk.width.is_fit() {
                // lets just output it and walk it
                let rect = cx.walk_turtle(Walk {
                    abs_pos: walk.abs_pos,
                    margin: walk.margin,
                    width: Size::Fixed(geom.measured_width),
                    height: Size::Fixed(height)
                });
                // lets do our y alignment
                self.draw_inner(cx, rect.pos + vec2(0.0, y_align), 0, text);
            }
            else {
                // otherwise we should check the ellipsis
                if let Some((ellip, at_x, dots)) = geom.ellip_pt {
                    // ok so how do we draw this
                    let rect = cx.walk_turtle(Walk {
                        abs_pos: walk.abs_pos,
                        margin: walk.margin,
                        width: Size::Fixed(geom.eval_width),
                        height: Size::Fixed(height)
                    });
                    self.draw_inner(cx, rect.pos+ vec2(0.0, y_align), 0, &text[0..ellip]);
                    self.draw_inner(cx, rect.pos + vec2(at_x, y_align), 0, &"..."[0..dots]);
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
                    self.draw_inner(cx, rect.pos + vec2(x_align, y_align), 0, text);
                }
            }
        }
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn closest_offset(&self, cx: &Cx, pos: Vec2) -> Option<usize> {
        let area = &self.draw_vars.area;
        
        if !area.is_valid(cx) {
            return None
        }
        
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2 {x: pos.x + scroll_pos.x, y: pos.y + scroll_pos.y};
        
        let base = area.get_read_ref(cx, id!(base), ShaderTy::Vec2).unwrap();
        let rect_size = area.get_read_ref(cx, id!(rect_size), ShaderTy::Vec2).unwrap();
        let font_size = area.get_read_ref(cx, id!(font_size), ShaderTy::Float).unwrap();
        let char_offset = area.get_read_ref(cx, id!(char_offset), ShaderTy::Float).unwrap();
        
        let text_style = &self.text_style;
        let line_spacing = text_style.line_spacing;
        
        let mut i = 0;
        while i < base.repeat {
            let index = base.stride * i;
            
            let y = base.buffer[index + 1];
            let fs = font_size.buffer[index];
            
            if y + fs * line_spacing > spos.y { // alright lets find our next x
                while i < base.repeat {
                    let index = base.stride * i;
                    let x = base.buffer[index + 0];
                    let y = base.buffer[index + 1];
                    let w = rect_size.buffer[index + 0];
                    
                    if x > spos.x + w * 0.5 || y > spos.y {
                        let prev_index = if i == 0 {0}else {base.stride * (i - 1)};
                        let prev_x = base.buffer[prev_index + 0];
                        let prev_w = rect_size.buffer[prev_index + 0];
                        if i < base.repeat - 1 && prev_x > spos.x + prev_w { // fix newline jump-back
                            return Some(char_offset.buffer[index] as usize);
                        }
                        return Some(char_offset.buffer[prev_index] as usize);
                    }
                    i += 1;
                }
            }
            i += 1;
        }
        return Some(char_offset.buffer[(base.repeat - 1) * base.stride] as usize);
    }
    
    pub fn get_monospace_base(&self, cx: &Cx) -> Vec2 {
        if self.text_style.font.font_id.is_none() {
            return Vec2::default();
        }
        let font_id = self.text_style.font.font_id.unwrap();
        if cx.fonts[font_id].is_none() {
            return Vec2::default();
        }
        let font = &cx.fonts[font_id].as_ref().unwrap().ttf_font;
        let slot = font.char_code_to_glyph_index_map[33];
        let glyph = &font.glyphs[slot];
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        Vec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.units_per_em)),
            y: self.text_style.line_spacing
        }
    }
}
