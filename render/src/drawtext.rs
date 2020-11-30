use crate::cx::*;

#[derive(Copy, Clone)]
pub enum Wrapping {
    Char,
    Word,
    Line,
    None,
    Ellipsis(f32)
}

#[repr(C, packed)]
pub struct DrawText {
    pub shader: Shader,
    pub area: Area,
    pub many: Option<ManyInstances>,
    pub many_old_area: Area,
    pub slots: usize,
    pub buf: Vec<char>,
    pub text_style: TextStyle,
    pub wrapping: Wrapping,
    pub font_scale: f32,
    pub draw_depth: f32,
    
    // instances
    pub font_t1: Vec2,
    pub font_t2: Vec2,
    pub color: Vec4,
    pub rect_pos: Vec2,
    pub rect_size: Vec2,
    pub char_depth: f32,
    pub base: Vec2,
    pub font_size: f32,
    pub char_offset: f32,
    pub marker: f32,
}


impl Clone for DrawText {
    fn clone(&self) -> Self {
        Self {
            shader: unsafe {self.shader.clone()},
            area: Area ::Empty,
            many: None,
            many_old_area: Area::Empty,
            slots: self.slots,
            buf: Vec::new(),
            
            text_style: self.text_style,
            wrapping: self.wrapping,
            font_scale: self.font_scale,
            draw_depth: self.draw_depth,
            
            //instances
            font_t1: Vec2::all(0.0),
            font_t2: Vec2::all(0.0),
            color: self.color,
            rect_pos: Vec2::all(0.0),
            rect_size: Vec2::all(0.0),
            char_depth: 0.0,
            base: Vec2::all(0.0),
            font_size: 0.0,
            char_offset: 0.0,
            marker: 0.0,
        }
    }
}

impl DrawText {
    
    pub fn new(cx: &mut Cx, shader: Shader) -> Self {
        Self::with_slots(cx, default_shader_overload!(cx, shader, self::shader), 0)
    }
    
    pub fn with_draw_depth(self, draw_depth: f32) -> Self {Self {draw_depth, ..self}}
    pub fn with_wrapping(self, wrapping: Wrapping) -> Self {Self {wrapping, ..self}}
    
    pub fn with_slots(cx: &mut Cx, shader: Shader, slots: usize) -> Self {
        Self {
            shader: shader,
            area: Area::Empty,
            many: None,
            many_old_area: Area::Empty,
            slots: slots + 18,
            buf: Vec::new(),
            
            text_style: live_text_style!(cx, self::text_style_unscaled),
            wrapping: Wrapping::Word,
            font_scale: 1.0,
            draw_depth: 0.0,
            
            font_t1: Vec2::all(0.0),
            font_t2: Vec2::all(0.0),
            color: Vec4::from_color_name("white").unwrap(),
            rect_pos: Vec2::all(0.0),
            rect_size: Vec2::all(0.0),
            char_depth: 0.0,
            base: Vec2::all(0.0),
            font_size: 0.0,
            char_offset: 0.0,
            marker: 0.0,
        }
    }
    
    pub fn register_draw_input(cx: &mut Cx) {
        cx.live_styles.register_draw_input(live_item_id!(self::DrawText), Self::live_draw_input())
    }
    
    pub fn live_draw_input() -> LiveDrawInput {
        let mut def = LiveDrawInput::default();
        let mp = module_path!();
        
        def.add_uniform(mp, "DrawText", "brightness", f32::ty_expr());
        def.add_uniform(mp, "DrawText", "curve", f32::ty_expr());
        def.add_uniform(mp, "DrawText", "texture", Texture2D::ty_expr());
        
        def.add_instance(mp, "DrawText", "font_t1", Vec2::ty_expr());
        def.add_instance(mp, "DrawText", "font_t2", Vec2::ty_expr());
        def.add_instance(mp, "DrawText", "color", Vec4::ty_expr());
        def.add_instance(mp, "DrawText", "rect_pos", Vec2::ty_expr());
        def.add_instance(mp, "DrawText", "rect_size", Vec2::ty_expr());
        def.add_instance(mp, "DrawText", "char_depth", f32::ty_expr());
        def.add_instance(mp, "DrawText", "base", Vec2::ty_expr());
        def.add_instance(mp, "DrawText", "font_size", f32::ty_expr());
        def.add_instance(mp, "DrawText", "char_offset", f32::ty_expr());
        def.add_instance(mp, "DrawText", "marker", f32::ty_expr());
        
        return def
    }
    
    pub fn style(cx: &mut Cx) {
        
        Self::register_draw_input(cx);
        
        live_body!(cx, r#"
            self::text_style_unscaled: TextStyle {
                font: "resources/Ubuntu-R.ttf",
                font_size: 8.0,
                brightness: 1.0,
                curve: 0.6,
                line_spacing: 1.4,
                top_drop: 1.2,
                height_factor: 1.3,
            }
            
            self::shader: Shader {
                
                use crate::shader_std::prelude::*;
                
                default_geometry: crate::shader_std::quad_2d;
                
                geometry geom: vec2;
                
                draw_input: self::DrawText;
                
                /*
                //texture texturez: texture2D;
                
                instance font_tc: vec4;
                instance color: vec4;
                instance rect_pos: vec2;
                instance rect_size: vec2;
                instance instance_z: float;
                instance base_x: float;
                instance base_y: float;
                instance font_size: float;
                instance char_offset: float;
                instance marker: float;*/
                
                varying tex_coord1: vec2;
                varying tex_coord2: vec2;
                varying tex_coord3: vec2;
                varying clipped: vec2;
                //let rect: vec4<Varying>;
                
                //uniform brightness: float;
                //uniform curve: float;
                
                fn get_color() -> vec4 {
                    return color;
                }
                
                fn pixel() -> vec4 {
                    let dx = dFdx(vec2(tex_coord1.x * 2048.0, 0.)).x;
                    let dp = 1.0 / 2048.0;
                    
                    // basic hardcoded mipmapping so it stops 'swimming' in VR
                    // mipmaps are stored in red/green/blue channel
                    let s = 1.0;
                    
                    if dx > 7.0 {
                        s = 0.7;
                    }
                    else if dx > 2.75 {
                        s = (
                            sample2d(texture, tex_coord3.xy + vec2(0., 0.)).z
                                + sample2d(texture, tex_coord3.xy + vec2(dp, 0.)).z
                                + sample2d(texture, tex_coord3.xy + vec2(0., dp)).z
                                + sample2d(texture, tex_coord3.xy + vec2(dp, dp)).z
                        ) * 0.25;
                    }
                    else if dx > 1.75 {
                        s = sample2d(texture, tex_coord3.xy).z;
                    }
                    else if dx > 1.3 {
                        s = sample2d(texture, tex_coord2.xy).y;
                    }
                    else {
                        s = sample2d(texture, tex_coord1.xy).x;
                    }
                    
                    s = pow(s, curve);
                    let col = get_color(); //color!(white);//get_color();
                    return vec4(s * col.rgb * brightness * col.a, s * col.a);
                }
                
                fn vertex() -> vec4 {
                    let min_pos = vec2(rect_pos.x, rect_pos.y);
                    let max_pos = vec2(rect_pos.x + rect_size.x, rect_pos.y - rect_size.y);
                    
                    clipped = clamp(
                        mix(min_pos, max_pos, geom) - draw_scroll.xy,
                        draw_clip.xy,
                        draw_clip.zw
                    );
                    
                    let normalized: vec2 = (clipped - min_pos + draw_scroll.xy) / vec2(rect_size.x, -rect_size.y);
                    //rect = vec4(min_pos.x, min_pos.y, max_pos.x, max_pos.y) - draw_scroll.xyxy;
                    
                    tex_coord1 = mix(
                        font_t1.xy,
                        font_t2.xy,
                        normalized.xy
                    );
                    
                    tex_coord2 = mix(
                        font_t1.xy,
                        font_t1.xy + (font_t2.xy - font_t1.xy) * 0.75,
                        normalized.xy
                    );
                    
                    tex_coord3 = mix(
                        font_t1.xy,
                        font_t1.xy + (font_t2.xy - font_t1.xy) * 0.6,
                        normalized.xy
                    );
                    
                    return camera_projection * (camera_view * (view_transform * vec4(
                        clipped.x,
                        clipped.y,
                        char_depth + draw_zbias,
                        1.
                    )));
                }
            }
        "#);
    }
    
    pub fn set_color(&mut self, cx: &mut Cx, v: Vec4) {
        self.color = v;
        write_draw_input!(cx, self.area(), self::DrawText::color, v);
    }
    
    pub fn last_animate(&mut self, animator: &Animator) {
        if let Some(v) = Vec4::last_animate(animator, live_item_id!(self::DrawText::color)) {
            self.color = v;
        }
    }
    
    pub fn animate(&mut self, cx: &mut Cx, animator: &mut Animator, time: f64) {
        if let Some(v) = Vec4::animate(cx, animator, time, live_item_id!(self::DrawText::color)) {
            self.set_color(cx, v);
        }
    }
    
    pub fn begin_many(&mut self, cx: &mut Cx) {
        self.many_old_area = self.area;
        let mi = cx.begin_many_aligned_instances(self.shader, self.slots);
        self.area = Area::Instance(InstanceArea {
            instance_count: 0,
            instance_offset: mi.instances.len(),
            ..mi.instance_area.clone()
        });
        self.many = Some(mi);
    }

        
    pub fn end_many(&mut self, cx: &mut Cx) {
        unsafe {
            if let Some(mi) = self.many.take() {
                let new_area = cx.end_many_instances(mi);
                self.area = cx.update_area_refs(self.many_old_area, new_area);
                self.write_uniforms(cx);
            }
        }
    }
    
    pub fn write_uniforms(&mut self, cx: &mut Cx) {
        if self.area().is_first_instance() {
            write_draw_input!(cx, self.area(), self::DrawText::texture, Texture2D(Some(cx.fonts_atlas.texture_id)));
            write_draw_input!(cx, self.area(), self::DrawText::brightness, self.text_style.brightness);
            write_draw_input!(cx, self.area(), self::DrawText::curve, self.text_style.curve);
        }
    }
    
    pub fn area(&self) -> Area {
        self.area
    }
    
    pub fn set_area(&mut self, area: Area) {
        self.area = area
    }
    
    pub fn shader(&self) -> Shader{
        self.shader
    }

    pub fn set_shader(&mut self, shader: Shader){
        self.shader = shader;
    }
    
    pub fn buf_truncate(&mut self, len:usize){
        unsafe {
            self.buf.truncate(len);
        }
    }
    
    pub fn buf_push_char(&mut self, c:char){
        unsafe{
            self.buf.push(c);
        }
    }
    
    pub fn buf_push_str(&mut self, val:&str){
        unsafe {
            for c in val.chars() {
                self.buf.push(c)
            }
        }
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx, pos: Vec2) {
        let mut buf = Vec::new();
        std::mem::swap(&mut buf, unsafe {&mut self.buf});
        self.draw_text_chunk(cx, pos, 0, &buf, | _, _, _, _ | {0.0});
        std::mem::swap(&mut buf, unsafe {&mut self.buf});
    }

    pub fn draw_text_rel(&mut self, cx: &mut Cx, pos: Vec2, val:&str) {
        self.buf_truncate(0);
        self.buf_push_str(val);
        self.draw_text(cx, pos + cx.get_turtle_origin());
    }
    
    pub fn draw_text_abs(&mut self, cx: &mut Cx, pos: Vec2, val:&str) {
        self.buf_truncate(0);
        self.buf_push_str(val);
        self.draw_text(cx, pos);
    }
    
    pub fn draw_text_chunk<F>(&mut self, cx: &mut Cx, pos: Vec2, char_offset: usize, chunk: &[char], mut char_callback: F)
    where F: FnMut(char, usize, f32, f32) -> f32
    {
        if pos.x.is_nan() || pos.y.is_nan() {
            return
        }
        
        let in_many = unsafe{self.many.is_some()};
        
        if !in_many{ 
            self.begin_many(cx);
        }
                
        let text_style = unsafe {&self.text_style};
        
        let mut walk_x = pos.x;
        let mut char_offset = char_offset;
        let font_id = text_style.font.font_id;
        
        let cxfont = &mut cx.fonts[font_id];
        
        let dpi_factor = cx.current_dpi_factor;
        
        //let geom_y = (geom_y * dpi_factor).floor() / dpi_factor;
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, text_style.font_size);
        
        let font = &mut cxfont.font_loaded.as_ref().unwrap();
        
        let font_size_logical = text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];

        
        let li = unsafe {if let Some(mi) = &mut self.many {mi} else {return}};
        
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
            let min_pos_x = walk_x + font_size_logical * glyph.bounds.p_min.x;
            let min_pos_y = pos.y - font_size_logical * glyph.bounds.p_min.y + text_style.font_size * text_style.top_drop;
            
            // compute subpixel shift
            let subpixel_x_fract = min_pos_x - (min_pos_x * dpi_factor).floor() / dpi_factor;
            let subpixel_y_fract = min_pos_y - (min_pos_y * dpi_factor).floor() / dpi_factor;
            
            
            // scale and snap it
            let scaled_min_pos_x = walk_x + font_size_logical * self.font_scale * glyph.bounds.p_min.x - subpixel_x_fract;
            let scaled_min_pos_y = pos.y - font_size_logical * self.font_scale * glyph.bounds.p_min.y + text_style.font_size * self.font_scale * text_style.top_drop - subpixel_y_fract;
            
            // only use a subpixel id for small fonts
            let subpixel_id = if text_style.font_size>32.0 {
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
                cx.fonts_atlas.atlas_todo.push(CxFontsAtlasTodo {
                    subpixel_x_fract,
                    subpixel_y_fract,
                    font_id,
                    atlas_page_id,
                    glyph_id,
                    subpixel_id
                });
                
                atlas_page.atlas_glyphs[glyph_id][subpixel_id] = Some(
                    cx.fonts_atlas.alloc_atlas_glyph(&cxfont.file, w, h)
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
            self.char_depth = self.draw_depth + 0.00001 * min_pos_x;
            self.base.x = walk_x;
            self.base.y = pos.y;
            self.font_size = text_style.font_size;
            self.char_offset = char_offset as f32;
            
            // self.marker = marker;
            self.marker = char_callback(*wc, char_offset, walk_x, advance);
            
            li.instances.extend_from_slice(unsafe {
                std::slice::from_raw_parts(&self.font_t1 as *const _ as *const f32, self.slots)
            });
            // !TODO make sure a derived shader adds 'empty' values here.
            
            walk_x += advance;
            char_offset += 1;
        }
        
        if !in_many{
            self.end_many(cx)
        }
    }
    
    pub fn draw_text_walk(&mut self, cx: &mut Cx, text: &str) {
        let in_many = unsafe{self.many.is_some()};
        
        if !in_many{ 
            self.begin_many(cx);
        }
        
        let mut buf = Vec::new();
        std::mem::swap(&mut buf, unsafe {&mut self.buf});
        buf.truncate(0);
        
        let mut width = 0.0;
        let mut elipct = 0;
        
        let text_style = unsafe {&self.text_style};
        let font_size = text_style.font_size;
        let line_spacing = text_style.line_spacing;
        let height_factor = text_style.height_factor;
        let mut iter = text.chars().peekable();
        
        let font_id = text_style.font.font_id;
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
                        buf.push(c);
                        emit = true
                    },
                    Wrapping::Word => {
                        buf.push(c);
                        if c == ' ' || c == '\t' || c == ',' || c == '\n' {
                            emit = true;
                        }
                    },
                    Wrapping::Line => {
                        buf.push(c);
                        if c == 10 as char || c == 13 as char {
                            emit = true;
                        }
                        newline = true;
                    },
                    Wrapping::None => {
                        buf.push(c);
                    },
                    Wrapping::Ellipsis(ellipsis_width) => {
                        if width>ellipsis_width { // output ...
                            if elipct < 3 {
                                buf.push('.');
                                elipct += 1;
                            }
                        }
                        else {
                            buf.push(c)
                        }
                    }
                }
            }
            if emit {
                let height = font_size * height_factor * self.font_scale;
                let rect = cx.walk_turtle(Walk {
                    width: Width::Fix(width),
                    height: Height::Fix(height),
                    margin: Margin::zero()
                });
                
                self.draw_text_chunk(cx, rect.pos, 0, &buf, | _, _, _, _ | {0.0});
                
                width = 0.0;
                buf.truncate(0);
                if newline {
                    cx.turtle_new_line_min_height(font_size * line_spacing * self.font_scale);
                }
            }
        }
        std::mem::swap(&mut buf, unsafe {&mut self.buf});
        if !in_many{
            self.end_many(cx)
        }
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn closest_text_offset(&self, cx: &Cx, pos: Vec2) -> Option<usize> {
        let area = unsafe {&self.area};
        
        if !area.is_valid(cx) {
            return None
        }
        
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2 {x: pos.x + scroll_pos.x, y: pos.y + scroll_pos.y};
        
        let base = area.get_read_ref(cx, live_item_id!(self::DrawText::base), Ty::Vec2).unwrap();
        let rect_size = area.get_read_ref(cx, live_item_id!(self::DrawText::rect_size), Ty::Vec2).unwrap();
        let font_size = area.get_read_ref(cx, live_item_id!(self::DrawText::font_size), Ty::Float).unwrap();
        let char_offset = area.get_read_ref(cx, live_item_id!(self::DrawText::char_offset), Ty::Float).unwrap();
        
        let text_style = unsafe {&self.text_style};
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
        let font_id = self.text_style.font.font_id;
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
