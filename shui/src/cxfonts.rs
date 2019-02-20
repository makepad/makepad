use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::mem;
use crate::cxtextures::*;

#[derive(Clone, Default)]
pub struct CxFonts{
    pub fonts:Vec<Font>
}

impl CxFonts{
    pub fn get(&mut self, id:usize)->&Font{
        let font = &mut self.fonts[id];
        font
    }

    pub fn load(&mut self, file_name: &str, tex:&mut CxTextures)->usize{
        let font_id = self.fonts.len();
        let mut out_tex = tex.add_empty();
        self.fonts.push(
            Font{
                font_id:font_id,
                texture_id: out_tex.texture_id,
                ..Font::read(file_name, &mut out_tex).unwrap()
            }
        );
        font_id
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
    pub font_id:usize,   
    pub width:usize,
    pub height:usize,
    pub slots:usize,
    pub rgbsize:usize,
    pub onesize:usize,
    pub kernsize:usize, 
    pub glyphs:Vec<Glyph>,
    pub unicodes:Vec<usize>,
    pub kerntable:Vec<Kern>,
    pub texture_id:usize
}

impl Font{
    pub fn read(inp: &str, tex:&mut crate::cxtextures::Texture) -> io::Result<Font> {
        let mut file = File::open(inp).unwrap();
        let _type_id = file.read_u32le();

        let mut ff = Font{
            font_id: 0,
            width: file.read_u16le() as usize,
            height: file.read_u16le() as usize,
            slots: file.read_u32le() as usize,
            rgbsize: file.read_u32le() as usize,
            onesize: file.read_u32le() as usize,
            kernsize:file.read_u32le() as usize,
            ..Default::default()
        };
        ff.unicodes.resize(65535, 0);

        ff.glyphs.reserve(ff.slots as usize);
        for _i in 0..(ff.slots as usize){
            ff.glyphs.push(Glyph{
                unicode: file.read_u32le(),
                x1: file.read_f32le(),
                y1: file.read_f32le(),
                x2: file.read_f32le(),
                y2: file.read_f32le(),
                advance: file.read_f32le(),
                tsingle: file.read_u32le() as usize,
                toffset: file.read_u32le() as usize,
                tw: file.read_u32le() as usize,
                th: file.read_u32le() as usize,
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
                i: file.read_u32le(),
                j: file.read_u32le(),
                kern: file.read_f32le()
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
        file.read(r_buf.as_mut_slice())?;
        file.read(g_buf.as_mut_slice())?;
        file.read(b_buf.as_mut_slice())?;
        file.read(s_buf.as_mut_slice())?;

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

        Ok(ff)
    }
}

trait ReadBytes : Read{
    #[inline]

    fn read_u16le(&mut self) -> u16 {
        let mut x = [0;2];
        self.read(&mut x).unwrap();
        let ret = (x[0] as u16) | (x[1] as u16) << 8;
        ret
    }

    fn read_u32le(&mut self) -> u32 {
        let mut x = [0;4];
        self.read(&mut x).unwrap();
        let ret = (x[0] as u32) | ((x[1] as u32) << 8) | ((x[2] as u32) << 16) | ((x[3] as u32) << 24);
        ret
    }

    fn read_f32le(&mut self) -> f32 {
        let mut x = [0;4];
        self.read(&mut x).unwrap();
        let ret = (x[0] as u32) | ((x[1] as u32) << 8) | ((x[2] as u32) << 16) | ((x[3] as u32) << 24);
        unsafe{
            let ret:f32 = mem::transmute(ret);
            ret
        }
    }
}

impl<W: Read> ReadBytes for W {}

