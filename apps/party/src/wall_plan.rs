//! The big screen's layout, and the size its pictures are decoded at.
//!
//! Every frame of every loop on the wall is a texture, so what the wall
//! holds is tiles x frames x side². Decoded at the phones' size (512 px),
//! 50 three-second loops would be 1.2 GB on the card that also holds the
//! hub's models. Decoded at the size a tile is drawn, the tiles shrink as
//! loops come, and the wall holds about the screen's pixels per loop frame:
//! it grows with the screen, not with the party. Only the one picture attract
//! mode shows across the screen is held at its full size.

/// Points between tiles, and round the wall.
pub const GAP: f64 = 8.0;

/// The columns and the tile side (layout points) that make `n` square tiles
/// largest in `w` x `h`.
pub fn grid(n: usize, w: f64, h: f64) -> (usize, f64) {
    (1..=n.max(1))
        .map(|cols| {
            let rows = n.div_ceil(cols).max(1);
            let side = ((w - GAP * (cols as f64 - 1.0)) / cols as f64).min((h - GAP * (rows as f64 - 1.0)) / rows as f64);
            (cols, side.max(1.0))
        })
        .fold((1, 0.0), |best, c| if c.1 > best.1 { c } else { best })
}

/// The side (pixels) a tile of `side` points is decoded at on a screen of
/// `dpi`: rounded up to a multiple of 32, so a small change of the layout
/// keeps what is decoded.
pub fn decode_side(side: f64, dpi: f64) -> u32 {
    (((side * dpi) / 32.0).ceil().max(1.0) as u32) * 32
}

/// Bytes of texture a wall of `w` x `h` points holds for `loops` loops of
/// `frames` frames each, whose pictures are `max` pixels (none is decoded
/// larger than it is).
pub fn wall_bytes(loops: usize, frames: usize, w: f64, h: f64, dpi: f64, max: u32) -> usize {
    let (_, side) = grid(loops, w, h);
    let px = decode_side(side, dpi).min(max) as usize;
    loops * frames * px * px * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_holds_about_the_screen_not_the_party() {
        // A 1080p screen beside the join panel; 3 s loops at 8 fps from the
        // phones' 512 px squares.
        let (w, h, frames, max) = (1504.0, 1064.0, 24, 512);
        for dpi in [1.0, 2.0] {
            let screen = w * dpi * h * dpi * frames as f64 * 4.0;
            for loops in [1, 10, 50, 200, 1000] {
                let bytes = wall_bytes(loops, frames, w, h, dpi, max);
                assert!(
                    bytes as f64 <= screen * 1.5,
                    "{loops} loops at dpi {dpi}: {} MB, the screen's own pixels {} MB",
                    bytes >> 20,
                    screen as usize >> 20
                );
            }
        }
        let fifty = wall_bytes(50, frames, w, h, 1.0, max);
        assert!(fifty < 200 << 20, "50 loops on a 1080p wall: {} MB", fifty >> 20);
        assert!(fifty * 8 < 50 * frames * 512 * 512 * 4, "a tenth or so of the phones' size");

        let (cols, side) = grid(4, w, h);
        assert_eq!(cols, 2, "four tiles make a square of two by two on a wide wall");
        assert!((side - (h - GAP) / 2.0).abs() < 1e-9);
        assert_eq!(decode_side(160.0, 1.0), 160);
        assert_eq!(decode_side(161.0, 1.0), 192, "rounded up, never blurred by an upscale");
    }
}
