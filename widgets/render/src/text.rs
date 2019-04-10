use crate::cx::*;
use std::iter::Peekable;

#[derive(Clone)]
pub enum Wrapping{
    Char,
    Word,
    Line,
    None,
    Ellipsis(f32)
}

#[derive(Clone)]
pub struct Text{
    pub font_id:usize,
    pub shader_id:usize,
    pub text:String,
    pub color: Vec4,
    pub font_size:f32,
    pub boldness:f32,
    pub line_spacing:f32,
    pub wrapping:Wrapping,
}

impl Style for Text{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_text_shader(cx);
        Self{
            shader_id:cx.add_shader(sh, "Text"),
            font_id:cx.load_font(&cx.font("normal_font")),
            text:"".to_string(),
            font_size:cx.size("font_size") as f32,
            line_spacing:1.15,
            boldness:0.,
            wrapping:Wrapping::Word,
            color:color("white")
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
            //let instance_clip:vec4<Instance>;
            let font_geom:vec4<Instance>;
            let font_tc:vec4<Instance>;
            let color:vec4<Instance>;
            let x:float<Instance>;
            let y:float<Instance>;
            //let w:float<Instance>;
            //let h:float<Instance>;
            let font_size:float<Instance>;
            //let font_base:float<Instance>;
            let tex_coord:vec2<Varying>;
            let clipped:vec2<Varying>;

            fn pixel()->vec4{
                let s:vec4 = sample2d(texture, tex_coord.xy);
                let sig_dist:float =  max(min(s.r, s.g), min(max(s.r, s.g), s.b)) - 0.5;
                let scale:float = pow(df_antialias(clipped) * 0.002,0.5);
                df_viewport(tex_coord * tex_size * scale);
                df_shape = -sig_dist - 0.5 / df_aa;
                return df_fill(color); 
            }
            
            fn vertex()->vec4{
                let shift:vec2 = -draw_list_scroll;
                let min_pos:vec2 = vec2(
                    x + font_size * font_geom.x,
                    y - font_size * font_geom.y + font_size// * font_base
                );
                let max_pos:vec2 = vec2(
                    x + font_size * font_geom.z,
                    y - font_size * font_geom.w + font_size// * font_base
                );
                
                clipped = clamp(
                    mix(min_pos, max_pos, geom) + shift,
                    draw_list_clip.xy,
                    draw_list_clip.zw
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

    pub fn begin_chunks(&mut self, cx:&mut Cx)->Area{
        if !cx.fonts[self.font_id].loaded{
            return Area::Empty
        }
        let area = cx.new_aligned_instance(self.shader_id);
        if area.need_uniforms_now(cx){
            //texture,
            area.push_uniform_texture_2d(cx, cx.fonts[self.font_id].texture_id);
            //tex_size
            area.push_uniform_vec2f(cx, cx.fonts[self.font_id].width as f32, cx.fonts[self.font_id].height as f32);
            //list_clip
            //area.push_uniform_vec4f(cx, -50000.0,-50000.0,50000.0,50000.0);
        }
        return area
    }

    pub fn draw_chunk(&mut self, cx:&mut Cx, geom_x:f32, geom_y:f32, area:&Area, chunk:&[char]){
        let mut geom_x = geom_x;
        let unicodes = &cx.fonts[self.font_id].unicodes;
        let glyphs = &cx.fonts[self.font_id].glyphs;
        let instance = match area{
            Area::Instance(inst)=>{
                let draw_list = &mut cx.draw_lists[inst.draw_list_id];
                let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                &mut draw_call.instance
            },
            _=>{
                let draw_list = &mut cx.draw_lists[0];
                let draw_call = &mut draw_list.draw_calls[0];
                &mut draw_call.instance
            }
        };

        for wc in chunk{
            let slot = unicodes[*wc as usize];
            let glyph = &glyphs[slot];
            let w = glyph.advance * self.font_size;
            
            let data = [
                /*font_geom*/ glyph.x1 ,glyph.y1 ,glyph.x2 ,glyph.y2,
                /*font_tc*/ glyph.tx1 ,glyph.ty1 ,glyph.tx2 ,glyph.ty2,
                /*color*/ self.color.x, self.color.y, self.color.z, self.color.w,
                /*x*/ geom_x,
                /*y*/ geom_y,
                // /*w*/ w,
                // /*h*/ height,
                /*font_size*/ self.font_size
                // /*font_base*/ 1.0
            ];
            instance.extend_from_slice(&data);
            /*
            for i in 0..15{
                instance.push(0.)
            }*/
            geom_x += w;
            //count += 1;
        }        
    }
  
    pub fn end_chunks(&mut self, cx:&mut Cx, count:usize)->Area{
        cx.set_count_of_aligned_instance(count)
    }

    pub fn draw_text(&mut self, cx:&mut Cx, text:&str)->Area{
        let area = self.begin_chunks(cx);
        if let Area::Empty = area{
            return area
        }

        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut count = 0;
        let mut elipct = 0;
        let font_size = self.font_size;
        let mut iter = text.chars().peekable();
        while let Some(c) = iter.next(){
            let last = iter.peek().is_none();

            let mut emit = last;
            let slot = if c < '\u{10000}'{
                cx.fonts[self.font_id].unicodes[c as usize]
            } else{
                0
            };

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
                    },
                    Wrapping::Ellipsis(ellipsis_width)=>{
                        if width>ellipsis_width{ // output ...
                            if elipct < 3{
                                chunk.push('.');
                                elipct += 1;
                            }
                        }
                        else{
                            chunk.push(c)
                        }
                    }
                }
            }
            if emit{
                let height = font_size * self.line_spacing;
                let geom = cx.walk_turtle(
                    Bounds::Fix(width), 
                    Bounds::Fix(height), 
                    Margin::zero(),
                    None
                );

                self.draw_chunk(cx, geom.x, geom.y, &area, &chunk);
                count += chunk.len();
                width = 0.0;
                chunk.truncate(0);
                match self.wrapping{
                    Wrapping::Line=>{
                        cx.turtle_new_line();
                    },
                    _=>()
                    
                }
            }
        }
        self.end_chunks(cx, count)
    }
}