use crate::cx::*;
use trapezoidator::Trapezoidator;
use geometry::{AffineTransformation, Transform, Vector};
use internal_iter::*;
use path::PathIterator;

impl Cx {
    /*
    pub fn load_font_style(&mut self, style: &str) -> Font {
        self.load_font_path(&self.font(style))
    }
    */
    pub fn load_font(&mut self, path: &str) -> Font {
        let found = self.fonts.iter().position( | v | v.path == path);
        if let Some(font_id) = found {
            return Font {
                font_id: Some(font_id),
            }
        }
        
        let font_id = self.fonts.len();
        self.fonts.push(CxFont {
            path: path.to_string(),
            ..Default::default()
        });
        
        return Font {
            font_id: Some(font_id)
        }
    }
}

#[derive(Copy, Clone, Default)]
pub struct Font{
    pub font_id: Option<usize>
}

pub struct TrapezoidText {
    shader: Shader,
    trapezoidator: Trapezoidator
}
 
impl TrapezoidText {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_trapezoid_shader(), "TrapezoidShader"),
            trapezoidator: Trapezoidator::default(),
        }
    }
    
    fn instance_a_xs()->InstanceVec2{uid!()}
    fn instance_a_ys()->InstanceVec4{uid!()}
    fn instance_chan()->InstanceFloat{uid!()}
    
    pub fn def_trapezoid_shader() -> ShaderGen { 
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sg.compose(shader_ast!({
            
            let geom: vec2<Geometry>;
            
            let a_xs: Self::instance_a_xs();
            let a_ys: Self::instance_a_ys();
            let chan: Self::instance_chan();
            
            let v_p0: vec2<Varying>;
            let v_p1: vec2<Varying>;
            let v_p2: vec2<Varying>;
            let v_p3: vec2<Varying>;
            let v_pixel: vec2<Varying>;
            
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
            
            fn compute_clamped_trapezoid_area(p_min: vec2, p_max: vec2) -> float {
                let a0 = compute_clamped_right_trapezoid_area(v_p0, v_p1, p_min, p_max);
                let a1 = compute_clamped_right_trapezoid_area(v_p2, v_p3, p_min, p_max);
                return a0 - a1;
            }
            
            fn pixel() -> vec4 {
                //if fmod(v_pixel.x,2.0) > 1.0 && fmod(v_pixel.y,2.0) > 1.0{
                //    return color("white")
               // }
               // return color("black");
                let p_min = v_pixel.xy - 0.5;
                let p_max = v_pixel.xy + 0.5;
                let t_area = compute_clamped_trapezoid_area(p_min, p_max);
                if chan < 0.5{
                    return vec4(t_area,0.,0.,0.);
                }
                if chan < 1.5{
                    return vec4(0., t_area, 0.,0.);
                }
                if chan < 2.5{
                    return vec4(0., 0., t_area,0.);
                }
                return vec4(t_area, t_area, t_area,0.);
                
                //let b_minx = p_min.x + 1.0 / 3.0;
                //let r_minx = p_min.x - 1.0 / 3.0;
                //let b_maxx = p_max.x + 1.0 / 3.0;
                //let r_maxx = p_max.x - 1.0 / 3.0;
                //return vec4(
                //    compute_clamped_trapezoid_area(vec2(r_minx, p_min.y), vec2(r_maxx, p_max.y)),
                //    compute_clamped_trapezoid_area(p_min, p_max),
                //    compute_clamped_trapezoid_area(vec2(b_minx, p_min.y), vec2(b_maxx, p_max.y)),
                //    0.0
                //);
            }
            
            fn vertex() -> vec4 {
                let pos_min = vec2(a_xs.x, min(a_ys.x, a_ys.y));
                let pos_max = vec2(a_xs.y, max(a_ys.z, a_ys.w));
                let pos = mix(pos_min - 1.0, pos_max + 1.0, geom);
                
                // set the varyings
                v_p0 = vec2(a_xs.x, a_ys.x);
                v_p1 = vec2(a_xs.y, a_ys.y);
                v_p2 = vec2(a_xs.x, a_ys.z);
                v_p3 = vec2(a_xs.y, a_ys.w);
                v_pixel = pos;
                //return vec4(vec2(pos.x, 1.0-pos.y) * 2.0 / vec2(4096.,4096.) - 1.0, 0.0, 1.0);
                return camera_projection * vec4(pos, 0.0, 1.0);
            }
        }))
    }

    // test api for directly drawing a glyph
    pub fn draw_char(&mut self, cx: &mut Cx, c:char, font_id:usize, font_size:f32) {
        // now lets make a draw_character function
        let inst = cx.new_instance(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
        }
        
        let trapezoids = {
            let cxfont = &cx.fonts[font_id];
            let font = cxfont.font_loaded.as_ref().unwrap();
            
            let slot = if c < '\u{10000}' {
                cx.fonts[font_id].font_loaded.as_ref().unwrap().char_code_to_glyph_index_map[c as usize]
            } else {
                0
            };
            
            if slot == 0 {
                return
            }
            let glyph = &cx.fonts[font_id].font_loaded.as_ref().unwrap().glyphs[slot];
            let dpi_factor = cx.current_dpi_factor;
            let pos = cx.get_turtle_pos(); 
            let font_scale_logical = font_size * 96.0 / (72.0 * font.units_per_em);
            let font_scale_pixels = font_scale_logical * dpi_factor;
            
            let mut trapezoids = Vec::new();
            trapezoids.extend_from_internal_iter(
                self.trapezoidator.trapezoidate(
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
                ),
            );
            trapezoids
        };
        for trapezoid in trapezoids {
            let data = [
                trapezoid.xs[0],
                trapezoid.xs[1],
                trapezoid.ys[0],
                trapezoid.ys[1],
                trapezoid.ys[2],
                trapezoid.ys[3],
                3.0
            ];
            inst.push_slice(cx, &data);
        }
    }
    
    // atlas drawing function used by CxAfterDraw
    pub fn draw_todo(&mut self, cx: &mut Cx, todo: CxFontsAtlasTodo) {
        let inst = cx.new_instance(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
        }
        
        let mut size = 1.0;
        for i in 0..3{
            if i == 1{
                size = 0.75;
            }
            if i == 2{
                size = 0.6;
            }
            let trapezoids = {
                let cxfont = &cx.fonts[todo.font_id];
                let font = cxfont.font_loaded.as_ref().unwrap();
                let atlas_page = &cxfont.atlas_pages[todo.atlas_page_id];
                let glyph = &font.glyphs[todo.glyph_id];
                
                if todo.glyph_id == font.char_code_to_glyph_index_map[10] ||
                todo.glyph_id == font.char_code_to_glyph_index_map[9] ||
                todo.glyph_id == font.char_code_to_glyph_index_map[13] {
                    return
                }
                
                let glyphtc = atlas_page.atlas_glyphs[todo.glyph_id][todo.subpixel_id].unwrap();
                let tx = glyphtc.tx1 * cx.fonts_atlas.texture_size.x + todo.subpixel_x_fract* atlas_page.dpi_factor;
                let ty = 1.0 + glyphtc.ty1 * cx.fonts_atlas.texture_size.y - todo.subpixel_y_fract* atlas_page.dpi_factor;
                
                let font_scale_logical = atlas_page.font_size * 96.0 / (72.0 * font.units_per_em);
                let font_scale_pixels = font_scale_logical * atlas_page.dpi_factor;
                
                let mut trapezoids = Vec::new();
                trapezoids.extend_from_internal_iter(
                    self.trapezoidator.trapezoidate(
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
                    ),
                );
                trapezoids
            };
            for trapezoid in trapezoids {
                let data = [
                    trapezoid.xs[0],
                    trapezoid.xs[1],
                    trapezoid.ys[0],
                    trapezoid.ys[1],
                    trapezoid.ys[2],
                    trapezoid.ys[3],
                    i as f32
                ];
                inst.push_slice(cx, &data);
            }
        }
    }
}

pub struct CxAfterDraw {
    pub trapezoid_text: TrapezoidText,
    pub atlas_pass: Pass,
    pub atlas_view: View,
    pub atlas_texture: Texture
}

impl CxAfterDraw {
    pub fn proto(cx: &mut Cx) -> Self {
        cx.fonts_atlas.texture_size = Vec2 {x: 4096.0, y: 4096.0};
        let mut atlas_texture = Texture::default();
        atlas_texture.set_desc(cx, None);
        cx.fonts_atlas.texture_id = atlas_texture.texture_id.unwrap();
        
        Self {
            trapezoid_text: TrapezoidText::style(cx),
            atlas_pass: Pass::default(),
            atlas_view: View {
                always_redraw: true,
                ..View::proto(cx)
            },
            atlas_texture: atlas_texture
        }
    }
    
    pub fn after_draw(&mut self, cx: &mut Cx) {
        //let start = Cx::profile_time_ns();
        
        // we need to start a pass that just uses the texture
        if cx.fonts_atlas.atlas_todo.len()>0 {
            self.atlas_pass.begin_pass(cx);
            self.atlas_pass.set_size(cx, cx.fonts_atlas.texture_size);
            self.atlas_pass.add_color_texture(cx, &mut self.atlas_texture, ClearColor::InitWith(Color::zero()));
            let _ = self.atlas_view.begin_view(cx, Layout::default());
            let mut atlas_todo = Vec::new();
            std::mem::swap(&mut cx.fonts_atlas.atlas_todo, &mut atlas_todo);
            for todo in atlas_todo {
                self.trapezoid_text.draw_todo(cx, todo);
                // ok we have to draw a font_id
                //break;
            }
            self.atlas_view.end_view(cx);
            self.atlas_pass.end_pass(cx);
        }
        //println!("TOTALT TIME {}", Cx::profile_time_ns() - start);
    }
}

#[derive(Default)]
pub struct CxFont {
    pub path: String,
    pub font_loaded: Option<font::Font>,
    pub atlas_pages: Vec<CxFontAtlasPage>,
}

pub const ATLAS_SUBPIXEL_SLOTS: usize = 64;

pub struct CxFontAtlasPage {
    pub dpi_factor: f32,
    pub font_size: f32,
    pub atlas_glyphs: Vec<[Option<CxFontAtlasGlyph>; ATLAS_SUBPIXEL_SLOTS]>
}

#[derive(Clone, Copy)]
pub struct CxFontAtlasGlyph {
    pub tx1: f32,
    pub ty1: f32,
    pub tx2: f32,
    pub ty2: f32,
}

#[derive(Default)]
pub struct CxFontsAtlasTodo {
    pub subpixel_x_fract: f32,
    pub subpixel_y_fract: f32,
    pub font_id: usize,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
    pub subpixel_id: usize
}

#[derive(Default)]
pub struct CxFontsAtlas {
    pub texture_id: usize,
    pub texture_size: Vec2,
    pub alloc_xpos: f32,
    pub alloc_ypos: f32,
    pub alloc_hmax: f32,
    pub atlas_todo: Vec<CxFontsAtlasTodo>,
}

impl CxFontsAtlas {
    pub fn alloc_atlas_glyph(&mut self, path: &str, w: f32, h: f32) -> CxFontAtlasGlyph {
        if w + self.alloc_xpos >= self.texture_size.x {
            self.alloc_xpos = 0.0;
            self.alloc_ypos += self.alloc_hmax + 1.0;
            self.alloc_hmax = 0.0;
        }
        if h + self.alloc_ypos >= self.texture_size.y {
            println!("FONT ATLAS FULL {}, TODO FIX THIS", path);
        }
        if h > self.alloc_hmax {
            self.alloc_hmax = h;
        }
        
        let tx1 = self.alloc_xpos / self.texture_size.x;
        let ty1 = self.alloc_ypos / self.texture_size.y;
        
        self.alloc_xpos += w + 1.0;
        
        if h > self.alloc_hmax {
            self.alloc_hmax = h;
        }
        
        CxFontAtlasGlyph {
            tx1: tx1,
            ty1: ty1,
            tx2: tx1 + (w / self.texture_size.x),
            ty2: ty1 + (h / self.texture_size.y)
        }
    }
}

impl CxFont {
    pub fn load_from_ttf_bytes(&mut self, bytes: &[u8]) -> ttf_parser::Result<()> {
        let font = ttf_parser::parse_ttf(bytes) ?;
        self.font_loaded = Some(font);
        Ok(())
    }
    
    pub fn get_atlas_page_id(&mut self, dpi_factor: f32, font_size: f32) -> usize {
        for (index, sg) in self.atlas_pages.iter().enumerate() {
            if sg.dpi_factor == dpi_factor
                && sg.font_size == font_size {
                return index
            }
        }
        if let Some(font) = &self.font_loaded {
            self.atlas_pages.push(CxFontAtlasPage {
                dpi_factor: dpi_factor,
                font_size: font_size,
                atlas_glyphs: {
                    let mut v = Vec::new();
                    v.resize(font.glyphs.len(), [None; ATLAS_SUBPIXEL_SLOTS]);
                    v
                }
            });
            self.atlas_pages.len() - 1
        }
        else {
            panic!("Font not loaded {}", self.path);
        }
    }
}