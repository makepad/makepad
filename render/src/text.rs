use crate::cx::*;
//use std::iter::Peekable;

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
    pub font:Font,
    pub shader:Shader,
    pub text:String,
    pub color: Color,
    pub font_size:f32,
    pub do_dpi_dilate:bool,
    pub brightness:f32,
    pub line_spacing:f32,
    pub wrapping:Wrapping,
}

impl Style for Text{
    fn style(cx:&mut Cx)->Self{
        Self{
            shader:cx.add_shader(Self::def_text_shader(), "Text"),
            font:cx.load_font_style("normal_font"),
            text:"".to_string(),
            font_size:cx.size("font_size") as f32,
            line_spacing:1.15,
            do_dpi_dilate:false,
            brightness:1.0,
            wrapping:Wrapping::Word,
            color:color("white")
        }
    }
}

impl Text{
    pub fn def_text_shader()->ShaderGen{
        // lets add the draw shader lib
        let mut sg = ShaderGen::new();
        sg.geometry_vertices = vec![
            0.0,0.0,
            1.0,0.0,
            1.0,1.0,
            0.0,1.0
        ];
        sg.geometry_indices = vec![
            0,1,2,
            0,3,2
        ];

        sg.compose(shader_ast!({
            let geom:vec2<Geometry>;
            let texturez:texture2d<Texture>;
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
            let char_offset:float<Instance>;
            let marker:float<Instance>;
            //let font_base:float<Instance>;
            let tex_coord:vec2<Varying>;
            let clipped:vec2<Varying>;
            let rect:vec4<Varying>;
            let brightness:float<Uniform>;

            fn pixel()->vec4{
                if marker>0.5{
                    df_viewport(clipped);
                    let center = (rect.xy+rect.zw)*0.5;
                    df_circle(center.x, center.y, 1.);
                    return df_fill(color); 
                }
                else{
                    let s = sample2d(texturez, tex_coord.xy);
                    let sig_dist =  max(min(s.r, s.g), min(max(s.r, s.g), s.b)) - 0.5;
                    //let scale = pow(df_antialias(clipped) * 0.002,0.5);
                    df_viewport(tex_coord * tex_size * (0.1 - dpi_dilate*0.03));
                    df_shape = (-sig_dist - (0.5 / df_aa)) - dpi_dilate*0.1;
                    return df_fill(color*brightness); 
                }
            }
            
            fn vertex()->vec4{
                let shift:vec2 = -view_scroll;

                let min_pos = vec2(
                    x + font_size * font_geom.x,
                    y - font_size * font_geom.y + font_size// * font_base
                );

                let max_pos = vec2(
                    x + font_size * font_geom.z,
                    y - font_size * font_geom.w + font_size// * font_base
                );
                
                clipped = clamp(
                    mix(min_pos, max_pos, geom) + shift,
                    view_clip.xy,
                    view_clip.zw
                );

                let normalized:vec2 = (clipped - min_pos - shift) / (max_pos - min_pos);
                rect = vec4(min_pos.x,min_pos.y,max_pos.x,max_pos.y)+shift.xyxy;

                tex_coord = mix(
                    font_tc.xy,
                    font_tc.zw,
                    normalized.xy
                );

                return vec4(clipped,0.,1.) * camera_projection;
            }
        }))
    }

    pub fn begin_text(&mut self, cx:&mut Cx)->AlignedInstance{
        let font_id = self.font.font_id.unwrap();
        if !cx.fonts[font_id].loaded{
            panic!("Font not loaded")
        }
        let inst = cx.new_instance(&self.shader, 0); 
        let aligned = cx.align_instance(inst);
        
        if aligned.inst.need_uniforms_now(cx){
            //texture,
            aligned.inst.push_uniform_texture_2d(cx, &self.font.texture);
            //tex_size
            aligned.inst.push_uniform_vec2f(cx, cx.fonts[font_id].width as f32, cx.fonts[font_id].height as f32);
            aligned.inst.push_uniform_float(cx, self.brightness);

            //list_clip
            //area.push_uniform_vec4f(cx, -50000.0,-50000.0,50000.0,50000.0);
        }
        return aligned
    }

    pub fn add_text<F>(&mut self, cx:&mut Cx, geom_x:f32, geom_y:f32, char_offset:usize, aligned:&mut AlignedInstance, chunk:&[char], mut char_callback:F)
    where F: FnMut(char, usize, f32, f32)->f32
    {
        let mut geom_x = geom_x;
        let mut char_offset = char_offset;
        let font_id = self.font.font_id.unwrap();
        let unicodes = &cx.fonts[font_id].unicodes;
        let glyphs = &cx.fonts[font_id].glyphs;
        let instance = {
            let cxview = &mut cx.views[aligned.inst.view_id];
            let draw_call = &mut cxview.draw_calls[ aligned.inst.draw_call_id];
            &mut draw_call.instance
        };

        for wc in chunk{
            let unicode = *wc as usize;
            let slot = unicodes[unicode as usize];
            let glyph = &glyphs[slot];
            let w = glyph.advance * self.font_size;
            let marker = char_callback(*wc, char_offset, geom_x, w);
            let data = [
                /*font_geom*/ glyph.x1 ,glyph.y1 ,glyph.x2 ,glyph.y2,
                /*font_tc*/ glyph.tx1 ,glyph.ty1 ,glyph.tx2 ,glyph.ty2,
                /*color*/ self.color.r, self.color.g, self.color.b, self.color.a,
                /*x*/ geom_x,
                /*y*/ geom_y,
                // /*w*/ w,
                // /*h*/ height,
                /*font_size*/ self.font_size,
                /*char_offset*/ char_offset as f32,
                /*marker*/ marker,
                // /*font_base*/ 1.0
            ];
            instance.extend_from_slice(&data);
            /*
            for i in 0..15{
                instance.push(0.)
            }*/
            
            geom_x += w;
            char_offset += 1;
            aligned.inst.instance_count += 1;
        }
    }
  
    pub fn end_text(&mut self, cx:&mut Cx, aligned:&AlignedInstance){
        cx.update_aligned_instance_count(aligned);
    }

    pub fn draw_text(&mut self, cx:&mut Cx, text:&str)->Area{
        let mut aligned = self.begin_text(cx);

        let mut chunk = Vec::new();
        let mut width = 0.0;
        let mut elipct = 0;
        let font_size = self.font_size;
        let mut iter = text.chars().peekable();
        let font_id = self.font.font_id.unwrap();
        while let Some(c) = iter.next(){
            let last = iter.peek().is_none();

            let mut emit = last;
            let slot = if c < '\u{10000}'{
                cx.fonts[font_id].unicodes[c as usize]
            } else{
                0
            };

            if slot != 0 {
                let glyph = &cx.fonts[font_id].glyphs[slot];
                width += glyph.advance * self.font_size;
                match self.wrapping{
                    Wrapping::Char=>{
                        chunk.push(c);
                        emit = true
                    },
                    Wrapping::Word=>{
                        chunk.push(c);
                        if c == ' ' || c == '\t' || c == '\n' || c==',' {
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

                self.add_text(cx, geom.x, geom.y, 0, &mut aligned, &chunk, |_,_,_,_|{0.0});
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
        self.end_text(cx, &aligned);
        aligned.inst.into_area()
    }

    pub fn find_closest_offset(&self, cx:&Cx, area:&Area, pos:Vec2)->usize{
        // ok so, we have a bunch of text geom,
        // now we need to find the closest offset
        // first we go find when the y>= y
        // then we scan for x<=x
        let scroll_pos = area.get_scroll_pos(cx);
        let spos = Vec2{x:pos.x + scroll_pos.x, y:pos.y + scroll_pos.y};
        let x_o = area.get_instance_offset(cx, "x");
        let y_o = area.get_instance_offset(cx, "y");
        let font_geom_o = area.get_instance_offset(cx, "font_geom")+2;
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
                        let font_size = read.buffer[read.offset + font_size_o + index* read.slots]; 
                        let w = read.buffer[read.offset + font_geom_o + index * read.slots] * font_size;
                        if x > spos.x + w*0.5 || y > spos.y{
                            let prev_index = if index == 0{0}else{index - 1};
                            let prev_x = read.buffer[read.offset + x_o +  prev_index * read.slots];
                            let prev_w = read.buffer[read.offset + font_geom_o + index * read.slots] * font_size;
                            if prev_x > spos.x + prev_w{
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

    pub fn get_monospace_base(&self, cx:&Cx)->Vec2{
        let font_id = self.font.font_id.unwrap();
        let slot = cx.fonts[font_id].unicodes[33 as usize];
        let glyph = &cx.fonts[font_id].glyphs[slot];
        //let font_size = if let Some(font_size) = font_size{font_size}else{self.font_size};
        Vec2{
            x: glyph.advance,
            y: self.line_spacing
        }
    }
}