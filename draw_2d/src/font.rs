pub use {
    std::{
        rc::Rc,
        cell::RefCell,
        io::prelude::*,
        fs::File,
        collections::HashMap,
    },
    crate::{
        makepad_platform::*,
        cx_2d::Cx2d,
        turtle::{Walk, Layout},
        view::{ManyInstances, View, ViewRedrawingApi},
        geometry::GeometryQuad2D,
        makepad_vector::font::Glyph,
        makepad_vector::trapezoidator::Trapezoidator,
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
        makepad_vector::internal_iter::*,
        makepad_vector::path::PathIterator,
    }
};

live_design!{
    DrawTrapezoidText= {{DrawTrapezoidText}} {
        
        varying v_p0: vec2;
        varying v_p1: vec2;
        varying v_p2: vec2;
        varying v_p3: vec2;
        varying v_pixel: vec2;
        
        fn intersect_line_segment_with_vertical_line(p0: vec2, p1: vec2, x: float) -> vec2 {
            return vec2(
                x,
                mix(p0.y, p1.y, (x - p0.x) / (p1.x - p0.x))
            );
        }
        
        fn intersect_line_segment_with_horizontal_line(p0: vec2, p1: vec2, y: float) -> vec2 {
            return vec2(
                mix(p0.x, p1.x, (y - p0.y) / (p1.y - p0.y)),
                y
            );
        }
        
        fn compute_clamped_right_trapezoid_area(p0: vec2, p1: vec2, p_min: vec2, p_max: vec2) -> float {
            let x0 = clamp(p0.x, p_min.x, p_max.x);
            let x1 = clamp(p1.x, p_min.x, p_max.x);
            if (p0.x < p_min.x && p_min.x < p1.x) {
                p0 = intersect_line_segment_with_vertical_line(p0, p1, p_min.x);
            }
            if (p0.x < p_max.x && p_max.x < p1.x) {
                p1 = intersect_line_segment_with_vertical_line(p0, p1, p_max.x);
            }
            if (p0.y < p_min.y && p_min.y < p1.y) {
                p0 = intersect_line_segment_with_horizontal_line(p0, p1, p_min.y);
            }
            if (p1.y < p_min.y && p_min.y < p0.y) {
                p1 = intersect_line_segment_with_horizontal_line(p1, p0, p_min.y);
            }
            if (p0.y < p_max.y && p_max.y < p1.y) {
                p1 = intersect_line_segment_with_horizontal_line(p0, p1, p_max.y);
            }
            if (p1.y < p_max.y && p_max.y < p0.y) {
                p0 = intersect_line_segment_with_horizontal_line(p1, p0, p_max.y);
            }
            p0 = clamp(p0, p_min, p_max);
            p1 = clamp(p1, p_min, p_max);
            let h0 = p_max.y - p0.y;
            let h1 = p_max.y - p1.y;
            let a0 = (p0.x - x0) * h0;
            let a1 = (p1.x - p0.x) * (h0 + h1) * 0.5;
            let a2 = (x1 - p1.x) * h1;
            return a0 + a1 + a2;
        }
        
        fn compute_clamped_trapezoid_area(self, p_min: vec2, p_max: vec2) -> float {
            let a0 = compute_clamped_right_trapezoid_area(self.v_p0, self.v_p1, p_min, p_max);
            let a1 = compute_clamped_right_trapezoid_area(self.v_p2, self.v_p3, p_min, p_max);
            return a0 - a1;
        }
        
        fn pixel(self) -> vec4 {
            let p_min = self.v_pixel.xy - 0.5;
            let p_max = self.v_pixel.xy + 0.5;
            let t_area = self.compute_clamped_trapezoid_area(p_min, p_max);
            if self.chan < 0.5 {
                return vec4(t_area, 0., 0., 0.);
            }
            if self.chan < 1.5 {
                return vec4(0., t_area, 0., 0.);
            }
            if self.chan < 2.5 {
                return vec4(0., 0., t_area, 0.);
            }
            return vec4(t_area, t_area, t_area, 0.);
        }
        
        fn vertex(self) -> vec4 {
            let pos_min = vec2(self.a_xs.x, min(self.a_ys.x, self.a_ys.y));
            let pos_max = vec2(self.a_xs.y, max(self.a_ys.z, self.a_ys.w));
            let pos = mix(pos_min - 1.0, pos_max + 1.0, self.geom_pos);
            
            // set the varyings
            self.v_p0 = vec2(self.a_xs.x, self.a_ys.x);
            self.v_p1 = vec2(self.a_xs.y, self.a_ys.y);
            self.v_p2 = vec2(self.a_xs.x, self.a_ys.z);
            self.v_p3 = vec2(self.a_xs.y, self.a_ys.w);
            self.v_pixel = pos;
            return self.camera_projection * vec4(pos, 0.0, 1.0);
        }
    }
}

pub struct CxFontsAtlas {
    pub fonts: Vec<Option<CxFont >>,
    pub path_to_font_id: HashMap<String, usize>,
    pub texture_id: TextureId,
    pub clear_buffer: bool,
    pub alloc: CxFontsAtlasAlloc
}

#[derive(Default)]
pub struct CxFontsAtlasAlloc {
    pub texture_size: DVec2,
    pub xpos: f64,
    pub ypos: f64,
    pub hmax: f64,
    pub todo: Vec<CxFontsAtlasTodo>,
}

impl CxFontsAtlas {
    pub fn new(texture_id: TextureId) -> Self {
        Self {
            fonts: Vec::new(),
            path_to_font_id: HashMap::new(),
            texture_id,
            clear_buffer: false,
            alloc: CxFontsAtlasAlloc {
                texture_size: DVec2 {x: 2048.0, y: 2048.0},
                xpos: 0.0,
                ypos: 0.0,
                hmax: 0.0,
                todo: Vec::new(),
            }
        }
    }
}
impl CxFontsAtlasAlloc {
    pub fn alloc_atlas_glyph(&mut self, w: f64, h: f64) -> CxFontAtlasGlyph {
        if w + self.xpos >= self.texture_size.x {
            self.xpos = 0.0;
            self.ypos += self.hmax + 1.0;
            self.hmax = 0.0;
        }
        if h + self.ypos >= self.texture_size.y {
            println!("FONT ATLAS FULL, TODO FIX THIS {} > {},", h + self.ypos, self.texture_size.y);
        }
        if h > self.hmax {
            self.hmax = h;
        }
        
        let tx1 = self.xpos / self.texture_size.x;
        let ty1 = self.ypos / self.texture_size.y;
        
        self.xpos += w + 1.0;
        
        if h > self.hmax {
            self.hmax = h;
        }
        
        CxFontAtlasGlyph {
            tx1: tx1,
            ty1: ty1,
            tx2: (tx1 + (w / self.texture_size.x)),
            ty2: (ty1 + (h / self.texture_size.y))
        }
    }
}

#[derive(Clone, Live)]
pub struct Font {
    #[rust] pub font_id: Option<usize>,
    #[live] pub path: LiveDependency
}

#[derive(Clone)]
pub struct CxFontsAtlasRc(pub Rc<RefCell<CxFontsAtlas >>);

impl LiveHook for Font {
    fn after_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        Cx2d::lazy_construct_font_atlas(cx);
        let atlas = cx.get_global::<CxFontsAtlasRc>().clone();
        self.font_id = Some(atlas.0.borrow_mut().get_font_by_path(cx, self.path.as_ref()));
    }
}

impl CxFontsAtlas {
    pub fn get_font_by_path(&mut self, cx: &Cx, path: &str) -> usize {
        
        if let Some(item) = self.path_to_font_id.get(path) {
            return *item;
        }
        let font_id = self.fonts.len();
        self.fonts.push(None);
        self.path_to_font_id.insert(path.to_string(), font_id);
        
        match cx.get_dependency(path) {
            Ok(data) => match CxFont::load_from_ttf_bytes(&data) {
                Err(_) => {
                    error!("Error loading font {} ", path);
                }
                Ok(cxfont) => {
                    self.fonts[font_id] = Some(cxfont);
                }
            }
            Err(err) => {
                error!("get_font_by_path - {} {}", path, err)
            }
        }
        font_id
    }
    
    pub fn reset_font_atlas_and_redraw(&mut self) {
        for cxfont in &mut self.fonts {
            if let Some(cxfont) = cxfont {
                cxfont.atlas_pages.clear();
            }
        }
        self.alloc.xpos = 0.;
        self.alloc.ypos = 0.;
        self.alloc.hmax = 0.;
        self.clear_buffer = true;
        todo!();
        //self.redraw_all();
    }
    
    pub fn get_internal_font_atlas_texture_id(&self) -> TextureId {
        self.texture_id
    }
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawTrapezoidText {
    #[rust] pub trapezoidator: Trapezoidator,
    #[live] pub geometry: GeometryQuad2D,
    #[calc] pub draw_vars: DrawVars,
    #[calc] pub a_xs: Vec2,
    #[calc] pub a_ys: Vec4,
    #[calc] pub chan: f32,
}

impl DrawTrapezoidText {
    
    // test api for directly drawing a glyph
    /*
    pub fn draw_char(&mut self, cx: &mut Cx, c: char, font_id: usize, font_size: f32, dpi_factor:f32) {
        // now lets make a draw_character function
        let many = cx.begin_many_instances(&self.draw_vars);
        if many.is_none(){
            return
        }
        let mut many = many.unwrap();
        
        let trapezoids = {
            let cxfont = cx.fonts[font_id].as_ref().unwrap();
            let font = &cxfont.ttf_font;
            
            let slot = if c < '\u{10000}' {
                cxfont.ttf_font.char_code_to_glyph_index_map[c as usize]
            } else {
                0
            };
            
            if slot == 0 {
                return
            }
            let glyph = &cxfont.ttf_font.glyphs[slot];
            //let dpi_factor = cx.current_dpi_factor;
            let pos = cx.get_turtle_pos();
            let font_scale_logical = font_size * 96.0 / (72.0 * font.units_per_em);
            let font_scale_pixels = font_scale_logical * dpi_factor;
            let mut trapezoids = Vec::new();
            let trapezoidate = self.trapezoidator.trapezoidate(
                glyph
                    .outline
                    .commands()
                    .map({
                    move | command | {
                        command.transform(
                            &AffineTransformation::identity()
                                .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
                                .uniform_scale(font_scale_pixels)
                                .translate(Vector::new(pos.x, pos.y))
                        )
                    }
                }).linearize(0.5),
            );
            if let Some(trapezoidate) = trapezoidate {
                trapezoids.extend_from_internal_iter(
                    trapezoidate
                );
            }
            trapezoids
        };
        for trapezoid in trapezoids {
            self.a_xs = Vec2 {x: trapezoid.xs[0], y: trapezoid.xs[1]};
            self.a_ys = Vec4 {x: trapezoid.ys[0], y: trapezoid.ys[1], z: trapezoid.ys[2], w: trapezoid.ys[3]};
            self.chan = 3.0;
            many.instances.extend_from_slice(self.draw_vars.as_slice());
        }
        
        cx.end_many_instances(many);
    }*/
    
    // atlas drawing function used by CxAfterDraw
    pub fn draw_todo(&mut self, fonts_atlas: &CxFontsAtlas, todo: CxFontsAtlasTodo, many: &mut ManyInstances) {
        //let fonts_atlas = cx.fonts_atlas_rc.0.borrow_mut();
        let mut size = 1.0;
        for i in 0..3 {
            if i == 1 {
                size = 0.75;
            }
            if i == 2 {
                size = 0.6;
            }
            let trapezoids = {
                let cxfont = fonts_atlas.fonts[todo.font_id].as_ref().unwrap();
                let font = &cxfont.ttf_font;
                let atlas_page = &cxfont.atlas_pages[todo.atlas_page_id];
                let glyph = &font.glyphs[todo.glyph_id];
                
                if todo.glyph_id == font.char_code_to_glyph_index_map[10] ||
                todo.glyph_id == font.char_code_to_glyph_index_map[9] ||
                todo.glyph_id == font.char_code_to_glyph_index_map[13] {
                    return
                }
                
                let glyphtc = atlas_page.atlas_glyphs[todo.glyph_id][todo.subpixel_id].unwrap();
                let tx = glyphtc.tx1 * fonts_atlas.alloc.texture_size.x + todo.subpixel_x_fract * atlas_page.dpi_factor;
                let ty = 1.0 + glyphtc.ty1 * fonts_atlas.alloc.texture_size.y - todo.subpixel_y_fract * atlas_page.dpi_factor;
                
                let font_scale_logical = atlas_page.font_size * 96.0 / (72.0 * font.units_per_em);
                let font_scale_pixels = font_scale_logical * atlas_page.dpi_factor;
                let mut trapezoids = Vec::new();
                //log_str(&format!("Serializing char {} {} {} {}", glyphtc.tx1 , cx.fonts_atlas.texture_size.x ,todo.subpixel_x_fract ,atlas_page.dpi_factor));
                let trapezoidate = self.trapezoidator.trapezoidate(
                    glyph
                        .outline
                        .commands()
                        .map({
                        move | command | {
                            command.transform(
                                &AffineTransformation::identity()
                                    .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
                                    .uniform_scale(font_scale_pixels * size)
                                    .translate(Vector::new(tx, ty))
                            )
                        }
                    }).linearize(0.5),
                );
                if let Some(trapezoidate) = trapezoidate {
                    trapezoids.extend_from_internal_iter(
                        trapezoidate
                    );
                }
                trapezoids
            };
            for trapezoid in trapezoids {
                self.a_xs = Vec2 {x: trapezoid.xs[0], y: trapezoid.xs[1]};
                self.a_ys = Vec4 {x: trapezoid.ys[0], y: trapezoid.ys[1], z: trapezoid.ys[2], w: trapezoid.ys[3]};
                self.chan = i as f32;
                many.instances.extend_from_slice(self.draw_vars.as_slice());
            }
        }
    }
}

#[derive(Clone)]
pub struct CxDrawFontsAtlasRc(pub Rc<RefCell<CxDrawFontsAtlas >>);

pub struct CxDrawFontsAtlas {
    pub draw_trapezoid_text: DrawTrapezoidText,
    pub atlas_pass: Pass,
    pub atlas_view: View,
    pub atlas_texture: Texture,
    pub counter: usize
}

impl CxDrawFontsAtlas {
    pub fn new(cx: &mut Cx) -> Self {
        
        let atlas_texture = Texture::new(cx);
        
        //cx.fonts_atlas.texture_id = Some(atlas_texture.texture_id());
        
        let draw_trapezoid_text = DrawTrapezoidText::new_local(cx);
        
        // ok we need to initialize drawtrapezoidtext from a live pointer.
        Self {
            counter: 0,
            draw_trapezoid_text,
            atlas_pass: Pass::new(cx),
            atlas_view: View::new(cx),
            atlas_texture: atlas_texture
        }
    }
}

impl<'a> Cx2d<'a> {
    pub fn lazy_construct_font_atlas(cx: &mut Cx){
        // ok lets fetch/instance our CxFontsAtlasRc
        if !cx.has_global::<CxFontsAtlasRc>() {
            
            let draw_fonts_atlas = CxDrawFontsAtlas::new(cx);
            let texture_id = draw_fonts_atlas.atlas_texture.texture_id();
            cx.set_global(CxDrawFontsAtlasRc(Rc::new(RefCell::new(draw_fonts_atlas))));
            
            let fonts_atlas = CxFontsAtlas::new(texture_id);
            cx.set_global(CxFontsAtlasRc(Rc::new(RefCell::new(fonts_atlas))));
        }
    }
        
    pub fn draw_font_atlas(&mut self) {
        let draw_fonts_atlas_rc = self.cx.get_global::<CxDrawFontsAtlasRc>().clone();
        let mut draw_fonts_atlas = draw_fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas_rc = self.fonts_atlas_rc.clone();
        let mut fonts_atlas = fonts_atlas_rc.0.borrow_mut();
        let fonts_atlas = &mut*fonts_atlas;
        //let start = Cx::profile_time_ns();
        // we need to start a pass that just uses the texture
        if fonts_atlas.alloc.todo.len()>0 {
            self.begin_pass(&draw_fonts_atlas.atlas_pass);
            
            let texture_size = fonts_atlas.alloc.texture_size;
            draw_fonts_atlas.atlas_pass.set_size(self.cx, texture_size);
            
            let clear = if fonts_atlas.clear_buffer {
                fonts_atlas.clear_buffer = false;
                PassClearColor::ClearWith(Vec4::default())
            }
            else {
                PassClearColor::InitWith(Vec4::default())
            };
            
            draw_fonts_atlas.atlas_pass.clear_color_textures(self.cx);
            draw_fonts_atlas.atlas_pass.add_color_texture(self.cx, &draw_fonts_atlas.atlas_texture, clear);
            draw_fonts_atlas.atlas_view.begin_always(self);

            let mut atlas_todo = Vec::new();
            std::mem::swap(&mut fonts_atlas.alloc.todo, &mut atlas_todo);
            
            if let Some(mut many) = self.begin_many_instances(&draw_fonts_atlas.draw_trapezoid_text.draw_vars) {

                for todo in atlas_todo {
                    draw_fonts_atlas.draw_trapezoid_text.draw_todo(fonts_atlas, todo, &mut many);
                }
                
                self.end_many_instances(many);
            }
            
            draw_fonts_atlas.counter += 1;
            draw_fonts_atlas.atlas_view.end(self);
            self.end_pass(&draw_fonts_atlas.atlas_pass);
        }
        //println!("TOTALT TIME {}", Cx::profile_time_ns() - start);
    }
}

#[derive(Clone)]
pub struct CxFont {
    pub ttf_font: makepad_vector::font::TTFFont,
    pub atlas_pages: Vec<CxFontAtlasPage>,
}

pub const ATLAS_SUBPIXEL_SLOTS: usize = 64;

#[derive(Clone)]
pub struct CxFontAtlasPage {
    pub dpi_factor: f64,
    pub font_size: f64,
    pub atlas_glyphs: Vec<[Option<CxFontAtlasGlyph>; ATLAS_SUBPIXEL_SLOTS]>
}

#[derive(Clone, Copy)]
pub struct CxFontAtlasGlyph {
    pub tx1: f64,
    pub ty1: f64,
    pub tx2: f64,
    pub ty2: f64,
}

#[derive(Default, Debug)]
pub struct CxFontsAtlasTodo {
    pub subpixel_x_fract: f64,
    pub subpixel_y_fract: f64,
    pub font_id: usize,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
    pub subpixel_id: usize
}



impl CxFont {
    pub fn load_from_ttf_bytes(bytes: &[u8]) -> makepad_vector::ttf_parser::Result<Self> {
        let ttf_font = makepad_vector::ttf_parser::parse_ttf(bytes) ?;
        Ok(Self {
            ttf_font,
            atlas_pages: Vec::new()
        })
    }
    
    pub fn get_atlas_page_id(&mut self, dpi_factor: f64, font_size: f64) -> usize {
        for (index, sg) in self.atlas_pages.iter().enumerate() {
            if sg.dpi_factor == dpi_factor
                && sg.font_size == font_size {
                return index
            }
        }
        self.atlas_pages.push(CxFontAtlasPage {
            dpi_factor: dpi_factor,
            font_size: font_size,
            atlas_glyphs: {
                let mut v = Vec::new();
                v.resize(self.ttf_font.glyphs.len(), [None; ATLAS_SUBPIXEL_SLOTS]);
                v
            }
        });
        self.atlas_pages.len() - 1
    }
}
