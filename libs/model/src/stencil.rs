//! A 5x7 stencil font rasterized into RGBA layers, for text decals (model
//! badges, serial numbers, warning labels). Glyph pixels get the ink colour,
//! everything else the background (usually transparent for alpha-masked decals).

/// 5x7 glyphs, one row per byte (bit 4 = leftmost column).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11], 'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e], 'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f], 'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f], 'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e], 'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], 'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11], 'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e], 'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d], 'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e], 'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e], 'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a], 'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04], 'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e], '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f], '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02], '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e], '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e], '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '-' => [0, 0, 0, 0x1f, 0, 0, 0], '.' => [0, 0, 0, 0, 0, 0x0c, 0x0c], '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0, 0x04],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10], ':' => [0, 0x0c, 0x0c, 0, 0x0c, 0x0c, 0], '+' => [0, 0x04, 0x04, 0x1f, 0x04, 0x04, 0],
        _ => [0; 7],
    }
}

/// RGBA8 image (row 0 = top = V 0) of `text` with one-dot padding; returns
/// (width, height, pixels). `cell` is pixels per font dot.
pub fn stencil_text(text: &str, cell: u32, ink: [u8; 4], background: [u8; 4]) -> (u32, u32, Vec<u8>) {
    let chars: Vec<char> = text.chars().take(64).collect();
    let cols = (chars.len().max(1) as u32) * 6 + 1;
    let cell = cell.clamp(1, 2048 / cols.max(1)).max(1);
    let (w, h) = (cols * cell, 9 * cell);
    let mut px = background.repeat((w * h) as usize);
    for (i, c) in chars.iter().enumerate() {
        let g = glyph(*c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (0x10 >> col) == 0 { continue; }
                let (x0, y0) = ((1 + i as u32 * 6 + col) * cell, (1 + row as u32) * cell);
                for y in y0..(y0 + cell).min(h) { for x in x0..(x0 + cell).min(w) { let o = ((y * w + x) * 4) as usize; px[o..o + 4].copy_from_slice(&ink); } }
            }
        }
    }
    (w, h, px)
}

/// The smooth, badge-ready sibling of [`stencil_text`]: each glyph's dots
/// are joined into round-capped strokes (neighbours across, down and on the
/// diagonals), anti-aliased, so a race number or a sponsor name reads as
/// lettering rather than as a grid of squares. `badge` puts it on a shape:
/// "circle" (a roundel: the fill disc, an optional border ring, transparent
/// outside), "plate" (a rounded rectangle) or "" (the text alone on the
/// background). Returns (width, height, RGBA8 pixels, row 0 = top).
pub fn stencil_badge(text: &str, cell: u32, ink: [u8; 4], fill: [u8; 4], border: Option<[u8; 4]>, badge: &str) -> (u32, u32, Vec<u8>) {
    let chars: Vec<char> = text.chars().take(64).collect();
    let n = chars.len().max(1) as f32;
    let cell = cell.clamp(2, 64) as f32;
    // Text box in dots: 6 per glyph (5 + gap), 7 tall, then the badge
    // margin around it.
    let (tw, th) = (n * 6.0 - 1.0, 7.0);
    let (margin_x, margin_y) = match badge { "circle" => { let d = (tw.max(th) * 1.35).max(9.0); ((d - tw) * 0.5, (d - th) * 0.5) } "plate" => (2.0, 1.6), _ => (1.0, 1.0) };
    let (w, h) = (((tw + margin_x * 2.0) * cell).round().min(2048.0) as u32, ((th + margin_y * 2.0) * cell).round().min(2048.0) as u32);
    let cell = (w as f32 / (tw + margin_x * 2.0)).min(h as f32 / (th + margin_y * 2.0));
    // Stroke segments in pixel space.
    let mut segs: Vec<([f32; 2], [f32; 2])> = Vec::new();
    // Where each glyph's strokes start in `segs`: a pixel only tests its
    // own glyph and the two beside it (anything further is more than a
    // glyph gap away and cannot cover it).
    let mut starts: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    for (i, c) in chars.iter().enumerate() {
        starts.push(segs.len());
        let g = glyph(*c);
        let on = |r: i32, c: i32| r >= 0 && r < 7 && c >= 0 && c < 5 && g[r as usize] & (0x10 >> c) != 0;
        let at = |r: i32, c: i32| [(margin_x + i as f32 * 6.0 + c as f32 + 0.5) * cell, (margin_y + r as f32 + 0.5) * cell];
        for r in 0..7 { for c in 0..5 {
            if !on(r, c) { continue; }
            segs.push((at(r, c), at(r, c)));
            for (dr, dc) in [(0, 1), (1, 0), (1, 1), (1, -1)] {
                // Diagonals only where the square corner is open (no L joint).
                if on(r + dr, c + dc) && (dr == 0 || dc == 0 || (!on(r + dr, c) && !on(r, c + dc))) { segs.push((at(r, c), at(r + dr, c + dc))); }
            }
        }}
    }
    starts.push(segs.len());
    let radius = cell * 0.58;
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let blend = |dst: &mut [u8], src: [u8; 4], a: f32| {
        let a = a.clamp(0.0, 1.0) * src[3] as f32 / 255.0;
        let da = dst[3] as f32 / 255.0;
        let out_a = a + da * (1.0 - a);
        for k in 0..3 { dst[k] = if out_a > 0.0 { ((src[k] as f32 * a + dst[k] as f32 * da * (1.0 - a)) / out_a).round() as u8 } else { 0 }; }
        dst[3] = (out_a * 255.0).round() as u8;
    };
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let o = ((y * w + x) * 4) as usize;
            // Background shape.
            let (inside, edge) = match badge {
                "circle" => { let r = cx.min(cy) - 0.5; let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt(); (r - d + 0.5, r - d) }
                "plate" => {
                    let rr = cell * 1.2;
                    let qx = ((fx - cx).abs() - (cx - rr - 0.5)).max(0.0);
                    let qy = ((fy - cy).abs() - (cy - rr - 0.5)).max(0.0);
                    let d = (qx * qx + qy * qy).sqrt();
                    (rr - d + 0.5, rr - d)
                }
                _ => (1.0, f32::MAX),
            };
            if inside > 0.0 {
                blend(&mut px[o..o + 4], fill, inside);
                if let Some(b) = border { if edge < cell * 0.9 { blend(&mut px[o..o + 4], b, (cell * 0.9 - edge).min(1.0) * inside.min(1.0)); } }
            }
            // Lettering.
            let mut dmin = f32::MAX;
            let gi = ((fx / cell - margin_x) / 6.0).floor() as i64;
            let lo = starts[(gi - 1).clamp(0, chars.len() as i64) as usize];
            let hi = starts[(gi + 2).clamp(0, chars.len() as i64) as usize];
            for (a, b) in &segs[lo..hi] {
                let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
                let l2 = ex * ex + ey * ey;
                let t = if l2 > 0.0 { (((fx - a[0]) * ex + (fy - a[1]) * ey) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (dx, dy) = (fx - (a[0] + ex * t), fy - (a[1] + ey * t));
                dmin = dmin.min((dx * dx + dy * dy).sqrt());
            }
            let cover = (radius - dmin + 0.5).clamp(0.0, 1.0);
            if cover > 0.0 { blend(&mut px[o..o + 4], ink, cover); }
        }
    }
    (w, h, px)
}

/// A number plate whose number each copy of a model shows for itself
/// (the renderer's `plate` layer, see `shaders/skinned.rs base_texel`):
/// the top half is `prefix` (4 characters) and three blank glyph cells on a
/// rounded plate, 45 dots wide; the bottom half is "0123456789" on the same
/// plate, 63 dots, resampled to the same width. Returns (w, h, rgba).
pub fn stencil_plate(prefix: &str, cell: u32, ink: [u8; 4], fill: [u8; 4], border: Option<[u8; 4]>) -> (u32, u32, Vec<u8>) {
    let head: String = prefix.chars().chain(std::iter::repeat(' ')).take(4).chain("   ".chars()).collect();
    let (w, h, top) = stencil_badge(&head, cell, ink, fill, border, "plate");
    let (sw, sh, strip) = stencil_badge("0123456789", cell, ink, fill, border, "plate");
    let mut px = vec![0u8; (w * h * 2 * 4) as usize];
    px[..top.len()].copy_from_slice(&top);
    // The strip, resampled (bilinear) into the lower half at the plate's size.
    for y in 0..h {
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / w as f32 * sw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
            let fy = ((y as f32 + 0.5) / h as f32 * sh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
            let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let o = (((h + y) * w + x) * 4) as usize;
            for c in 0..4 {
                let at = |xx: u32, yy: u32| strip[((yy * sw + xx) * 4) as usize + c] as f32;
                let v = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
                px[o + c] = v.round() as u8;
            }
        }
    }
    (w, h * 2, px)
}

#[cfg(test)]
mod badge_tests {
    #[test]
    fn a_roundel_is_round_and_carries_its_number() {
        let (w, h, px) = super::stencil_badge("7", 16, [0, 0, 0, 255], [255, 255, 255, 255], Some([200, 0, 0, 255]), "circle");
        assert_eq!(w, h, "a roundel is square");
        let a = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize];
        assert_eq!(a(0, 0), 0, "outside the disc is clear");
        assert_eq!(a(w / 2, 2), 255, "the rim is opaque");
        // Some ink in the middle: the glyph.
        let dark = (0..w * h).filter(|i| px[(*i * 4) as usize] < 60 && px[(*i * 4 + 3) as usize] == 255).count();
        assert!(dark > (w * h / 40) as usize, "{dark}");
    }
}
