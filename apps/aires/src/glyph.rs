use makepad_widgets::TrayImage;

const SIZE: u32 = 36;

fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

pub fn image(count: usize) -> TrayImage {
    // Top, upper right, lower right, bottom, lower left, upper left, middle.
    const DIGITS: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];
    const SEGMENTS: [((f32, f32), (f32, f32)); 7] = [
        ((0.0, 0.0), (5.0, 0.0)), ((6.0, 1.0), (6.0, 6.0)),
        ((6.0, 8.0), (6.0, 13.0)), ((0.0, 14.0), (5.0, 14.0)),
        ((-1.0, 8.0), (-1.0, 13.0)), ((-1.0, 1.0), (-1.0, 6.0)),
        ((0.0, 7.0), (5.0, 7.0)),
    ];
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let n = count.min(99);
    let digits = if n < 10 { [(n, 15.0), (0, 0.0)] } else { [(n / 10, 10.0), (n % 10, 21.0)] };
    let digit_count = if n < 10 { 1 } else { 2 };
    let mut alpha = vec![0; (SIZE * SIZE) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = (x as f32, y as f32);
            // A rounded box joined to a tapered tail, with a continuous outline.
            let (dx, dy) = ((p.0 - 17.5).abs() - 11.5, (p.1 - 16.0).abs() - 8.5);
            let box_d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - 3.5;
            let tail_d = (7.0 - p.0).max(p.0 + p.1 - 40.0).max(24.0 - p.1);
            let bubble = box_d.min(tail_d);
            let mut v = cover(if count == 0 { bubble.abs() - 1.0 } else { bubble });
            if count > 0 {
                for &(digit, left) in &digits[..digit_count] {
                    for (bit, &(a, b)) in SEGMENTS.iter().enumerate() {
                        if DIGITS[digit] & (1 << bit) != 0 {
                            v *= 1.0 - cover(segment(p, (a.0 + left, a.1 + 9.0), (b.0 + left, b.1 + 9.0)) - 0.8);
                        }
                    }
                }
            }
            if count > 99 {
                v = v.max(cover(segment(p, (28.0, 30.0), (34.0, 30.0)) - 0.8));
                v = v.max(cover(segment(p, (31.0, 27.0), (31.0, 33.0)) - 0.8));
            }
            alpha[(y * SIZE + x) as usize] = (v * 255.0).round() as u8;
        }
    }
    TrayImage { width: SIZE, height: SIZE, alpha }
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_have_distinct_full_size_masks() {
        let masks = [0, 1, 7, 42, 150].map(super::image);
        for (i, mask) in masks.iter().enumerate() {
            assert_eq!(mask.alpha.len(), 36 * 36, "each mask has 36 by 36 pixels");
            for other in &masks[i + 1..] {
                assert_ne!(mask.alpha, other.alpha, "each count has a distinct glyph");
            }
        }
    }
}
