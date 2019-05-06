use crate::cx::*;

#[derive(Clone)]
pub struct BinaryDep{
    pub name:String,
    pub vec_obj:Vec<u8>,
    pub parse:isize
}

impl BinaryDep{
    pub fn new_from_vec(name:String, vec_obj:Vec<u8>)->BinaryDep{

        BinaryDep{
            name:name, 
            vec_obj:vec_obj,
            parse:0
        }
    }

    pub fn u8(&mut self)->Result<u8, String>{
        if self.parse + 1 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u8 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u8).read();
            self.parse += 1;
            Ok(ret)
        }
    }

    pub fn u16(&mut self)->Result<u16, String>{
        if self.parse+2 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u16 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u16).read();
            self.parse += 2;
            Ok(ret)
        }
    }

    pub fn u32(&mut self)->Result<u32, String>{
        if self.parse+4 > self.vec_obj.len() as isize{
            return Err(format!("Eof on u32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const u32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn f32(&mut self)->Result<f32, String>{
        if self.parse+4 > self.vec_obj.len() as isize{
            return Err(format!("Eof on f32 file {} offset {}", self.name, self.parse))
        }
        unsafe{
            let ret = (self.vec_obj.as_ptr().offset(self.parse) as *const f32).read();
            self.parse += 4;
            Ok(ret)
        }
    }

    pub fn read(&mut self, out:&mut [u8])->Result<usize, String>{
        let len = out.len();
        if self.parse + len as isize > self.vec_obj.len() as isize{
             return Err(format!("Eof on read file {} len {} offset {}", self.name, out.len(), self.parse));
        };
        //unsafe{
            for i in 0..len{
                out[i] = self.vec_obj[self.parse as usize + i];
            };
            self.parse += len as isize;
        //}
        Ok(len)
    }
}
impl Cx{
    pub fn load_font(&mut self, file_name: &str)->usize{
        let found = self.fonts.iter().position(|v| v.name == file_name);
        if !found.is_none(){
            return found.unwrap()
        }
        let font_id = self.fonts.len();
        self.fonts.push(Font{
            name:file_name.to_string(),
            loaded:false,
            ..Default::default()
        });
        font_id
    }

    pub fn load_font_from_binary_dep(&mut self, bin_dep: &mut BinaryDep)-> Result<(), String>{
        let found = self.fonts.iter().position(|v| v.name == bin_dep.name);
        if found.is_none(){
            return Err("Binary dep not a font".to_string());
        }
        let (font,font_id, texture_id)={
            let mut out_tex = self.new_empty_texture_2d();
            (Font::from_binary_dep(bin_dep, &mut out_tex)?, found.unwrap(), out_tex.texture_id)
        };
        self.fonts[font_id] = Font{
            font_id:self.fonts.len(),
            texture_id: texture_id,
            loaded:true,
            ..font
        };
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct Glyph{
    pub unicode:u32,
    pub x1:f32,
    pub y1:f32,
    pub x2:f32,
    pub y2:f32,
    pub advance:f32,
    pub tsingle:usize,
    pub toffset:usize,
    pub tw:usize,
    pub th:usize,
    pub tx1:f32,
    pub ty1:f32,
    pub tx2:f32,
    pub ty2:f32
}

#[derive(Default, Clone)]
pub struct Kern{
    pub i:u32,
    pub j:u32,
    pub kern:f32
}

#[derive(Default, Clone)]
pub struct Font{
    pub name:String,
    pub loaded:bool,
    pub font_id:usize,   
    pub width:usize,
    pub height:usize,
    pub slots:usize,
    pub rgbsize:usize,
    pub onesize:usize,
    pub kernsize:usize, 
    pub scale:f32,
    pub glyphs:Vec<Glyph>,
    pub unicodes:Vec<usize>,
    pub kerntable:Vec<Kern>,
    pub texture_id:usize
}

impl Font{
    pub fn from_binary_dep(inp: &mut BinaryDep, tex:&mut Texture2D) -> Result<Font, String> {
        let _type_id = inp.u32()?;

        let mut ff = Font{
            font_id: 0,
            width: inp.u16()? as usize,
            height: inp.u16()? as usize,
            slots: inp.u32()? as usize,
            rgbsize: inp.u32()? as usize,
            onesize: inp.u32()? as usize,
            kernsize:inp.u32()? as usize,
            scale:inp.f32()?,
            ..Default::default()
        };
        ff.unicodes.resize(65535, 0);

        ff.glyphs.reserve(ff.slots as usize);
        for _i in 0..(ff.slots as usize){
            ff.glyphs.push(Glyph{
                unicode: inp.u32()?,
                x1: inp.f32()?,
                y1: inp.f32()?,
                x2: inp.f32()?,
                y2: inp.f32()?,
                advance: inp.f32()?,
                tsingle: inp.u32()? as usize,
                toffset: inp.u32()? as usize,
                tw: inp.u32()? as usize,
                th: inp.u32()? as usize,
                tx1:0.0,
                ty1:0.0,
                tx2:0.0,
                ty2:0.0
            })
        }
        // read the kerning table
        ff.kerntable.reserve(ff.kernsize as usize);
        for _i in 0..(ff.kernsize){
            ff.kerntable.push(Kern{
                i: inp.u32()?,
                j: inp.u32()?,
                kern: inp.f32()?
            })
        }

        // now lets read the texture
        let mut r_buf: Vec<u8> = Vec::with_capacity(ff.rgbsize as usize);//[u8; usize ff.texpage];
        let mut g_buf: Vec<u8> = Vec::with_capacity(ff.rgbsize as usize);
        let mut b_buf: Vec<u8> = Vec::with_capacity(ff.rgbsize as usize);
        let mut s_buf: Vec<u8> = Vec::with_capacity(ff.onesize as usize);

        r_buf.resize(r_buf.capacity(), 0);
        g_buf.resize(g_buf.capacity(), 0);
        b_buf.resize(b_buf.capacity(), 0);
        s_buf.resize(s_buf.capacity(), 0);

        // lets allocate a texture
        tex.resize(ff.width, ff.height);

        // ok lets read the different buffers
        inp.read(r_buf.as_mut_slice())?;
        inp.read(g_buf.as_mut_slice())?;
        inp.read(b_buf.as_mut_slice())?;
        inp.read(s_buf.as_mut_slice())?;

        let mut ox = 0;
        let mut oy = 0;
        let mut mh = 0;
        for i in 0..(ff.slots as usize){
            let b = &mut ff.glyphs[i];

            if ox + b.tw >= ff.width{
                ox = 0;
                oy = mh +1;
                mh = 0;
            }

            if b.th > mh{
                mh = b.th
            }

            if b.tsingle != 0{
                let mut ow = b.toffset;
                for y in 0..b.th{
                    for x in 0..b.tw{
                        let v = s_buf[ow as usize] as u32;
                        tex.image[ (x + ox + ((y + oy) * ff.width))] = (v<<16) | (v<<8) | v;
                        ow = ow + 1;
                    }
                }
            }
            else{
                let mut ow = b.toffset;
                for y in 0..b.th{
                    for x in 0..b.tw{
                        let r = r_buf[ow as usize] as u32;
                        let g = g_buf[ow as usize] as u32;
                        let b = b_buf[ow as usize] as u32;
                        tex.image[ (x + ox + ((y + oy) * ff.width))] = (r<<16) | (g<<8) | b;
                        ow = ow + 1;
                    }
                }
            }
            b.tx1 = (ox as f32) / (ff.width as f32);
            b.ty1 = ((oy+b.th) as f32) / (ff.height as f32);
            b.tx2 = ((ox+b.tw) as f32) / (ff.width as f32);
            b.ty2 = (oy as f32) / (ff.height as f32);
            ff.unicodes[b.unicode as usize] = i as usize;
            //ff.unicodes.insert(b.unicode, i as  u32);
            ox += b.tw+1;
        }
        /*
        ff.unicodes[32] = ff.glyphs.len();
        ff.glyphs.push(Glyph{
            unicode:32,
            x1:0.0,
            y1:-0.3,
            x2:0.5,
            y2:1.0,
            advance:0.5,
            tsingle:0,
            toffset:0,
            tw:0,
            th:0,
            tx1:0.0,
            ty1:0.0,
            tx2:0.0,
            ty2:0.0,
        });*/

        let mut excl_slot = ff.glyphs[ff.unicodes[33]].clone();

        // set texture coord to 0
        excl_slot.tx1 = 0.0;
        excl_slot.ty1 = 0.0;
        excl_slot.tx2 = 0.0;
        excl_slot.ty2 = 0.0;

        ff.unicodes[32] = ff.glyphs.len();
        ff.glyphs.push(Glyph{
            unicode:32,
            ..excl_slot.clone()
        });

        ff.unicodes[10] = ff.glyphs.len();
        ff.glyphs.push(Glyph{
            unicode:10,
            ..excl_slot.clone()
        });

        ff.unicodes[9] = ff.glyphs.len();
        ff.glyphs.push(Glyph{
            unicode:9,
            ..excl_slot.clone()
        });

        Ok(ff)
    }
}