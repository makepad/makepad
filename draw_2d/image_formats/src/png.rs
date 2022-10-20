// image_formats::png
// by Desmond Germans, 2019

use crate::ImageBuffer;

// Inflate algorithm
const LITLEN_LENGTH: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LITLEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_DIST: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const HCORD: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

// PNG types
const TYPE_L1: u16 = 0x0100;
const TYPE_C1: u16 = 0x0103;
const TYPE_L2: u16 = 0x0200;
const TYPE_C2: u16 = 0x0203;
const TYPE_L4: u16 = 0x0400;
const TYPE_C4: u16 = 0x0403;
const TYPE_L8: u16 = 0x0800;
const TYPE_RGB8: u16 = 0x0802;
const TYPE_C8: u16 = 0x0803;
const TYPE_LA8: u16 = 0x0804;
const TYPE_RGBA8: u16 = 0x0806;
const TYPE_L16: u16 = 0x1000;
const TYPE_RGB16: u16 = 0x1002;
const TYPE_LA16: u16 = 0x1004;
const TYPE_RGBA16: u16 = 0x1006;

// grayscale distributions
const GRAY2: [f32; 4] = [0.0, 0.33333333, 0.66666667, 1.0];

const GRAY4: [f32; 16] = [
    0.0,
    0.06666667,
    0.13333333,
    0.2,
    0.26666667,
    0.33333333,
    0.4,
    0.46666667,
    0.53333333,
    0.6,
    0.66666667,
    0.73333333,
    0.8,
    0.86666667,
    0.93333333,
    1.0
];

const TABLE: usize = 8; // 8 seems to be a good balance
const TABLE_SIZE: usize = 1 << TABLE;

fn bit_reverse(value: u32, width: u32) -> u32 {
    let mut result: u32 = 0;
    for i in 0..width {
        let bit: u32 = (value >> i) & 1;
        result |= bit << (width - i - 1);
    }
    result
}

fn insert_code(tables: &mut Vec<[i16; TABLE_SIZE]>, ofs: u32, code: u16, length: u8) -> u32 {
    let shift = 32 - TABLE;
    if (length as usize) > TABLE {
        let pos: usize = ((ofs >> shift) & ((TABLE_SIZE - 1) as u32)) as usize;
        let p = bit_reverse(pos as u32, TABLE as u32) as usize;
        let mut n: i16 = tables.len() as i16;
        if tables[0][p] == 0 {
            tables.push([0i16; TABLE_SIZE]);
            tables[0][p] = -n;
        }
        else {
            n = -tables[0][p];
        }
        let shift = 32 - TABLE - TABLE;
        let pos = ((ofs >> shift) & ((TABLE_SIZE - 1) as u32)) as usize;
        let count = TABLE_SIZE >> (length - TABLE as u8) as usize;
        for i in pos..pos + count {
            let p = bit_reverse(i as u32, TABLE as u32) as usize;
            tables[n as usize][p] = ((code << 5) | (length as u16)) as i16;
        }
        (count << shift) as u32
    }
    else {
        let pos = ((ofs >> shift) & ((TABLE_SIZE - 1) as u32)) as usize;
        let count = TABLE_SIZE >> length as usize;
        for i in pos..pos + count {
            let p = bit_reverse(i as u32, TABLE as u32) as usize;
            tables[0][p] = ((code << 5) | (length as u16)) as i16;
        }
        (count << shift) as u32
    }
}

fn create_huffman_tables(lengths: &[u8]) -> Vec<[i16; TABLE_SIZE]> {
    let mut tables: Vec<[i16; TABLE_SIZE]> = Vec::new();
    tables.push([0i16; TABLE_SIZE]);
    let mut ofs: u64 = 0;
    for i in 1..25 {
        for k in 0..lengths.len() {
            if lengths[k] == i {
                let size = insert_code(&mut tables, ofs as u32, k as u16, lengths[k]);
                ofs += size as u64;
            }
        }
    }
    tables
}

struct ZipReader<'a> {
    block: &'a [u8],
    rp: usize,
    bit: u32,
    cache: u32,
}

impl<'a> ZipReader<'a> {
    fn new(block: &'a [u8]) -> ZipReader<'a> {
        ZipReader {
            block: block,
            rp: 6, // first 2 bytes a
            bit: 32,
            cache: ((block[2] as u32) | ((block[3] as u32) << 8) | ((block[4] as u32) << 16) | ((block[5] as u32) << 24)) as u32,
        }
    }
    
    fn align(&mut self) -> usize {
        if self.bit < 32 {
            self.bit = 32;
            self.rp - 3
        }
        else {
            self.rp - 4
        }
    }
    
    fn set(&mut self, rp: usize) {
        self.rp = rp;
        self.cache = (self.block[self.rp] as u32) |
        ((self.block[self.rp + 1] as u32) << 8) |
        ((self.block[self.rp + 2] as u32) << 16) |
        ((self.block[self.rp + 3] as u32) << 24);
        self.rp += 4;
        self.bit = 32;
    }
    
    fn read_bits(&mut self, n: u32) -> Result<u32, String> {
        let result: u32 = self.cache & ((1 << n) - 1);
        self.cache >>= n;
        self.bit -= n;
        while self.bit <= 24 {
            if self.rp >= self.block.len() {
                return Err(format!("data corrupt (read_bits rp ({}) >= len ({}))", self.rp, self.block.len()));
            }
            self.cache |= (self.block[self.rp] as u32) << self.bit;
            self.rp += 1;
            self.bit += 8;
        }
        Ok(result)
    }
    
    fn read_symbol(&mut self, prefix: &Vec<[i16; TABLE_SIZE]>) -> Result<u32, String> {
        
        let mut n: usize = 0;
        let mut index = (self.cache & (TABLE_SIZE - 1) as u32) as usize;
        let mut stuff = prefix[n][index];
        let mut already_shifted = 0;
        while stuff < 0 {
            self.cache >>= TABLE;
            self.bit -= TABLE as u32;
            while self.bit <= 24 {
                if self.rp >= self.block.len() {
                    return Err(format!("data corrupt (read_symbol rp ({}) >= len ({}))", self.rp, self.block.len()));
                }
                self.cache |= (self.block[self.rp] as u32) << self.bit;
                self.rp += 1;
                self.bit += 8;
            }
            already_shifted += TABLE;
            n = (-stuff) as usize;
            index = (self.cache & (TABLE_SIZE - 1) as u32) as usize;
            stuff = prefix[n][index];
        }
        let symbol = stuff >> 5;
        let length = (stuff & 31) - already_shifted as i16;
        self.cache >>= length;
        self.bit -= length as u32;
        while self.bit <= 24 {
            if self.rp >= self.block.len() {
                return Err(format!("data corrupt (read_symbol rp ({}) >= len ({}))", self.rp, self.block.len()));
            }
            self.cache |= (self.block[self.rp] as u32) << self.bit;
            self.rp += 1;
            self.bit += 8;
        }
        Ok(symbol as u32)
    }
}

fn inflate(src: &[u8], inflated_size: usize) -> Result<Vec<u8>, String> {
    
    let mut dst: Vec<u8> = vec![0; inflated_size as usize];
    let mut reader = ZipReader::new(&src);
    let mut dp: usize = 0;
    
    // create default litlen table
    let mut lengths: [u8; 288] = [0; 288];
    for i in 0..144 {
        lengths[i] = 8;
    }
    for i in 144..256 {
        lengths[i] = 9;
    }
    for i in 256..280 {
        lengths[i] = 7;
    }
    for i in 280..288 {
        lengths[i] = 8;
    }
    
    let default_hlitlen_tables = create_huffman_tables(&lengths);
    #[allow(unused_assignments)]
    let mut current_hlitlen_tables: Vec<[i16; TABLE_SIZE]> = Vec::new();
    
    // create default dist table
    let lengths: [u8; 32] = [5; 32];
    let default_hdist_tables = create_huffman_tables(&lengths);
    #[allow(unused_assignments)]
    let mut current_hdist_tables: Vec<[i16; TABLE_SIZE]> = Vec::new();
    
    let mut hlitlen_tables: &Vec<[i16; TABLE_SIZE]> = &default_hlitlen_tables;
    let mut hdist_tables: &Vec<[i16; TABLE_SIZE]> = &default_hdist_tables;
    
    // main loop
    let mut is_final = false;
    while !is_final {
        
        // get final block and type bits
        match reader.read_bits(1) {
            Ok(value) => {if value == 1 {is_final = true;} else {is_final = false;}},
            Err(msg) => {return Err(msg);},
        }
        let block_type = match reader.read_bits(2) {
            Ok(value) => {value},
            Err(msg) => {return Err(msg);},
        };
        
        // process uncompressed data
        match block_type {
            0 => {
                let mut sp = reader.align();
                let length = (((src[sp + 1] as u16) << 8) | (src[sp] as u16)) as usize;
                sp += 4;
                if (sp + length > src.len()) || (dp + length > dst.len()) {
                    return Err(format!("data corrupt (sp ({}) or dp ({}) + length ({}) too big)", sp, dp, length));
                }
                dst[dp..dp + length].copy_from_slice(&src[sp..sp + length]);
                sp += length;
                dp += length;
                reader.set(sp);
            },
            1 => {
                hlitlen_tables = &default_hlitlen_tables;
                hdist_tables = &default_hdist_tables;
            },
            2 => {
                // get table metrics
                let hlit = match reader.read_bits(5) {
                    Ok(value) => {value + 257},
                    Err(msg) => {return Err(msg);},
                } as usize;
                let hdist = match reader.read_bits(5) {
                    Ok(value) => {value + 1},
                    Err(msg) => {return Err(msg);},
                } as usize;
                let hclen = match reader.read_bits(4) {
                    Ok(value) => {value + 4},
                    Err(msg) => {return Err(msg);},
                };
                
                // get length codes
                let mut lengths: [u8; 20] = [0; 20];
                for i in 0..hclen {
                    lengths[HCORD[i as usize]] = match reader.read_bits(3) {
                        Ok(value) => {value},
                        Err(msg) => {return Err(msg);},
                    } as u8;
                }
                let hctree_tables = create_huffman_tables(&lengths);
                
                // no really, get length codes
                let mut lengths: [u8; 320] = [0; 320];
                let mut ll: usize = 0;
                while ll < hlit + hdist {
                    let code = match reader.read_symbol(&hctree_tables) {
                        Ok(value) => {value},
                        Err(msg) => {return Err(msg);},
                    };
                    if code == 16 {
                        let length = match reader.read_bits(2) {
                            Ok(value) => {value + 3},
                            Err(msg) => {return Err(msg);},
                        };
                        for _i in 0..length { // TODO: for loop might be expressed differently in rust
                            lengths[ll] = lengths[ll - 1];
                            ll += 1;
                        }
                    }
                    else if code == 17 {
                        let length = match reader.read_bits(3) {
                            Ok(value) => {value + 3},
                            Err(msg) => {return Err(msg);},
                        };
                        for _i in 0..length { // TODO: for loop might be expressed differently in rust
                            lengths[ll] = 0;
                            ll += 1;
                        }
                    }
                    else if code == 18 {
                        let length = match reader.read_bits(7) {
                            Ok(value) => {value + 11},
                            Err(msg) => {return Err(msg);},
                        };
                        for _i in 0..length { // TODO: for loop might be expressed differently in rust
                            lengths[ll] = 0;
                            ll += 1;
                        }
                    }
                    else {
                        lengths[ll] = code as u8;
                        ll += 1;
                    }
                }
                
                current_hlitlen_tables = create_huffman_tables(&lengths[0..hlit]);
                current_hdist_tables = create_huffman_tables(&lengths[hlit..hlit + hdist]);
                
                hlitlen_tables = &current_hlitlen_tables;
                hdist_tables = &current_hdist_tables;
            },
            3 => {
                return Ok(dst);
                //return Err("data corrupt (block type 3)".to_string());
            },
            _ => {},
        }
        if (block_type == 1) || (block_type == 2) {
            // read them codes
            while dp < dst.len() {
                let mut code = match reader.read_symbol(&hlitlen_tables) {
                    Ok(value) => {value},
                    Err(msg) => {return Err(msg);},
                };
                if code < 256 {
                    dst[dp] = code as u8;
                    dp += 1;
                }
                else if code == 256 {
                    break;
                }
                else {
                    // get lit/len length and extra bit entries
                    code -= 257;
                    let mut length = LITLEN_LENGTH[code as usize] as usize;
                    let extra = LITLEN_EXTRA[code as usize] as u32;
                    
                    // read extra bits
                    if extra > 0 {
                        length += match reader.read_bits(extra) {
                            Ok(value) => {value},
                            Err(msg) => {return Err(msg);},
                        } as usize;
                    }
                    
                    // get dist length and extra bit entries
                    code = match reader.read_symbol(&hdist_tables) {
                        Ok(value) => {value},
                        Err(msg) => {return Err(msg);},
                    };
                    let mut dist = DIST_DIST[code as usize] as usize;
                    let extra = DIST_EXTRA[code as usize] as u32;
                    
                    // read extra bits
                    if extra > 0 {
                        dist += match reader.read_bits(extra) {
                            Ok(value) => {value},
                            Err(msg) => {return Err(msg);},
                        } as usize;
                    }
                    
                    // copy block
                    if dp + length > dst.len() {
                        length = dst.len() - dp;
                        //return Err(format!("data corrupt (dp ({}) + length ({}) exceeds dst ({}))",dp,length,dst.len()));
                    }
                    if dist > dp {
                        return Err(format!("data corrupt (dp ({}) - dist ({}) negative)", dp, dist));
                    }
                    if dp + length - dist > dst.len() {
                        return Err(format!("data corrupt (dp ({}) - dist ({}) + length ({}) exceeds dst ({}))", dp, dist, length, dst.len()));
                    }
                    for i in 0..length {
                        dst[dp + i] = dst[dp - dist + i];
                    }
                    dp += length;
                }
            }
        }
    }
    Ok(dst)
}

fn unfilter(src: &[u8], height: usize, stride: usize, bpp: usize) -> Vec<u8> {
   
    let mut dst: Vec<u8> = vec![0; stride * height * bpp];
    let mut sp: usize = 0;
    let mut dp: usize = 0;
    for y in 0..height {
        let ftype = src[sp];
        sp += 1;
        for x in 0..stride {
            let mut s = src[sp] as i32;
            sp += 1;
            let a: i32 = if x >= bpp {dst[dp - bpp] as i32} else {0};
            let b: i32 = if y >= 1 {dst[dp - stride] as i32} else {0};
            let c: i32 = if (y >= 1) && (x >= bpp) {dst[dp - stride - bpp] as i32} else {0};
            s += match ftype {
                0 => {0},
                1 => {a},
                2 => {b},
                3 => {(a + b) >> 1},
                4 => {
                    let d: i32 = a + b - c;
                    let da: i32 = d - a;
                    let pa: i32 = if da < 0 {-da} else {da};
                    let db: i32 = d - b;
                    let pb: i32 = if db < 0 {-db} else {db};
                    let dc: i32 = d - c;
                    let pc: i32 = if dc < 0 {-dc} else {dc};
                    if (pa <= pb) && (pa <= pc) {a} else if pb <= pc {b} else {c}
                },
                _ => {0},
            };
            if s >= 256 {s -= 256};
            if s < 0 {s += 256};
            dst[dp] = s as u8;
            dp += 1;
        }
    }
    dst
}

fn clampf(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    }
    else if v > max {
        max
    }
    else {
        v
    }
}

fn make_lf(l: f32, gamma: f32) -> u32 {
    let ul = (clampf(l.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    return 0xFF000000 | (ul << 16) | (ul << 8) | ul;
}

fn make_rgbaf(r: f32, g: f32, b: f32, a: f32, gamma: f32) -> u32 {
    let ur = (clampf(r.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ug = (clampf(g.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ub = (clampf(b.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ua = (clampf(a.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    return (ua << 24) | (ur << 16) | (ug << 8) | ub;
}

fn make_c(c: u32, gamma: f32) -> u32 {
    let r = (((c >> 16) & 255) as f32) / 255.0;
    let g = (((c >> 8) & 255) as f32) / 255.0;
    let b = ((c & 255) as f32) / 255.0;
    let a = ((c >> 24) as f32) / 255.0;
    let ur = (clampf(r.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ug = (clampf(g.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ub = (clampf(b.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    let ua = (clampf(a.powf(gamma), 0.0, 1.0) * 255.0) as u32;
    return (ua << 24) | (ur << 16) | (ug << 8) | ub;
}

fn decode_pixels(dst: &mut [u32], src: &[u8], width: usize, height: usize, stride: usize, x0: usize, y0: usize, dx: usize, dy: usize, itype: u16, palette: &[u32; 256], gamma: f32) {
    let mut sp = 0;
    match itype {
        TYPE_L1 => {
            for y in 0..height {
                for x in 0..(width / 8) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..8 {
                        let l = if (d & (0x80 >> i)) != 0 {1.0} else {0.0};
                        dst[(y0 + y * dy) * stride + x0 + (x * 8 + i) * dx] = make_lf(l, gamma);
                    }
                }
                if (width & 7) != 0 {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..8 {
                        let l = if (d & (0x80 >> i)) != 0 {1.0} else {0.0};
                        dst[(y0 + y * dy) * stride + x0 + ((width & 0xFFFFFFF8) + i) * dx] = make_lf(l, gamma);
                    }
                }
            }
        },
        TYPE_C1 => {
            for y in 0..height {
                for x in 0..(width / 8) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..8 {
                        let c = if (d & (0x80 >> i)) != 0 {palette[1]} else {palette[0]};
                        dst[(y0 + y * dy) * stride + x0 + (x * 8 + i) * dx] = make_c(c, gamma);
                    }
                }
                if (width & 7) != 0 {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..(width & 7) {
                        let c = if (d & (0x80 >> i)) != 0 {palette[1]} else {palette[0]};
                        dst[(y0 + y * dy) * stride + x0 + ((width & 0xFFFFFFF8) + i) * dx] = make_c(c, gamma);
                    }
                }
            }
        },
        TYPE_L2 => {
            for y in 0..height {
                for x in 0..(width / 4) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..4 {
                        dst[(y0 + y * dy) * stride + x0 + (x * 4 + i) * dx] = make_lf(GRAY2[((d >> ((3 - i) * 2)) & 3) as usize], gamma);
                    }
                }
                if (width & 3) != 0 {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..(width & 3) {
                        dst[(y0 + y * dy) * stride + x0 + ((width & 0xFFFFFFFC) + i) * dx] = make_lf(GRAY2[((d >> ((3 - i) * 2)) & 3) as usize], gamma);
                    }
                }
            }
        },
        TYPE_C2 => {
            for y in 0..height {
                for x in 0..(width / 4) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..4 {
                        dst[(y0 + y * dy) * stride + x0 + (x * 4 + i) * dx] = make_c(palette[((d >> ((3 - i) * 2)) & 3) as usize], gamma);
                    }
                }
                if (width & 3) != 0 {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..(width & 3) {
                        dst[(y0 + y * dy) * stride + x0 + ((width & 0xFFFFFFFC) + i) * dx] = make_c(palette[((d >> ((3 - i) * 2)) & 3) as usize], gamma);
                    }
                }
            }
        },
        TYPE_L4 => {
            for y in 0..height {
                for x in 0..(width / 2) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..2 {
                        dst[(y0 + y * dy) * stride + x0 + (x * 2 + i) * dx] = make_lf(GRAY4[((d >> ((1 >> i) * 4)) & 15) as usize], gamma);
                    }
                }
                if (width & 1) != 0 {
                    dst[(y0 + y * dy) * stride + x0 + (width & 0xFFFFFFFE) * dx] = make_lf(GRAY4[(src[sp] >> 4) as usize], gamma);
                    sp += 1;
                }
            }
        },
        TYPE_C4 => {
            for y in 0..height {
                for x in 0..(width / 2) {
                    let d = src[sp];
                    sp += 1;
                    for i in 0..2 {
                        dst[(y0 + y * dy) * stride + x0 + (x * 2 + i) * dx] = make_c(palette[((d >> ((1 >> i) * 4)) & 15) as usize], gamma);
                    }
                }
                if (width & 1) != 0 {
                    dst[(y0 + y * dy) * stride + x0 + (width & 0xFFFFFFFE) * dx] = make_c(palette[(src[sp] >> 4) as usize], gamma);
                    sp += 1;
                }
            }
        },
        TYPE_L8 => {
            for y in 0..height {
                for x in 0..width {
                    let l = (src[sp] as f32) / 255.0;
                    sp += 1;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_lf(l, gamma);
                }
            }
        },
        TYPE_RGB8 => {
            for y in 0..height {
                for x in 0..width {
                    let r = (src[sp] as f32) / 255.0;
                    let g = (src[sp + 1] as f32) / 255.0;
                    let b = (src[sp + 2] as f32) / 255.0;
                    sp += 3;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(r, g, b, 1.0, gamma);
                }
            }
        },
        TYPE_C8 => {
            for y in 0..height {
                for x in 0..width {
                    let c = src[sp];
                    sp += 1;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_c(palette[c as usize], gamma);
                }
            }
        },
        TYPE_LA8 => {
            for y in 0..height {
                for x in 0..width {
                    let l = (src[sp] as f32) / 255.0;
                    let a = (src[sp + 1] as f32) / 255.0;
                    sp += 2;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(l, l, l, a, gamma);
                }
            }
        },
        TYPE_RGBA8 => {
            if gamma == 1.0 {
                for y in 0..height {
                    for x in 0..width {
                        let r = src[sp] as u32; // as f32) / 255.0;
                        let g = src[sp + 1]as u32; // as f32) / 255.0;
                        let b = src[sp + 2]as u32; // as f32) / 255.0;
                        let a = src[sp + 3]as u32; // as f32) / 255.0;
                        sp += 4;
                        dst[(y0 + y * dy) * stride + x0 + x * dx] = (a << 24) | (r<< 16) | (g << 8) | b;
                    }
                }
            }
            else {
                for y in 0..height {
                    for x in 0..width {
                        let r = (src[sp] as f32) / 255.0;
                        let g = (src[sp + 1] as f32) / 255.0;
                        let b = (src[sp + 2] as f32) / 255.0;
                        let a = (src[sp + 3] as f32) / 255.0;
                        sp += 4;
                        dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(r, g, b, a, gamma);
                    }
                }
            }
            
        },
        TYPE_L16 => {
            for y in 0..height {
                for x in 0..width {
                    let l = (src[sp] as f32) / 255.0;
                    sp += 2;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_lf(l, gamma);
                }
            }
        },
        TYPE_RGB16 => {
            for y in 0..height {
                for x in 0..width {
                    let r = (src[sp] as f32) / 255.0;
                    let g = (src[sp + 2] as f32) / 255.0;
                    let b = (src[sp + 4] as f32) / 255.0;
                    sp += 6;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(r, g, b, 1.0, gamma);
                }
            }
        },
        TYPE_LA16 => {
            for y in 0..height {
                for x in 0..width {
                    let l = (src[sp] as f32) / 255.0;
                    let a = (src[sp + 2] as f32) / 255.0;
                    sp += 4;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(l, l, l, a, gamma);
                }
            }
        },
        TYPE_RGBA16 => {
            for y in 0..height {
                for x in 0..width {
                    let r = (src[sp] as f32) / 255.0;
                    let g = (src[sp + 2] as f32) / 255.0;
                    let b = (src[sp + 4] as f32) / 255.0;
                    let a = (src[sp + 6] as f32) / 255.0;
                    sp += 8;
                    dst[(y0 + y * dy) * stride + x0 + x * dx] = make_rgbaf(r, g, b, a, gamma);
                }
            }
        },
        _ => {
        },
    }
}

fn from_be16(src: &[u8]) -> u16 {
    ((src[0] as u16) << 8) | (src[1] as u16)
}

fn from_be32(src: &[u8]) -> u32 {
    ((src[0] as u32) << 24) | ((src[1] as u32) << 16) | ((src[2] as u32) << 8) | (src[3] as u32)
}

pub fn test(src: &[u8]) -> Option<(usize, usize)> {
    if (src[0] == 0x89) && (src[1] == 0x50) && (src[2] == 0x4E) && (src[3] == 0x47) && (src[4] == 0x0D) && (src[5] == 0x0A) && (src[6] == 0x1A) && (src[7] == 0x0A) {
        let mut sp: usize = 8;
        while sp < src.len() {
            let chunk_length = from_be32(&src[sp..sp + 4]) as usize;
            sp += 4;
            let chunk_type = from_be32(&src[sp..sp + 4]);
            sp += 4;
            if chunk_type == 0x49484452 { // IHDR
                let width = from_be32(&src[sp..sp + 4]) as usize;
                sp += 4;
                let height = from_be32(&src[sp..sp + 4]) as usize;
                sp += 4;
                let t = from_be16(&src[sp..sp + 2]);
                sp += 5;
                if (t == TYPE_L1) | (t == TYPE_L2) | (t == TYPE_L4) | (t == TYPE_L8) | (t == TYPE_L16) |
                (t == TYPE_LA8) | (t == TYPE_LA16) |
                (t == TYPE_C1) | (t == TYPE_C2) | (t == TYPE_C4) | (t == TYPE_C8) |
                (t == TYPE_RGB8) | (t == TYPE_RGB16) |
                (t == TYPE_RGBA8) | (t == TYPE_RGBA16) {
                    return Some((width, height));
                }
            }
            else if chunk_type == 0x49454E44 { // IEND
                break;
            }
            else {
                sp += chunk_length;
            }
            sp += 4;
        }
    }
    None
}

pub fn decode(src: &[u8]) -> Result<ImageBuffer, String> {
    if (src[0] != 0x89) ||
    (src[1] != 0x50) ||
    (src[2] != 0x4E) ||
    (src[3] != 0x47) ||
    (src[4] != 0x0D) ||
    (src[5] != 0x0A) ||
    (src[6] != 0x1A) ||
    (src[7] != 0x0A) {
        return Err("invalid PNG".to_string());
    }
    let mut sp: usize = 8;
    let mut width: usize = 0;
    let mut height: usize = 0;
    let mut itype: u16 = 0;
    #[allow(unused_assignments)]
    let mut compression: u8 = 0;
    #[allow(unused_assignments)]
    let mut filter: u8 = 0;
    let mut interlace: u8 = 0;
    let mut stride: usize = 0;
    let mut bpp: usize = 0;
    let mut need_plte = false;
    let mut plte_present = false;
    let mut zipped_data: Vec<u8> = vec![0; src.len()];
    let mut dp: usize = 0;
    let mut idat_found = false;
    let mut iend_found = false;
    let mut palette: [u32; 256] = [0; 256];
    let mut _background: u32 = 0xFF000000;
    let mut gamma: f32 = 1.0;
    while sp < src.len() {
        let chunk_length = from_be32(&src[sp..sp + 4]) as usize;
        sp += 4;
        let chunk_type = from_be32(&src[sp..sp + 4]);
        sp += 4;
        match chunk_type {
            0x49484452 => { // IHDR
                width = from_be32(&src[sp..]) as usize;
                height = from_be32(&src[sp + 4..]) as usize;
                itype = from_be16(&src[sp + 8..]);
                compression = src[sp + 10];
                filter = src[sp + 11];
                interlace = src[sp + 12];
                if (width >= 65536) ||
                (height >= 65536) ||
                (compression != 0) ||
                (filter != 0) ||
                (interlace > 1) {
                    return Err("Invalid PNG".to_string());
                }
                match itype {
                    TYPE_L1 => {stride = (width + 7) / 8; bpp = 1;},
                    TYPE_C1 => {stride = (width + 7) / 8; bpp = 1; need_plte = true;},
                    TYPE_L2 => {stride = (width + 3) / 4; bpp = 1;},
                    TYPE_C2 => {stride = (width + 3) / 4; bpp = 1; need_plte = true;},
                    TYPE_L4 => {stride = (width + 1) / 2; bpp = 1;},
                    TYPE_C4 => {stride = (width + 1) / 2; bpp = 1; need_plte = true;},
                    TYPE_L8 => {stride = width; bpp = 1;},
                    TYPE_RGB8 => {stride = width * 3; bpp = 3;},
                    TYPE_C8 => {stride = width; bpp = 1; need_plte = true;},
                    TYPE_LA8 => {stride = width * 2; bpp = 2;},
                    TYPE_RGBA8 => {stride = width * 4; bpp = 4;},
                    TYPE_L16 => {stride = width * 2; bpp = 2;},
                    TYPE_RGB16 => {stride = width * 6; bpp = 6;},
                    TYPE_LA16 => {stride = width * 2; bpp = 4;},
                    TYPE_RGBA16 => {stride = width * 4; bpp = 8;},
                    _ => {return Err("Invalid PNG".to_string());}
                }
                sp += chunk_length;
            },
            0x49444154 => { // IDAT
                zipped_data[dp..dp + chunk_length].copy_from_slice(&src[sp..sp + chunk_length]);
                sp += chunk_length;
                dp += chunk_length;
                idat_found = true;
            },
            0x49454E44 => { // IEND
                iend_found = true;
                break;
            },
            0x504C5445 => { // PLTE
                plte_present = true;
                if chunk_length > 768 {
                    return Err("Invalid PNG".to_string());
                }
                for i in 0..(chunk_length / 3) {
                    let r = src[sp];
                    let g = src[sp + 1];
                    let b = src[sp + 2];
                    sp += 3;
                    palette[i] = 0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                }
            },
            0x624B4744 => { // bKGD
                if (itype == TYPE_C1) || (itype == TYPE_C2) || (itype == TYPE_C4) || (itype == TYPE_C8) {
                    _background = palette[src[sp] as usize];
                }
                else if (itype == TYPE_L1) || (itype == TYPE_L2) || (itype == TYPE_L4) || (itype == TYPE_L8) || (itype == TYPE_LA8) || (itype == TYPE_L16) || (itype == TYPE_LA16) {
                    let level = src[sp];
                    _background = 0xFF000000 | ((level as u32) << 16) | ((level as u32) << 8) | (level as u32);
                }
                else {
                    let r = src[sp];
                    let g = src[sp + 2];
                    let b = src[sp + 4];
                    _background = 0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                }
                sp += chunk_length;
            },
            0x6348524D => { // cHRM
                //println!("cHRM {}",chunk_length);
                // chromaticity coordinates of display
                sp += chunk_length;
            },
            // dSIG (digital signature)
            0x65584966 => { // eXIf
                //println!("eXIf {}",chunk_length);
                // EXIF metadata
                sp += chunk_length;
            },
            0x67414D41 => { // gAMA
                let level = ((src[sp] as u32) << 24) | ((src[sp + 1] as u32) << 16) | ((src[sp + 2] as u32) << 8) | (src[sp + 3] as u32);
                gamma = (level as f32) / 100000.0;
                sp += chunk_length;
            },
            0x68495354 => { // hIST
                //println!("hIST {}",chunk_length);
                // histogram
                sp += chunk_length;
            },
            // iCCP (ICC color profile)
            0x69545874 => { // iTXt
                //println!("iTXt {}",chunk_length);
                // UTF-8 text
                sp += chunk_length;
            },
            0x70485973 => { // pHYs
                //println!("pHYs {}",chunk_length);
                // pixel aspect ratio
                sp += chunk_length;
            },
            0x73424954 => { // sBIT
                //println!("sBIT {}",chunk_length);
                // color accuracy
                sp += chunk_length;
            },
            0x73504C54 => { // sPLT
                //println!("sPLT {}",chunk_length);
                // palette in case colors are not available
                sp += chunk_length;
            },
            // sRGB (sRGB colorspace)
            // sTER (stereo)
            0x74455874 => { // tEXt
                //println!("tEXt {}",chunk_length);
                // text in ISO/IEC 8859-1
                sp += chunk_length;
            },
            0x74494D45 => { // tIME
                //println!("tIME {}",chunk_length);
                // time of last change to image
                sp += chunk_length;
            },
            0x74524E53 => { // tRNS
                //println!("tRNS {}",chunk_length);
                // transparency information
                sp += chunk_length;
            }
            0x7A545874 => { // zTXt
                //println!("zTXt {}",chunk_length);
                // compressed text
                sp += chunk_length;
            },
            _ => { // anything else just ignore
                //println!("unknown chunk: {:02X} {:02X} {:02X} {:02X}",chunk_type >> 24,(chunk_type >> 16) & 255,(chunk_type >> 8) & 255,chunk_type & 255);
                sp += chunk_length;
            },
        }
        sp += 4; // also skip the CRC
    }
    
    // sanity check the palette
    if need_plte && !plte_present {
        return Err("Invalid PNG".to_string());
    }
    
    // sanity check the data
    if !idat_found || !iend_found {
        return Err("Invalid PNG".to_string());
    }
    
    if interlace == 1 {
        let ax0: [usize; 7] = [0, 4, 0, 2, 0, 1, 0];
        let ay0: [usize; 7] = [0, 0, 4, 0, 2, 0, 1];
        let adx: [usize; 7] = [8, 8, 4, 4, 2, 2, 1];
        let ady: [usize; 7] = [8, 8, 8, 4, 4, 2, 2];
        let mut awidth: [usize; 7] = [0; 7];
        let mut aheight: [usize; 7] = [0; 7];
        let mut astride: [usize; 7] = [0; 7];
        let mut apresent: [bool; 7] = [false; 7];
        let mut adsize: [usize; 7] = [0; 7];
        let mut total_dsize = 0;
        //println!("size: {}x{}",width,height);
        for i in 0..7 {
            awidth[i] = (width + adx[i] - ax0[i] - 1) / adx[i];
            aheight[i] = (height + ady[i] - ay0[i] - 1) / ady[i];
            astride[i] = match itype {
                TYPE_L1 => {(awidth[i] + 7) / 8},
                TYPE_C1 => {(awidth[i] + 7) / 8},
                TYPE_L2 => {(awidth[i] + 3) / 4},
                TYPE_C2 => {(awidth[i] + 3) / 4},
                TYPE_L4 => {(awidth[i] + 1) / 2},
                TYPE_C4 => {(awidth[i] + 1) / 2},
                TYPE_L8 => {awidth[i]},
                TYPE_RGB8 => {awidth[i] * 3},
                TYPE_C8 => {awidth[i]},
                TYPE_LA8 => {awidth[i] * 2},
                TYPE_RGBA8 => {awidth[i] * 4},
                TYPE_L16 => {awidth[i] * 2},
                TYPE_RGB16 => {awidth[i] * 6},
                TYPE_LA16 => {awidth[i] * 4},
                TYPE_RGBA16 => {awidth[i] * 8},
                _ => {0},
            };
            apresent[i] = (awidth[i] != 0) && (aheight[i] != 0);
            adsize[i] = if apresent[i] {(astride[i] + 1) * aheight[i]} else {0};
            total_dsize += adsize[i];
            //println!("{}: size {}x{}, offset {},{}, step {},{}",i,awidth[i],aheight[i],ax0[i],ay0[i],adx[i],ady[i]);
        }
        let filtered_data = match inflate(&zipped_data, total_dsize) {
            Ok(data) => {data},
            Err(msg) => {return Err(msg);},
        };
        let mut sp = 0;
        let mut result = ImageBuffer::new(width, height);
        
        for i in 0..7 {
            if apresent[i] {
                let raw_data = unfilter(&filtered_data[sp..sp + adsize[i]], aheight[i], astride[i], bpp);
                decode_pixels(&mut result.data, &raw_data, awidth[i], aheight[i], width, ax0[i], ay0[i], adx[i], ady[i], itype as u16, &palette, gamma);
                sp += adsize[i];
            }
        }
        Ok(result)
    } else
    {
        //let after0 = Instant::now();
        
        let filtered_data = match inflate(&zipped_data, (stride + 1) * height) {
            Ok(data) => {data},
            Err(msg) => {return Err(msg);},
        };
        
        //let after_inflate = Instant::now();
        
        let raw_data =  unfilter(&filtered_data, height, stride, bpp);
        
        //let after_unfilter = Instant::now();
        
        let mut result = ImageBuffer::new(width, height);
        decode_pixels(&mut result.data, &raw_data, width, height, width, 0, 0, 1, 1, itype as u16, &palette, gamma);
        
        //let after_decode = Instant::now();
        
        //let total_duration = after_decode.duration_since(after0);
        //let inflate_duration = after_inflate.duration_since(after0);
        //let inflate_percentage = (100.0 * inflate_duration.as_secs_f32()) / (total_duration.as_secs_f32());
        //let unfilter_duration = after_unfilter.duration_since(after_inflate);
        //let unfilter_percentage = (100.0 * unfilter_duration.as_secs_f32()) / (total_duration.as_secs_f32());
        //let decode_duration = after_decode.duration_since(after_unfilter);
        //let decode_percentage = (100.0 * decode_duration.as_secs_f32()) / (total_duration.as_secs_f32());
        
        //println!("inflate: {} us ({}%)",inflate_duration.as_micros(),inflate_percentage);
        //println!("unfilter: {} us ({}%)",unfilter_duration.as_micros(),unfilter_percentage);
        //println!("decode: {} us ({}%)",decode_duration.as_micros(),decode_percentage);
        //println!("------------------");
        //println!("total: {} us (100.0%)",total_duration.as_micros());
        
        Ok(result)
    }
}

pub fn encode(_image: &ImageBuffer) -> Result<Vec<u8>, String> {
    Err("not implemented".to_string())
}