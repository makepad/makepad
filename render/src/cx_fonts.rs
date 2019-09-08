use crate::cx::*;
use trapezoidator::Trapezoidator;
use geometry::{AffineTransformation, Transform, Vector};
use internal_iter::*;
use path::PathIterator;

#[derive(Default, Clone)]
pub struct Font {
    pub font_id: Option<usize>,
}

impl Font {
    pub fn get_atlas_texture_id(&self, cx: &Cx) -> usize {
        cx.fonts[self.font_id.unwrap()].atlas_texture_id
    }
}

impl Cx {
    
    pub fn load_font_style(&mut self, style: &str) -> Font {
        self.load_font_path(&self.font(style))
    }
    
    pub fn load_font_path(&mut self, path: &str) -> Font {
        let texture_size = Vec2 {x: 4096.0, y: 4096.0};
        let found = self.fonts.iter().position( | v | v.path == path);
        if let Some(font_id) = found {
            return Font {
                font_id: Some(font_id),
            }
        }
        
        let atlas_texture_id = self.alloc_texture_id();
        self.textures[atlas_texture_id].desc = TextureDesc {
            format: TextureFormat::RenderBGRA,
            width: Some(texture_size.x as usize),
            height: Some(texture_size.y as usize),
            multisample: None
        };
        
        let font_id = self.fonts.len();
        self.fonts.push(CxFont {
            path: path.to_string(),
            atlas_texture_id: atlas_texture_id,
            texture_size: texture_size,
            ..Default::default()
        });
        
        return Font {
            font_id: Some(font_id)
        }
    }
    
}

pub struct CxAppFontPass {
    pub pass: Pass,
    pub view: View<NoScroll>
}

pub struct CxAfterDraw {
    pub trapezoid_text: TrapezoidText,
    pub font_passes: Vec<CxAppFontPass>
}

pub struct TrapezoidText {
    shader: Shader,
    trapezoidator: Trapezoidator
}

impl TrapezoidText {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            shader: cx.add_shader(Self::def_trapezoid_shader(), "TrapezoidShader"),
            trapezoidator: Trapezoidator::default()
        }
    }
    
    pub fn def_trapezoid_shader() -> ShaderGen {
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        sg.geometry_indices = vec![0, 1, 2, 2, 3, 0];
        
        sg.compose(shader_ast!({
            
            let geom: vec2<Geometry>;
            
            let a_xs: vec2<Instance>;
            let a_ys: vec4<Instance>;
            
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
                let p_min = v_pixel.xy - 0.5;
                let p_max = v_pixel.xy + 0.5;
                let b_minx = p_min.x + 1.0 / 3.0;
                let r_minx = p_min.x - 1.0 / 3.0;
                let b_maxx = p_max.x + 1.0 / 3.0;
                let r_maxx = p_max.x - 1.0 / 3.0;
                return vec4(
                    compute_clamped_trapezoid_area(vec2(r_minx, p_min.y), vec2(r_maxx, p_max.y)),
                    compute_clamped_trapezoid_area(p_min, p_max),
                    compute_clamped_trapezoid_area(vec2(b_minx, p_min.y), vec2(b_maxx, p_max.y)),
                    1.0
                );
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
                //return vec4(pos * 2.0 / vec2(1600.,1200.) - 10.0, 0.0, 1.0);
                return camera_projection * vec4(pos, 0.0, 1.0);
            }
        }))
    }
    
    pub fn draw_character(&mut self, cx: &mut Cx, x:f32, y:f32, scale: f32, unicode: char, font: &Font) {
        // now lets make a draw_character function
        let inst = cx.new_instance(&self.shader, 1);
        if inst.need_uniforms_now(cx) {
        }
        
        let trapezoids = {
            let font = cx.fonts[font.font_id.unwrap()].font_loaded.as_ref().unwrap();
            
            let mut trapezoids = Vec::new();
            
            let slot = font.char_code_to_glyph_index_map[unicode as usize];
            let glyph = &font.glyphs[slot];
            
            trapezoids.extend_from_internal_iter(
                self.trapezoidator.trapezoidate(
                    glyph
                        .outline
                        .commands()
                        .map({
                            move |command| {
                                command.transform(
                                    &AffineTransformation::identity()
                                        .translate(Vector::new(glyph.horizontal_metrics.left_side_bearing - glyph.bounds.p_min.x, 0.0))
                                        .uniform_scale(scale)
                                        .translate(Vector::new(x,y))
                                )
                            }
                        })
                        .linearize(1.0),
                ),
            );
            trapezoids
        };
        for trapezoid in trapezoids{
            let data = [
                trapezoid.xs[0], trapezoid.xs[1],
                trapezoid.ys[0], trapezoid.ys[1], trapezoid.ys[2], trapezoid.ys[3],
            ];
            inst.push_slice(cx, &data);
        }
    }
}

impl CxAfterDraw {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            trapezoid_text: TrapezoidText::style(cx),
            font_passes: Vec::new()
        }
    }
    
    pub fn after_draw(&mut self, cx: &mut Cx) {
    }
}

#[derive(Default)]
pub struct CxFont {
    pub path: String,
    pub atlas_texture_id: usize,
    pub font_loaded: Option<font::Font>,
    pub texture_size: Vec2,
    pub alloc_xpos: f32,
    pub alloc_ypos: f32,
    pub alloc_hmax: f32,
    pub sized_atlas: Vec<SizedAtlasGlyph>,
    pub glyphs_to_atlas: Vec<(usize, usize)>,
}

impl CxFont {
    pub fn load_from_ttf_bytes(&mut self, path: &str, bytes: &[u8]) -> ttf_parser::Result<()> {
        let font = ttf_parser::parse_ttf(bytes) ?;
        self.font_loaded = Some(font);
        Ok(())
    }
    
    pub fn fetch_atlas_index(&mut self, dpi_factor: f32, font_size: f32) -> usize {
        for (index, sg) in self.sized_atlas.iter().enumerate() {
            if sg.dpi_factor == dpi_factor && sg.font_size == font_size {
                return index
            }
        }
        if let Some(font) = &self.font_loaded {
            self.sized_atlas.push(SizedAtlasGlyph {
                dpi_factor: dpi_factor,
                font_size: font_size,
                atlas_glyphs: {
                    let mut v = Vec::new();
                    v.resize(font.glyphs.len(), None);
                    v
                }
            });
            self.sized_atlas.len() - 1
            
        }
        else {
            panic!("Font not loaded {}", self.path);
        }
    }
}

pub struct SizedAtlasGlyph {
    pub dpi_factor: f32,
    pub font_size: f32,
    pub atlas_glyphs: Vec<Option<AtlasGlyph>>
}

#[derive(Clone)]
pub struct AtlasGlyph {
    pub tx1: f32,
    pub ty1: f32,
    pub tx2: f32,
    pub ty2: f32,
}
