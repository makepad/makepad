// Stripped down version of lode-png for Rust port of Lode-PNG.

use crate::lode_png::lode_png_types::*;

use std::collections::HashMap;

pub fn lodepng_decode_memory(inp: &[u8], colortype: ColorType, bitdepth: u32) -> Result<(Vec<u8>, usize, usize), PNGError> {
    let mut state = State::new();
    state.info_raw_mut().colortype = colortype;
    state.info_raw_mut().set_bitdepth(bitdepth);
    lodepng_decode(&mut state, inp)
}

pub fn lodepng_decode(state: &mut State, inp: &[u8]) -> Result<(Vec<u8>, usize, usize), PNGError> {
    let (decoded, w, h) = decode_generic(state, inp)?;

    if !state.decoder.color_convert || lodepng_color_mode_equal(&state.info_raw, &state.info_png.color) {
        /*store the info_png color settings on the info_raw so that the info_raw still reflects what colortype
            the raw image has to the end user*/
        if !state.decoder.color_convert {
            /*color conversion needed; sort of copy of the data*/
            state.info_raw = state.info_png.color.clone();
        }
        Ok((decoded, w, h))
    } else {
        /*TODO: check if this works according to the statement in the documentation: "The converter can convert
            from greyscale input color type, to 8-bit greyscale or greyscale with alpha"*/
        if !(state.info_raw.colortype == ColorType::RGB || state.info_raw.colortype == ColorType::RGBA) && (state.info_raw.bitdepth() != 8) {
            return Err(PNGError::new(56)); /*unsupported color mode conversion*/
        }
        let mut out = zero_vec(state.info_raw.raw_size(w as u32, h as u32))?;
        lodepng_convert(&mut out, &decoded, &state.info_raw, &state.info_png.color, w as u32, h as u32)?;
        Ok((out, w, h))
    }
}

pub fn lodepng_encode_memory(image: &[u8], w: u32, h: u32, colortype: ColorType, bitdepth: u32) -> Result<Vec<u8>, PNGError> {
    let mut state = State::default();
    state.info_raw_mut().colortype = colortype;
    state.info_raw_mut().set_bitdepth(bitdepth);
    state.info_png_mut().color.colortype = colortype;
    state.info_png_mut().color.set_bitdepth(bitdepth);
    lodepng_encode(image, w, h, &mut state)
}

pub const LODEPNG_VERSION_STRING: &[u8] = b"Makepad-1.0\0";

pub fn lodepng_encode(image: &[u8], w: u32, h: u32, state: &mut State) -> Result<Vec<u8>, PNGError> {
    let w = w as usize;
    let h = h as usize;

    let mut info = state.info_png.clone();
    if (info.color.colortype == ColorType::PALETTE || state.encoder.force_palette) && (info.color.palette().is_empty() || info.color.palette().len() > 256) {
        return Err(PNGError::new(68));
    }
    if state.encoder.auto_convert {
        info.color = auto_choose_color(image, w, h, &state.info_raw)?;
    }
    if state.info_png.interlace_method > 1 {
        return Err(PNGError::new(71));
    }
    check_png_color_validity(info.color.colortype, info.color.bitdepth())?; /*tEXt and/or zTXt */
    check_lode_color_validity(state.info_raw.colortype, state.info_raw.bitdepth())?; /*LodePNG version id in text chunk */

    let data = if !lodepng_color_mode_equal(&state.info_raw, &info.color) {
        let raw_size = (w * h * (info.color.bpp() as usize) + 7) / 8;
        let mut converted = zero_vec(raw_size)?;
        lodepng_convert(&mut converted, image, &info.color, &state.info_raw, w as u32, h as u32)?;
        pre_process_scanlines(&converted, w, h, &info, &state.encoder)?
    } else {
        pre_process_scanlines(image, w, h, &info, &state.encoder)?
    };

    let mut outv = Vec::with_capacity(1024);
    write_signature(&mut outv);

    add_chunk_ihdr(&mut outv, w, h, info.color.colortype, info.color.bitdepth() as usize, info.interlace_method as u8)?;
    add_unknown_chunks(&mut outv, &info.unknown_chunks[ChunkPosition::IHDR as usize])?;
    if info.color.colortype == ColorType::PALETTE {
        add_chunk_plte(&mut outv, &info.color)?;
    }
    if state.encoder.force_palette && (info.color.colortype == ColorType::RGB || info.color.colortype == ColorType::RGBA) {
        add_chunk_plte(&mut outv, &info.color)?;
    }
    if info.color.colortype == ColorType::PALETTE && get_palette_translucency(info.color.palette()) != PaletteTranslucency::Opaque {
        add_chunk_trns(&mut outv, &info.color)?;
    }
    if (info.color.colortype == ColorType::GREY || info.color.colortype == ColorType::RGB) && info.color.key().is_some() {
        add_chunk_trns(&mut outv, &info.color)?;
    }
    if info.background_defined {
        add_chunk_bkgd(&mut outv, &info)?;
    }
    if info.phys_defined {
        add_chunk_phys(&mut outv, &info)?;
    }
    add_unknown_chunks(&mut outv, &info.unknown_chunks[ChunkPosition::PLTE as usize])?;
    add_chunk_idat(&mut outv, &data)?;
    if info.time_defined {
        add_chunk_time(&mut outv, &info.time)?;
    }
    for t in &info.texts {
        if t.key.len() > 79 {
            return Err(PNGError::new(66));
        }
        if t.key.is_empty() {
            return Err(PNGError::new(67));
        }
        if state.encoder.text_compression {
            add_chunk_ztxt(&mut outv, &t.key, &t.value)?;
        } else {
            add_chunk_text(&mut outv, &t.key, &t.value)?;
        }
    }
    if state.encoder.add_id {
        let alread_added_id_text = info.texts.iter().any(|t| *t.key == b"LodePNG"[..]);
        if !alread_added_id_text {
            /*it's shorter as tEXt than as zTXt chunk*/
            add_chunk_text(&mut outv, b"LodePNG", LODEPNG_VERSION_STRING)?;
        }
    }
    for (k, l, t, s) in info.itext_keys() {
        if k.as_bytes().len() > 79 {
            return Err(PNGError::new(66));
        }
        if k.as_bytes().is_empty() {
            return Err(PNGError::new(67));
        }
        add_chunk_itxt(&mut outv, state.encoder.text_compression, k, l, t, s)?;
    }
    add_unknown_chunks(&mut outv, &info.unknown_chunks[ChunkPosition::IDAT as usize])?;
    add_chunk_iend(&mut outv)?;
    Ok(outv)
}

/* ////////////////////////////////////////////////////////////////////////// */
/* / Zlib                                                                   / */
/* ////////////////////////////////////////////////////////////////////////// */
pub fn lodepng_zlib_decompress(inp: &[u8]) -> Result<Vec<u8>, PNGError> {
    if inp.len() < 2 {
        return Err(PNGError::new(53));
    }
    /*read information from zlib header*/
    if (inp[0] as u32 * 256 + inp[1] as u32) % 31 != 0 {
        /*error: 256 * in[0] + in[1] must be a multiple of 31, the FCHECK value is supposed to be made that way*/
        return Err(PNGError::new(24));
    }
    let cm = inp[0] as u32 & 15;
    let cinfo = ((inp[0] as u32) >> 4) & 15;
    let fdict = ((inp[1] as u32) >> 5) & 1;
    if cm != 8 || cinfo > 7 {
        //error: only compression method 8: inflate with sliding window of 32k is supported by the PNG spec*/
        return Err(PNGError::new(25));
    }
    if fdict != 0 {
        /*error: the specification of PNG says about the zlib stream:
              "The additional flags shall not specify a preset dictionary."*/
        return Err(PNGError::new(26));
    }
    
    match crate::lode_png::miniz_oxide::decompress_to_vec_zlib(inp){
        Ok(out)=>{
            Ok(out)
        }
        Err(_)=>{
            Err(PNGError::new(96))
        }
    }
}

pub fn zlib_decompress(inp: &[u8]) -> Result<Vec<u8>, PNGError> {
    lodepng_zlib_decompress(inp)
}

pub fn lodepng_zlib_compress(outv: &mut Vec<u8>, inp: &[u8]) -> Result<(), PNGError> {
    let out = crate::lode_png::miniz_oxide::compress_to_vec_zlib(inp, 9);
    outv.copy_from_slice(&out);
    Ok(())
}

/* compress using the default or custom zlib function */

pub fn zlib_compress(inp: &[u8]) -> Result<Vec<u8>, PNGError> {
    let mut out = Vec::try_with_capacity(inp.len() / 2)?;
    lodepng_zlib_compress(&mut out, inp)?;
    Ok(out)
}

/*8 bytes PNG signature, aka the magic bytes*/
fn write_signature(out: &mut Vec<u8>) {
    out.push(137u8);
    out.push(80u8);
    out.push(78u8);
    out.push(71u8);
    out.push(13u8);
    out.push(10u8);
    out.push(26u8);
    out.push(10u8);
}

#[inline]
fn zero_vec(size: usize) -> Result<Vec<u8>, PNGError> {
    let mut vec = Vec::try_with_capacity(size)?;
    vec.resize(size, 0u8);
    Ok(vec)
}

#[derive(Eq, PartialEq)]
enum PaletteTranslucency {
    Opaque,
    Key,
    Semi,
}

/*
palette must have 4 * palettesize bytes allocated, and given in format RGBARGBARGBARGBA…
returns 0 if the palette is opaque,
returns 1 if the palette has a single color with alpha 0 ==> color key
returns 2 if the palette is semi-translucent.
*/
fn get_palette_translucency(palette: &[RGBA]) -> PaletteTranslucency {
    let mut key = PaletteTranslucency::Opaque;
    let mut r = 0;
    let mut g = 0;
    let mut b = 0;
    /*the value of the color with alpha 0, so long as color keying is possible*/
    let mut i = 0;
    while i < palette.len() {
        if key == PaletteTranslucency::Opaque && palette[i].a == 0 {
            r = palette[i].r;
            g = palette[i].g;
            b = palette[i].b;
            key = PaletteTranslucency::Key;
            i = 0;
            /*restart from beginning, to detect earlier opaque colors with key's value*/
            continue;
        } else if palette[i].a != 255 {
            return PaletteTranslucency::Semi;
        } else if key == PaletteTranslucency::Key && r == palette[i].r && g == palette[i].g && b == palette[i].b {
            /*when key, no opaque RGB may have key's RGB*/
            return PaletteTranslucency::Semi;
        }
        i += 1;
    }
    key
}

/*The opposite of the remove_padding_bits function
  olinebits must be >= ilinebits*/
fn add_padding_bits(out: &mut [u8], inp: &[u8], olinebits: usize, ilinebits: usize, h: usize) {
    let diff = olinebits - ilinebits; /*bit pointers*/
    let mut obp = 0;
    let mut ibp = 0;
    for _ in 0..h {
        for _ in 0..ilinebits {
            let bit = read_bit_from_reversed_stream(&mut ibp, inp);
            set_bit_of_reversed_stream(&mut obp, out, bit);
        }
        for _ in 0..diff {
            set_bit_of_reversed_stream(&mut obp, out, 0u8);
        }
    }
}

/*out must be buffer big enough to contain uncompressed IDAT chunk data, and in must contain the full image.
return value is error**/
fn pre_process_scanlines(inp: &[u8], w: usize, h: usize, info_png: &Info, settings: &EncoderSettings) -> Result<Vec<u8>, PNGError> {
    let h = h as usize;
    let w = w as usize;
    /*
      This function converts the pure 2D image with the PNG's colortype, into filtered-padded-interlaced data. Steps:
      *) if no Adam7: 1) add padding bits (= posible extra bits per scanline if bpp < 8) 2) filter
      *) if adam7: 1) adam7_interlace 2) 7x add padding bits 3) 7x filter
      */
    let bpp = info_png.color.bpp() as usize;
    if info_png.interlace_method == 0 {
        let outsize = h + (h * ((w * bpp + 7) / 8));
        let mut out = zero_vec(outsize)?;
        /*image size plus an extra byte per scanline + possible padding bits*/
        if bpp < 8 && w * bpp != ((w * bpp + 7) / 8) * 8 {
            let mut padded = zero_vec(h * ((w * bpp + 7) / 8))?; /*we can immediately filter into the out buffer, no other steps needed*/
            add_padding_bits(&mut padded, inp, ((w * bpp + 7) / 8) * 8, w * bpp, h);
            filter(&mut out, &padded, w, h, &info_png.color, settings)?;
        } else {
            filter(&mut out, inp, w, h, &info_png.color, settings)?;
        }
        Ok(out)
    } else {
        let (passw, passh, filter_passstart, padded_passstart, passstart) = adam7_get_pass_values(w, h, bpp);
        let outsize = filter_passstart[7];
        /*image size plus an extra byte per scanline + possible padding bits*/
        let mut out = zero_vec(outsize)?;
        let mut adam7 = zero_vec(passstart[7] + 1)?;
        adam7_interlace(&mut adam7, inp, w, h, bpp);
        for i in 0..7 {
            if bpp < 8 {
                let mut padded = zero_vec(padded_passstart[i + 1] - padded_passstart[i])?;
                add_padding_bits(
                    &mut padded,
                    &adam7[passstart[i]..],
                    ((passw[i] as usize * bpp + 7) / 8) * 8,
                    passw[i] as usize * bpp,
                    passh[i] as usize,
                );
                filter(&mut out[filter_passstart[i]..], &padded, passw[i] as usize, passh[i] as usize, &info_png.color, settings)?;
            } else {
                filter(
                    &mut out[filter_passstart[i]..],
                    &adam7[padded_passstart[i]..],
                    passw[i] as usize,
                    passh[i] as usize,
                    &info_png.color,
                    settings,
                )?;
            }
        }
        Ok(out)
    }
}

/*
  For PNG filter method 0
  out must be a buffer with as size: h + (w * h * bpp + 7) / 8, because there are
  the scanlines with 1 extra byte per scanline
  */
fn filter(out: &mut [u8], inp: &[u8], w: usize, h: usize, info: &ColorMode, settings: &EncoderSettings) -> Result<(), PNGError> {
    let bpp = info.bpp() as usize;

    /*the width of a scanline in bytes, not including the filter type*/
    let linebytes = ((w * bpp + 7) / 8) as usize;
    /*bytewidth is used for filtering, is 1 when bpp < 8, number of bytes per pixel otherwise*/
    let bytewidth = (bpp + 7) / 8;
    let mut prevline = None;
    /*
      There is a heuristic called the minimum sum of absolute differences heuristic, suggested by the PNG standard:
       *  If the image type is Palette, or the bit depth is smaller than 8, then do not filter the image (i.e.
          use fixed filtering, with the filter None).
       * (The other case) If the image type is Grayscale or RGB (with or without Alpha), and the bit depth is
         not smaller than 8, then use adaptive filtering heuristic as follows: independently for each row, apply
         all five filters and select the filter that produces the smallest sum of absolute values per row.
      This heuristic is used if filter strategy is FilterStrategy::MINSUM and filter_palette_zero is true.

      If filter_palette_zero is true and filter_strategy is not FilterStrategy::MINSUM, the above heuristic is followed,
      but for "the other case", whatever strategy filter_strategy is set to instead of the minimum sum
      heuristic is used.
      */
    let strategy = if settings.filter_palette_zero && (info.colortype == ColorType::PALETTE || info.bitdepth() < 8) {
        FilterStrategy::ZERO
    } else {
        settings.filter_strategy
    };
    if bpp == 0 {
        return Err(PNGError::new(31));
    }
    match strategy {
        FilterStrategy::ZERO => for y in 0..h {
            let outindex = (1 + linebytes) * y;
            let inindex = linebytes * y;
            out[outindex] = 0u8;
            filter_scanline(&mut out[(outindex + 1)..], &inp[inindex..], prevline, linebytes, bytewidth, 0u8);
            prevline = Some(&inp[inindex..]);
        },
        FilterStrategy::MINSUM => {
            let mut sum: [usize; 5] = [0, 0, 0, 0, 0];
            let mut attempt = [
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
            ];
            let mut smallest = 0;
            let mut best_type = 0;
            for y in 0..h {
                for type_ in 0..5 {
                    filter_scanline(&mut attempt[type_], &inp[(y * linebytes)..], prevline, linebytes, bytewidth, type_ as u8);
                    sum[type_] = if type_ == 0 {
                        attempt[type_][0..linebytes].iter().map(|&s| s as usize).sum()
                    } else {
                        /*For differences, each byte should be treated as signed, values above 127 are negative
                          (converted to signed char). filter_type 0 isn't a difference though, so use unsigned there.
                          This means filter_type 0 is almost never chosen, but that is justified.*/
                        attempt[type_][0..linebytes].iter().map(|&s| if s < 128 { s } else { 255 - s } as usize).sum()
                    };
                    /*check if this is smallest sum (or if type == 0 it's the first case so always store the values)*/
                    if type_ == 0 || sum[type_] < smallest {
                        best_type = type_; /*now fill the out values*/
                        smallest = sum[type_];
                    };
                }
                prevline = Some(&inp[(y * linebytes)..]);
                out[y * (linebytes + 1)] = best_type as u8;
                /*the first byte of a scanline will be the filter type*/
                for x in 0..linebytes {
                    out[y * (linebytes + 1) + 1 + x] = attempt[best_type][x];
                } /*try the 5 filter types*/
            } /*the filter type itself is part of the scanline*/
        },
        FilterStrategy::ENTROPY => {
            let mut sum: [f32; 5] = [0., 0., 0., 0., 0.];
            let mut smallest = 0.;
            let mut best_type = 0;
            let mut attempt = [
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
                zero_vec(linebytes)?,
            ];
            for y in 0..h {
                for type_ in 0..5 {
                    filter_scanline(&mut attempt[type_], &inp[(y * linebytes)..], prevline, linebytes, bytewidth, type_ as u8);
                    let mut count: [u32; 256] = [0; 256];
                    for x in 0..linebytes {
                        count[attempt[type_][x] as usize] += 1;
                    }
                    count[type_] += 1;
                    sum[type_] = 0.;
                    for &c in count.iter() {
                        let p = c as f32 / ((linebytes + 1) as f32);
                        sum[type_] += if c == 0 { 0. } else { (1. / p).log2() * p };
                    }
                    /*check if this is smallest sum (or if type == 0 it's the first case so always store the values)*/
                    if type_ == 0 || sum[type_] < smallest {
                        best_type = type_; /*now fill the out values*/
                        smallest = sum[type_]; /*the first byte of a scanline will be the filter type*/
                    }; /*the extra filterbyte added to each row*/
                }
                prevline = Some(&inp[(y * linebytes)..]);
                out[y * (linebytes + 1)] = best_type as u8;
                for x in 0..linebytes {
                    out[y * (linebytes + 1) + 1 + x] = attempt[best_type][x];
                }
            }
        },
    };
    Ok(())
}

fn filter_scanline(out: &mut [u8], scanline: &[u8], prevline: Option<&[u8]>, length: usize, bytewidth: usize, filter_type: u8) {
    match filter_type {
        0 => {
            out[..length].clone_from_slice(&scanline[..length]);
        },
        1 => {
            out[..bytewidth].clone_from_slice(&scanline[..bytewidth]);
            for i in bytewidth..length {
                out[i] = scanline[i].wrapping_sub(scanline[i - bytewidth]);
            }
        },
        2 => if let Some(prevline) = prevline {
            for i in 0..length {
                out[i] = scanline[i].wrapping_sub(prevline[i]);
            }
        } else {
            out[..length].clone_from_slice(&scanline[..length]);
        },
        3 => if let Some(prevline) = prevline {
            for i in 0..bytewidth {
                out[i] = scanline[i].wrapping_sub(prevline[i] >> 1);
            }
            for i in bytewidth..length {
                let s = scanline[i - bytewidth] as u16 + prevline[i] as u16;
                out[i] = scanline[i].wrapping_sub((s >> 1) as u8);
            }
        } else {
            out[..bytewidth].clone_from_slice(&scanline[..bytewidth]);
            for i in bytewidth..length {
                out[i] = scanline[i].wrapping_sub(scanline[i - bytewidth] >> 1);
            }
        },
        4 => if let Some(prevline) = prevline {
            for i in 0..bytewidth {
                out[i] = scanline[i].wrapping_sub(prevline[i]);
            }
            for i in bytewidth..length {
                out[i] = scanline[i].wrapping_sub(paeth_predictor(scanline[i - bytewidth].into(), prevline[i].into(), prevline[i - bytewidth].into()));
            }
        } else {
            out[..bytewidth].clone_from_slice(&scanline[..bytewidth]);
            for i in bytewidth..length {
                out[i] = scanline[i].wrapping_sub(scanline[i - bytewidth]);
            }
        },
        _ => {},
    };
}

fn paeth_predictor(a: i16, b: i16, c: i16) -> u8 {
    let pa = (b - c).abs();
    let pb = (a - c).abs();
    let pc = (a + b - c - c).abs();
    if pc < pa && pc < pb {
        c as u8
    } else if pb < pa {
        b as u8
    } else {
        a as u8
    }
}

pub(crate) fn lodepng_get_bpp_lct(colortype: ColorType, bitdepth: u32) -> u32 {
    assert!(bitdepth >= 1 && bitdepth <= 16);
    /*bits per pixel is amount of channels * bits per channel*/
    let ch = colortype.channels() as u32;
    ch * if ch > 1 {
        if bitdepth == 8 {
            8
        } else {
            16
        }
    } else {
        bitdepth
    }
}

pub fn lodepng_get_raw_size_lct(w: u32, h: u32, colortype: ColorType, bitdepth: u32) -> usize {
    /*will not overflow for any color type if roughly w * h < 268435455*/
    let bpp = lodepng_get_bpp_lct(colortype, bitdepth) as usize;
    let n = w as usize * h as usize;
    ((n / 8) * bpp) + ((n & 7) * bpp + 7) / 8
}

#[inline]
pub(crate) fn lodepng_read32bit_int(buffer: &[u8]) -> u32 {
    ((buffer[0] as u32) << 24) | ((buffer[1] as u32) << 16) | ((buffer[2] as u32) << 8) | buffer[3] as u32
}

#[inline(always)]
fn lodepng_set32bit_int(buffer: &mut [u8], value: u32) {
    buffer[0] = ((value >> 24) & 255) as u8;
    buffer[1] = ((value >> 16) & 255) as u8;
    buffer[2] = ((value >> 8) & 255) as u8;
    buffer[3] = ((value) & 255) as u8;
}

#[inline(always)]
fn add32bit_int(buffer: &mut Vec<u8>, value: u32) {
    buffer.push(((value >> 24) & 255) as u8);
    buffer.push(((value >> 16) & 255) as u8);
    buffer.push(((value >> 8) & 255) as u8);
    buffer.push(((value) & 255) as u8);
}

#[inline]
fn lodepng_add32bit_int(buffer: &mut Vec<u8>, value: u32) {
    add32bit_int(buffer, value);
}

fn add_color_bits(out: &mut [u8], index: usize, bits: u32, mut inp: u32) {
    let m = match bits {
        1 => 7,
        2 => 3,
        _ => 1,
    };
    /*p = the partial index in the byte, e.g. with 4 palettebits it is 0 for first half or 1 for second half*/
    let p = index & m; /*filter out any other bits of the input value*/
    inp &= (1 << bits) - 1;
    inp <<= bits * (m - p) as u32;
    if p == 0 {
        out[index * bits as usize / 8] = inp as u8;
    } else {
        out[index * bits as usize / 8] |= inp as u8;
    }
}

pub type ColorTree = HashMap<(u8, u8, u8, u8), u16>;

#[inline(always)]
fn rgba8_to_pixel(out: &mut [u8], i: usize, mode: &ColorMode, tree: &mut ColorTree, /*for palette*/ r: u8, g: u8, b: u8, a: u8) -> Result<(), PNGError> {
    match mode.colortype {
        ColorType::GREY => {
            let grey = r; /*((unsigned short)r + g + b) / 3*/
            if mode.bitdepth() == 8 {
                out[i] = grey; /*take the most significant bits of grey*/
            } else if mode.bitdepth() == 16 {
                out[i * 2 + 0] = {
                    out[i * 2 + 1] = grey; /*color not in palette*/
                    out[i * 2 + 1]
                }; /*((unsigned short)r + g + b) / 3*/
            } else {
                let grey = (grey >> (8 - mode.bitdepth())) & ((1 << mode.bitdepth()) - 1); /*no error*/
                add_color_bits(out, i, mode.bitdepth(), grey.into());
            };
        },
        ColorType::RGB => if mode.bitdepth() == 8 {
            out[i * 3 + 0] = r;
            out[i * 3 + 1] = g;
            out[i * 3 + 2] = b;
        } else {
            out[i * 6 + 0] = r;
            out[i * 6 + 1] = r;
            out[i * 6 + 2] = g;
            out[i * 6 + 3] = g;
            out[i * 6 + 4] = b;
            out[i * 6 + 5] = b;
        },
        ColorType::PALETTE => {
            let index = *tree.get(&(r, g, b, a)).ok_or(PNGError::new(82))?;
            if mode.bitdepth() == 8 {
                out[i] = index as u8;
            } else {
                add_color_bits(out, i, mode.bitdepth(), u32::from(index));
            };
        },
        ColorType::GREY_ALPHA => {
            let grey = r;
            if mode.bitdepth() == 8 {
                out[i * 2 + 0] = grey;
                out[i * 2 + 1] = a;
            } else if mode.bitdepth() == 16 {
                out[i * 4 + 0] = grey;
                out[i * 4 + 1] = grey;
                out[i * 4 + 2] = a;
                out[i * 4 + 3] = a;
            }
        },
        ColorType::RGBA => if mode.bitdepth() == 8 {
            out[i * 4 + 0] = r;
            out[i * 4 + 1] = g;
            out[i * 4 + 2] = b;
            out[i * 4 + 3] = a;
        } else {
            out[i * 8 + 0] = r;
            out[i * 8 + 1] = r;
            out[i * 8 + 2] = g;
            out[i * 8 + 3] = g;
            out[i * 8 + 4] = b;
            out[i * 8 + 5] = b;
            out[i * 8 + 6] = a;
            out[i * 8 + 7] = a;
        },
        ColorType::BGRA |
        ColorType::BGR |
        ColorType::BGRX => {
            return Err(PNGError::new(31));
        },
    };
    Ok(())
}

/*put a pixel, given its RGBA16 color, into image of any color 16-bitdepth type*/
#[inline(always)]
fn rgba16_to_pixel(out: &mut [u8], i: usize, mode: &ColorMode, r: u16, g: u16, b: u16, a: u16) {
    match mode.colortype {
        ColorType::GREY => {
            let grey = r;
            out[i * 2 + 0] = (grey >> 8) as u8;
            out[i * 2 + 1] = grey as u8;
        },
        ColorType::RGB => {
            out[i * 6 + 0] = (r >> 8) as u8;
            out[i * 6 + 1] = r as u8;
            out[i * 6 + 2] = (g >> 8) as u8;
            out[i * 6 + 3] = g as u8;
            out[i * 6 + 4] = (b >> 8) as u8;
            out[i * 6 + 5] = b as u8;
        },
        ColorType::GREY_ALPHA => {
            let grey = r;
            out[i * 4 + 0] = (grey >> 8) as u8;
            out[i * 4 + 1] = grey as u8;
            out[i * 4 + 2] = (a >> 8) as u8;
            out[i * 4 + 3] = a as u8;
        },
        ColorType::RGBA => {
            out[i * 8 + 0] = (r >> 8) as u8;
            out[i * 8 + 1] = r as u8;
            out[i * 8 + 2] = (g >> 8) as u8;
            out[i * 8 + 3] = g as u8;
            out[i * 8 + 4] = (b >> 8) as u8;
            out[i * 8 + 5] = b as u8;
            out[i * 8 + 6] = (a >> 8) as u8;
            out[i * 8 + 7] = a as u8;
        },
        ColorType::BGR |
        ColorType::BGRA |
        ColorType::BGRX |
        ColorType::PALETTE => unreachable!(),
    };
}

/*Get RGBA8 color of pixel with index i (y * width + x) from the raw image with given color type.*/
fn get_pixel_color_rgba8(inp: &[u8], i: usize, mode: &ColorMode) -> (u8, u8, u8, u8) {
    match mode.colortype {
        ColorType::GREY => {
            if mode.bitdepth() == 8 {
                let t = inp[i];
                let a = if mode.key() == Some((u16::from(t), u16::from(t), u16::from(t))) {
                    0
                } else {
                    255
                };
                (t, t, t, a)
            } else if mode.bitdepth() == 16 {
                let t = inp[i * 2 + 0];
                let g = 256 * inp[i * 2 + 0] as u16 + inp[i * 2 + 1] as u16;
                let a = if mode.key() == Some((g, g, g)) {
                    0
                } else {
                    255
                };
                (t, t, t, a)
            } else {
                let highest = (1 << mode.bitdepth()) - 1;
                /*highest possible value for this bit depth*/
                let mut j = i as usize * mode.bitdepth() as usize;
                let value = read_bits_from_reversed_stream(&mut j, inp, mode.bitdepth() as usize);
                let t = ((value * 255) / highest) as u8;
                let a = if mode.key() == Some((t as u16, t as u16, t as u16)) {
                    0
                } else {
                    255
                };
                (t, t, t, a)
            }
        },
        ColorType::RGB => if mode.bitdepth() == 8 {
            let r = inp[i * 3 + 0];
            let g = inp[i * 3 + 1];
            let b = inp[i * 3 + 2];
            let a = if mode.key() == Some((u16::from(r), u16::from(g), u16::from(b))) {
                0
            } else {
                255
            };
            (r, g, b, a)
        } else {
            (
                inp[i * 6 + 0],
                inp[i * 6 + 2],
                inp[i * 6 + 4],
                if mode.key()
                    == Some((
                        256 * inp[i * 6 + 0] as u16 + inp[i * 6 + 1] as u16,
                        256 * inp[i * 6 + 2] as u16 + inp[i * 6 + 3] as u16,
                        256 * inp[i * 6 + 4] as u16 + inp[i * 6 + 5] as u16,
                    )) {
                    0
                } else {
                    255
                },
            )
        },
        ColorType::PALETTE => {
            let index = if mode.bitdepth() == 8 {
                inp[i] as usize
            } else {
                let mut j = i as usize * mode.bitdepth() as usize;
                read_bits_from_reversed_stream(&mut j, inp, mode.bitdepth() as usize) as usize
            };
            let pal = mode.palette();
            if index >= pal.len() {
                /*This is an error according to the PNG spec, but common PNG decoders make it black instead.
                  Done here too, slightly faster due to no error handling needed.*/
                (0, 0, 0, 255)
            } else {
                let p = pal[index];
                (p.r, p.g, p.b, p.a)
            }
        },
        ColorType::GREY_ALPHA => if mode.bitdepth() == 8 {
            let t = inp[i * 2 + 0];
            (t, t, t, inp[i * 2 + 1])
        } else {
            let t = inp[i * 4 + 0];
            (t, t, t, inp[i * 4 + 2])
        },
        ColorType::RGBA => if mode.bitdepth() == 8 {
            (inp[i * 4 + 0], inp[i * 4 + 1], inp[i * 4 + 2], inp[i * 4 + 3])
        } else {
            (inp[i * 8 + 0], inp[i * 8 + 2], inp[i * 8 + 4], inp[i * 8 + 6])
        },
        ColorType::BGRA => {
            (inp[i * 4 + 2], inp[i * 4 + 1], inp[i * 4 + 0], inp[i * 4 + 3])
        },
        ColorType::BGR => {
            let b = inp[i * 3 + 0];
            let g = inp[i * 3 + 1];
            let r = inp[i * 3 + 2];
            let a = if mode.key() == Some((u16::from(r), u16::from(g), u16::from(b))) {
                0
            } else {
                255
            };
            (r, g, b, a)
        },
        ColorType::BGRX => {
            let b = inp[i * 4 + 0];
            let g = inp[i * 4 + 1];
            let r = inp[i * 4 + 2];
            let a = if mode.key() == Some((u16::from(r), u16::from(g), u16::from(b))) {
                0
            } else {
                255
            };
            (r, g, b, a)
        }
    }
}
/*Similar to get_pixel_color_rgba8, but with all the for loops inside of the color
mode test cases, optimized to convert the colors much faster, when converting
to RGBA or RGB with 8 bit per cannel. buffer must be RGBA or RGB output with
enough memory, if has_alpha is true the output is RGBA. mode has the color mode
of the input buffer.*/
fn get_pixel_colors_rgba8(buffer: &mut [u8], numpixels: usize, has_alpha: bool, inp: &[u8], mode: &ColorMode) {
    let num_channels = if has_alpha { 4 } else { 3 };
    match mode.colortype {
        ColorType::GREY => {
            if mode.bitdepth() == 8 {
                for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                    buffer[0] = inp[i];
                    buffer[1] = inp[i];
                    buffer[2] = inp[i];
                    if has_alpha {
                        let a = inp[i] as u16;
                        buffer[3] = if mode.key() == Some((a, a, a)) {
                            0
                        } else {
                            255
                        };
                    }
                }
            } else if mode.bitdepth() == 16 {
                for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                    buffer[0] = inp[i * 2];
                    buffer[1] = inp[i * 2];
                    buffer[2] = inp[i * 2];
                    if has_alpha {
                        let a = 256 * inp[i * 2 + 0] as u16 + inp[i * 2 + 1] as u16;
                        buffer[3] = if mode.key() == Some((a, a, a)) {
                            0
                        } else {
                            255
                        };
                    };
                }
            } else {
                let highest = (1 << mode.bitdepth()) - 1;
                /*highest possible value for this bit depth*/
                let mut j = 0;
                for buffer in buffer.chunks_mut(num_channels).take(numpixels) {
                    let value = read_bits_from_reversed_stream(&mut j, inp, mode.bitdepth() as usize);
                    buffer[0] = ((value * 255) / highest) as u8;
                    buffer[1] = ((value * 255) / highest) as u8;
                    buffer[2] = ((value * 255) / highest) as u8;
                    if has_alpha {
                        let a = value as u16;
                        buffer[3] = if mode.key() == Some((a, a, a)) {
                            0
                        } else {
                            255
                        };
                    };
                }
            };
        },
        ColorType::RGB => {
            if mode.bitdepth() == 8 {
                for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                    buffer[0] = inp[i * 3 + 0];
                    buffer[1] = inp[i * 3 + 1];
                    buffer[2] = inp[i * 3 + 2];
                    if has_alpha {
                        buffer[3] = if mode.key() == Some((buffer[0] as u16, buffer[1] as u16, buffer[2] as u16)) {
                            0
                        } else {
                            255
                        };
                    };
                }
            } else {
                for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                    buffer[0] = inp[i * 6 + 0];
                    buffer[1] = inp[i * 6 + 2];
                    buffer[2] = inp[i * 6 + 4];
                    if has_alpha {
                        let r = 256 * inp[i * 6 + 0] as u16 + inp[i * 6 + 1] as u16;
                        let g = 256 * inp[i * 6 + 2] as u16 + inp[i * 6 + 3] as u16;
                        let b = 256 * inp[i * 6 + 4] as u16 + inp[i * 6 + 5] as u16;
                        buffer[3] = if mode.key() == Some((r, g, b)) {
                            0
                        } else {
                            255
                        };
                    };
                }
            };
        },
        ColorType::PALETTE => {
            let mut j = 0;
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                let index = if mode.bitdepth() == 8 {
                    inp[i] as usize
                } else {
                    read_bits_from_reversed_stream(&mut j, inp, mode.bitdepth() as usize) as usize
                };
                let pal = mode.palette();
                if index >= pal.len() {
                    /*This is an error according to the PNG spec, but most PNG decoders make it black instead.
                        Done here too, slightly faster due to no error handling needed.*/
                    buffer[0] = 0;
                    buffer[1] = 0;
                    buffer[2] = 0;
                    if has_alpha {
                        buffer[3] = 255u8;
                    }
                } else {
                    let p = pal[index as usize];
                    buffer[0] = p.r;
                    buffer[1] = p.g;
                    buffer[2] = p.b;
                    if has_alpha {
                        buffer[3] = p.a;
                    }
                };
            }
        },
        ColorType::GREY_ALPHA => if mode.bitdepth() == 8 {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 2 + 0];
                buffer[1] = inp[i * 2 + 0];
                buffer[2] = inp[i * 2 + 0];
                if has_alpha {
                    buffer[3] = inp[i * 2 + 1];
                };
            }
        } else {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 4 + 0];
                buffer[1] = inp[i * 4 + 0];
                buffer[2] = inp[i * 4 + 0];
                if has_alpha {
                    buffer[3] = inp[i * 4 + 2];
                };
            }
        },
        ColorType::RGBA => if mode.bitdepth() == 8 {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 4 + 0];
                buffer[1] = inp[i * 4 + 1];
                buffer[2] = inp[i * 4 + 2];
                if has_alpha {
                    buffer[3] = inp[i * 4 + 3];
                }
            }
        } else {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 8 + 0];
                buffer[1] = inp[i * 8 + 2];
                buffer[2] = inp[i * 8 + 4];
                if has_alpha {
                    buffer[3] = inp[i * 8 + 6];
                }
            }
        },
        ColorType::BGR => {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 3 + 2];
                buffer[1] = inp[i * 3 + 1];
                buffer[2] = inp[i * 3 + 0];
                if has_alpha {
                    buffer[3] = if mode.key() == Some((buffer[0] as u16, buffer[1] as u16, buffer[2] as u16)) {
                        0
                    } else {
                        255
                    };
                };
            }
        },
        ColorType::BGRX => {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 4 + 2];
                buffer[1] = inp[i * 4 + 1];
                buffer[2] = inp[i * 4 + 0];
                if has_alpha {
                    buffer[3] = if mode.key() == Some((buffer[0] as u16, buffer[1] as u16, buffer[2] as u16)) {
                        0
                    } else {
                        255
                    };
                };
            }
        },
        ColorType::BGRA => {
            for (i, buffer) in buffer.chunks_mut(num_channels).take(numpixels).enumerate() {
                buffer[0] = inp[i * 4 + 2];
                buffer[1] = inp[i * 4 + 1];
                buffer[2] = inp[i * 4 + 0];
                if has_alpha {
                    buffer[3] = inp[i * 4 + 3];
                }
            }
        },
    };
}
/*Get RGBA16 color of pixel with index i (y * width + x) from the raw image with
given color type, but the given color type must be 16-bit itself.*/
#[inline(always)]
fn get_pixel_color_rgba16(inp: &[u8], i: usize, mode: &ColorMode) -> (u16, u16, u16, u16) {
    match mode.colortype {
        ColorType::GREY => {
            let t = 256 * inp[i * 2 + 0] as u16 + inp[i * 2 + 1] as u16;
            (t,t,t,
            if mode.key() == Some((t,t,t)) {
                0
            } else {
                0xffff
            })
        },
        ColorType::RGB => {
            let r = 256 * inp[i * 6 + 0] as u16 + inp[i * 6 + 1] as u16;
            let g = 256 * inp[i * 6 + 2] as u16 + inp[i * 6 + 3] as u16;
            let b = 256 * inp[i * 6 + 4] as u16 + inp[i * 6 + 5] as u16;
            let a = if mode.key() == Some((r, g, b)) {
                0
            } else {
                0xffff
            };
            (r, g, b, a)
        },
        ColorType::GREY_ALPHA => {
            let t = 256 * inp[i * 4 + 0] as u16 + inp[i * 4 + 1] as u16;
            let a = 256 * inp[i * 4 + 2] as u16 + inp[i * 4 + 3] as u16;
            (t, t, t, a)
        },
        ColorType::RGBA => (
            256 * inp[i * 8 + 0] as u16 + inp[i * 8 + 1] as u16,
            256 * inp[i * 8 + 2] as u16 + inp[i * 8 + 3] as u16,
            256 * inp[i * 8 + 4] as u16 + inp[i * 8 + 5] as u16,
            256 * inp[i * 8 + 6] as u16 + inp[i * 8 + 7] as u16,
        ),
        ColorType::BGR |
        ColorType::BGRA |
        ColorType::BGRX |
        ColorType::PALETTE => unreachable!(),
    }
}

#[inline(always)]
fn read_bits_from_reversed_stream(bitpointer: &mut usize, bitstream: &[u8], nbits: usize) -> u32 {
    let mut result = 0;
    for _ in 0..nbits {
        result <<= 1;
        result |= read_bit_from_reversed_stream(bitpointer, bitstream) as u32;
    }
    result
}

fn read_chunk_plte(color: &mut ColorMode, data: &[u8]) -> Result<(), PNGError> {
    color.palette_clear();
    for c in data.chunks(3).take(data.len() / 3) {
        color.palette_add(RGBA {
            r: c[0],
            g: c[1],
            b: c[2],
            a: 255,
        })?;
    }
    Ok(())
}

fn read_chunk_trns(color: &mut ColorMode, data: &[u8]) -> Result<(), PNGError> {
    if color.colortype == ColorType::PALETTE {
        let pal = color.palette_mut();
        if data.len() > pal.len() {
            return Err(PNGError::new(38));
        }
        for (i, &d) in data.iter().enumerate() {
            pal[i].a = d;
        }
    } else if color.colortype == ColorType::GREY {
        if data.len() != 2 {
            return Err(PNGError::new(30));
        }
        let t = 256 * data[0] as u16 + data[1] as u16;
        color.set_key(t, t, t);
    } else if color.colortype == ColorType::RGB {
        if data.len() != 6 {
            return Err(PNGError::new(41));
        }
        color.set_key(
            256 * data[0] as u16 + data[1] as u16,
            256 * data[2] as u16 + data[3] as u16,
            256 * data[4] as u16 + data[5] as u16,
        );
    } else {
        return Err(PNGError::new(42));
    }
    Ok(())
}

/*background color chunk (bKGD)*/
fn read_chunk_bkgd(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    let chunk_length = data.len();
    if info.color.colortype == ColorType::PALETTE {
        /*error: this chunk must be 1 byte for indexed color image*/
        if chunk_length != 1 {
            return Err(PNGError::new(43)); /*error: this chunk must be 2 bytes for greyscale image*/
        } /*error: this chunk must be 6 bytes for greyscale image*/
        info.background_defined = true; /* OK */
        info.background_r = {
            info.background_g = {
                info.background_b = data[0].into();
                info.background_b
            };
            info.background_g
        };
    } else if info.color.colortype == ColorType::GREY || info.color.colortype == ColorType::GREY_ALPHA {
        if chunk_length != 2 {
            return Err(PNGError::new(44));
        }
        info.background_defined = true;
        info.background_r = {
            info.background_g = {
                info.background_b = 256 * data[0] as u16 + data[1] as u16;
                info.background_b
            };
            info.background_g
        };
    } else if info.color.colortype == ColorType::RGB || info.color.colortype == ColorType::RGBA {
        if chunk_length != 6 {
            return Err(PNGError::new(45));
        }
        info.background_defined = true;
        info.background_r = 256 * data[0] as u16 + data[1] as u16;
        info.background_g = 256 * data[2] as u16 + data[3] as u16;
        info.background_b = 256 * data[4] as u16 + data[5] as u16;
    }
    Ok(())
}
/*text chunk (tEXt)*/
fn read_chunk_text(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    let (keyword, str) = split_at_nul(data);
    if keyword.is_empty() || keyword.len() > 79 {
        return Err(PNGError::new(89));
    }
    /*even though it's not allowed by the standard, no error is thrown if
        there's no null termination char, if the text is empty*/
    info.push_text(keyword, str)
}

/*compressed text chunk (zTXt)*/
fn read_chunk_ztxt(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    let mut length = 0;
    while length < data.len() && data[length] != 0 {
        length += 1
    }
    if length + 2 >= data.len() {
        return Err(PNGError::new(75));
    }
    if length < 1 || length > 79 {
        return Err(PNGError::new(89));
    }
    let key = &data[0..length];
    if data[length + 1] != 0 {
        return Err(PNGError::new(72));
    }
    /*the 0 byte indicating compression must be 0*/
    let string2_begin = length + 2; /*no null termination, corrupt?*/
    if string2_begin > data.len() {
        return Err(PNGError::new(75)); /*will fail if zlib error, e.g. if length is too small*/
    }
    let inl = &data[string2_begin..];
    let decoded = zlib_decompress(inl)?;
    info.push_text(key, &decoded)?;
    Ok(())
}

fn split_at_nul(data: &[u8]) -> (&[u8], &[u8]) {
    let mut part = data.splitn(2, |&b| b == 0);
    (part.next().unwrap(), part.next().unwrap_or(&data[0..0]))
}

/*international text chunk (iTXt)*/
fn read_chunk_itxt(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    /*Quick check if the chunk length isn't too small. Even without check
        it'd still fail with other error checks below if it's too short. This just gives a different error code.*/
    if data.len() < 5 {
        /*iTXt chunk too short*/
        return Err(PNGError::new(30));
    }

    let (key, data) = split_at_nul(data);
    if key.is_empty() || key.len() > 79 {
        return Err(PNGError::new(89));
    }
    if data.len() < 2 {
        return Err(PNGError::new(75));
    }
    let compressed_flag = data[0] != 0;
    if data[1] != 0 {
        return Err(PNGError::new(72));
    }
    let (langtag, data) = split_at_nul(&data[2..]);
    let (transkey, data) = split_at_nul(data);

    let decoded;
    let rest = if compressed_flag {
        decoded = zlib_decompress(data)?;
        &decoded[..]
    } else {
        data
    };
    info.push_itext(key, langtag, transkey, rest)?;
    Ok(())
}

fn read_chunk_time(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    let chunk_length = data.len();
    if chunk_length != 7 {
        return Err(PNGError::new(73));
    }
    info.time_defined = true;
    info.time.year = 256 * data[0] as u16 + data[1] as u16;
    info.time.month = data[2];
    info.time.day = data[3];
    info.time.hour = data[4];
    info.time.minute = data[5];
    info.time.second = data[6];
    Ok(())
}

fn read_chunk_phys(info: &mut Info, data: &[u8]) -> Result<(), PNGError> {
    let chunk_length = data.len();
    if chunk_length != 9 {
        return Err(PNGError::new(74));
    }
    info.phys_defined = true;
    info.phys_x = 16777216 * data[0] as u32 + 65536 * data[1] as u32 + 256 * data[2] as u32 + data[3] as u32;
    info.phys_y = 16777216 * data[4] as u32 + 65536 * data[5] as u32 + 256 * data[6] as u32 + data[7] as u32;
    info.phys_unit = data[8];
    Ok(())
}

fn add_chunk_idat(out: &mut Vec<u8>, data: &[u8]) -> Result<(), PNGError> {
    let zlib = zlib_compress(data)?;
    add_chunk(out, b"IDAT", &zlib)?;
    Ok(())
}

fn add_chunk_iend(out: &mut Vec<u8>) -> Result<(), PNGError> {
    add_chunk(out, b"IEND", &[])
}

fn add_chunk_text(out: &mut Vec<u8>, keyword: &[u8], textstring: &[u8]) -> Result<(), PNGError> {
    if keyword.is_empty() || keyword.len() > 79 {
        return Err(PNGError::new(89));
    }
    let mut text = Vec::try_with_capacity(keyword.len() + 1 + textstring.len())?;
    text.extend_from_slice(keyword);
    text.push(0);
    text.extend_from_slice(textstring);
    add_chunk(out, b"tEXt", &text)
}

fn add_chunk_ztxt(out: &mut Vec<u8>, keyword: &[u8], textstring: &[u8]) -> Result<(), PNGError> {
    if keyword.is_empty() || keyword.len() > 79 {
        return Err(PNGError::new(89));
    }
    let v = zlib_compress(textstring)?;
    let mut data = Vec::try_with_capacity(keyword.len() + 2 + v.len())?;
    data.extend_from_slice(keyword);
    data.push(0u8);
    data.push(0u8);
    data.extend_from_slice(&v);
    add_chunk(out, b"zTXt", &data)?;
    Ok(())
}

fn add_chunk_itxt(
    out: &mut Vec<u8>, compressed: bool, keyword: &str, langtag: &str, transkey: &str, textstring: &str,
) -> Result<(), PNGError> {
    let k_len = keyword.len();
    if k_len < 1 || k_len > 79 {
        return Err(PNGError::new(89));
    }
    let mut data = Vec::with_capacity(2048);
    data.extend_from_slice(keyword.as_bytes()); data.push(0);
    data.push(compressed as u8);
    data.push(0);
    data.extend_from_slice(langtag.as_bytes()); data.push(0);
    data.extend_from_slice(transkey.as_bytes()); data.push(0);
    if compressed {
        let compressed_data = zlib_compress(textstring.as_bytes())?;
        data.extend_from_slice(&compressed_data);
    } else {
        data.extend_from_slice(textstring.as_bytes());
    }
    add_chunk(out, b"iTXt", &data)
}

fn add_chunk_bkgd(out: &mut Vec<u8>, info: &Info) -> Result<(), PNGError> {
    let mut bkgd = Vec::with_capacity(16);
    if info.color.colortype == ColorType::GREY || info.color.colortype == ColorType::GREY_ALPHA {
        bkgd.push((info.background_r >> 8) as u8);
        bkgd.push((info.background_r & 255) as u8);
    } else if info.color.colortype == ColorType::RGB || info.color.colortype == ColorType::RGBA {
        bkgd.push((info.background_r >> 8) as u8);
        bkgd.push((info.background_r & 255) as u8);
        bkgd.push((info.background_g >> 8) as u8);
        bkgd.push((info.background_g & 255) as u8);
        bkgd.push((info.background_b >> 8) as u8);
        bkgd.push((info.background_b & 255) as u8);
    } else if info.color.colortype == ColorType::PALETTE {
        bkgd.push((info.background_r & 255) as u8);
    }
    add_chunk(out, b"bKGD", &bkgd)
}

fn add_chunk_ihdr(out: &mut Vec<u8>, w: usize, h: usize, colortype: ColorType, bitdepth: usize, interlace_method: u8) -> Result<(), PNGError> {
    let mut header = Vec::with_capacity(16);
    add32bit_int(&mut header, w as u32);
    add32bit_int(&mut header, h as u32);
    header.push(bitdepth as u8);
    header.push(colortype as u8);
    header.push(0u8);
    header.push(0u8);
    header.push(interlace_method);
    add_chunk(out, b"IHDR", &header)
}

fn add_chunk_trns(out: &mut Vec<u8>, info: &ColorMode) -> Result<(), PNGError> {
    let mut trns = Vec::with_capacity(32);
    if info.colortype == ColorType::PALETTE {
        let palette = info.palette();
        let mut amount = palette.len();
        /*the tail of palette values that all have 255 as alpha, does not have to be encoded*/
        let mut i = palette.len();
        while i != 0 {
            if palette[i - 1].a == 255 {
                amount -= 1;
            } else {
                break;
            };
            i -= 1;
        }
        for p in &palette[0..amount] {
            trns.push(p.a);
        }
    } else if info.colortype == ColorType::GREY {
        if let Some((r, _, _)) = info.key() {
            trns.push((r >> 8) as u8);
            trns.push((r & 255) as u8);
        };
    } else if info.colortype == ColorType::RGB {
        if let Some((r, g, b)) = info.key() {
            trns.push((r >> 8) as u8);
            trns.push((r & 255) as u8);
            trns.push((g >> 8) as u8);
            trns.push((g & 255) as u8);
            trns.push((b >> 8) as u8);
            trns.push((b & 255) as u8);
        };
    }
    add_chunk(out, b"tRNS", &trns)
}

fn add_chunk_plte(out: &mut Vec<u8>, info: &ColorMode) -> Result<(), PNGError> {
    let mut plte = Vec::with_capacity(1024);
    for p in info.palette() {
        plte.push(p.r);
        plte.push(p.g);
        plte.push(p.b);
    }
    add_chunk(out, b"PLTE", &plte)
}

fn add_chunk_time(out: &mut Vec<u8>, time: &Time) -> Result<(), PNGError> {
    let data = [
        (time.year >> 8) as u8,
        (time.year & 255) as u8,
        time.month as u8,
        time.day as u8,
        time.hour as u8,
        time.minute as u8,
        time.second as u8,
    ];
    add_chunk(out, b"tIME", &data)
}

fn add_chunk_phys(out: &mut Vec<u8>, info: &Info) -> Result<(), PNGError> {
    let mut data = Vec::with_capacity(16);
    add32bit_int(&mut data, info.phys_x);
    add32bit_int(&mut data, info.phys_y);
    data.push(info.phys_unit as u8);
    add_chunk(out, b"pHYs", &data)
}

/*chunk_name must be string of 4 characters*/
pub(crate) fn add_chunk(out: &mut Vec<u8>, type_: &[u8; 4], data: &[u8]) -> Result<(), PNGError> {
    let length = data.len() as usize;
    if length > (1 << 31) {
        return Err(PNGError::new(77));
    }
    let previous_length = out.len();
    if Vec::try_reserve(out, length + 12).is_err(){
        return Err(PNGError::new(95))
    }
    /*1: length*/
    lodepng_add32bit_int(out, length as u32);
    /*2: chunk name (4 letters)*/
    out.extend_from_slice(&type_[..]);
    /*3: the data*/
    out.extend_from_slice(data);
    /*4: CRC (of the chunkname characters and the data)*/
    lodepng_add32bit_int(out, 0);
    lodepng_chunk_generate_crc(&mut out[previous_length..]);
    Ok(())
}

/*shared values used by multiple Adam7 related functions*/
pub const ADAM7_IX: [u32; 7] = [0, 4, 0, 2, 0, 1, 0];
/*x start values*/
pub const ADAM7_IY: [u32; 7] = [0, 0, 4, 0, 2, 0, 1];
/*y start values*/
pub const ADAM7_DX: [u32; 7] = [8, 8, 4, 4, 2, 2, 1];
/*x delta values*/
pub const ADAM7_DY: [u32; 7] = [8, 8, 8, 4, 4, 2, 2];

fn adam7_get_pass_values(w: usize, h: usize, bpp: usize) -> ([u32; 7], [u32; 7], [usize; 8], [usize; 8], [usize; 8]) {
    let mut passw: [u32; 7] = [0; 7];
    let mut passh: [u32; 7] = [0; 7];
    let mut filter_passstart: [usize; 8] = [0; 8];
    let mut padded_passstart: [usize; 8] = [0; 8];
    let mut passstart: [usize; 8] = [0; 8];

    /*the passstart values have 8 values: the 8th one indicates the byte after the end of the 7th (= last) pass*/
    /*calculate width and height in pixels of each pass*/
    for i in 0..7 {
        passw[i] = (w as u32 + ADAM7_DX[i] - ADAM7_IX[i] - 1) / ADAM7_DX[i]; /*if passw[i] is 0, it's 0 bytes, not 1 (no filter_type-byte)*/
        passh[i] = (h as u32 + ADAM7_DY[i] - ADAM7_IY[i] - 1) / ADAM7_DY[i]; /*bits padded if needed to fill full byte at end of each scanline*/
        if passw[i] == 0 {
            passh[i] = 0; /*only padded at end of reduced image*/
        }
        if passh[i] == 0 {
            passw[i] = 0;
        };
    }
    filter_passstart[0] = 0;
    padded_passstart[0] = 0;
    passstart[0] = 0;
    for i in 0..7 {
        filter_passstart[i + 1] = filter_passstart[i] + if passw[i] != 0 && passh[i] != 0 {
            passh[i] as usize * (1 + (passw[i] as usize * bpp + 7) / 8)
        } else {
            0
        };
        padded_passstart[i + 1] = padded_passstart[i] + passh[i] as usize * ((passw[i] as usize * bpp + 7) / 8) as usize;
        passstart[i + 1] = passstart[i] + (passh[i] as usize * passw[i] as usize * bpp + 7) / 8;
    }
    (passw, passh, filter_passstart, padded_passstart, passstart)
}

/*
in: Adam7 interlaced image, with no padding bits between scanlines, but between
 reduced images so that each reduced image starts at a byte.
out: the same pixels, but re-ordered so that they're now a non-interlaced image with size w*h
bpp: bits per pixel
out has the following size in bits: w * h * bpp.
in is possibly bigger due to padding bits between reduced images.
out must be big enough AND must be 0 everywhere if bpp < 8 in the current implementation
(because that's likely a little bit faster)
NOTE: comments about padding bits are only relevant if bpp < 8
*/
fn adam7_deinterlace(out: &mut [u8], inp: &[u8], w: usize, h: usize, bpp: usize) {
    let (passw, passh, _, _, passstart) = adam7_get_pass_values(w, h, bpp);
    if bpp >= 8 {
        for i in 0..7 {
            let bytewidth = bpp / 8;
            for y in 0..passh[i] {
                for x in 0..passw[i] {
                    let pixelinstart = passstart[i] + (y * passw[i] + x) as usize * bytewidth;
                    let pixeloutstart = ((ADAM7_IY[i] + y * ADAM7_DY[i]) as usize * w + ADAM7_IX[i] as usize + x as usize * ADAM7_DX[i] as usize) * bytewidth;

                    out[pixeloutstart..(bytewidth + pixeloutstart)]
                        .clone_from_slice(&inp[pixelinstart..(bytewidth + pixelinstart)])
                }
            }
        }
    } else {
        for i in 0..7 {
            let ilinebits = bpp * passw[i] as usize;
            let olinebits = bpp * w;
            for y in 0..passh[i] as usize {
                for x in 0..passw[i] as usize {
                    let mut ibp = (8 * passstart[i]) + (y * ilinebits + x * bpp) as usize;
                    let mut obp = ((ADAM7_IY[i] as usize + y * ADAM7_DY[i] as usize) * olinebits + (ADAM7_IX[i] as usize + x * ADAM7_DX[i] as usize) * bpp) as usize;
                    for _ in 0..bpp {
                        let bit = read_bit_from_reversed_stream(&mut ibp, inp);
                        /*note that this function assumes the out buffer is completely 0, use set_bit_of_reversed_stream otherwise*/
                        set_bit_of_reversed_stream0(&mut obp, out, bit);
                    }
                }
            }
        }
    };
}

/* ////////////////////////////////////////////////////////////////////////// */
/* / Reading and writing single bits and bytes from/to stream for LodePNG   / */
/* ////////////////////////////////////////////////////////////////////////// */
#[inline(always)]
fn read_bit_from_reversed_stream(bitpointer: &mut usize, bitstream: &[u8]) -> u8 {
    let result = ((bitstream[(*bitpointer) >> 3] >> (7 - ((*bitpointer) & 7))) & 1) as u8;
    *bitpointer += 1;
    result
}

fn set_bit_of_reversed_stream0(bitpointer: &mut usize, bitstream: &mut [u8], bit: u8) {
    /*the current bit in bitstream must be 0 for this to work*/
    if bit != 0 {
        /*earlier bit of huffman code is in a lesser significant bit of an earlier byte*/
        bitstream[(*bitpointer) >> 3] |= bit << (7 - ((*bitpointer) & 7));
    }
    *bitpointer += 1;
}

fn set_bit_of_reversed_stream(bitpointer: &mut usize, bitstream: &mut [u8], bit: u8) {
    /*the current bit in bitstream may be 0 or 1 for this to work*/
    if bit == 0 {
        bitstream[(*bitpointer) >> 3] &= (!(1 << (7 - ((*bitpointer) & 7)))) as u8;
    } else {
        bitstream[(*bitpointer) >> 3] |= 1 << (7 - ((*bitpointer) & 7));
    }
    *bitpointer += 1;
}
/* ////////////////////////////////////////////////////////////////////////// */
/* / PNG chunks                                                             / */
/* ////////////////////////////////////////////////////////////////////////// */
#[inline]
pub fn lodepng_chunk_length(chunk: &[u8]) -> usize {
    lodepng_read32bit_int(chunk) as usize
}

pub fn lodepng_chunk_generate_crc(chunk: &mut [u8]) {
    let ch = ChunkRef::new(chunk).unwrap();
    let length = ch.len();
    let crc = ch.crc();
    lodepng_set32bit_int(&mut chunk[8 + length..], crc);
}

#[inline]
pub(crate) fn chunk_append(out: &mut Vec<u8>, chunk: &[u8]) -> Result<(), PNGError> {
    let total_chunk_length = lodepng_chunk_length(chunk) as usize + 12;
    Ok(out.try_extend_from_slice(&chunk[0..total_chunk_length])?)
}

/* ////////////////////////////////////////////////////////////////////////// */
/* / Color types and such                                                   / */
/* ////////////////////////////////////////////////////////////////////////// */
fn check_png_color_validity(colortype: ColorType, bd: u32) -> Result<(), PNGError> {
    /*allowed color type / bits combination*/
    match colortype {
        ColorType::GREY => if !(bd == 1 || bd == 2 || bd == 4 || bd == 8 || bd == 16) {
            return Err(PNGError::new(37));
        },
        ColorType::PALETTE => if !(bd == 1 || bd == 2 || bd == 4 || bd == 8) {
            return Err(PNGError::new(37));
        },
        ColorType::RGB | ColorType::GREY_ALPHA | ColorType::RGBA => if !(bd == 8 || bd == 16) {
            return Err(PNGError::new(37));
        },
        _ => {
            return Err(PNGError::new(31))
        },
    }
    Ok(())
}
/// Internally BGRA is allowed
fn check_lode_color_validity(colortype: ColorType, bd: u32) -> Result<(), PNGError> {
    match colortype {
        ColorType::BGRA | ColorType::BGRX | ColorType::BGR if bd == 8 => {
            Ok(())
        },
        ct => check_png_color_validity(ct, bd),
    }
}

pub fn lodepng_color_mode_equal(a: &ColorMode, b: &ColorMode) -> bool {
    a.colortype == b.colortype &&
    a.bitdepth() == b.bitdepth() &&
    a.key() == b.key() &&
    a.palette() == b.palette()
}

pub fn lodepng_convert(out: &mut [u8], inp: &[u8], mode_out: &ColorMode, mode_in: &ColorMode, w: u32, h: u32) -> Result<(), PNGError> {
    let numpixels = w as usize * h as usize;
    if lodepng_color_mode_equal(mode_out, mode_in) {
        let numbytes = mode_in.raw_size(w, h);
        out[..numbytes].clone_from_slice(&inp[..numbytes]);
        return Ok(());
    }
    let mut tree = ColorTree::new();
    if mode_out.colortype == ColorType::PALETTE {
        let mut palette = mode_out.palette();
        let palsize = 1 << mode_out.bitdepth();
        /*if the user specified output palette but did not give the values, assume
            they want the values of the input color type (assuming that one is palette).
            Note that we never create a new palette ourselves.*/
        if palette.is_empty() {
            palette = mode_in.palette();
        }
        palette = &palette[0..palette.len().min(palsize)];
        for (i, p) in palette.iter().enumerate() {
            tree.insert((p.r, p.g, p.b, p.a), i as u16);
        }
    }
    if mode_in.bitdepth() == 16 && mode_out.bitdepth() == 16 {
        for i in 0..numpixels {
            let (r, g, b, a) = get_pixel_color_rgba16(inp, i, mode_in);
            rgba16_to_pixel(out, i, mode_out, r, g, b, a);
        }
    } else if mode_out.bitdepth() == 8 && mode_out.colortype == ColorType::RGBA {
        get_pixel_colors_rgba8(out, numpixels as usize, true, inp, mode_in);
    } else if mode_out.bitdepth() == 8 && mode_out.colortype == ColorType::RGB {
        get_pixel_colors_rgba8(out, numpixels as usize, false, inp, mode_in);
    } else {
        for i in 0..numpixels {
            let (r, g, b, a) = get_pixel_color_rgba8(inp, i, mode_in);
            rgba8_to_pixel(out, i, mode_out, &mut tree, r, g, b, a)?;
        }
    }
    Ok(())
}

/*out must be buffer big enough to contain full image, and in must contain the full decompressed data from
the IDAT chunks (with filter index bytes and possible padding bits)
return value is error*/
/*
  This function converts the filtered-padded-interlaced data into pure 2D image buffer with the PNG's colortype.
  Steps:
  *) if no Adam7: 1) unfilter 2) remove padding bits (= posible extra bits per scanline if bpp < 8)
  *) if adam7: 1) 7x unfilter 2) 7x remove padding bits 3) adam7_deinterlace
  NOTE: the in buffer will be overwritten with intermediate data!
  */
fn postprocess_scanlines(out: &mut [u8], inp: &mut [u8], w: usize, h: usize, info_png: &Info) -> Result<(), PNGError> {
    let bpp = info_png.color.bpp() as usize;
    if bpp == 0 {
        return Err(PNGError::new(31));
    }
    if info_png.interlace_method == 0 {
        if bpp < 8 && w as usize * bpp != ((w as usize * bpp + 7) / 8) * 8 {
            unfilter_aliased(inp, 0, 0, w, h, bpp)?;
            remove_padding_bits(out, inp, w as usize * bpp, ((w as usize * bpp + 7) / 8) * 8, h);
        } else {
            unfilter(out, inp, w, h, bpp)?;
        };
    } else {
        let (passw, passh, filter_passstart, padded_passstart, passstart) = adam7_get_pass_values(w, h, bpp);
        for i in 0..7 {
            unfilter_aliased(inp, padded_passstart[i], filter_passstart[i], passw[i] as usize, passh[i] as usize, bpp)?;
            if bpp < 8 {
                /*remove padding bits in scanlines; after this there still may be padding
                        bits between the different reduced images: each reduced image still starts nicely at a byte*/
                remove_padding_bits_aliased(
                    inp,
                    passstart[i],
                    padded_passstart[i],
                    passw[i] as usize * bpp,
                    ((passw[i] as usize * bpp + 7) / 8) * 8,
                    passh[i] as usize,
                );
            };
        }
        adam7_deinterlace(out, inp, w, h, bpp);
    }
    Ok(())
}

/*
  For PNG filter method 0
  this function unfilters a single image (e.g. without interlacing this is called once, with Adam7 seven times)
  out must have enough bytes allocated already, in must have the scanlines + 1 filter_type byte per scanline
  w and h are image dimensions or dimensions of reduced image, bpp is bits per pixel
  in and out are allowed to be the same memory address (but aren't the same size since in has the extra filter bytes)
  */
fn unfilter(out: &mut [u8], inp: &[u8], w: usize, h: usize, bpp: usize) -> Result<(), PNGError> {
    let mut prevline = None;

    /*bytewidth is used for filtering, is 1 when bpp < 8, number of bytes per pixel otherwise*/
    let bytewidth = (bpp + 7) / 8;
    let linebytes = (w * bpp + 7) / 8;
    let in_linebytes = 1 + linebytes; /*the extra filterbyte added to each row*/

    for (out_line, in_line) in out.chunks_mut(linebytes).zip(inp.chunks(in_linebytes)).take(h) {
        let filter_type = in_line[0];
        unfilter_scanline(out_line, &in_line[1..], prevline, bytewidth, filter_type, linebytes)?;
        prevline = Some(out_line);
    }
    Ok(())
}

fn unfilter_aliased(inout: &mut [u8], out_off: usize, in_off: usize, w: usize, h: usize, bpp: usize) -> Result<(), PNGError> {
    let mut prevline = None;
    /*bytewidth is used for filtering, is 1 when bpp < 8, number of bytes per pixel otherwise*/
    let bytewidth = (bpp + 7) / 8;
    let linebytes = (w * bpp + 7) / 8;
    for y in 0..h as usize {
        let outindex = linebytes * y;
        let inindex = (1 + linebytes) * y; /*the extra filterbyte added to each row*/
        let filter_type = inout[in_off + inindex];
        unfilter_scanline_aliased(inout, out_off + outindex, in_off + inindex + 1, prevline, bytewidth, filter_type, linebytes)?;
        prevline = Some(out_off + outindex);
    }
    Ok(())
}

/*
  For PNG filter method 0
  unfilter a PNG image scanline by scanline. when the pixels are smaller than 1 byte,
  the filter works byte per byte (bytewidth = 1)
  precon is the previous unfiltered scanline, recon the result, scanline the current one
  the incoming scanlines do NOT include the filter_type byte, that one is given in the parameter filter_type instead
  recon and scanline MAY be the same memory address! precon must be disjoint.
  */
fn unfilter_scanline(recon: &mut [u8], scanline: &[u8], precon: Option<&[u8]>, bytewidth: usize, filter_type: u8, length: usize) -> Result<(), PNGError> {
    match filter_type {
        0 => recon.clone_from_slice(scanline),
        1 => {
            recon[0..bytewidth].clone_from_slice(&scanline[0..bytewidth]);
            for i in bytewidth..length {
                recon[i] = scanline[i].wrapping_add(recon[i - bytewidth]);
            }
        },
        2 => if let Some(precon) = precon {
            for i in 0..length {
                recon[i] = scanline[i].wrapping_add(precon[i]);
            }
        } else {
            recon.clone_from_slice(scanline);
        },
        3 => if let Some(precon) = precon {
            for i in 0..bytewidth {
                recon[i] = scanline[i].wrapping_add(precon[i] >> 1);
            }
            for i in bytewidth..length {
                let t = recon[i - bytewidth] as u16 + precon[i] as u16;
                recon[i] = scanline[i].wrapping_add((t >> 1) as u8);
            }
        } else {
            recon[0..bytewidth].clone_from_slice(&scanline[0..bytewidth]);
            for i in bytewidth..length {
                recon[i] = scanline[i].wrapping_add(recon[i - bytewidth] >> 1);
            }
        },
        4 => if let Some(precon) = precon {
            for i in 0..bytewidth {
                recon[i] = scanline[i].wrapping_add(precon[i]);
            }
            for i in bytewidth..length {
                recon[i] = scanline[i].wrapping_add(paeth_predictor(
                    recon[i - bytewidth] as i16,
                    precon[i] as i16,
                    precon[i - bytewidth] as i16,
                ));
            }
        } else {
            recon[0..bytewidth].clone_from_slice(&scanline[0..bytewidth]);
            for i in bytewidth..length {
                recon[i] = scanline[i].wrapping_add(recon[i - bytewidth]);
            }
        },
        _ => return Err(PNGError::new(36)),
    }
    Ok(())
}

fn unfilter_scanline_aliased(inout: &mut [u8], recon: usize, scanline: usize, precon: Option<usize>, bytewidth: usize, filter_type: u8, length: usize) -> Result<(), PNGError> {
    match filter_type {
        0 => for i in 0..length {
            inout[recon + i] = inout[scanline + i];
        },
        1 => {
            for i in 0..bytewidth {
                inout[recon + i] = inout[scanline + i];
            }
            for i in bytewidth..length {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[recon + i - bytewidth]);
            }
        },
        2 => if let Some(precon) = precon {
            for i in 0..length {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[precon + i]);
            }
        } else {
            for i in 0..length {
                inout[recon + i] = inout[scanline + i];
            }
        },
        3 => if let Some(precon) = precon {
            for i in 0..bytewidth {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[precon + i] >> 1);
            }
            for i in bytewidth..length {
                let t = inout[recon + i - bytewidth] as u16 + inout[precon + i] as u16;
                inout[recon + i] = inout[scanline + i].wrapping_add((t >> 1) as u8);
            }
        } else {
            for i in 0..bytewidth {
                inout[recon + i] = inout[scanline + i];
            }
            for i in bytewidth..length {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[recon + i - bytewidth] >> 1);
            }
        },
        4 => if let Some(precon) = precon {
            for i in 0..bytewidth {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[precon + i]);
            }
            for i in bytewidth..length {
                inout[recon + i] = inout[scanline + i].wrapping_add(paeth_predictor(
                    inout[recon + i - bytewidth] as i16,
                    inout[precon + i] as i16,
                    inout[precon + i - bytewidth] as i16,
                ));
            }
        } else {
            for i in 0..bytewidth {
                inout[recon + i] = inout[scanline + i];
            }
            for i in bytewidth..length {
                inout[recon + i] = inout[scanline + i].wrapping_add(inout[recon + i - bytewidth]);
            }
        },
        _ => return Err(PNGError::new(36)),
    }
    Ok(())
}

/*
  After filtering there are still padding bits if scanlines have non multiple of 8 bit amounts. They need
  to be removed (except at last scanline of (Adam7-reduced) image) before working with pure image buffers
  for the Adam7 code, the color convert code and the output to the user.
  in and out are allowed to be the same buffer, in may also be higher but still overlapping; in must
  have >= ilinebits*h bits, out must have >= olinebits*h bits, olinebits must be <= ilinebits
  also used to move bits after earlier such operations happened, e.g. in a sequence of reduced images from Adam7
  only useful if (ilinebits - olinebits) is a value in the range 1..7
  */
fn remove_padding_bits(out: &mut [u8], inp: &[u8], olinebits: usize, ilinebits: usize, h: usize) {
    let diff = ilinebits - olinebits; /*input and output bit pointers*/
    let mut ibp = 0;
    let mut obp = 0;
    for _ in 0..h {
        for _ in 0..olinebits {
            let bit = read_bit_from_reversed_stream(&mut ibp, inp);
            set_bit_of_reversed_stream(&mut obp, out, bit);
        }
        ibp += diff;
    }
}

fn remove_padding_bits_aliased(inout: &mut [u8], out_off: usize, in_off: usize, olinebits: usize, ilinebits: usize, h: usize) {
    let diff = ilinebits - olinebits; /*input and output bit pointers*/
    let mut ibp = 0;
    let mut obp = 0;
    for _ in 0..h {
        for _ in 0..olinebits {
            let bit = read_bit_from_reversed_stream(&mut ibp, &inout[in_off..]);
            set_bit_of_reversed_stream(&mut obp, &mut inout[out_off..], bit);
        }
        ibp += diff;
    }
}

/*
in: non-interlaced image with size w*h
out: the same pixels, but re-ordered according to PNG's Adam7 interlacing, with
 no padding bits between scanlines, but between reduced images so that each
 reduced image starts at a byte.
bpp: bits per pixel
there are no padding bits, not between scanlines, not between reduced images
in has the following size in bits: w * h * bpp.
out is possibly bigger due to padding bits between reduced images
NOTE: comments about padding bits are only relevant if bpp < 8
*/
fn adam7_interlace(out: &mut [u8], inp: &[u8], w: usize, h: usize, bpp: usize) {
    let (passw, passh, _, _, passstart) = adam7_get_pass_values(w, h, bpp);
    let bpp = bpp;
    if bpp >= 8 {
        for i in 0..7 {
            let bytewidth = bpp / 8;
            for y in 0..passh[i] as usize {
                for x in 0..passw[i] as usize {
                    let pixelinstart = ((ADAM7_IY[i] as usize + y * ADAM7_DY[i] as usize) * w as usize + ADAM7_IX[i] as usize + x * ADAM7_DX[i] as usize) * bytewidth;
                    let pixeloutstart = passstart[i] + (y * passw[i] as usize + x) * bytewidth;
                    out[pixeloutstart..(bytewidth + pixeloutstart)]
                        .clone_from_slice(&inp[pixelinstart..(bytewidth + pixelinstart)]);
                }
            }
        }
    } else {
        for i in 0..7 {
            let ilinebits = bpp * passw[i] as usize;
            let olinebits = bpp * w;
            for y in 0..passh[i] as usize {
                for x in 0..passw[i] as usize {
                    let mut ibp = (ADAM7_IY[i] as usize + y * ADAM7_DY[i] as usize) * olinebits + (ADAM7_IX[i] as usize + x * ADAM7_DX[i] as usize) * bpp;
                    let mut obp = (8 * passstart[i]) + (y * ilinebits + x * bpp);
                    for _ in 0..bpp {
                        let bit = read_bit_from_reversed_stream(&mut ibp, inp);
                        set_bit_of_reversed_stream(&mut obp, out, bit);
                    }
                }
            }
        }
    };
}

/* ////////////////////////////////////////////////////////////////////////// */
/* / PNG Decoder                                                            / */
/* ////////////////////////////////////////////////////////////////////////// */
/*read the information from the header and store it in the Info. return value is error*/
pub fn lodepng_inspect(decoder: &DecoderSettings, inp: &[u8], read_chunks: bool) -> Result<(Info, usize, usize), PNGError> {
    if inp.len() < 33 {
        /*error: the data length is smaller than the length of a PNG header*/
        return Err(PNGError::new(27));
    }
    /*when decoding a new PNG image, make sure all parameters created after previous decoding are reset*/
    let mut info_png = Info::default();
    if inp[0..8] != [137, 80, 78, 71, 13, 10, 26, 10] {
        /*error: the first 8 bytes are not the correct PNG signature*/
        return Err(PNGError::new(28));
    }
    let mut chunks = ChunksIter { data: &inp[8..] };
    let ihdr = chunks.next().ok_or(PNGError::new(28))??;
    if &ihdr.name() != b"IHDR" {
        /*error: it doesn't start with a IHDR chunk!*/
        return Err(PNGError::new(29));
    }
    if ihdr.len() != 13 {
        /*error: header size must be 13 bytes*/
        return Err(PNGError::new(94));
    }
    /*read the values given in the header*/
    let w = lodepng_read32bit_int(&inp[16..]) as usize;
    let h = lodepng_read32bit_int(&inp[20..]) as usize;
    let bitdepth = inp[24];
    if bitdepth == 0 || bitdepth > 16 {
        return Err(PNGError::new(29));
    }
    info_png.color.set_bitdepth(inp[24] as u32);
    info_png.color.colortype = match inp[25] {
        0 => ColorType::GREY,
        2 => ColorType::RGB,
        3 => ColorType::PALETTE,
        4 => ColorType::GREY_ALPHA,
        6 => ColorType::RGBA,
        _ => return Err(PNGError::new(31)),
    };
    info_png.interlace_method = inp[28];
    if w == 0 || h == 0 {
        return Err(PNGError::new(93));
    }
    if !decoder.ignore_crc && !ihdr.check_crc() {
        return Err(PNGError::new(57));
    }
    if info_png.interlace_method > 1 {
        /*error: only interlace methods 0 and 1 exist in the specification*/
        return Err(PNGError::new(34));
    }
    if read_chunks {
        for ch in chunks {
            let ch = ch?;
            match &ch.name() {
                b"IDAT" | b"IEND" => break,
                b"PLTE" => {
                    read_chunk_plte(&mut info_png.color, ch.data())?;
                },
                b"tRNS" => {
                    read_chunk_trns(&mut info_png.color, ch.data())?;
                },
                b"bKGD" => {
                    read_chunk_bkgd(&mut info_png, ch.data())?;
                },
                _ => {},
            }
        }
    }
    check_png_color_validity(info_png.color.colortype, info_png.color.bitdepth())?;
    Ok((info_png, w, h))
}

/*read a PNG, the result will be in the same color type as the PNG (hence "generic")*/
fn decode_generic(state: &mut State, inp: &[u8]) -> Result<(Vec<u8>, usize, usize), PNGError> {
    let mut found_iend = false; /*the data from idat chunks*/
    /*for unknown chunk order*/
    let mut unknown = false;
    let mut critical_pos = ChunkPosition::IHDR;
    /*provide some proper output values if error will happen*/
    let (info, w, h) = lodepng_inspect(&state.decoder, inp, false)?;
    state.info_png = info;

    /*reads header and resets other parameters in state->info_png*/
    let numpixels = match w.checked_mul(h) {
        Some(n) => n,
        None => {
            return Err(PNGError::new(92));
        },
    };
    /*multiplication overflow possible further below. Allows up to 2^31-1 pixel
      bytes with 16-bit RGBA, the rest is room for filter bytes.*/
    if numpixels > (isize::MAX as usize - 1) / 4 / 2 {
        return Err(PNGError::new(92)); /*first byte of the first chunk after the header*/
    }
    let mut idat = Vec::try_with_capacity(inp.len() - 33)?;
    let chunks = ChunksIter {
        data: &inp[33..],
    };
    /*loop through the chunks, ignoring unknown chunks and stopping at IEND chunk.
      IDAT data is put at the start of the in buffer*/
    for ch in chunks {
        let ch = ch?;
        /*length of the data of the chunk, excluding the length bytes, chunk type and CRC bytes*/
        let data = ch.data();
        match &ch.name() {
            b"IDAT" => {
                idat.try_extend_from_slice(data)?;
                critical_pos = ChunkPosition::IDAT;
            },
            b"IEND" => {
                found_iend = true;
            },
            b"PLTE" => {
                read_chunk_plte(&mut state.info_png.color, data)?;
                critical_pos = ChunkPosition::PLTE;
            },
            b"tRNS" => {
                read_chunk_trns(&mut state.info_png.color, data)?;
            },
            b"bKGD" => {
                read_chunk_bkgd(&mut state.info_png, data)?;
            },
            b"tEXt" => if state.decoder.read_text_chunks {
                read_chunk_text(&mut state.info_png, data)?;
            },
            b"zTXt" => if state.decoder.read_text_chunks {
                read_chunk_ztxt(&mut state.info_png, data)?;
            },
            b"iTXt" => if state.decoder.read_text_chunks {
                read_chunk_itxt(&mut state.info_png, data)?;
            },
            b"tIME" => {
                read_chunk_time(&mut state.info_png, data)?;
            },
            b"pHYs" => {
                read_chunk_phys(&mut state.info_png, data)?;
            },
            _ => {
                if !ch.is_ancillary() {
                    return Err(PNGError::new(69));
                }
                unknown = true;
                if state.decoder.remember_unknown_chunks {
                    state.info_png.push_unknown_chunk(critical_pos, ch.whole_chunk_data())?;
                }
            },
        };
        if !state.decoder.ignore_crc && !unknown && !ch.check_crc() {
            return Err(PNGError::new(57));
        }
        if found_iend {
            break;
        }
    }
    /*predict output size, to allocate exact size for output buffer to avoid more dynamic allocation.
      If the decompressed size does not match the prediction, the image must be corrupt.*/
    let predict = if state.info_png.interlace_method == 0 {
        /*The extra *h is added because this are the filter bytes every scanline starts with*/
        state.info_png.color.raw_size_idat(w, h).ok_or(PNGError::new(91))? + h
    } else {
        /*Adam-7 interlaced: predicted size is the sum of the 7 sub-images sizes*/
        let color = &state.info_png.color;
        adam7_expected_size(color, w, h).ok_or(PNGError::new(91))?
    };
    let mut scanlines = zlib_decompress(&idat)?;
    if scanlines.len() != predict {
        /*decompressed size doesn't match prediction*/
        return Err(PNGError::new(91));
    }
    let mut out = zero_vec(state.info_png.color.raw_size(w as u32, h as u32))?;
    postprocess_scanlines(&mut out, &mut scanlines, w, h, &state.info_png)?;
    Ok((out, w, h))
}

fn adam7_expected_size(color: &ColorMode, w: usize, h: usize) -> Option<usize> {
    let mut predict = color.raw_size_idat((w + 7) >> 3, (h + 7) >> 3)? + ((h + 7) >> 3);
    if w > 4 {
        predict += color.raw_size_idat((w + 3) >> 3, (h + 7) >> 3)? + ((h + 7) >> 3);
    }
    predict += color.raw_size_idat((w + 3) >> 2, (h + 3) >> 3)? + ((h + 3) >> 3);
    if w > 2 {
        predict += color.raw_size_idat((w + 1) >> 2, (h + 3) >> 2)? + ((h + 3) >> 2);
    }
    predict += color.raw_size_idat((w + 1) >> 1, (h + 1) >> 2)? + ((h + 1) >> 2);
    if w > 1 {
        predict += color.raw_size_idat((w + 0) >> 1, (h + 1) >> 1)? + ((h + 1) >> 1);
    }
    predict += color.raw_size_idat(w + 0, (h + 0) >> 1)? + ((h + 0) >> 1);
    Some(predict)
}

fn add_unknown_chunks(out: &mut Vec<u8>, data: &[u8]) -> Result<(), PNGError> {
    let chunks = ChunksIter { data };
    for ch in chunks {
        chunk_append(out, ch?.whole_chunk_data())?;
    }
    Ok(())
}


/*profile must already have been inited with mode.
It's ok to set some parameters of profile to done already.*/
pub fn get_color_profile(inp: &[u8], w: u32, h: u32, mode: &ColorMode) -> Result<ColorProfile, PNGError> {
    let mut profile = ColorProfile::default();
    let numpixels: usize = w as usize * h as usize;
    let mut colored_done = mode.is_greyscale_type();
    let mut alpha_done = !mode.can_have_alpha();
    let mut numcolors_done = false;
    let bpp = mode.bpp() as usize;
    let mut bits_done = bpp == 1;
    let maxnumcolors = match bpp {
        1 => 2,
        2 => 4,
        4 => 16,
        5..=8 => 256,
        _ => 257,
    };

    /*Check if the 16-bit input is truly 16-bit*/
    let mut sixteen = false;
    if mode.bitdepth() == 16 {
        for i in 0..numpixels {
            let (r, g, b, a) = get_pixel_color_rgba16(inp, i, mode);
            if (r & 255) != ((r >> 8) & 255) || (g & 255) != ((g >> 8) & 255) || (b & 255) != ((b >> 8) & 255) || (a & 255) != ((a >> 8) & 255) {
                /*first and second byte differ*/
                sixteen = true;
                break;
            };
        }
    }
    if sixteen {
        profile.bits = 16;
        bits_done = true;
        numcolors_done = true;
        /*counting colors no longer useful, palette doesn't support 16-bit*/
        for i in 0..numpixels {
            let (r, g, b, a) = get_pixel_color_rgba16(inp, i, mode);
            if !colored_done && (r != g || r != b) {
                profile.colored = true;
                colored_done = true;
            }
            if !alpha_done {
                let matchkey = r == profile.key_r && g == profile.key_g && b == profile.key_b;
                if a != 65535 && (a != 0 || (profile.key && !matchkey)) {
                    profile.alpha = true;
                    profile.key = false;
                    alpha_done = true;
                } else if a == 0 && !profile.alpha && !profile.key {
                    profile.key = true;
                    profile.key_r = r;
                    profile.key_g = g;
                    profile.key_b = b;
                } else if a == 65535 && profile.key && matchkey {
                    profile.alpha = true;
                    profile.key = false;
                    alpha_done = true;
                };
            }
            if alpha_done && numcolors_done && colored_done && bits_done {
                break;
            };
        }
        if profile.key && !profile.alpha {
            for i in 0..numpixels {
                let (r, g, b, a) = get_pixel_color_rgba16(inp, i, mode);
                if a != 0 && r == profile.key_r && g == profile.key_g && b == profile.key_b {
                    profile.alpha = true;
                    profile.key = false;
                }
            }
        }
    } else {
        let mut tree = ColorTree::new();
        for i in 0..numpixels {
            let (r, g, b, a) = get_pixel_color_rgba8(inp, i, mode);
            if !bits_done && profile.bits < 8 {
                let bits = get_value_required_bits(r);
                if bits > profile.bits {
                    profile.bits = bits;
                };
            }
            bits_done = profile.bits as usize >= bpp;
            if !colored_done && (r != g || r != b) {
                profile.colored = true;
                colored_done = true;
                if profile.bits < 8 {
                    profile.bits = 8;
                };
                /*PNG has no colored modes with less than 8-bit per channel*/
            }
            if !alpha_done {
                let matchkey = r as u16 == profile.key_r && g as u16 == profile.key_g && b as u16 == profile.key_b;
                if a != 255 && (a != 0 || (profile.key && !matchkey)) {
                    profile.alpha = true;
                    profile.key = false;
                    alpha_done = true;
                    if profile.bits < 8 {
                        profile.bits = 8;
                    };
                /*PNG has no alphachannel modes with less than 8-bit per channel*/
                } else if a == 0 && !profile.alpha && !profile.key {
                    profile.key = true;
                    profile.key_r = r as u16;
                    profile.key_g = g as u16;
                    profile.key_b = b as u16;
                } else if a == 255 && profile.key && matchkey {
                    profile.alpha = true;
                    profile.key = false;
                    alpha_done = true;
                    if profile.bits < 8 {
                        profile.bits = 8;
                    };
                    /*PNG has no alphachannel modes with less than 8-bit per channel*/
                };
            }
            if !numcolors_done && tree.get(&(r, g, b, a)).is_none() {
                tree.insert((r, g, b, a), profile.numcolors as u16);
                if profile.numcolors < 256 {
                    profile.palette[profile.numcolors as usize] = RGBA { r, g, b, a };
                }
                profile.numcolors += 1;
                numcolors_done = profile.numcolors >= maxnumcolors;
            }
            if alpha_done && numcolors_done && colored_done && bits_done {
                break;
            };
        }
        if profile.key && !profile.alpha {
            for i in 0..numpixels {
                let (r, g, b, a) = get_pixel_color_rgba8(inp, i, mode);
                if a != 0 && r as u16 == profile.key_r && g as u16 == profile.key_g && b as u16 == profile.key_b {
                    profile.alpha = true;
                    profile.key = false;
                    /*PNG has no alphachannel modes with less than 8-bit per channel*/
                    if profile.bits < 8 {
                        profile.bits = 8;
                    };
                };
            }
        }
        /*make the profile's key always 16-bit for consistency - repeat each byte twice*/
        profile.key_r += profile.key_r << 8;
        profile.key_g += profile.key_g << 8;
        profile.key_b += profile.key_b << 8;
    }
    Ok(profile)
}

/*Automatically chooses color type that gives smallest amount of bits in the
output image, e.g. grey if there are only greyscale pixels, palette if there
are less than 256 colors, …
Updates values of mode with a potentially smaller color model. mode_out should
contain the user chosen color model, but will be overwritten with the new chosen one.*/
pub fn auto_choose_color(image: &[u8], w: usize, h: usize, mode_in: &ColorMode) -> Result<ColorMode, PNGError> {
    let mut mode_out = ColorMode::new();
    let mut prof = get_color_profile(image, w as u32, h as u32, mode_in)?;

    mode_out.clear_key();
    if prof.key && w * h <= 16 {
        prof.alpha = true;
        prof.key = false;
        /*PNG has no alphachannel modes with less than 8-bit per channel*/
        if prof.bits < 8 {
            prof.bits = 8;
        };
    }
    let n = prof.numcolors;
    let palettebits = if n <= 2 {
        1
    } else if n <= 4 {
        2
    } else if n <= 16 {
        4
    } else {
        8
    };
    let palette_ok = (n <= 256 && prof.bits <= 8) &&
        (w * h >= (n * 2) as usize) &&
        (prof.colored || prof.bits > palettebits);
    if palette_ok {
        let pal = &prof.palette[0..prof.numcolors as usize];
        /*remove potential earlier palette*/
        mode_out.palette_clear();
        for p in pal {
            mode_out.palette_add(*p)?;
        }
        mode_out.colortype = ColorType::PALETTE;
        mode_out.set_bitdepth(palettebits.into());
        if mode_in.colortype == ColorType::PALETTE && mode_in.palette().len() >= mode_out.palette().len() && mode_in.bitdepth() == mode_out.bitdepth() {
            /*If input should have same palette colors, keep original to preserve its order and prevent conversion*/
            mode_out = mode_in.clone();
        };
    } else {
        mode_out.set_bitdepth(prof.bits.into());
        mode_out.colortype = if prof.alpha {
            if prof.colored {
                ColorType::RGBA
            } else {
                ColorType::GREY_ALPHA
            }
        } else if prof.colored {
            ColorType::RGB
        } else {
            ColorType::GREY
        };
        if prof.key {
            let mask = ((1 << mode_out.bitdepth()) - 1) as u16;
            /*profile always uses 16-bit, mask converts it*/
            mode_out.set_key(
                prof.key_r as u16 & mask,
                prof.key_g as u16 & mask,
                prof.key_b as u16 & mask);
        };
    }
    Ok(mode_out)
}


/*Returns how many bits needed to represent given value (max 8 bit)*/
fn get_value_required_bits(value: u8) -> u8 {
    match value {
        0 | 255 => 1,
        x if x % 17 == 0 => {
            /*The scaling of 2-bit and 4-bit values uses multiples of 85 and 17*/
            if value % 85 == 0 { 2 } else { 4 }
        },
        _ => 8,
    }
}
