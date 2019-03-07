use crate::cx::*;

#[derive(Clone)]
pub enum Wrapping{
    Char,
    Word,
    Line,
    None
}

#[derive(Clone)]
pub struct Text{
    pub font_id:usize,
    pub shader_id:usize,
    pub text:String,
    pub color: Vec4,
    pub font_size:f32,
    pub line_spacing:f32,
    pub wrapping:Wrapping,
}

impl Style for Text{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_text_shader(cx);
        Self{
            shader_id:cx.add_shader(sh),
            font_id:cx.load_font(&cx.style.normal_font.clone()),
            text:"".to_string(),
            font_size:cx.style.font_size,
            line_spacing:1.1,
            wrapping:Wrapping::Word,
            color:cx.style.text_normal.clone()
        }
    }
}

impl Text{
    pub fn def_text_shader(cx:&mut Cx)->Shader{
        // lets add the draw shader lib
        let mut sh = cx.new_shader();
        sh.geometry_vertices = vec![
            0.0,0.0,
            1.0,0.0,
            1.0,1.0,
            0.0,1.0
        ];
        sh.geometry_indices = vec![
            0,1,2,
            0,3,2
        ];

        sh.add_ast(shader_ast!({
            let geom:vec2<Geometry>;
            let texture:texture2d<Texture>;
            let tex_size:vec2<Uniform>;
            //let list_clip:vec4<Uniform>;
            let draw_call_clip:vec4<Instance>;
            let font_geom:vec4<Instance>;
            let font_tc:vec4<Instance>;
            let color:vec4<Instance>;
            let x:float<Instance>;
            let y:float<Instance>;
            let w:float<Instance>;
            let h:float<Instance>;
            let font_size:float<Instance>;
            let font_base:float<Instance>;
            let tex_coord:vec2<Varying>;

            fn pixel()->vec4{
                let s:vec4 = sample2d(texture, tex_coord.xy);
                let sig_dist:float =  max(min(s.r, s.g), min(max(s.r, s.g), s.b)) - 0.5;

                df_viewport(tex_coord * tex_size * 0.07);
                df_shape = -sig_dist - 0.5 / df_aa;
                return df_fill(color); 
            }
            
            fn vertex()->vec4{
                let shift:vec2 = -draw_list_scroll;
                let min_pos:vec2 = vec2(
                    x + font_size * font_geom.x,
                    y - font_size * font_geom.y + font_size * font_base
                );
                let max_pos:vec2 = vec2(
                    x + font_size * font_geom.z,
                    y - font_size * font_geom.w + font_size * font_base
                );
                
                let clipped:vec2 = clamp(
                    mix(min_pos, max_pos, geom) + shift,
                    max(draw_call_clip.xy, draw_list_clip.xy),
                    min(draw_call_clip.zw, draw_list_clip.zw)
                );
                let normalized:vec2 = (clipped - min_pos - shift) / (max_pos - min_pos);

                tex_coord = mix(
                    font_tc.xy,
                    font_tc.zw,
                    normalized.xy
                );

                return vec4(clipped,0.,1.) * camera_projection;
            }
        }));
        sh
    }

    pub fn draw_text(&mut self, cx:&mut Cx, text:&str)->Area{
        if !cx.fonts[self.font_id].loaded{
            return Area::Empty
        }

        let area = cx.new_aligned_instance(self.shader_id);

        if area.need_uniforms_now(cx){
            area.uniform_texture_2d(cx, "texture", cx.fonts[self.font_id].texture_id);
            area.uniform_vec2f(cx, "tex_size", cx.fonts[self.font_id].width as f32, cx.fonts[self.font_id].height as f32);
            area.uniform_vec4f(cx, "list_clip", -50000.0,-50000.0,50000.0,50000.0);
        }

        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut count = 0;
        for (last,c) in text.chars().identify_last(){
            let mut slot = 0;
            let mut emit = last;

            if c < '\u{10000}'{
                slot = cx.fonts[self.font_id].unicodes[c as usize];
            }

            if slot != 0 {
                let glyph = &cx.fonts[self.font_id].glyphs[slot];
                width += glyph.advance * self.font_size;
                match self.wrapping{
                    Wrapping::Char=>{
                        chunk.push(c);
                        emit = true
                    },
                    Wrapping::Word=>{
                        chunk.push(c);
                        if c == 32 as char || c == 9 as char|| c == 10 as char|| c == 13 as char{
                            emit = true;
                        }
                    },
                    Wrapping::Line=>{
                        chunk.push(c);
                        if c == 10 as char|| c == 13 as char{
                            emit = true;
                        }
                    },
                    Wrapping::None=>{
                        chunk.push(c);
                    }
                }
            }
            if emit{
                let height = self.font_size * self.line_spacing;
                let mut geom = cx.walk_turtle(
                    Bounds::Fix(width), 
                    Bounds::Fix(height), 
                    Margin::zero(),
                    None
                );
                for wc in &chunk{
                    let slot = cx.fonts[self.font_id].unicodes[*wc as usize];
                    let glyph = &cx.fonts[self.font_id].glyphs[slot];
                    let w = glyph.advance * self.font_size;
                    
                    let data = [
                        /*draw_clip*/ -50000.0,-50000.0,50000.0,50000.0,
                        /*font_geom*/ glyph.x1 ,glyph.y1 ,glyph.x2 ,glyph.y2,
                        /*font_tc*/ glyph.tx1 ,glyph.ty1 ,glyph.tx2 ,glyph.ty2,
                        /*color*/ 1.0,1.0,1.0,1.0,
                        /*x*/ geom.x,
                        /*y*/ geom.y,
                        /*w*/ w,
                        /*h*/ height,
                        /*font_size*/ self.font_size,
                        /*font_base*/ 1.0
                    ];
                    area.push_data(cx, &data);

                    geom.x += w;
                    count += 1;
                }
                width = 0.0;
                chunk.truncate(0);
            }
        }
        cx.instance_aligned_set_count(count)
    }
}

// identifying last item in iterator

trait IdentifyLast: Iterator + Sized {
    fn identify_last(self) -> Iter<Self>;
}

impl<T> IdentifyLast for T where T: Iterator {
    fn identify_last(mut self) -> Iter<Self> {
        let e = self.next();
        Iter {
            iter: self,
            buffer: e,
        }
    }
}

struct Iter<T> where T: Iterator {
    iter: T,
    buffer: Option<T::Item>,
}

impl<T> Iterator for Iter<T> where T: Iterator {
    type Item = (bool, T::Item);

    fn next(&mut self) -> Option<Self::Item> {
        match self.buffer.take() {
            None => None,
            Some(e) => {
                match self.iter.next() {
                    None => Some((true, e)),
                    Some(f) => {
                        self.buffer = Some(f);
                        Some((false, e))
                    },
                }
            },
        }
    }
}