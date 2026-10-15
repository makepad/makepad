//! Reading text out of a picture: a vision model reads the photo (the hub's
//! chat model with its vision tower, or the phone's own one offline), one line
//! per line of text ([`parse_read_lines`]); the lines then go through the same
//! chat path as typed text in one numbered job ([`numbered_prompt`] /
//! [`parse_numbered`]).

/// A picture handed to a reader: RGBA8, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl From<crate::camera::Rgba> for Frame {
    fn from(p: crate::camera::Rgba) -> Frame {
        Frame { width: p.width as u32, height: p.height as u32, rgba: p.pixels }
    }
}

/// One piece of text and where it sits, as fractions of the picture
/// (0,0 = top left, 1,1 = bottom right).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One line per box, `n|text`, for a single translation job.
pub fn numbered_prompt(boxes: &[TextBox]) -> String {
    boxes
        .iter()
        .enumerate()
        .map(|(i, b)| format!("{}|{}", i + 1, b.text.replace('\n', " ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The reply of a numbered job back to one string per box (empty when the
/// model left a line out).
pub fn parse_numbered(reply: &str, count: usize) -> Vec<String> {
    let mut out = vec![String::new(); count];
    for line in reply.lines() {
        let Some((n, text)) = line.split_once('|') else { continue };
        let Ok(n) = n.trim().parse::<usize>() else { continue };
        if n >= 1 && n <= count {
            out[n - 1] = text.trim().to_string();
        }
    }
    out
}

/// The system prompt of that job.
pub fn numbered_system(to: crate::lang::Lang, style: &crate::prompt::Style) -> String {
    // No source language: the model translates whatever it reads into `to`
    // (the app's flip setting); text already in `to` goes to the other side.
    let other = to.other();
    let script = |l: crate::lang::Lang| {
        if l == crate::lang::Lang::Zh {
            format!("{} Chinese", if style.traditional { "Traditional" } else { "Simplified" })
        } else {
            l.name().to_string()
        }
    };
    let (to_name, other_name) = (script(to), script(other));
    format!(
        "You are a translation engine. The user's message is several short pieces of text read from a photo (a menu, a sign), \
         one per line as `number|text`, in any language. Translate each piece into {to_name}; a piece that is already {to_name} goes into {other_name} instead. \
         Answer with the same lines, `number|translation`, one per input line, in the same order. \
         Keep prices, numbers and units. Output only those lines."
    )
}

// ------------------------------------------------- the phone's own model

/// The side of the square picture the on-device model sees: every photo is
/// fitted into it, so the vision tower compiles one graph for all of them.
pub const ON_DEVICE_SIDE: u32 = 512;
/// The side a photo goes to the hub at: its vision tower takes up to 1024 px,
/// and small print needs the pixels.
pub const HUB_SIDE: u32 = 1024;

/// A frame as PNG (the hub's chat turns carry pictures as PNG).
pub fn png(frame: &Frame) -> Result<Vec<u8>, String> {
    use makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 || frame.rgba.len() < w * h * 4 {
        return Err("no picture".to_string());
    }
    let options = EncoderOptions::default().set_width(w).set_height(h).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGBA);
    let mut out = Vec::new();
    makepad_zune_png::PngEncoder::new(&frame.rgba[..w * h * 4], options).encode(&mut out).map_err(|e| format!("png: {e:?}"))?;
    Ok(out)
}

pub const ON_DEVICE_PROMPT: &str = "Read the text in this photo.";

/// The on-device model only READS in the picture pass: one line of the photo's
/// text per output line, as written, nothing else. Translating is a second,
/// text-only turn over exactly those lines (numbered, like the hub path), so
/// the app pairs each original with its translation itself: a 4B model asked
/// for `original => translation` pairs reversed them and cost twice the tokens.
pub fn on_device_read_system() -> String {
    "You read the text in a photo (a menu, a sign, a notice). For every line of text in the photo, in reading order, \
     output that line exactly as written, on a line of its own. Output only the text."
        .to_string()
}

/// The lines the model read, as boxes stacked down the picture (the model
/// does not say where each line sits; a text detector will, later).
pub fn parse_read_lines(text: &str) -> Vec<TextBox> {
    let lines: Vec<String> = text
        .lines()
        .map(|line| line.trim().trim_matches('`').trim().to_string())
        .filter(|line| !line.is_empty())
        .take(10)
        .collect();
    let step = 0.9 / lines.len().max(1) as f32;
    lines
        .into_iter()
        .enumerate()
        .map(|(i, text)| TextBox { text, x: 0.05, y: 0.05 + i as f32 * step, w: 0.9, h: step.min(0.08) })
        .collect()
}

/// `frame` scaled to fit a `side` x `side` square (area averaging), centred on
/// white.
pub fn fit_square(frame: &Frame, side: u32) -> Frame {
    let (w, h) = (frame.width.max(1) as usize, frame.height.max(1) as usize);
    let side_u = side as usize;
    let scale = (side_u as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * scale).round() as usize).clamp(1, side_u), ((h as f32 * scale).round() as usize).clamp(1, side_u));
    let (ox, oy) = ((side_u - tw) / 2, (side_u - th) / 2);
    let mut rgba = vec![255u8; side_u * side_u * 4];
    if frame.rgba.len() < w * h * 4 {
        return Frame { width: side, height: side, rgba };
    }
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &frame.rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let at = ((oy + ty) * side_u + ox + tx) * 4;
            for c in 0..4 {
                rgba[at + c] = (sum[c] / count) as u8;
            }
        }
    }
    Frame { width: side, height: side, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_parse_and_pictures_fit() {
        let boxes = parse_read_lines("今日菜单\n\n`绿茶 18元`\n");
        assert_eq!(boxes.len(), 2);
        assert!(boxes[1].y > boxes[0].y);
        assert_eq!(boxes.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), vec!["今日菜单", "绿茶 18元"]);
        assert!(parse_read_lines("\n \n").is_empty());
        let wide = Frame { width: 4, height: 2, rgba: vec![0; 32] };
        let fitted = fit_square(&wide, 8);
        assert_eq!((fitted.width, fitted.height, fitted.rgba.len()), (8, 8, 256));
        // Never scaled up: the 4x2 picture sits centred (rows 3-4, columns
        // 2-5), the rest is white padding.
        assert_eq!(fitted.rgba[0], 255);
        assert_eq!(fitted.rgba[(3 * 8 + 2) * 4], 0);
        assert_eq!(fitted.rgba[(3 * 8 + 1) * 4], 255);
    }

    #[test]
    fn numbered_jobs_round_trip() {
        let boxes = parse_read_lines("今日菜单\n宫保鸡丁  38元\n麻婆豆腐  26元\n红烧肉  58元");
        let prompt = numbered_prompt(&boxes);
        assert!(prompt.starts_with("1|今日菜单\n2|宫保鸡丁"));
        let reply = "1|Today's menu\n2| Kung Pao chicken  38 yuan\nnoise\n9|out of range\n4|Red-braised pork 58 yuan";
        let got = parse_numbered(reply, boxes.len());
        assert_eq!(got[0], "Today's menu");
        assert_eq!(got[1], "Kung Pao chicken  38 yuan");
        assert_eq!(got[2], "");
        assert_eq!(got[3], "Red-braised pork 58 yuan");
    }

    #[test]
    fn a_frame_becomes_a_png() {
        let frame = Frame { width: 3, height: 2, rgba: vec![200; 24] };
        let png = png(&frame).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 3);
        assert!(super::png(&Frame::default()).is_err());
    }

}
