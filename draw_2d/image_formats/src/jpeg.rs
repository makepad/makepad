// image_formats::jpeg
// by Desmond Germans, 2019

use crate::ImageBuffer;

const TYPE_Y: u16 = 0x000C;
const TYPE_YUV420: u16 = 0x3900;
const TYPE_YUV422: u16 = 0x0390;
const TYPE_YUV440: u16 = 0x1390;
const TYPE_YUV444: u16 = 0x00E4;
const TYPE_RGB444: u16 = 0x01E4;

const FOLDING: [u8; 64] = [56u8, 57, 8, 40, 9, 58, 59, 10, 41, 0, 48, 1, 42, 11, 60, 61, 12, 43, 2, 49, 16, 32, 17, 50, 3, 44, 13, 62, 63, 14, 45, 4, 51, 18, 33, 24, 25, 34, 19, 52, 5, 46, 15, 47, 6, 53, 20, 35, 26, 27, 36, 21, 54, 7, 55, 22, 37, 28, 29, 38, 23, 39, 30, 31,];
const FC0: f32 = 1.0;
const FC1: f32 = 0.98078528;
const FC2: f32 = 0.92387953;
const FC3: f32 = 0.83146961;
const FC4: f32 = 0.70710678;
const FC5: f32 = 0.55557023;
const FC6: f32 = 0.38268343;
const FC7: f32 = 0.19509032;

const FIX: u8 = 8;
const ONE: f32 = (1 << FIX) as f32;
const C0: i32 = (FC0 * ONE) as i32;
const C1: i32 = (FC1 * ONE) as i32;
const C2: i32 = (FC2 * ONE) as i32;
const C3: i32 = (FC3 * ONE) as i32;
const C4: i32 = (FC4 * ONE) as i32;
const C5: i32 = (FC5 * ONE) as i32;
const C6: i32 = (FC6 * ONE) as i32;
const C7: i32 = (FC7 * ONE) as i32;

const C7PC1: i32 = C7 + C1;
const C5PC3: i32 = C5 + C3;
const C7MC1: i32 = C7 - C1;
const C5MC3: i32 = C5 - C3;
const C0S: i32 = C0 >> 1;
const C6PC2: i32 = C6 + C2;
const C6MC2: i32 = C6 - C2;

fn from_le16(src: &[u8]) -> u16 {
    ((src[1] as u16) << 8) | (src[0] as u16)
}

fn from_le32(src: &[u8]) -> u32 {
    ((src[3] as u32) << 24) | ((src[2] as u32) << 16) | ((src[1] as u32) << 8) | (src[0] as u32)
}

fn from_be16(src: &[u8]) -> u16 {
    ((src[0] as u16) << 8) | (src[1] as u16)
}

fn from_be32(src: &[u8]) -> u32 {
    ((src[0] as u32) << 24) | ((src[1] as u32) << 16) | ((src[2] as u32) << 8) | (src[3] as u32)
}

fn make_coeff(cat: u8, code: isize) -> i32 {
    let mcat = cat - 1;
    let hmcat = 1 << mcat;
    let base = code & (hmcat - 1);
    if (code & hmcat) != 0 {
        (base + hmcat) as i32
    }
    else {
        (base + 1 - (1 << cat)) as i32
    }
}

#[derive(Copy, Clone)]
struct Table {
    prefix: [u16; 65536],
}

impl Table {
    pub fn new_empty() -> Table {
        Table {
            prefix: [0u16; 65536],
        }
    }
    
    pub fn new(bits: [u8; 16], huffval: [u8; 256]) -> Table {
        let mut prefix = [0u16; 65536];
        let mut dp = 0;
        let mut count = 0;
        for i in 1..17 {
            //println!("{}-bit codes:",i);
            for _k in 0..bits[i - 1] {
                //println!("    code {}: {}",count,huffval[count]);
                let runcat = huffval[count] as u16;
                for _l in 0..(65536 >> i) {
                    prefix[dp] = (runcat << 8) | (i as u16);
                    dp += 1;
                }
                count += 1;
            }
        }
        Table {
            prefix: prefix,
        }
    }
}

fn jpeg_get8(block: &[u8], rp: &mut usize) -> u8 {
    let mut b = block[*rp];
    //println!("[{:02X}]",b);
    *rp += 1;
    if b == 0xFF {
        b = block[*rp];
        //println!("[{:02X}]",b);
        *rp += 1;
        if b == 0 {
            return 0xFF;
        }
    }
    b
}

struct Reader<'a> {
    block: &'a [u8],
    rp: usize,
    bit: u32,
    cache: u32,
}

impl<'a> Reader<'a> {
    pub fn new(block: &'a [u8]) -> Reader<'a> {
        let mut rp = 0usize;
        let mut bit = 0u32;
        let mut cache = 0u32;
        while bit <= 24 {
            let b = jpeg_get8(block, &mut rp);
            cache |= (b as u32) << (24 - bit);
            bit += 8;
        }
        Reader {
            block: block,
            rp: rp,
            bit: bit,
            cache: cache,
        }
    }
    
    fn restock(&mut self) {
        while self.bit <= 24 {
            let b = if self.rp >= self.block.len() {0} else {jpeg_get8(self.block, &mut self.rp)}; // fill with 0 if at end of block
            self.cache |= (b as u32) << (24 - self.bit);
            self.bit += 8;
        }
    }
    
    pub fn peek(&self, n: usize) -> u32 {
        self.cache >> (32 - n)
    }
    
    pub fn skip(&mut self, n: usize) {
        self.cache <<= n;
        self.bit -= n as u32;
        self.restock();
    }
    
    pub fn get1(&mut self) -> bool {
        let result = self.peek(1) == 1;
        //println!(" 1 bit : {}",if result { 1 } else { 0 });
        self.skip(1);
        result
    }
    
    pub fn getn(&mut self, n: usize) -> u32 {
        let result = self.peek(n);
        //println!("{} bits: ({:0b}) {}",n,self.cache >> (32 - n),result);
        self.skip(n);
        result
    }
    
    pub fn get_code(&mut self, table: &Table) -> u8 {
        let index = self.cache >> 16;
        let d = table.prefix[index as usize];
        let symbol = (d >> 8) & 255;
        let n = d & 255;
        //println!("{} bits: ({:0b}) runcat {:02X}",n,index >> (16 - n),symbol);
        self.skip(n as usize);
        symbol as u8
    }
    
    pub fn enter(&mut self, rp: usize) {
        self.rp = rp;
        self.cache = 0;
        self.bit = 0;
        self.restock();
    }
    
    pub fn leave(&mut self) -> usize {
        //println!("leave: bit = {}, rp = {}",self.bit,self.rp);
        // superspecial case: no JPEG data read at all (only during initial programming)
        if (self.bit == 32) && (self.rp == 4) {
            return 0;
        }
        /*// first search FFD9 to elimiate stray FFs past end of buffer
		if (self.block[self.rp - 5] == 0xFF) && (self.block[self.rp - 4] == 0xD9) {
			return self.rp - 5;
		}
		if (self.block[self.rp - 4] == 0xFF) && (self.block[self.rp - 3] == 0xD9) {
			return self.rp - 4;
		}
		if (self.block[self.rp - 3] == 0xFF) && (self.block[self.rp - 2] == 0xD9) {
			return self.rp - 3;
		}
		if (self.block[self.rp - 2] == 0xFF) && (self.block[self.rp - 1] == 0xD9) {
			return self.rp - 2;
		}
		if (self.block[self.rp - 1] == 0xFF) && (self.block[self.rp] == 0xD9) {
			return self.rp - 1;
		}*/
        // anything else
        for _i in 0..((self.bit + 7) / 8) - 2 {
            if (self.block[self.rp - 1] == 0x00) && (self.block[self.rp - 2] == 0xFF) {
                self.rp -= 1;
            }
            self.rp -= 1;
        }
        //println!("and leaving with rp = {} ({:02X} {:02X})",self.rp,self.block[self.rp],self.block[self.rp + 1]);
        self.rp
    }
}

fn unpack_sequential(reader: &mut Reader, coeffs: &mut [i32], dcht: &Table, acht: &Table, dc: &mut i32) {
    let cat = reader.get_code(dcht);
    if cat > 0 {
        let code = reader.getn(cat as usize);
        *dc += make_coeff(cat, code as isize) as i32;
    }
    coeffs[FOLDING[0] as usize] = *dc;
    //println!("DC {}",*dc);
    let mut i = 1;
    while i < 64 {
        let runcat = reader.get_code(acht);
        let run = runcat >> 4;
        let cat = runcat & 15;
        if cat > 0 {
            let code = reader.getn(cat as usize);
            let coeff = make_coeff(cat, code as isize) as i32;
            i += run;
            coeffs[FOLDING[i as usize] as usize] = coeff;
            //println!("coeffs[{}] = {}",i,coeff);
        }
        else {
            if run == 15 { // ZRL
                i += 15;
                //println!("ZRL");
            }
            else { // EOB
                //println!("EOB");
                break;
            }
        }
        i += 1;
    }
}

fn unpack_progressive_start_dc(reader: &mut Reader, coeffs: &mut[i32], dcht: &Table, dc: &mut i32, shift: u8) {
    let cat = reader.get_code(dcht);
    if cat > 0 {
        let code = reader.getn(cat as usize);
        *dc += make_coeff(cat, code as isize) as i32;
    }
    //println!("DC = {}",*dc << shift);
    coeffs[FOLDING[0] as usize] = *dc << shift;
}

fn unpack_progressive_start_ac(reader: &mut Reader, coeffs: &mut[i32], acht: &Table, start: u8, end: u8, shift: u8, eobrun: &mut usize) {
    if *eobrun != 0 {
        *eobrun -= 1;
    }
    else {
        let mut i = start;
        while i <= end {
            let runcat = reader.get_code(acht);
            let run = runcat >> 4;
            let cat = runcat & 15;
            if cat != 0 {
                let code = reader.getn(cat as usize);
                let coeff = make_coeff(cat, code as isize);
                i += run;
                coeffs[FOLDING[i as usize] as usize] = (coeff << shift) as i32;
            }
            else {
                if run == 15 {
                    i += 15;
                }
                else {
                    *eobrun = 1 << run;
                    if run != 0 {
                        *eobrun += reader.getn(run as usize) as usize;
                    }
                    *eobrun -= 1;
                    break;
                }
            }
            i += 1;
        }
    }
}

fn unpack_progressive_refine_dc(reader: &mut Reader, coeffs: &mut[i32], shift: u8) {
    if reader.get1() {
        coeffs[FOLDING[0] as usize] |= 1 << shift;
    }
}

fn update_nonzeros(reader: &mut Reader, coeffs: &mut[i32], start: u8, end: u8, shift: u8, count: u8) -> u8 {
    let mut i = start;
    let mut k = count;
    while i <= end {
        if coeffs[FOLDING[i as usize] as usize] != 0 {
            if reader.get1() {
                if coeffs[FOLDING[i as usize] as usize] > 0 {
                    coeffs[FOLDING[i as usize] as usize] += 1 << shift;
                }
                else {
                    coeffs[FOLDING[i as usize] as usize] -= 1 << shift;
                }
            }
        }
        else {
            if k == 0 {
                return i;
            }
            k -= 1;
        }
        i += 1;
    }
    i
}

fn unpack_progressive_refine_ac(reader: &mut Reader, coeffs: &mut[i32], acht: &Table, start: u8, end: u8, shift: u8, eobrun: &mut usize) {
    if *eobrun != 0 {
        update_nonzeros(reader, &mut coeffs[0..64], start, end, shift, 64);
        *eobrun -= 1;
    }
    else {
        let mut i = start;
        while i <= end {
            let runcat = reader.get_code(acht);
            let run = runcat >> 4;
            let cat = runcat & 15;
            if cat != 0 {
                let sb = reader.get1();
                i = update_nonzeros(reader, &mut coeffs[0..64], i, end, shift, run);
                if sb {
                    coeffs[FOLDING[i as usize] as usize] = 1 << shift;
                }
                else {
                    coeffs[FOLDING[i as usize] as usize] = 11 << shift;
                }
            }
            else {
                if run == 15 {
                    i = update_nonzeros(reader, &mut coeffs[0..64], i, end, shift, 15);
                }
                else {
                    *eobrun = 1 << run;
                    if run != 0 {
                        *eobrun += reader.getn(run as usize) as usize;
                    }
                    *eobrun -= 1;
                    update_nonzeros(reader, &mut coeffs[0..64], i, end, shift, 64);
                    break;
                }
            }
        }
    }
}

fn unpack_block(reader: &mut Reader, coeffs: &mut [i32], dcht: &Table, acht: &Table, dc: &mut i32, start: u8, end: u8, shift: u8, refine: bool, eobrun: &mut usize) {
    if refine {
        if start == 0 {
            unpack_progressive_refine_dc(reader, &mut coeffs[0..64], shift);
        }
        else {
            unpack_progressive_refine_ac(reader, &mut coeffs[0..64], &acht, start, end, shift, eobrun);
        }
    }
    else {
        if start == 0 {
            if (end == 63) && (shift == 0) {
                unpack_sequential(reader, &mut coeffs[0..64], &dcht, &acht, dc);
            }
            else {
                unpack_progressive_start_dc(reader, &mut coeffs[0..64], &dcht, dc, shift);
            }
        }
        else {
            unpack_progressive_start_ac(reader, &mut coeffs[0..64], &acht, start, end, shift, eobrun);
        }
    }
}

fn unpack_macroblock(
    reader: &mut Reader,
    coeffs: &mut [i32],
    dcht: &[Table],
    acht: &[Table],
    dt: &[usize],
    at: &[usize],
    dc: &mut [i32],
    start: u8,
    end: u8,
    shift: u8,
    refine: bool,
    eobrun: &mut usize,
    itype: u16,
    rescnt: &mut usize,
    resint: usize,
    mask: u8
) {
    match itype {
        TYPE_Y => {
            if (mask & 1) != 0 {
                unpack_block(reader, &mut coeffs[0..64], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
            }
        },
        TYPE_YUV420 => {
            if (mask & 1) != 0 {
                unpack_block(reader, &mut coeffs[0..64], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
                unpack_block(reader, &mut coeffs[64..128], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
                unpack_block(reader, &mut coeffs[128..192], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
                unpack_block(reader, &mut coeffs[192..256], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
            }
            if (mask & 2) != 0 {
                unpack_block(reader, &mut coeffs[256..320], &dcht[dt[1]], &acht[at[1]], &mut dc[1], start, end, shift, refine, eobrun);
            }
            if (mask & 4) != 0 {
                unpack_block(reader, &mut coeffs[320..384], &dcht[dt[2]], &acht[at[2]], &mut dc[2], start, end, shift, refine, eobrun);
            }
        },
        TYPE_YUV422 | TYPE_YUV440 => {
            if (mask & 1) != 0 {
                unpack_block(reader, &mut coeffs[0..64], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
                unpack_block(reader, &mut coeffs[64..128], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
            }
            if (mask & 2) != 0 {
                unpack_block(reader, &mut coeffs[128..192], &dcht[dt[1]], &acht[at[1]], &mut dc[1], start, end, shift, refine, eobrun);
            }
            if (mask & 4) != 0 {
                unpack_block(reader, &mut coeffs[192..256], &dcht[dt[2]], &acht[at[2]], &mut dc[2], start, end, shift, refine, eobrun);
            }
        },
        TYPE_YUV444 | TYPE_RGB444 => {
            if (mask & 1) != 0 {
                unpack_block(reader, &mut coeffs[0..64], &dcht[dt[0]], &acht[at[0]], &mut dc[0], start, end, shift, refine, eobrun);
            }
            if (mask & 2) != 0 {
                unpack_block(reader, &mut coeffs[64..128], &dcht[dt[1]], &acht[at[1]], &mut dc[1], start, end, shift, refine, eobrun);
            }
            if (mask & 4) != 0 {
                unpack_block(reader, &mut coeffs[128..192], &dcht[dt[2]], &acht[at[2]], &mut dc[2], start, end, shift, refine, eobrun);
            }
        },
        _ => {},
    }
    if resint != 0 {
        *rescnt -= 1;
        if *rescnt == 0 {
            let mut tsp = reader.leave();
            if (reader.block[tsp] == 0xFF) && ((reader.block[tsp + 1] >= 0xD0) && (reader.block[tsp + 1] < 0xD8)) {
                tsp += 2;
                *rescnt = resint;
                dc[0] = 0;
                dc[1] = 0;
                dc[2] = 0;
            }
            reader.enter(tsp);
        }
    }
}

fn partial_idct(out: &mut [i32], inp: &[i32]) {
    
    for i in 0..8 {
        let x3 = inp[i];
        let x1 = inp[i + 8];
        let x5 = inp[i + 16];
        let x7 = inp[i + 24];
        let x6 = inp[i + 32];
        let x2 = inp[i + 40];
        let x4 = inp[i + 48];
        let x0 = inp[i + 56];
        
        let q17 = C1 * (x1 + x7);
        let q35 = C3 * (x3 + x5);
        let r3 = C7PC1 * x1 - q17;
        let d3 = C5PC3 * x3 - q35;
        let r0 = C7MC1 * x7 + q17;
        let d0 = C5MC3 * x5 + q35;
        let b0 = r0 + d0;
        let d2 = r3 + d3;
        let d1 = r0 - d0;
        let b3 = r3 - d3;
        let b1 = C4 * ((d1 + d2) >> FIX);
        let b2 = C4 * ((d1 - d2) >> FIX);
        let q26 = C2 * (x2 + x6);
        let p04 = C4 * (x0 + x4) + C0S;
        let n04 = C4 * (x0 - x4) + C0S;
        let p26 = C6MC2 * x6 + q26;
        let n62 = C6PC2 * x2 - q26;
        let a0 = p04 + p26;
        let a1 = n04 + n62;
        let a3 = p04 - p26;
        let a2 = n04 - n62;
        let y0 = (a0 + b0) >> (FIX + 1);
        let y1 = (a1 + b1) >> (FIX + 1);
        let y3 = (a3 + b3) >> (FIX + 1);
        let y2 = (a2 + b2) >> (FIX + 1);
        let y7 = (a0 - b0) >> (FIX + 1);
        let y6 = (a1 - b1) >> (FIX + 1);
        let y4 = (a3 - b3) >> (FIX + 1);
        let y5 = (a2 - b2) >> (FIX + 1);
        
        out[i] = y0;
        out[i + 8] = y1;
        out[i + 16] = y3;
        out[i + 24] = y2;
        out[i + 32] = y7;
        out[i + 40] = y6;
        out[i + 48] = y4;
        out[i + 56] = y5;
    }
}


//fn m32x4s(a: bool) -> Mask::<i32, 4> {Mask::<i32, 4>::from_array([a; 4])}

// this is not faster :) I think LLVM autovectorises the top version
/*
fn i32x4v(a: i32, b: i32, c: i32, d: i32) -> i32x4 {i32x4::from_array([a, b, c, d])}
fn i32x4s(a: i32) -> i32x4 {i32x4::from_array([a; 4])}

fn _partial_idct_simd(out: &mut [i32], inp: &[i32]) {
    for i in 0..2 {
        let i = i * 4;
        let x3 = i32x4v(inp[i + 0 + 0], inp[i + 1 + 0], inp[i + 2 + 0], inp[i + 3 + 0]);
        let x1 = i32x4v(inp[i + 0 + 8], inp[i + 1 + 8], inp[i + 2 + 8], inp[i + 3 + 8]);
        let x5 = i32x4v(inp[i + 0 + 16], inp[i + 1 + 16], inp[i + 2 + 16], inp[i + 3 + 16]);
        let x7 = i32x4v(inp[i + 0 + 24], inp[i + 1 + 24], inp[i + 2 + 24], inp[i + 3 + 24]);
        let x6 = i32x4v(inp[i + 0 + 32], inp[i + 1 + 32], inp[i + 2 + 32], inp[i + 3 + 32]);
        let x2 = i32x4v(inp[i + 0 + 40], inp[i + 1 + 40], inp[i + 2 + 40], inp[i + 3 + 40]);
        let x4 = i32x4v(inp[i + 0 + 48], inp[i + 1 + 48], inp[i + 2 + 48], inp[i + 3 + 48]);
        let x0 = i32x4v(inp[i + 0 + 56], inp[i + 1 + 56], inp[i + 2 + 56], inp[i + 3 + 56]);
        
        let q17 = i32x4s(C1) * (x1 + x7);
        let q35 = i32x4s(C3) * (x3 + x5);
        let r3 = i32x4s(C7PC1) * x1 - q17;
        let d3 = i32x4s(C5PC3) * x3 - q35;
        let r0 = i32x4s(C7MC1) * x7 + q17;
        let d0 = i32x4s(C5MC3) * x5 + q35;
        let b0 = r0 + d0;
        let d2 = r3 + d3;
        let d1 = r0 - d0;
        let b3 = r3 - d3;
        let b1 = i32x4s(C4) * ((d1 + d2) >> i32x4s(8));
        let b2 = i32x4s(C4) * ((d1 - d2) >> i32x4s(8));
        let q26 = i32x4s(C2) * (x2 + x6);
        let p04 = i32x4s(C4) * (x0 + x4) + i32x4s(C0S);
        let n04 = i32x4s(C4) * (x0 - x4) + i32x4s(C0S);
        let p26 = i32x4s(C6MC2) * x6 + q26;
        let n62 = i32x4s(C6PC2) * x2 - q26;
        let a0 = p04 + p26;
        let a1 = n04 + n62;
        let a3 = p04 - p26;
        let a2 = n04 - n62;
        let y0 = (a0 + b0) >> i32x4s(9);
        let y1 = (a1 + b1) >> i32x4s(9);
        let y3 = (a3 + b3) >> i32x4s(9);
        let y2 = (a2 + b2) >> i32x4s(9);
        let y7 = (a0 - b0) >> i32x4s(9);
        let y6 = (a1 - b1) >> i32x4s(9);
        let y4 = (a3 - b3) >> i32x4s(9);
        let y5 = (a2 - b2) >> i32x4s(9);
        
        for l in 0..4 {
            out[i + l + 0] = y0[l];
            out[i + l + 8] = y1[l];
            out[i + l + 16] = y3[l];
            out[i + l + 24] = y2[l];
            out[i + l + 32] = y7[l];
            out[i + l + 40] = y6[l];
            out[i + l + 48] = y4[l];
            out[i + l + 56] = y5[l];
        }
    }
}*/

fn unswizzle_transpose_swizzle(out: &mut [i32], inp: &[i32]) {
    out[0] = inp[3];
    out[1] = inp[11];
    out[2] = inp[27];
    out[3] = inp[19];
    out[4] = inp[51];
    out[5] = inp[59];
    out[6] = inp[43];
    out[7] = inp[35];
    out[8] = inp[1];
    out[9] = inp[9];
    out[10] = inp[25];
    out[11] = inp[17];
    out[12] = inp[49];
    out[13] = inp[57];
    out[14] = inp[41];
    out[15] = inp[33];
    
    out[16] = inp[5];
    out[17] = inp[13];
    out[18] = inp[29];
    out[19] = inp[21];
    out[20] = inp[53];
    out[21] = inp[61];
    out[22] = inp[45];
    out[23] = inp[37];
    out[24] = inp[7];
    out[25] = inp[15];
    out[26] = inp[31];
    out[27] = inp[23];
    out[28] = inp[55];
    out[29] = inp[63];
    out[30] = inp[47];
    out[31] = inp[39];
    
    out[32] = inp[6];
    out[33] = inp[14];
    out[34] = inp[30];
    out[35] = inp[22];
    out[36] = inp[54];
    out[37] = inp[62];
    out[38] = inp[46];
    out[39] = inp[38];
    out[40] = inp[2];
    out[41] = inp[10];
    out[42] = inp[26];
    out[43] = inp[18];
    out[44] = inp[50];
    out[45] = inp[58];
    out[46] = inp[42];
    out[47] = inp[34];
    
    out[48] = inp[4];
    out[49] = inp[12];
    out[50] = inp[28];
    out[51] = inp[20];
    out[52] = inp[52];
    out[53] = inp[60];
    out[54] = inp[44];
    out[55] = inp[36];
    out[56] = inp[0];
    out[57] = inp[8];
    out[58] = inp[24];
    out[59] = inp[16];
    out[60] = inp[48];
    out[61] = inp[56];
    out[62] = inp[40];
    out[63] = inp[32];
}

fn unswizzle_transpose(out: &mut [i32], inp: &[i32]) {
    out[0] = inp[0];
    out[1] = inp[8];
    out[2] = inp[24];
    out[3] = inp[16];
    out[4] = inp[48];
    out[5] = inp[56];
    out[6] = inp[40];
    out[7] = inp[32];
    out[8] = inp[1];
    out[9] = inp[9];
    out[10] = inp[25];
    out[11] = inp[17];
    out[12] = inp[49];
    out[13] = inp[57];
    out[14] = inp[41];
    out[15] = inp[33];
    
    out[16] = inp[2];
    out[17] = inp[10];
    out[18] = inp[26];
    out[19] = inp[18];
    out[20] = inp[50];
    out[21] = inp[58];
    out[22] = inp[42];
    out[23] = inp[34];
    out[24] = inp[3];
    out[25] = inp[11];
    out[26] = inp[27];
    out[27] = inp[19];
    out[28] = inp[51];
    out[29] = inp[59];
    out[30] = inp[43];
    out[31] = inp[35];
    
    out[32] = inp[4];
    out[33] = inp[12];
    out[34] = inp[28];
    out[35] = inp[20];
    out[36] = inp[52];
    out[37] = inp[60];
    out[38] = inp[44];
    out[39] = inp[36];
    out[40] = inp[5];
    out[41] = inp[13];
    out[42] = inp[29];
    out[43] = inp[21];
    out[44] = inp[53];
    out[45] = inp[61];
    out[46] = inp[45];
    out[47] = inp[37];
    
    out[48] = inp[6];
    out[49] = inp[14];
    out[50] = inp[30];
    out[51] = inp[22];
    out[52] = inp[54];
    out[53] = inp[62];
    out[54] = inp[46];
    out[55] = inp[38];
    out[56] = inp[7];
    out[57] = inp[15];
    out[58] = inp[31];
    out[59] = inp[23];
    out[60] = inp[55];
    out[61] = inp[63];
    out[62] = inp[47];
    out[63] = inp[39];
}

fn convert_block(block: &mut [i32], qtable: &[i32]) {
    let mut temp0 = [0i32; 64];
    for i in 0..64 {
        temp0[i] = block[i] * qtable[i];
    }
    let mut temp1 = [0i32; 64];
    partial_idct(&mut temp1, &temp0);
    let mut temp2 = [0i32; 64];
    unswizzle_transpose_swizzle(&mut temp2, &temp1);
    let mut temp3 = [0i32; 64];
    partial_idct(&mut temp3, &temp2);
    unswizzle_transpose(block, &temp3);
}

fn convert_blocks(coeffs: &mut [i32], count: usize, pattern: u16, qtable: &[[i32; 64]], qt: &[usize; 3]) {
    let mut curp = pattern;
    for i in 0..count {
        if (curp & 3) == 3 {
            curp = pattern;
        }
        convert_block(&mut coeffs[i * 64..i * 64 + 64], &qtable[qt[(curp & 3) as usize]]);
        curp >>= 2;
    }
}

fn clamp<T: std::cmp::PartialOrd>(v: T, min: T, max: T) -> T {
    if v < min {
        return min;
    }
    else if v > max {
        return max;
    }
    v
}

fn draw_rgb(image: &mut ImageBuffer, px: usize, py: usize, r: i32, g: i32, b: i32) {
    image.data[py * image.width + px] = 0xFF000000 | ((clamp(r, 0, 255) as u32) << 16) | ((clamp(g, 0, 255) as u32) << 8) | (clamp(b, 0, 255) as u32);
}

fn draw_yuv(image: &mut ImageBuffer, px: usize, py: usize, y: i32, u: i32, v: i32) {
    let r = ((y << 8) + 359 * v) >> 8;
    let g = ((y << 8) - 88 * u - 183 * v) >> 8;
    let b = ((y << 8) + 454 * u) >> 8;
    draw_rgb(image, px, py, r, g, b);
}

fn draw_macroblock_y(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            draw_yuv(image, x0 + k, y0 + i, (coeffs[i * 8 + k] + 128) as i32, 0, 0);
        }
    }
}

fn draw_macroblock_yuv420(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            let by = (i >> 3) * 2 + (k >> 3);
            let si = i & 7;
            let sk = k & 7;
            let y = coeffs[by * 64 + si * 8 + sk] + 128;
            let hi = i >> 1;
            let hk = k >> 1;
            let u = coeffs[256 + hi * 8 + hk];
            let v = coeffs[320 + hi * 8 + hk];
            draw_yuv(image, x0 + k, y0 + i, y as i32, u as i32, v as i32);
        }
    }
}

fn draw_macroblock_yuv422_normal(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            let by = k >> 3;
            let sk = k & 7;
            let hk = k >> 1;
            let y = coeffs[by * 64 + i * 8 + sk] + 128;
            let u = coeffs[128 + i * 8 + hk];
            let v = coeffs[192 + i * 8 + hk];
            draw_yuv(image, x0 + k, y0 + i, y as i32, u as i32, v as i32);
        }
    }
}

#[cfg(feature="nightly")]
fn draw_macroblock_yuv422(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    if width & 3 != 0 {
        return draw_macroblock_yuv422_normal(image, x0, y0, width, height, coeffs);
    }
    return draw_macroblock_yuv422_simd(image, x0, y0, width, height, coeffs);
}

#[cfg(not(feature="nightly"))]
fn draw_macroblock_yuv422(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    return draw_macroblock_yuv422_normal(image, x0, y0, width, height, coeffs);
}

#[cfg(feature="nightly")]
fn draw_macroblock_yuv422_simd(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    use std::simd::*;

    fn draw_rgb_simd(image: &mut ImageBuffer, px: usize, py: usize, r: i32x4, g: i32x4, b: i32x4) {
        for i in 0..4 {
            image.data[py * image.width + px + i] = 0xFF000000 | ((r[i] as u32) << 16) | ((g[i] as u32) << 8) | (b[i] as u32);
        }
    }

    fn i32x4v(a: i32, b: i32, c: i32, d: i32) -> i32x4 {i32x4::from_array([a, b, c, d])}
    fn i32x4s(a: i32) -> i32x4 {i32x4::from_array([a; 4])}

    fn draw_yuv_simd(image: &mut ImageBuffer, px: usize, py: usize, y: i32x4, u: i32x4, v: i32x4) {
        // ok lets simd this.
        let r = ((y << i32x4s(8)) + i32x4s(359) * v) >> i32x4s(8);
        let g = ((y << i32x4s(8)) - i32x4s(88) * u - i32x4s(183) * v) >> i32x4s(8);
        let b = ((y << i32x4s(8)) + i32x4s(454) * u) >> i32x4s(8);
        draw_rgb_simd(image, px, py, r.clamp(i32x4s(0), i32x4s(255)), g.clamp(i32x4s(0), i32x4s(255)), b.clamp(i32x4s(0), i32x4s(255)));
    }

    for i in 0..height {
        for k in (0..width).step_by(4) {
            let k0 = k;
            let hk0 = k0 >> 1;
            let by0 = k0 >> 3;
            let sk0 = k0 & 7;
            let k1 = k + 1;
            let hk1 = k1 >> 1;
            let by1 = k1 >> 3;
            let sk1 = k1 & 7;
            let k2 = k + 2;
            let hk2 = k2 >> 1;
            let by2 = k2 >> 3;
            let sk2 = k2 & 7;
            let k3 = k + 3;
            let hk3 = k3 >> 1;
            let by3 = k3 >> 3;
            let sk3 = k3 & 7;
            let y = i32x4v(coeffs[by0 * 64 + i * 8 + sk0], coeffs[by1 * 64 + i * 8 + sk1], coeffs[by2 * 64 + i * 8 + sk2], coeffs[by3 * 64 + i * 8 + sk3]) + i32x4s(128);
            let u = i32x4v(coeffs[128 + i * 8 + hk0], coeffs[128 + i * 8 + hk1], coeffs[128 + i * 8 + hk2], coeffs[128 + i * 8 + hk3]);
            let v = i32x4v(coeffs[192 + i * 8 + hk0], coeffs[192 + i * 8 + hk1], coeffs[192 + i * 8 + hk2], coeffs[192 + i * 8 + hk3]);
            draw_yuv_simd(image, x0 + k, y0 + i, y, u, v);
        }
    }
}

fn draw_macroblock_yuv440(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            let by = i >> 3;
            let si = k & 7;
            let y = coeffs[by * 64 + si * 8 + k] + 128;
            let hi = i >> 1;
            let u = coeffs[128 + hi * 8 + k];
            let v = coeffs[192 + hi * 8 + k];
            draw_yuv(image, x0 + k, y0 + i, y as i32, u as i32, v as i32);
        }
    }
}

fn draw_macroblock_yuv444(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            let y = coeffs[i * 8 + k] + 128;
            let u = coeffs[64 + i * 8 + k];
            let v = coeffs[128 + i * 8 + k];
            draw_yuv(image, x0 + k, y0 + i, y as i32, u as i32, v as i32);
        }
    }
}

fn draw_macroblock_rgb444(image: &mut ImageBuffer, x0: usize, y0: usize, width: usize, height: usize, coeffs: &[i32]) {
    for i in 0..height {
        for k in 0..width {
            let r = coeffs[i * 8 + k] + 128;
            let g = coeffs[64 + i * 8 + k] + 128;
            let b = coeffs[128 + i * 8 + k] + 128;
            draw_rgb(image, x0 + k, y0 + i, r as i32, g as i32, b as i32);
        }
    }
}

pub fn test(src: &[u8]) -> Option<(usize, usize)> {
    let mut sp = 0;
    if from_be16(&src[sp..sp + 2]) != 0xFFD8 {
        return None;
    }
    sp += 2;
    while sp < src.len() {
        let marker = from_be16(&src[sp..sp + 2]);
        let length = from_be16(&src[sp + 2..sp + 4]) as usize;
        match marker {
            0xFFC0 | 0xFFC1 | 0xFFC2 => {
                let width = from_be16(&src[sp + 5..sp + 7]) as usize;
                let height = from_be16(&src[sp + 7..sp + 9]) as usize;
                let components = src[sp + 9];
                if (components == 1) || (components == 3) { // does not support RGBA or CMYK JPEGs
                    return Some((width, height));
                }
                return None;
            },
            _ => {},
        }
        sp += length + 2;
    }
    None
}

pub fn decode(src: &[u8]) -> Result<ImageBuffer, String> {
    if from_be16(&src[0..2]) != 0xFFD8 {
        return Err("Invalid JPEG 1".to_string());
    }
    let mut qtable = [[0i32; 64]; 4];
    let mut dcht = [Table::new_empty(); 4];
    let mut acht = [Table::new_empty(); 4];
    let mut qt = [0usize; 3];
    let mut dt = [0usize; 3];
    let mut at = [0usize; 3];
    #[allow(unused_assignments)]
    let mut width = 1;
    #[allow(unused_assignments)]
    let mut height = 1;
    #[allow(unused_assignments)]
    let mut itype = 0; // image type
    #[allow(unused_assignments)]
    let mut mbtotal = 0; // total number of macroblocks
    #[allow(unused_assignments)]
    let mut mbwidth = 0;
    #[allow(unused_assignments)]
    let mut mbheight = 0;
    #[allow(unused_assignments)]
    let mut cpmb = 0;
    let mut coeffs: Vec<i32> = Vec::new(); // the coefficients
    #[allow(unused_assignments)]
    let mut resint = 0;
    #[allow(unused_assignments)]
    let mut sp = 2;
    while sp < src.len() {
        let marker = from_be16(&src[sp..sp + 2]);
        let length = if marker != 0xFFD9 {from_be16(&src[sp + 2..sp + 4]) as usize} else {0};
        //println!("marker {:04X}, length {}",marker,length);
        match marker {
            0xFFC0 | 0xFFC1 | 0xFFC2 => { // baseline sequential, extended sequential, progressive
                //println!("precision {}",src[sp + 4]);
                if src[sp + 4] != 8 {
                    return Err("Invalid JPEG 2".to_string());
                }
                height = from_be16(&src[sp + 5..sp + 7]) as usize;
                width = from_be16(&src[sp + 7..sp + 9]) as usize;
                let components = src[sp + 9];
                //println!("size {}x{}, components {}",width,height,components);
                if (components != 1) && (components != 3) {
                    return Err("Invalid JPEG 3".to_string());
                }
                let mut samp = [0u8; 3];
                let mut tsp = sp + 10;
                for i in 0..components {
                    if src[tsp] != i + 1 {
                        return Err("Invalid JPEG 4".to_string());
                    }
                    samp[i as usize] = src[tsp + 1];
                    qt[i as usize] = src[tsp + 2] as usize;
                    tsp += 3;
                    //println!("{}: samp {:02X}, qt {}",i,samp[i as usize],qt[i as usize]);
                }
                if components == 3 {
                    if (samp[1] != 0x11) || (samp[2] != 0x11) {
                        return Err("Invalid JPEG 5".to_string());
                    }
                    let sw = ((samp[0] >> 4) * 8) as usize;
                    let sh = ((samp[0] & 15) * 8) as usize;
                    //println!("one macroblock = {}x{}",sw,sh);
                    mbwidth = (width + sw - 1) / sw;
                    mbheight = (height + sh - 1) / sh;
                    //println!("{}x{} macroblocks ({}x{} pixels)",mbwidth,mbheight,mbwidth * sw,mbheight * sh);
                    cpmb = 128 + 64 * ((samp[0] >> 4) as usize) * ((samp[0] & 15) as usize);
                    itype = match samp[0] {
                        0x11 => TYPE_YUV444,
                        0x12 => TYPE_YUV440,
                        0x21 => TYPE_YUV422,
                        0x22 => TYPE_YUV420,
                        _ => {
                            return Err("Invalid JPEG 6".to_string());
                        },
                    };
                }
                else {
                    mbwidth = (width + 7) / 8;
                    mbheight = (height + 7) / 8;
                    cpmb = 64;
                    itype = TYPE_Y;
                }
                mbtotal = mbwidth * mbheight;
                coeffs.resize(mbtotal * cpmb as usize, 0);
                //println!("type {:04X}, {} macroblocks in total, {} coefficients per row",itype,mbtotal,mbstride);
                //println!("size {}x{}, macroblocks {}",width,height,mbtotal);
            },
            0xFFC4 => { // huffman tables
                let mut tsp = sp + 4;
                while tsp < sp + length + 2 {
                    let d = src[tsp];
                    tsp += 1;
                    let tc = d >> 4;
                    let n = d & 15;
                    //println!("tc = {}, n = {}",tc,n);
                    let mut bits = [0u8; 16];
                    let mut total = 0usize;
                    for i in 0..16 {
                        bits[i] = src[tsp];
                        tsp += 1;
                        total += bits[i] as usize;
                    }
                    if total >= 256 {
                        return Err("Invalid JPEG 7".to_string());
                    }
                    //println!("total codes: {}",total);
                    let mut huffval = [0u8; 256];
                    for i in 0..total {
                        huffval[i] = src[tsp];
                        //println!("code {}: run {}, cat {}",i,huffval[i] >> 4,huffval[i] & 15);
                        tsp += 1;
                    }
                    let table = Table::new(bits, huffval);
                    if tc != 0 {
                        acht[n as usize] = table;
                    }
                    else {
                        dcht[n as usize] = table;
                    }
                }
            },
            0xFFD8 => { // image start
            },
            0xFFDA => { // scan start
                //println!("scan start");
                let mut tsp = sp + 4;
                let count = src[tsp];
                tsp += 1;
                // acht[4], dcht[4]
                let mut mask = 0;
                for _i in 0..count {
                    let index = src[tsp] - 1;
                    tsp += 1;
                    mask |= 1 << index;
                    let n = src[tsp];
                    tsp += 1;
                    dt[index as usize] = (n >> 4) as usize;
                    at[index as usize] = (n & 15) as usize;
                    //println!("index {}, dt {}, at {}",index,n >> 4,n & 15);
                }
                let start = src[tsp];
                tsp += 1;
                let end = src[tsp];
                tsp += 1;
                let d = src[tsp];
                tsp += 1;
                let refine = (d & 0xF0) != 0;
                let shift = d & 15;
                //println!("start = {}, end = {}, refine = {}, shift = {}",start,end,refine,shift);
                let mut reader = Reader::new(&src[tsp..]);
                let mut rescnt = resint;
                let mut eobrun = 0;
                let mut dc = [0i32; 3];
                for i in 0..mbtotal {
                    //println!("macroblock {}:",i);
                    unpack_macroblock(&mut reader, &mut coeffs[i * cpmb..(i + 1) * cpmb], &dcht, &acht, &dt, &at, &mut dc, start, end, shift, refine, &mut eobrun, itype, &mut rescnt, resint, mask);
                }
                sp = (tsp + reader.leave()) - length - 2;
                //println!("sp = {}, ({:02X} {:02X})",sp,src[sp + length + 2],src[sp + length + 2 + 1]);
            },
            0xFFDB => { // quantization tables
                let mut tsp = sp + 4;
                while tsp < sp + length + 2 {
                    let d = src[tsp];
                    tsp += 1;
                    let n = d & 15;
                    //println!("updating qtable[{}]",n);
                    if (d >> 4) != 0 {
                        for k in 0..64 {
                            qtable[n as usize][FOLDING[k as usize] as usize] = from_be16(&src[tsp..tsp + 2]) as i32;
                            tsp += 2;
                        }
                    }
                    else {
                        for k in 0..64 {
                            qtable[n as usize][FOLDING[k as usize] as usize] = src[tsp] as i32;
                            tsp += 1;
                        }
                    }
                }
            },
            0xFFDD => { // restart interval
                resint = from_be16(&src[sp + 4..sp + 6]) as usize;
            },
            0xFFE1 => { // EXIF
                let header = from_be32(&src[sp + 4..sp + 8]);
                if header == 0x45786966 { // Exif
                    let start = sp + 10;
                    let mut tsp = start;
                    let le = from_be16(&src[tsp..tsp + 2]) == 0x4949; // figure out endianness
                    tsp += 4; // skip 0x2A
                    tsp += (if le {from_le32(&src[tsp..tsp + 4])} else {from_be32(&src[tsp..tsp + 4])} -8) as usize; // go to IFD0
                    let entries = if le {from_le16(&src[tsp..tsp + 2])} else {from_be16(&src[tsp..tsp + 2])}; // number of entries
                    tsp += 2;
                    for _i in 0..entries {
                        let tag = if le {from_le16(&src[tsp..tsp + 2])} else {from_be16(&src[tsp..tsp + 2])};
                        tsp += 2;
                        let format = if le {from_le16(&src[tsp..tsp + 2])} else {from_be16(&src[tsp..tsp + 2])};
                        tsp += 2;
                        if format > 12 {
                            return Err("Invalid JPEG 8".to_string());
                        }
                        let components = if le {from_le32(&src[tsp..tsp + 4])} else {from_be32(&src[tsp..tsp + 4])};
                        tsp += 4;
                        let data = if le {from_le32(&src[tsp..tsp + 4])} else {from_be32(&src[tsp..tsp + 4])};
                        tsp += 4;
                        let elsize = [0usize, 1, 1, 2, 4, 8, 1, 0, 2, 4, 8, 4, 8];
                        let total = elsize[format as usize] * (components as usize);
                        let mut dsp = start + data as usize;
                        if total <= 4 {
                            dsp = tsp - 4;
                        }
                        //println!("EXIF tag {:04X}, format {}, components {}, data {:08X}",tag,format,components,data);
                        match tag {
                            0x0106 => { // photometric interpretation
                                let pe = if le {from_le16(&src[dsp..dsp + 2])} else {from_be16(&src[dsp..dsp + 2])};
                                if (pe != 2) || (itype != TYPE_YUV444) {
                                    return Err("Invalid JPEG 9".to_string());
                                }
                                itype = TYPE_RGB444;
                            },
                            0xA001 => { // colorspace
                            },
                            _ => {
                            }
                        }
                    }
                }
            },
            0xFFC8 | 0xFFDC | 0xFFE0 | 0xFFE2..=0xFFEF | 0xFFF0..=0xFFFF => { // other accepted markers
            },
            _ => { // image end
                //println!("end");
                let mut image = ImageBuffer::new(width, height);
                match itype {
                    TYPE_Y => {convert_blocks(&mut coeffs, mbtotal, TYPE_Y, &qtable, &qt);},
                    TYPE_YUV420 => {convert_blocks(&mut coeffs, mbtotal * 6, TYPE_YUV420, &qtable, &qt);},
                    TYPE_YUV422 => {convert_blocks(&mut coeffs, mbtotal * 4, TYPE_YUV422, &qtable, &qt);},
                    TYPE_YUV440 => {convert_blocks(&mut coeffs, mbtotal * 4, TYPE_YUV440, &qtable, &qt);},
                    TYPE_YUV444 => {convert_blocks(&mut coeffs, mbtotal * 3, TYPE_YUV444, &qtable, &qt);},
                    TYPE_RGB444 => {convert_blocks(&mut coeffs, mbtotal * 3, TYPE_RGB444, &qtable, &qt);},
                    _ => {},
                }
                #[allow(unused_assignments)]
                let mut mb = 0;
                for i in 0..mbheight - 1 {
                    for k in 0..mbwidth - 1 {
                        match itype {
                            TYPE_Y => {draw_macroblock_y(&mut image, k * 8, i * 8, 8, 8, &coeffs[mb..mb + 64]); mb += 64;},
                            TYPE_YUV420 => {draw_macroblock_yuv420(&mut image, k * 16, i * 16, 16, 16, &coeffs[mb..mb + 384]); mb += 384;},
                            TYPE_YUV422 => {draw_macroblock_yuv422(&mut image, k * 16, i * 8, 16, 8, &coeffs[mb..mb + 256]); mb += 256;},
                            TYPE_YUV440 => {draw_macroblock_yuv440(&mut image, k * 8, i * 16, 8, 16, &coeffs[mb..mb + 256]); mb += 256;},
                            TYPE_YUV444 => {draw_macroblock_yuv444(&mut image, k * 8, i * 8, 8, 8, &coeffs[mb..mb + 192]); mb += 192;},
                            TYPE_RGB444 => {draw_macroblock_rgb444(&mut image, k * 8, i * 8, 8, 8, &coeffs[mb..mb + 192]); mb += 192;},
                            _ => {},
                        }
                    }
                    match itype {
                        TYPE_Y => {draw_macroblock_y(&mut image, mbwidth * 8 - 8, i * 8, width - (mbwidth - 1) * 8, 8, &coeffs[mb..mb + 64]); mb += 64;},
                        TYPE_YUV420 => {draw_macroblock_yuv420(&mut image, mbwidth * 16 - 16, i * 16, width - (mbwidth - 1) * 16, 16, &coeffs[mb..mb + 384]); mb += 384;},
                        TYPE_YUV422 => {draw_macroblock_yuv422(&mut image, mbwidth * 16 - 16, i * 8, width - (mbwidth - 1) * 16, 8, &coeffs[mb..mb + 256]); mb += 256;},
                        TYPE_YUV440 => {draw_macroblock_yuv440(&mut image, mbwidth * 8 - 8, i * 16, width - (mbwidth - 1) * 8, 16, &coeffs[mb..mb + 256]); mb += 256;},
                        TYPE_YUV444 => {draw_macroblock_yuv444(&mut image, mbwidth * 8 - 8, i * 8, width - (mbwidth - 1) * 8, 8, &coeffs[mb..mb + 192]); mb += 192;},
                        TYPE_RGB444 => {draw_macroblock_rgb444(&mut image, mbwidth * 8 - 8, i * 8, width - (mbwidth - 1) * 8, 8, &coeffs[mb..mb + 192]); mb += 192;},
                        _ => {},
                    }
                }
                for k in 0..mbwidth - 1 {
                    match itype {
                        TYPE_Y => {draw_macroblock_y(&mut image, k * 8, mbheight * 8 - 8, 8, mbheight * 8 - height, &coeffs[mb..mb + 64]); mb += 64;},
                        TYPE_YUV420 => {draw_macroblock_yuv420(&mut image, k * 16, mbheight * 16 - 16, 16, height - (mbheight - 1) * 16, &coeffs[mb..mb + 384]); mb += 384;},
                        TYPE_YUV422 => {draw_macroblock_yuv422(&mut image, k * 16, mbheight * 8 - 8, 16, height - (mbheight - 1) * 8, &coeffs[mb..mb + 256]); mb += 256;},
                        TYPE_YUV440 => {draw_macroblock_yuv440(&mut image, k * 8, mbheight * 16 - 16, 8, height - (mbheight - 1) * 16, &coeffs[mb..mb + 256]); mb += 256;},
                        TYPE_YUV444 => {draw_macroblock_yuv444(&mut image, k * 8, mbheight * 8 - 8, 8, height - (mbheight - 1) * 8, &coeffs[mb..mb + 192]); mb += 192;},
                        TYPE_RGB444 => {draw_macroblock_rgb444(&mut image, k * 8, mbheight * 8 - 8, 8, height - (mbheight - 1) * 8, &coeffs[mb..mb + 192]); mb += 192;},
                        _ => {},
                    }
                }
                match itype {
                    TYPE_Y => {draw_macroblock_y(&mut image, mbwidth * 8 - 8, mbheight * 8 - 8, width - (mbwidth - 1) * 8, height - (mbheight - 1) * 8, &coeffs[mb..mb + 64]);},
                    TYPE_YUV420 => {draw_macroblock_yuv420(&mut image, mbwidth * 16 - 16, mbheight * 16 - 16, width - (mbwidth - 1) * 16, height - (mbheight - 1) * 16, &coeffs[mb..mb + 384]);},
                    TYPE_YUV422 => {draw_macroblock_yuv422(&mut image, mbwidth * 16 - 16, mbheight * 8 - 8, width - (mbwidth - 1) * 16, height - (mbheight - 1) * 8, &coeffs[mb..mb + 256]);},
                    TYPE_YUV440 => {draw_macroblock_yuv440(&mut image, mbwidth * 8 - 8, mbheight * 16 - 16, width - (mbwidth - 1) * 8, height - (mbheight - 1) * 16, &coeffs[mb..mb + 256]);},
                    TYPE_YUV444 => {draw_macroblock_yuv444(&mut image, mbwidth * 8 - 8, mbheight * 8 - 8, width - (mbwidth - 1) * 8, height - (mbheight - 1) * 8, &coeffs[mb..mb + 192]);},
                    TYPE_RGB444 => {draw_macroblock_rgb444(&mut image, mbwidth * 8 - 8, mbheight * 8 - 8, width - (mbwidth - 1) * 8, height - (mbheight - 1) * 8, &coeffs[mb..mb + 192]);},
                    _ => {},
                }
                return Ok(image);
            },
            
            /*_ => {
                return Err("Invalid JPEG 10".to_string());
            },*/
        }
        sp += length + 2;
    }
    Err("Invalid JPEG 11".to_string())
}

pub fn encode(_image: &ImageBuffer) -> Result<Vec<u8>, String> {
    Err("not implemented yet".to_string())
}
