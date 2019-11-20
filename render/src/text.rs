
use crate::cx::*;

#[derive(Clone)]
pub enum Wrapping {
    Char,
    Word,
    Line,
    None,
    Ellipsis(f32)
}

#[derive(Clone, Copy)]
pub struct TextStyle {
    pub font: Font,
    pub font_size: f32,
    pub brightness: f32,
    pub curve: f32,
    pub line_spacing: f32,
    pub top_drop: f32,
    pub height_factor: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: Font::default(),
            font_size: 8.0,
            brightness: 1.0,
            curve: 0.7,
            line_spacing: 1.4,
            top_drop: 1.1,
            height_factor: 1.3,
        }
    }
}

#[derive(Clone)]
pub struct Text {
    pub class: ClassId,
    pub text_style: TextStyle,
    pub shader: Shader,
    pub color: Color,
    pub z: f32,
    pub wrapping: Wrapping,
    pub font_scale: f32,
    pub do_h_scroll: bool,
    pub do_v_scroll: bool,
}


impl Text {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            class: ClassId::base(),
            text_style: TextStyle::default(),
            shader: cx.add_shader(Self::def_text_shader(), "TextAtlas"),
            do_h_scroll: true,
            do_v_scroll: true,
            z: 0.0,
            wrapping: Wrapping::Word,
            color: color("white"),
            font_scale: 1.0,
            /*
            font: cx.load_font_path("resources/Ubuntu-R.ttf"),
            font_size: 8.0,
            font_scale: 1.0,
            line_spacing: 1.4,
            top_drop: 1.1,
            height_factor: 1.3,
            curve: 0.7,
            brightness: 1.0,
            */
        }
    }
    
    pub fn instance_font_tc() -> InstanceVec4 {uid!()}
    pub fn instance_color() -> InstanceColor {uid!()}
    pub fn instance_x() -> InstanceFloat {uid!()}
    pub fn instance_y() -> InstanceFloat {uid!()}
    pub fn instance_w() -> InstanceFloat {uid!()}
    pub fn instance_h() -> InstanceFloat {uid!()}
    pub fn instance_z() -> InstanceFloat {uid!()}
    pub fn instance_base_x() -> InstanceFloat {uid!()}
    pub fn instance_base_y() -> InstanceFloat {uid!()}
    pub fn instance_font_size() -> InstanceFloat {uid!()}
    pub fn instance_marker() -> InstanceFloat {uid!()}
    pub fn instance_char_offset() -> InstanceFloat {uid!()}
    
    pub fn uniform_zbias() -> UniformFloat {uid!()}
    pub fn uniform_brightness() -> UniformFloat {uid!()}
    pub fn uniform_curve() -> UniformFloat {uid!()}
    pub fn uniform_view_do_scroll() -> UniformVec2 {uid!()}
    
    pub fn def_text_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 0, 3, 2];
        sg.compose(shader_ast!({
            let geom: vec2<Geometry>;
            let texturez: texture2d<Texture>;
            
            let font_tc: Self::instance_font_tc();
            let color: Self::instance_color();
            let x: Self::instance_x();
            let y: Self::instance_y();
            let w: Self::instance_w();
            let h: Self::instance_h();
            let z: Self::instance_z();
            let base_x: Self::instance_base_x();
            let base_y: Self::instance_base_y();
            let font_size: Self::instance_font_size();
            let char_offset: Self::instance_char_offset();
            let marker: Self::instance_marker();
            
            let tex_coord1: vec2<Varying>;
            let tex_coord2: vec2<Varying>;
            let tex_coord3: vec2<Varying>;
            let clipped: vec2<Varying>;
            let rect: vec4<Varying>;
            
            let zbias: Self::uniform_zbias();
            let brightness: Self::uniform_brightness();
            let curve: Self::uniform_curve();
            let view_do_scroll: Self::uniform_view_do_scroll();
            
            fn pixel() -> vec4 {
                let dx = dfdx(vec2(tex_coord1.x * 4096.0, 0.)).x;
                let dp = 1.0 / 4096.0;
                
                // basic hardcoded mipmapping so it stops 'swimming' in VR
                let s = 1.0;
                if dx > 5.0 {
                    s = 0.7;
                }
                else if dx > 2.75 { // combine 3x3
                    s = (
                        sample2d(texturez, tex_coord3.xy + vec2(0., 0.)).z
                            + sample2d(texturez, tex_coord3.xy + vec2(dp, 0.)).z
                            + sample2d(texturez, tex_coord3.xy + vec2(0., dp)).z
                            + sample2d(texturez, tex_coord3.xy + vec2(dp, dp)).z
                    ) * 0.25;
                }
                else if dx > 1.75 { // combine 3x3
                    s = sample2d(texturez, tex_coord3.xy).z;
                }
                else if dx > 1.3 { // combine 2x2
                    s = sample2d(texturez, tex_coord2.xy).y;
                }
                else {
                    s = sample2d(texturez, tex_coord1.xy).x;
                }
                s = pow(s, curve);
                return vec4(s * color.rgb * brightness * color.a, s * color.a); // + color("#a");
            }
            
            fn vertex() -> vec4 {
                let shift: vec2 = -view_scroll * view_do_scroll; // + vec2(x, y);
                
                let min_pos = vec2(x, y);
                let max_pos = vec2(x + w, y - h);
                
                clipped = clamp(
                    mix(min_pos, max_pos, geom) + shift,
                    view_clip.xy,
                    view_clip.zw
                );
                
                let normalized: vec2 = (clipped - min_pos - shift) / (max_pos - min_pos);
                rect = vec4(min_pos.x, min_pos.y, max_pos.x, max_pos.y) + shift.xyxy;
                
                tex_coord1 = mix(
                    font_tc.xy,
                    font_tc.zw,
                    normalized.xy
                );
                
                tex_coord2 = mix(
                    font_tc.xy,
                    font_tc.xy + (font_tc.zw - font_tc.xy) * 0.75,
                    normalized.xy
                );
                
                tex_coord3 = mix(
                    font_tc.xy,
                    font_tc.xy + (font_tc.zw - font_tc.xy) * 0.6,
                    normalized.xy
                );
                
                return camera_projection * (camera_view * (view_transform * vec4(clipped.x, clipped.y, z + zbias, 1.)));
            }
        }))
    }
    
    pub fn begin_text(&mut self, cx: &mut Cx) -> AlignedInstance {
        
        //let font_id = self.font.font_id.unwrap();
        let inst = cx.new_instance(&self.shader, 0);
        let aligned = cx.align_instance(inst);
        let text_style = &self.text_style;
        let brightness = text_style.brightness;
        let curve = text_style.curve;
        if aligned.inst.need_uniforms_now(cx) {
            
            // cx.fonts[font_id].width as f32 , cx.fonts[font_id].height as f32
            aligned.inst.push_uniform_texture_2d_id(cx, cx.fonts_atlas.texture_id);
            //tex_size
            //aligned.inst.push_uniform_vec2(cx, self.font.texture_size);
            
            aligned.inst.push_uniform_float(cx, 0.);
            aligned.inst.push_uniform_float(cx, brightness);
            aligned.inst.push_uniform_float(cx, curve);
            aligned.inst.push_uniform_vec2f(
                cx,
                if self.do_h_scroll {1.0}else {0.0},
                if self.do_v_scroll {1.0}else {0.0}
            );
            //aligned.inst.push_uniform_float(cx, if self.do_subpixel_aa{1.0}else{0.0});
            //list_clip
            //area.push_uniform_vec4f(cx, -50000.0,-50000.0,50000.0,50000.0);
        }
        return aligned
    }
    
    pub fn add_text<F>(&mut self, cx: &mut Cx, geom_x: f32, geom_y: f32, char_offset: usize, aligned: &mut AlignedInstance, chunk: &[char], mut char_callback: F)
    where F: FnMut(char, usize, f32, f32) -> f32
    {
        let text_style = &self.text_style;
        let mut geom_x = geom_x;
        let mut char_offset = char_offset;
        let font_id = text_style.font.font_id.unwrap();
        
        let cxfont = &mut cx.fonts[font_id];
        
        let dpi_factor = cx.current_dpi_factor;
        
        //let geom_y = (geom_y * dpi_factor).floor() / dpi_factor;
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, text_style.font_size);
        
        let font = &mut cxfont.font_loaded.as_ref().unwrap();
        
        let font_size_logical = text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];
        
        let instance = {
            let cxview = &mut cx.views[aligned.inst.view_id];
            let draw_call = &mut cxview.draw_calls[aligned.inst.draw_call_id];
            &mut draw_call.instance
        };
        
        for wc in chunk {
            let unicode = *wc as usize;
            let glyph_id = font.char_code_to_glyph_index_map[unicode];
            if glyph_id >= font.glyphs.len() {
                println!("GLYPHID OUT OF BOUNDS {} {} len is {}", unicode, glyph_id, font.glyphs.len());
                continue;
            }
            let glyph = &font.glyphs[glyph_id];
            
            let advance = glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
            
            // snap width/height to pixel granularity
            let w = ((glyph.bounds.p_max.x - glyph.bounds.p_min.x) * font_size_pixels).ceil() + 1.0;
            let h = ((glyph.bounds.p_max.y - glyph.bounds.p_min.y) * font_size_pixels).ceil() + 1.0;
            
            // this one needs pixel snapping
            let min_pos_x = geom_x + font_size_logical * glyph.bounds.p_min.x;
            let min_pos_y = geom_y - font_size_logical * glyph.bounds.p_min.y + text_style.font_size * text_style.top_drop;
            
            // compute subpixel shift
            let subpixel_x_fract = min_pos_x - (min_pos_x * dpi_factor).floor() / dpi_factor;
            let subpixel_y_fract = min_pos_y - (min_pos_y * dpi_factor).floor() / dpi_factor;
            
            // scale and snap it
            let scaled_min_pos_x = geom_x + font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let scaled_min_pos_y = geom_y - font_size_logical * self.font_scale * glyph.bounds.p_min.y + text_style.font_size * self.font_scale * text_style.top_drop - subpixel_y_fract;
            
            // only use a subpixel id for really small fonts
            let subpixel_id = if text_style.font_size>12.0 {
                0
            }
            else { // subtle 64 index subpixel id
                ((subpixel_y_fract * 7.0) as usize) << 3 |
                (subpixel_x_fract * 7.0) as usize
            };
            
            let tc = if let Some(tc) = &atlas_page.atlas_glyphs[glyph_id][subpixel_id] {
                tc
            }
            else {
                // see if we can fit it
                // allocate slot
                cx.fonts_atlas.atlas_todo.push(CxFontsAtlasTodo {
                    subpixel_x_fract,
                    subpixel_y_fract,
                    font_id,
                    atlas_page_id,
                    glyph_id,
                    subpixel_id
                });
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id] = Some(
                    cx.fonts_atlas.alloc_atlas_glyph(&cxfont.path, w, h)
                );
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id].as_ref().unwrap()
            };
            
            // lets allocate
            let marker = char_callback(*wc, char_offset, geom_x, advance);
            
            let data = [
                tc.tx1,
                tc.ty1,
                tc.tx2,
                tc.ty2,
                self.color.r, // color
                self.color.g,
                self.color.b,
                self.color.a,
                scaled_min_pos_x,
                scaled_min_pos_y,
                w * self.font_scale / dpi_factor,
                h * self.font_scale / dpi_factor,
                self.z + 0.00001 * min_pos_x, //slight z-bias so we don't get z-fighting with neighbouring chars overlap a bit
                geom_x,
                geom_y,
                text_style.font_size,
                char_offset as f32, // char_offset
                marker, // marker
            ];
            instance.extend_from_slice(&data);
            // !TODO make sure a derived shader adds 'empty' values here.
            
            geom_x += advance;
            char_offset += 1;
            aligned.inst.instance_count += 1;
        }
    }
    
    pub fn end_text(&mut self, cx: &mut Cx, aligned: &AlignedInstance) -> Area {
        cx.update_aligned_instance_count(aligned);
        aligned.inst.into()
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx, text: &str) -> Area {
        let mut aligned = self.begin_text(cx);
        
        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut elipct = 0;
        let text_style = &self.text_style;
        let font_size = text_style.font_size;
        let line_spacing = text_style.line_spacing;
        let height_factor = text_style.height_factor;
        let mut iter = text.chars().peekable();
        
        let font_id = text_style.font.font_id.unwrap();
        let font_size_logical = text_style.font_size * 96.0 / (72.0 * cx.fonts[font_id].font_loaded.as_ref().unwrap().units_per_em);
        
        while let Some(c) = iter.next() {
            let last = iter.peek().is_none();
            
            let mut emit = last;
            let mut newline = false;
            let slot = if c < '\u{10000}' {
                cx.fonts[font_id].font_loaded.as_ref().unwrap().char_code_to_glyph_index_map[c as usize]
            } else {
                0
            };
            if c == '\n' {
                emit = true;
                newline = true;
            }            
            if slot != 0 {
                let glyph = &cx.fonts[font_id].font_loaded.as_ref().unwrap().glyphs[slot];
                width += glyph.horizontal_metrics.advance_width * font_size_logical * self.font_scale;
                match self.wrapping {
                    Wrapping::Char => {
                        chunk.push(c);
                        emit = true
                    },
                    Wrapping::Word => {
                        chunk.push(c);
                        if c == ' ' || c == '\t' || c == ',' || c == '\n'{
                            emit = true;
                        }
                    },
                    Wrapping::Line => {
                        chunk.push(c);
                        if c == 10 as char || c == 13 as char {
                            emit = true;
                        }
                        newline = true;
                    },
                    Wrapping::None => {
                        chunk.push(c);
                    },
                    Wrapping::Ellipsis(ellipsis_width) => {
                        if width>ellipsis_width { // output ...
                            if elipct < 3 {
                                chunk.push('.');
                                elipct += 1;
                            }
                        }
                        else {
                            chunk.push(c)
                        }
                    }
                }
            }
            if emit {
                let height = font_size * height_factor * self.font_scale;
                let geom = cx.walk_turtle(Walk {
                    width: Width::Fix(width),
                    height: Height::Fix(height),
                    margin: Margin::zero()
                });
                
                self.add_text(cx, geom.x, geom.y, 0, &mut aligned, &chunk, | _, _, _, _ | {0.0});
                width = 0.0;
                chunk.truncate(0);
                if newline {
                    cx.turtle_new_line_min_height(font_size * line_spacing * self.font_scale);
                }
            }
        }
        self.end_text(cx, &aligned)
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn find_closest_offset(&self, cx: &Cx, area: &Area, pos: Vec2) -> usize {
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2 {x: pos.x + scroll_pos.x, y: pos.y + scroll_pos.y};
        let x_o = area.get_instance_offset(cx, Self::instance_base_x().instance_type()).unwrap();
        let y_o = area.get_instance_offset(cx, Self::instance_base_y().instance_type()).unwrap();
        let w_o = area.get_instance_offset(cx, Self::instance_w().instance_type()).unwrap();
        let font_size_o = area.get_instance_offset(cx, Self::instance_font_size().instance_type()).unwrap();
        let char_offset_o = area.get_instance_offset(cx, Self::instance_char_offset().instance_type()).unwrap();
        let read = area.get_read_ref(cx);
        let text_style = &self.text_style;
        let line_spacing = text_style.line_spacing;
        let mut index = 0;
        if let Some(read) = read {
            while index < read.count {
                let y = read.buffer[read.offset + y_o + index * read.slots];
                let font_size = read.buffer[read.offset + font_size_o + index * read.slots];
                if y + font_size * line_spacing > spos.y { // alright lets find our next x
                    while index < read.count {
                        let x = read.buffer[read.offset + x_o + index * read.slots];
                        let y = read.buffer[read.offset + y_o + index * read.slots];
                        //let font_size = read.buffer[read.offset + font_size_o + index* read.slots];
                        let w = read.buffer[read.offset + w_o + index * read.slots];
                        if x > spos.x + w * 0.5 || y > spos.y {
                            let prev_index = if index == 0 {0}else {index - 1};
                            let prev_x = read.buffer[read.offset + x_o + prev_index * read.slots];
                            let prev_w = read.buffer[read.offset + w_o + index * read.slots];
                            if index < read.count - 1 && prev_x > spos.x + prev_w { // fix newline jump-back
                                return read.buffer[read.offset + char_offset_o + index * read.slots] as usize;
                            }
                            return read.buffer[read.offset + char_offset_o + prev_index * read.slots] as usize;
                        }
                        index += 1;
                    }
                }
                index += 1;
            }
            if read.count == 0 {
                return 0
            }
            return read.buffer[read.offset + char_offset_o + (read.count - 1) * read.slots] as usize;
        }
        return 0
    }
    
    pub fn get_monospace_base(&self, cx: &Cx) -> Vec2 {
        let font_id = self.text_style.font.font_id.unwrap();
        let font = cx.fonts[font_id].font_loaded.as_ref().unwrap();
        let slot = font.char_code_to_glyph_index_map[33];
        let glyph = &font.glyphs[slot];
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        Vec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.units_per_em)),
            y: self.text_style.line_spacing
        }
    }
}
