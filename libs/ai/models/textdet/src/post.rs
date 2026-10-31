//! Picture in (resize + normalise) and boxes out (DB post-processing).

use crate::cpu::Map;
use crate::ir::{Postprocess, Preprocess};

/// The network input for a picture, and the scale back to it.
pub struct Prepared {
    pub input: Map,
    /// Network pixels per picture pixel, per axis.
    pub scale_x: f32,
    pub scale_y: f32,
}

/// Network input size for a `width x height` picture: the longer side at most
/// `max_side`, both sides multiples of `multiple` (at least one multiple).
pub fn input_size(width: usize, height: usize, pre: &Preprocess) -> (usize, usize) {
    let longer = width.max(height).max(1) as f32;
    let ratio = (pre.max_side as f32 / longer).min(1.0);
    let round = |v: usize| -> usize {
        let m = pre.multiple.max(1);
        ((((v as f32 * ratio) / m as f32).round() as usize).max(1)) * m
    };
    (round(width), round(height))
}

/// Bilinear resize of RGBA8 into the normalised `[3, h, w]` input.
pub fn prepare(width: usize, height: usize, rgba: &[u8], pre: &Preprocess) -> Result<Prepared, String> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Err(format!("picture {width}x{height} with {} bytes", rgba.len()));
    }
    let (ow, oh) = input_size(width, height, pre);
    let mut input = Map::zeros(3, oh, ow);
    let sx = width as f32 / ow as f32;
    let sy = height as f32 / oh as f32;
    for y in 0..oh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..ow {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for c in 0..3 {
                let p = |xx: usize, yy: usize| rgba[(yy * width + xx) * 4 + c] as f32;
                let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                let bottom = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                let v = (top * (1.0 - ty) + bottom * ty) / 255.0;
                let ch = if pre.bgr { 2 - c } else { c };
                input.data[(ch * oh + y) * ow + x] = (v - pre.mean[ch]) / pre.std[ch];
            }
        }
    }
    Ok(Prepared { input, scale_x: ow as f32 / width as f32, scale_y: oh as f32 / height as f32 })
}

/// One detected text line, as fractions of the picture (0,0 = top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Mean text probability inside the region.
    pub score: f32,
}

/// Connected text regions of the probability map -> grown, axis-aligned line
/// boxes in picture fractions, in reading order (top to bottom, then left to
/// right within a line).
pub fn boxes(prob: &Map, prep_scale: (f32, f32), picture: (usize, usize), post: &Postprocess) -> Vec<LineBox> {
    let (w, h) = (prob.w, prob.h);
    let p = &prob.data[..w * h];
    let mut label = vec![0u32; w * h];
    let mut stack = Vec::new();
    let mut out = Vec::new();
    let mut next = 0u32;
    for start in 0..w * h {
        if label[start] != 0 || p[start] <= post.thresh {
            continue;
        }
        next += 1;
        label[start] = next;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        let (mut sum, mut area) = (0.0f32, 0usize);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            sum += p[i];
            area += 1;
            let mut visit = |j: usize| {
                if label[j] == 0 && p[j] > post.thresh {
                    label[j] = next;
                    stack.push(j);
                }
            };
            if x > 0 { visit(i - 1); }
            if x + 1 < w { visit(i + 1); }
            if y > 0 { visit(i - w); }
            if y + 1 < h { visit(i + w); }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if bw.min(bh) < post.min_size {
            continue;
        }
        let score = sum / area as f32;
        if score < post.box_thresh {
            continue;
        }
        // DB's unclip: grow the shrunk text kernel by area * ratio / perimeter.
        let d = bw * bh * post.unclip / (2.0 * (bw + bh));
        let (gx0, gy0) = ((x0 as f32 - d).max(0.0), (y0 as f32 - d).max(0.0));
        let (gx1, gy1) = ((x1 as f32 + 1.0 + d).min(w as f32), (y1 as f32 + 1.0 + d).min(h as f32));
        if (gx1 - gx0).min(gy1 - gy0) < post.min_size + 2.0 {
            continue;
        }
        let (pw, ph) = (picture.0 as f32 * prep_scale.0, picture.1 as f32 * prep_scale.1);
        out.push(LineBox { x: gx0 / pw, y: gy0 / ph, w: (gx1 - gx0) / pw, h: (gy1 - gy0) / ph, score });
    }
    reading_order(&mut out);
    merge_touching(out)
}

/// Pieces of one line whose grown boxes overlap (a gap the network left
/// between two words) become one line box, score averaged by area.
pub fn merge_touching(boxes: Vec<LineBox>) -> Vec<LineBox> {
    let mut out: Vec<LineBox> = Vec::with_capacity(boxes.len());
    for b in boxes {
        if let Some(last) = out.last_mut() {
            let overlap_y = (last.y + last.h).min(b.y + b.h) - last.y.max(b.y);
            let same_line = overlap_y > 0.5 * last.h.min(b.h);
            if same_line && b.x <= last.x + last.w {
                let (x0, y0) = (last.x.min(b.x), last.y.min(b.y));
                let (x1, y1) = ((last.x + last.w).max(b.x + b.w), (last.y + last.h).max(b.y + b.h));
                let (a0, a1) = (last.w * last.h, b.w * b.h);
                *last = LineBox { x: x0, y: y0, w: x1 - x0, h: y1 - y0, score: (last.score * a0 + b.score * a1) / (a0 + a1) };
                continue;
            }
        }
        out.push(b);
    }
    out
}

/// Top to bottom; boxes whose vertical centres lie within half a line height
/// of each other count as one line and go left to right.
pub fn reading_order(boxes: &mut [LineBox]) {
    boxes.sort_by(|a, b| (a.y + a.h / 2.0).total_cmp(&(b.y + b.h / 2.0)));
    let mut i = 0;
    while i < boxes.len() {
        let centre = boxes[i].y + boxes[i].h / 2.0;
        let mut j = i + 1;
        while j < boxes.len() && (boxes[j].y + boxes[j].h / 2.0 - centre).abs() < boxes[i].h.min(boxes[j].h) / 2.0 {
            j += 1;
        }
        boxes[i..j].sort_by(|a, b| a.x.total_cmp(&b.x));
        i = j;
    }
}
