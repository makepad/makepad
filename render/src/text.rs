
use crate::cx::*;
//use std::iter::Peekable;

#[derive(Clone)]
pub enum Wrapping {
    Char,
    Word,
    Line,
    None,
    Ellipsis(f32)
}

#[derive(Clone)]
pub struct Text {
    pub font: Font,
    pub shader: Shader,
    pub text: String,
    pub color: Color,
    pub font_size: f32,
    pub font_scale: f32, 
    pub do_h_scroll: bool,
    pub do_v_scroll: bool,
    pub brightness: f32,
    pub line_spacing: f32,
    pub top_drop: f32,
    pub z: f32,
    pub wrapping: Wrapping,
}

impl Text {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_text_shader(), "TextAtlas"),
            font: cx.load_font_path("resources/Ubuntu-R.ttf"),
            do_h_scroll: true,
            do_v_scroll: true,
            text: "".to_string(),
            font_size: 8.0,
            font_scale: 1.0,
            line_spacing: 1.3,
            top_drop: 1.4,
            brightness: 1.0,
            z: 0.0,
            wrapping: Wrapping::Word,
            color: color("white")
        }
    }
    
    pub fn def_text_shader() -> ShaderGen {
        // lets add the draw shader lib
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 0, 3, 2];
        sg.compose(shader_ast!({
            let geom: vec2<Geometry>;
            let texturez: texture2d<Texture>;
            let font_tc: vec4<Instance>;
            let color: vec4<Instance>;
            let x: float<Instance>;
            let y: float<Instance>;
            let w: float<Instance>;
            let h: float<Instance>;
            let z: float<Instance>;
            let base_x: float<Instance>;
            let base_y: float<Instance>;
            let font_size: float<Instance>;
            let char_offset: float<Instance>;
            let marker: float<Instance>;
            let tex_coord1: vec2<Varying>;
            let tex_coord2: vec2<Varying>;
            let tex_coord3: vec2<Varying>;
            let clipped: vec2<Varying>;
            let rect: vec4<Varying>;
            let zbias: float<Uniform>;
            let brightness: float<Uniform>;
            let view_do_scroll: vec2<Uniform>;

            fn pixel() -> vec4 {
                let dx = dfdx(vec2(tex_coord1.x * 4096.0,0.)).x;

                let dp = 1.0 / 4096.0;

                // basic hardcoded mipmapping so it stops 'swimming' in VR
                if dx > 2.75 { // combine 3x3
                    let s = (
                        sample2d(texturez, tex_coord3.xy + vec2(0.,0.)).z
                        + sample2d(texturez, tex_coord3.xy + vec2(dp,0.)).z
                        + sample2d(texturez, tex_coord3.xy + vec2(0.,dp)).z
                        + sample2d(texturez, tex_coord3.xy + vec2(dp,dp)).z)*0.25;
                    return vec4(s * color.rgb * brightness * color.a, s * color.a);
                }
                else if dx > 1.75 { // combine 3x3
                    let s = sample2d(texturez, tex_coord3.xy).z;
                    return vec4(s * color.rgb * brightness * color.a, s * color.a);
                }
                else if dx > 1.3 { // combine 2x2
                    let s = sample2d(texturez, tex_coord2.xy).y;
                    return vec4(s * color.rgb * brightness * color.a, s * color.a);
                }
                else {
                    let s = sample2d(texturez, tex_coord1.xy).x;
                    return vec4(vec3(s,s,s) * color.rgb * brightness * color.a, s * color.a);
                }
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
                    font_tc.xy+(font_tc.zw - font_tc.xy)*0.75,
                    normalized.xy
                );

                tex_coord3 = mix(
                    font_tc.xy,
                    font_tc.xy+(font_tc.zw - font_tc.xy)*0.6,
                    normalized.xy
                );
                
                return camera_projection * (camera_view * (view_transform * vec4(clipped.x,clipped.y, z + zbias, 1.)));
            }
        }))
    }
    
    pub fn begin_text(&mut self, cx: &mut Cx) -> AlignedInstance {
        
        //let font_id = self.font.font_id.unwrap();
        let inst = cx.new_instance(&self.shader, 0);
        let aligned = cx.align_instance(inst);
        
        if aligned.inst.need_uniforms_now(cx) {
            
            // cx.fonts[font_id].width as f32 , cx.fonts[font_id].height as f32
            aligned.inst.push_uniform_texture_2d_id(cx, cx.fonts_atlas.texture_id);
            //tex_size
            //aligned.inst.push_uniform_vec2(cx, self.font.texture_size);
            
            aligned.inst.push_uniform_float(cx, 0.);
            aligned.inst.push_uniform_float(cx, self.brightness);
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
        let mut geom_x = geom_x;
        let mut char_offset = char_offset;
        let font_id = self.font.font_id.unwrap();
        
        let cxfont = &mut cx.fonts[font_id];
        
        let dpi_factor = cx.current_dpi_factor;
        
        let geom_y = (geom_y * dpi_factor).floor() / dpi_factor;
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, self.font_size);
        
        let font = &mut cxfont.font_loaded.as_ref().unwrap();
        
        let font_size_logical = self.font_size * 96.0 / (72.0 * font.units_per_em);
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
            let min_pos_y = geom_y - font_size_logical * glyph.bounds.p_min.y + self.font_size * self.top_drop;
            
            // compute subpixel shift
            let subpixel_x_fract = min_pos_x - (min_pos_x * dpi_factor).floor() / dpi_factor;
            let subpixel_y_fract = min_pos_y - (min_pos_y * dpi_factor).floor() / dpi_factor;

            // scale it
            let scaled_min_pos_x = geom_x + font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let scaled_min_pos_y = geom_y - font_size_logical * self.font_scale * glyph.bounds.p_min.y + self.font_size * self.font_scale * self.top_drop - subpixel_y_fract;
            
            //println!("{}", subpixel_y_fract);
            
            // only use a subpixel id for really small fonts
            let subpixel_id = if self.font_size>12.0 {
                0
            }
            else {
                (subpixel_x_fract * (ATLAS_SUBPIXEL_SLOTS as f32 - 1.0)) as usize
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
                self.z+0.00001*min_pos_x, //slight z-bias so we don't get z-fighting with neighbouring charsa
                geom_x,
                geom_y,
                self.font_size,
                char_offset as f32, // char_offset
                marker, // marker
            ];
            instance.extend_from_slice(&data);
            
            geom_x += advance;
            char_offset += 1;
            aligned.inst.instance_count += 1;
        }
    }
    
    pub fn end_text(&mut self, cx: &mut Cx, aligned: &AlignedInstance) {
        cx.update_aligned_instance_count(aligned);
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx, text: &str) -> Area {
        let mut aligned = self.begin_text(cx);
        
        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut elipct = 0;
        let font_size = self.font_size;
        let mut iter = text.chars().peekable();
        
        let font_id = self.font.font_id.unwrap();
        let font_size_logical = self.font_size * 96.0 / (72.0 * cx.fonts[font_id].font_loaded.as_ref().unwrap().units_per_em);
        
        while let Some(c) = iter.next() {
            let last = iter.peek().is_none();
            
            let mut emit = last;
            
            let slot = if c < '\u{10000}' {
                cx.fonts[font_id].font_loaded.as_ref().unwrap().char_code_to_glyph_index_map[c as usize]
            } else {
                0
            };
            
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
                        if c == ' ' || c == '\t' || c == '\n' || c == ',' {
                            emit = true;
                        }
                    },
                    Wrapping::Line => {
                        chunk.push(c);
                        if c == 10 as char || c == 13 as char {
                            emit = true;
                        }
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
                let height = font_size * self.line_spacing * self.font_scale;
                let geom = cx.walk_turtle(
                    Bounds::Fix(width),
                    Bounds::Fix(height),
                    Margin::zero(),
                    None
                );
                
                self.add_text(cx, geom.x, geom.y, 0, &mut aligned, &chunk, | _, _, _, _ | {0.0});
                width = 0.0;
                chunk.truncate(0);
                match self.wrapping {
                    Wrapping::Line => {
                        cx.turtle_new_line();
                    },
                    _ => ()
                    
                }
            }
        }
        self.end_text(cx, &aligned);
        aligned.inst.into_area()
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn find_closest_offset(&self, cx: &Cx, area: &Area, pos: Vec2) -> usize {
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2{x:pos.x + scroll_pos.x, y:pos.y + scroll_pos.y};
        let x_o = area.get_instance_offset(cx, "base_x");
        let y_o = area.get_instance_offset(cx, "base_y");
        let w_o = area.get_instance_offset(cx, "w");
        let font_size_o = area.get_instance_offset(cx, "font_size");
        let char_offset_o = area.get_instance_offset(cx, "char_offset");
        let read = area.get_read_ref(cx);
        let line_spacing = self.line_spacing;
        let mut index = 0;
        if let Some(read) = read{
            while index < read.count{
                let y = read.buffer[read.offset + y_o + index * read.slots];
                let font_size = read.buffer[read.offset + font_size_o + index * read.slots];
                if y + font_size * line_spacing > spos.y{ // alright lets find our next x
                    while index < read.count{
                        let x = read.buffer[read.offset + x_o + index * read.slots];
                        let y = read.buffer[read.offset + y_o + index * read.slots];
                        //let font_size = read.buffer[read.offset + font_size_o + index* read.slots]; 
                        let w = read.buffer[read.offset + w_o + index * read.slots];
                        if x > spos.x + w*0.5 || y > spos.y{
                            let prev_index = if index == 0{0}else{index - 1};
                            let prev_x = read.buffer[read.offset + x_o +  prev_index * read.slots];
                            let prev_w = read.buffer[read.offset + w_o + index * read.slots];
                            if index < read.count - 1 && prev_x > spos.x + prev_w{ // fix newline jump-back
                                return read.buffer[read.offset + char_offset_o + index * read.slots] as usize;
                            }
                            return read.buffer[read.offset + char_offset_o +  prev_index * read.slots] as usize;
                        }
                        index += 1;
                    }
                }
                index += 1;
            }
            if read.count == 0{
                return 0
            }
            return read.buffer[read.offset + char_offset_o +  (read.count - 1) * read.slots] as usize;
        }
        return 0
    }
                
        /*
        if let Some(read) = read {
            while index < read.count {
                let y = read.buffer[read.offset + y_o + index * read.slots];
                //let h = read.buffer[read.offset + h_o + index * read.slots];
                let font_size = read.buffer[read.offset + font_size_o + index * read.slots];
                
                // ok we need to check, wether our mouse cursor hits this char rect.
                
                
                if y  + 0.5*font_size * line_spacing > spos.y { // alright lets find our next x
                    println!("{}", index);

                    while index < read.count {
                        let x = read.buffer[read.offset + x_o + index * read.slots];
                        let y = read.buffer[read.offset + y_o + index * read.slots];
                        //let h = read.buffer[read.offset + h_o + index * read.slots];
                        //let font_size = read.buffer[read.offset + font_size_o + index * read.slots];
                        let w = read.buffer[read.offset + w_o + index * read.slots]; //read.buffer[read.offset + font_geom_o + index * read.slots] * font_size;
                        if x > spos.x + w * 0.5{// || y - font_size * line_spacing> spos.y {
                            let prev_index = if index == 0 {0}else {index - 1};
                            let prev_x = read.buffer[read.offset + x_o + prev_index * read.slots];
                            let prev_w = read.buffer[read.offset + w_o + index * read.slots];
                            if prev_x > spos.x + prev_w {
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
        */
    
    pub fn get_monospace_base(&self, cx: &Cx) -> Vec2 {
        let font_id = self.font.font_id.unwrap();
        let font = cx.fonts[font_id].font_loaded.as_ref().unwrap();
        let slot = font.char_code_to_glyph_index_map[33];
        let glyph = &font.glyphs[slot];
        
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        Vec2 {
            x: glyph.horizontal_metrics.advance_width * (96.0 / (72.0 * font.units_per_em)),
            y: self.line_spacing
        }
    }
}
