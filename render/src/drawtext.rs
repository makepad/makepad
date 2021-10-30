use crate::cx::*;

live_body!{
    use crate::shader_std::*;
    use crate::geometrygen::GeometryQuad2D;
    use crate::font::Font;

    Wrapping: Enum{
        rust_type: {{Wrapping}}
    }

    TextStyle: Struct {
        rust_type: {{TextStyle}}
        font: Font {
            path: "resources/Ubuntu-R.ttf"
        }
    }
    
    DrawText: DrawShader2D {
        //debug: true;
        rust_type: {{DrawText}}
        geometry: GeometryQuad2D {}
        
        wrapping: Wrapping::Ellipsis(3.0)
        text_style: TextStyle {}
        
        uniform curve: float
        uniform brightness: float
        texture texture: texture2D
        
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
                    sample2d(self.texture, self.tex_coord3.xy + vec2(0., 0.)).z
                        + sample2d(self.texture, self.tex_coord3.xy + vec2(dp, 0.)).z
                        + sample2d(self.texture, self.tex_coord3.xy + vec2(0., dp)).z
                        + sample2d(self.texture, self.tex_coord3.xy + vec2(dp, dp)).z
                ) * 0.25;
            }
            else if dx > 1.75 {
                s = sample2d(self.texture, self.tex_coord3.xy).z;
            }
            else if dx > 1.3 {
                s = sample2d(self.texture, self.tex_coord2.xy).y;
            }
            else {
                s = sample2d(self.texture, self.tex_coord1.xy).x;
            }
            
            s = pow(s, self.curve);
            let col = self.get_color(); //color!(white);//get_color();
            return vec4(s * col.rgb * self.brightness * col.a, s * col.a);
        }
    }
}

#[derive(Clone, Live, LiveUpdateHooks)]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live(8.0)] pub font_size: f32,
    #[live(1.0)] pub brightness: f32,
    #[live(0.6)] pub curve: f32,
    #[live(1.4)] pub line_spacing: f32,
    #[live(1.1)] pub top_drop: f32,
    #[live(1.3)] pub height_factor: f32,
}

#[derive(Debug, Copy, Live, Clone)]
pub enum Wrapping {
    #[default()] Char,
    #[live()] Word,
    #[live()] Line,
    #[live()] None,
    #[live(1.0)] Ellipsis(f32),
}

impl LiveUpdateHooks for Wrapping{
    fn after_live_update(&mut self, _cx: &mut Cx, _live_ptr:LivePtr) {
        println!("{:?}", self);
    }
}

#[derive(Live)]
#[repr(C,)]
pub struct DrawText {
    #[hidden()] pub buf: Vec<char>,
    #[hidden()] pub area: Area,
    #[hidden()] pub many_instances: Option<ManyInstances>,
    
    #[live()] pub geometry: GeometryQuad2D,
    #[live()] pub text_style: TextStyle,
    #[live(Wrapping::None)] pub wrapping: Wrapping,
    #[live()] pub font_scale: f32,
    #[live(1.0)] pub draw_depth: f32,
    
    #[local()] pub draw_call_vars: DrawCallVars,
    // these values are all generated
    #[local()] pub font_t1: Vec2,
    #[local()] pub font_t2: Vec2,
    #[local()] pub color: Vec4,
    #[local()] pub rect_pos: Vec2,
    #[local()] pub rect_size: Vec2,
    #[local()] pub char_depth: f32,
    #[local()] pub base: Vec2,
    #[local()] pub font_size: f32,
    #[local()] pub char_offset: f32,
    #[local()] pub marker: f32,
}

impl LiveUpdateHooks for DrawText {
    fn live_update_value_unknown(&mut self, cx: &mut Cx, id: Id, ptr: LivePtr) {
        self.draw_call_vars.update_var(cx, ptr, id);
    }
    
    fn before_live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {
        self.draw_call_vars.init_shader(cx, DrawShaderPtr(live_ptr), &self.geometry);
    }
    
    fn after_live_update(&mut self, cx: &mut Cx, _live_ptr: LivePtr) {
        self.draw_call_vars.init_slicer(cx);
    }
}

impl DrawText {
    
    pub fn buf_push_str(&mut self, val: &str) {
        for c in val.chars() {
            self.buf.push(c)
        }
    }
    
    pub fn draw_text(&mut self, cx: &mut Cx, pos: Vec2) {
        self.draw_text_chunk(cx, pos, 0, None, | _, _, _, _ | {0.0});
    }
    
    pub fn draw_text_rel(&mut self, cx: &mut Cx, pos: Vec2, val: &str) {
        self.buf.truncate(0);
        self.buf_push_str(val);
        self.draw_text(cx, pos + cx.get_turtle_origin());
    }
    
    pub fn draw_text_abs(&mut self, cx: &mut Cx, pos: Vec2, val: &str) {
        self.buf.truncate(0);
        self.buf_push_str(val);
        self.draw_text(cx, pos);
    }
    
    pub fn begin_many_instances(&mut self, cx: &mut Cx) {
        let mi = cx.begin_many_aligned_instances(&self.draw_call_vars);
        self.many_instances = Some(mi);
    }
    
    pub fn end_many_instances(&mut self, cx: &mut Cx) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.area = cx.update_area_refs(self.area, new_area);
        }
    }
    
    pub fn draw_text_chunk<F>(&mut self, cx: &mut Cx, pos: Vec2, char_offset: usize, chunk: Option<&[char]>, mut char_callback: F)
    where F: FnMut(char, usize, f32, f32) -> f32
    {
        
        if pos.x.is_nan() || pos.y.is_nan() || self.text_style.font.font_id.is_none() {
            return
        }
        
        // lets use a many
        
        let in_many = self.many_instances.is_some();
        
        if !in_many {
            self.begin_many_instances(cx);
        }
        
        //let text_style = &self.text_style;
        
        let chunk = chunk.unwrap_or(&self.buf);
        
        let mut walk_x = pos.x;
        let mut char_offset = char_offset;
        
        let font_id = self.text_style.font.font_id.unwrap();
        let cxfont = &mut cx.fonts[font_id];
        let dpi_factor = cx.current_dpi_factor;
        
        //let geom_y = (geom_y * dpi_factor).floor() / dpi_factor;
        let atlas_page_id = cxfont.get_atlas_page_id(dpi_factor, self.text_style.font_size);
        
        let font = &mut cxfont.font_loaded.as_ref().unwrap();
        
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * font.units_per_em);
        let font_size_pixels = font_size_logical * dpi_factor;
        
        let atlas_page = &mut cxfont.atlas_pages[atlas_page_id];
        
        let mi = if let Some(mi) = &mut self.many_instances {mi} else {return};
        
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
            self.font_size = self.text_style.font_size;
            self.char_offset = char_offset as f32;
            
            // self.marker = marker;
            self.marker = char_callback(*wc, char_offset, walk_x, advance);
            
            mi.instances.extend_from_slice(self.draw_call_vars.instances_slice());
            // !TODO make sure a derived shader adds 'empty' values here.
            
            walk_x += advance;
            char_offset += 1;
        }
        
        if !in_many {
            self.end_many_instances(cx)
        }
    }
    
    pub fn draw_text_walk(&mut self, cx: &mut Cx, text: &str) {
        
        if self.text_style.font.font_id.is_none() {
            return
        }
        
        let in_many = self.many_instances.is_some();
        
        if !in_many {
            self.begin_many_instances(cx);
        }
        
        let mut width = 0.0;
        let mut elipct = 0;
        
        self.buf.truncate(0);
        
        let mut iter = text.chars().peekable();
        
        let font_id = self.text_style.font.font_id.unwrap();
        let font_size_logical = self.text_style.font_size * 96.0 / (72.0 * cx.fonts[font_id].font_loaded.as_ref().unwrap().units_per_em);
        
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
                        self.buf.push(c);
                        emit = true;
                    },
                    Wrapping::Word => {
                        self.buf.push(c);
                        if c == ' ' || c == '\t' || c == ',' || c == '\n' {
                            emit = true;
                        }
                    },
                    Wrapping::Line => {
                        self.buf.push(c);
                        if c == 10 as char || c == 13 as char {
                            emit = true;
                        }
                        newline = true;
                    },
                    Wrapping::None => {
                        self.buf.push(c);
                    },
                    Wrapping::Ellipsis(ellipsis_width) => {
                        if width>ellipsis_width { // output ...
                            if elipct < 3 {
                                self.buf.push('.');
                                elipct += 1;
                            }
                        }
                        else {
                            self.buf.push(c)
                        }
                    }
                }
            }
            if emit {
                let height = self.text_style.font_size * self.text_style.height_factor * self.font_scale;
                let rect = cx.walk_turtle(Walk {
                    width: Width::Fix(width),
                    height: Height::Fix(height),
                    margin: Margin::zero()
                });
                
                self.draw_text_chunk(cx, rect.pos, 0, None, | _, _, _, _ | {0.0});
                
                width = 0.0;
                self.buf.truncate(0);
                if newline {
                    cx.turtle_new_line_min_height(self.font_size * self.text_style.line_spacing * self.font_scale);
                }
            }
        }
        if !in_many {
            self.end_many_instances(cx)
        }
    }
    
    // looks up text with the behavior of a text selection mouse cursor
    pub fn closest_text_offset(&self, cx: &Cx, pos: Vec2) -> Option<usize> {
        let area = &self.area;
        
        if !area.is_valid(cx) {
            return None
        }
        
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2 {x: pos.x + scroll_pos.x, y: pos.y + scroll_pos.y};
        
        let base = area.get_read_ref(cx, id!(base), Ty::Vec2).unwrap();
        let rect_size = area.get_read_ref(cx, id!(rect_size), Ty::Vec2).unwrap();
        let font_size = area.get_read_ref(cx, id!(font_size), Ty::Float).unwrap();
        let char_offset = area.get_read_ref(cx, id!(char_offset), Ty::Float).unwrap();
        
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

/*
impl LiveUpdateValue for Wrapping {
    fn live_update_value(&mut self, _cx: &mut Cx, id: Id, _ptr: LivePtr) {
        match id {
            _ => ()
        }
    }
}

// how could we compile this away
impl LiveNew for Wrapping {
    fn live_new(_cx: &mut Cx) -> Self {
        // the default
        Self::Char
    }
    
    fn live_type() -> LiveType {
        LiveType(std::any::TypeId::of::<Wrapping>())
    }
    
    fn live_register(cx: &mut Cx) {
        struct Factory();
        impl LiveFactory for Factory {
            fn live_new(&self, cx: &mut Cx) -> Box<dyn LiveUpdate> {
                Box::new(Wrapping ::live_new(cx))
            }
            
            fn live_fields(&self, _fields: &mut Vec<LiveField>) {
            }
        }
        cx.register_factory(Wrapping::live_type(), Box::new(Factory()));
    }
}

impl LiveUpdate for Wrapping {
    fn live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) {
        self.before_live_update(cx, live_ptr);
        let node = cx.shader_registry.live_registry.resolve_ptr(live_ptr);
        match &node.value{
            LiveValue::IdPack(id_pack)=>{
                let id = cx.shader_registry.live_registry.find_enum_origin(*id_pack, node.id_pack);
                match id{
                    id!(Char)=>*self = Wrapping::Char,
                    id!(Word)=>*self = Wrapping::Word,
                    id!(Line)=>*self = Wrapping::Line,
                    id!(None)=>*self = Wrapping::None,
                    _=>{
                        println!("Enum Wrapping cannot find id {}", id);
                    }
                }
            },
            LiveValue::Class{class, node_start, node_count}=>{
                let id = cx.shader_registry.live_registry.find_enum_origin(*class, node.id_pack);
                match id{
                    id!(Test)=>{
                        if let Self::Test{..} = self{}
                        else{*self = Self::Test{x:1.0}}
                        if let Self::Test{x} = self{
                            let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start, *node_count);
                            while let Some((prop_id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {
                                match prop_id{
                                    id!(x)=>(*x).live_update(cx, live_ptr),
                                    _=>{
                                         println!("Enum Wrapping cannot find named struct {} property {}", id, prop_id);
                                    }
                                }
                            }
                        }
                    },
                    _=>{ // some warning? id is not found
                        println!("Enum Wrapping cannot find named struct {}", id);
                    }
                }
            },
            LiveValue::Call{target, node_start, node_count}=>{
                // find origin
                let id = cx.shader_registry.live_registry.find_enum_origin(*target, node.id_pack);
                match id{
                    id!(Ellipsis)=>{
                        if let Self::Ellipsis{..} = self{}
                        else{*self = Self::Ellipsis(1.0)}
                        if let Self::Ellipsis(var0) = self{
                            let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start, *node_count);
                            while let Some((count, live_ptr)) = iter.next_prop() {
                                match count{
                                    0=>(*var0).live_update(cx, live_ptr),
                                    _=>{
                                         println!("Enum Wrapping cannot find tuple struct {} arg {}", id, count);
                                    }
                                }
                            }
                        }
                    },
                    _=>{ // some warning? id is not found
                        println!("Enum Wrapping cannot find tuple struct {}", id);
                    }
                }
            }
            _=>() // error
        }
        self.after_live_update(cx, live_ptr);
    }
}*/
