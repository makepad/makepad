//! Pictures in a conversation: what a person's line carries ([`PictureRef`]), and the picture
//! budget of a tool result: pictures scaled down alike until their PNGs fit it
//! ([`fit_pictures`]). The scaling and the encoding are CPU work: run them off the UI thread.

use makepad_micro_serde::*;
use makepad_zune_png::makepad_zune_core::{
    bit_depth::BitDepth,
    bytestream::ZCursor,
    colorspace::ColorSpace,
    options::{DecoderOptions, EncoderOptions},
};
use makepad_zune_png::{PngDecoder, PngEncoder};

/// A picture a person's line carried, as the transcript keeps it: the picture itself is a file
/// beside the conversation's saved transcript, named by the hash of its bytes.
#[derive(Clone, Debug, PartialEq, Eq, SerJson, DeJson)]
pub struct PictureRef {
    /// The name the person, the model and the app's tools know it by (`keys.webp`), one
    /// picture's in its conversation.
    pub name: String,
    /// Its file: the SHA-256 of its bytes in hex, and its format's extension.
    pub file: String,
    /// Its pixels, as it was dropped.
    pub width: u32,
    pub height: u32,
    /// Its file's bytes.
    pub bytes: u64,
}

/// RGBA8 pixels, top-down, tightly packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaPicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The base64 bytes of `bytes` bytes.
pub fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

/// A PNG of the picture: RGB when it is opaque (the alpha would only cost bytes), else RGBA.
pub fn encode_png(picture: &RgbaPicture) -> Result<Vec<u8>, String> {
    let (width, height) = (picture.width as usize, picture.height as usize);
    if width == 0 || height == 0 || picture.rgba.len() != width * height * 4 {
        return Err(format!("a {width} x {height} picture needs {} bytes of pixels, not {}", width * height * 4, picture.rgba.len()));
    }
    let opaque = picture.rgba.chunks_exact(4).all(|p| p[3] == 255);
    let rgb: Vec<u8>;
    let (pixels, space) = if opaque {
        rgb = picture.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        (&rgb[..], ColorSpace::RGB)
    } else {
        (&picture.rgba[..], ColorSpace::RGBA)
    };
    let mut out = Vec::new();
    PngEncoder::new(pixels, EncoderOptions::new(width, height, space, BitDepth::Eight))
        .encode(&mut out)
        .map_err(|e| format!("the PNG does not encode: {e:?}"))?;
    Ok(out)
}

/// The pixels of a PNG (8 or 16 bits, grey or colour, with or without alpha).
pub fn decode_png(bytes: &[u8]) -> Result<RgbaPicture, String> {
    let options = DecoderOptions::default()
        .set_max_width(1 << 14)
        .set_max_height(1 << 14)
        .set_strict_mode(true)
        .png_set_confirm_crc(true)
        .png_set_strip_to_8bit(true)
        .png_set_decode_animated(false);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(|e| format!("the PNG does not read: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("the PNG has no size")?;
    let raw = decoder.decode_raw().map_err(|e| format!("the PNG does not read: {e:?}"))?;
    let pixels = width * height;
    let rgba = match decoder.colorspace() {
        Some(ColorSpace::RGBA) if raw.len() == pixels * 4 => raw,
        Some(ColorSpace::RGB) if raw.len() == pixels * 3 => raw.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        Some(ColorSpace::Luma) if raw.len() == pixels => raw.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        Some(ColorSpace::LumaA) if raw.len() == pixels * 2 => raw.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        _ => return Err("the PNG's pixel format is not one read here".into()),
    };
    Ok(RgbaPicture { width: width as u32, height: height as u32, rgba })
}

/// The region `[x0 y0 x1 y1]` (fractions of the picture's width and height, x0 < x1 and y0 < y1,
/// each 0 to 1) in whole pixels of a `width` x `height` picture: the pixels it touches, at least one.
pub fn rect_pixels(width: u32, height: u32, [x0, y0, x1, y1]: [f64; 4]) -> [u32; 4] {
    let at = |f: f64, side: u32, up: bool| ((if up { (f * side as f64).ceil() } else { (f * side as f64).floor() }) as u32).min(side);
    let (left, top) = (at(x0, width, false).min(width.saturating_sub(1)), at(y0, height, false).min(height.saturating_sub(1)));
    [left, top, at(x1, width, true).max(left + 1), at(y1, height, true).max(top + 1)]
}

/// The part of the picture a region of fractions covers ([`rect_pixels`]), at its own pixels.
pub fn crop(picture: &RgbaPicture, rect: [f64; 4]) -> RgbaPicture {
    let [x0, y0, x1, y1] = rect_pixels(picture.width, picture.height, rect);
    let row = picture.width as usize * 4;
    let rgba = (y0 as usize..y1 as usize).flat_map(|y| &picture.rgba[y * row + x0 as usize * 4..y * row + x1 as usize * 4]).copied().collect();
    RgbaPicture { width: x1 - x0, height: y1 - y0, rgba }
}

/// One picture for the model: no larger than `max_side` pixels a side, and its PNG's base64 within
/// `budget` bytes, as large as both let it be.
pub fn fit_picture(picture: RgbaPicture, max_side: u32, budget: usize) -> Result<Vec<u8>, String> {
    let longest = picture.width.max(picture.height);
    let picture = if longest > max_side {
        let scale = max_side as f64 / longest as f64;
        let side = |s: u32| ((s as f64 * scale).round() as u32).clamp(1, max_side);
        area_average(&picture, side(picture.width), side(picture.height))
    } else {
        picture
    };
    let mut pngs = fit_pictures(&[(picture, [1, 1])], budget, max_side, |_, _, _| {})?;
    Ok(pngs.remove(0).0)
}

/// Pictures as PNGs whose base64 fits in `budget` bytes together, none more than `max_side`
/// pixels a side: whole if they fit, else all scaled down alike (each pixel the average of the
/// area it covers) as little as fits them, within a few hundredths. RGB when a picture is opaque: the alpha would only cost bytes. Each
/// picture is a grid of equal cells (columns, rows), as a sheet of views is, and stays whole
/// cells. `mark` paints on each at each size tried, given its index and its cells' size: what
/// should stay crisp (words, marks) rather than shrink with it. Each PNG comes with its cells' size.
pub fn fit_pictures(pictures: &[(RgbaPicture, [u32; 2])], budget: usize, max_side: u32, mut mark: impl FnMut(usize, &mut RgbaPicture, [u32; 2])) -> Result<Vec<(Vec<u8>, [u32; 2])>, String> {
    if pictures.iter().any(|(_, grid)| grid.contains(&0)) {
        return Err("a picture's grid has no cells across or down".into());
    }
    let cells: Vec<[u32; 2]> = pictures.iter().map(|(image, grid)| [image.width / grid[0], image.height / grid[1]]).collect();
    let largest = cells.iter().flat_map(|c| *c).max().unwrap_or(1);
    // The largest cells may take: a picture's cells are its grid's share of `max_side` at most.
    let top = pictures
        .iter()
        .zip(&cells)
        .flat_map(|((_, grid), cell)| (0..2).map(move |k| ((max_side / grid[k]) as u64 * largest as u64 / cell[k].max(1) as u64) as u32))
        .fold(largest, u32::min);
    if top == 0 {
        return Err(format!("a grid of more than {max_side} cells across cannot be {max_side} pixels a side"));
    }
    // All of them with their cells `side` of the largest cell's pixels: the PNGs and their bytes.
    let mut at = |side: u32| -> Result<(Vec<(Vec<u8>, [u32; 2])>, usize), String> {
        let mut pngs = Vec::with_capacity(pictures.len());
        for (i, (image, grid)) in pictures.iter().enumerate() {
            let cell = cells[i].map(|c| ((c as u64 * side as u64).div_ceil(largest as u64) as u32).max(1));
            let mut sized = if cell == cells[i] { image.clone() } else { area_average(image, cell[0] * grid[0], cell[1] * grid[1]) };
            mark(i, &mut sized, cell);
            pngs.push((encode_png(&sized)?, cell));
        }
        let bytes = pngs.iter().map(|(png, _)| base64_len(png.len())).sum();
        Ok((pngs, bytes))
    };
    // The largest side that fits, between the largest tried that fits and the smallest that does
    // not. A PNG's bytes go about as a power of its side (2 for pixels alone; more where the
    // average smooths away a dither): each next try aims a hundredth under the budget along the
    // power the last two tries show.
    let (mut fits, mut over): (Option<(u32, Vec<(Vec<u8>, [u32; 2])>)>, u32) = (None, top + 1);
    let mut tried: Vec<(u32, usize)> = Vec::new();
    let mut side = top;
    loop {
        let (pngs, bytes) = at(side)?;
        tried.push((side, bytes));
        if bytes <= budget {
            if side == top || bytes * 100 >= budget * 97 {
                return Ok(pngs);
            }
            fits = Some((side, pngs));
        } else {
            over = side;
        }
        let low = fits.as_ref().map_or(0, |(side, _)| *side);
        if over - low <= 1 {
            return fits.map(|(_, pngs)| pngs).ok_or_else(|| format!("{} picture(s) cannot fit in {budget} bytes", pictures.len()));
        }
        let power = match tried[..] {
            [.., (s1, b1), (s2, b2)] if s1 != s2 && b1 != b2 => ((b1 as f64 / b2 as f64).ln() / (s1 as f64 / s2 as f64).ln()).clamp(1.0, 4.0),
            _ => 2.0,
        };
        let aim = side as f64 * (budget as f64 * 0.99 / bytes as f64).powf(1.0 / power);
        side = if aim > low as f64 && aim < over as f64 { aim.round() as u32 } else { (low + over) / 2 };
        side = side.clamp(low + 1, over - 1);
    }
}

/// The picture at `width` x `height` (no larger than it), each pixel the average of the area of
/// the picture it covers: across, then down.
pub fn area_average(image: &RgbaPicture, width: u32, height: u32) -> RgbaPicture {
    // Each new pixel's run of old ones along an axis, with the share of each it covers.
    let spans = |from: u32, to: u32| -> Vec<Vec<(usize, f32)>> {
        let step = from as f64 / to as f64;
        (0..to)
            .map(|i| {
                let (a, b) = (i as f64 * step, (i + 1) as f64 * step);
                (a.floor() as usize..(b.ceil() as usize).min(from as usize)).map(|j| (j, ((b.min(j as f64 + 1.0) - a.max(j as f64)) / step) as f32)).collect()
            })
            .collect()
    };
    let (from_w, w) = (image.width as usize, width as usize);
    let (across, down) = (spans(image.width, width), spans(image.height, height));
    let mut row = vec![0f32; image.height as usize * w * 4];
    for y in 0..image.height as usize {
        for (x, span) in across.iter().enumerate() {
            for &(j, share) in span {
                for c in 0..4 {
                    row[(y * w + x) * 4 + c] += image.rgba[(y * from_w + j) * 4 + c] as f32 * share;
                }
            }
        }
    }
    let mut rgba = vec![0u8; w * height as usize * 4];
    for (y, span) in down.iter().enumerate() {
        for x in 0..w {
            for c in 0..4 {
                let v: f32 = span.iter().map(|&(j, share)| row[(j * w + x) * 4 + c] * share).sum();
                rgba[(y * w + x) * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    RgbaPicture { width, height, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of no cells across or down is an error, not a division by zero.
    #[test]
    fn a_grid_of_no_cells_is_an_error() {
        let picture = RgbaPicture { width: 2, height: 2, rgba: vec![255; 16] };
        for grid in [[0, 1], [1, 0]] {
            assert!(fit_pictures(&[(picture.clone(), grid)], 1 << 20, 64, |_, _, _| {}).is_err(), "{grid:?}");
        }
        assert!(fit_pictures(&[(picture, [1, 1])], 1 << 20, 64, |_, _, _| {}).is_ok(), "one cell fits");
    }
}
