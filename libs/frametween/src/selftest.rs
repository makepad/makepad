//! THE SYNTHETIC PAIR AND HOW TO READ IT — the shared half of the gate
//! that decides, from pixels, whether a tier actually MOVED the picture or
//! merely blended two copies of it.
//!
//! A crossfade and an optical-flow tween are trivially told apart if you
//! ask the right question. Put a bright marker block on a textured field,
//! move the whole thing sideways, and look at the middle:
//!
//! - a **blend** puts the marker in BOTH places at half strength, so
//!   nothing in the row reaches the marker's real colour;
//! - a **tween** puts ONE marker, at full strength, halfway between.
//!
//! So the measurement is: how wide is the band that reaches full marker
//! strength, and where is its centre. `examples/tween_gate.rs` runs this
//! on the GPU for every mode; the unit tests below run the measurement
//! against pictures built on the CPU, which is what keeps the *ruler*
//! honest.

/// The gate's frame size. Big enough that the flow pyramid has four real
/// levels (level 0 is a quarter of this, and the top is 16 cells).
pub const GATE_W: usize = 512;
pub const GATE_H: usize = 512;
/// The marker block's width, and where it starts in each endpoint.
pub const BLOCK_W: usize = 96;
pub const BLOCK_A_X: usize = 112;
pub const BLOCK_B_X: usize = 208;
/// Where an honest in-betweener puts it at t = 0.5.
pub const BLOCK_MID_X: usize = (BLOCK_A_X + BLOCK_B_X) / 2;

const MARKER: [u8; 3] = [235, 40, 30];

/// One endpoint of the synthetic pair: vertical stripes (so block matching
/// has something to lock onto everywhere) translated by `shift`, with the
/// marker block riding along at `block_x`.
pub fn gate_frame(shift: usize, block_x: usize) -> Vec<u8> {
    let mut rgb = vec![0u8; GATE_W * GATE_H * 3];
    for y in 0..GATE_H {
        for x in 0..GATE_W {
            let at = (y * GATE_W + x) * 3;
            let px = if x >= block_x && x < block_x + BLOCK_W {
                MARKER
            } else {
                // Two beat frequencies so the pattern does not repeat
                // inside the matcher's search window and match the wrong
                // stripe, plus a vertical term so the field is not a
                // one-dimensional ambiguity.
                let u = (x + GATE_W - shift % GATE_W) as f32;
                let s = ((u * 0.196).sin() * 0.5 + 0.5) * 0.6
                    + ((u * 0.071).sin() * 0.5 + 0.5) * 0.25
                    + ((y as f32 * 0.083).sin() * 0.5 + 0.5) * 0.15;
                let v = (s * 190.0) as u8 + 20;
                [v, v, v]
            };
            rgb[at..at + 3].copy_from_slice(&px);
        }
    }
    rgb
}

/// The pair the gate judges: the same field 96 px apart, marker included.
pub fn gate_pair() -> (Vec<u8>, Vec<u8>) {
    let shift = BLOCK_B_X - BLOCK_A_X;
    (gate_frame(0, BLOCK_A_X), gate_frame(shift, BLOCK_B_X))
}

/// What the middle row says about where the marker ended up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockReading {
    /// Columns reaching full marker strength.
    pub full_width: usize,
    /// Centre of that band (meaningless when `full_width` is 0).
    pub full_center: usize,
    /// Columns that carry marker colour at ALL — a blend's two ghosts land
    /// here and nowhere else.
    pub tinted_width: usize,
}

impl BlockReading {
    /// A real in-between: one marker, full strength, roughly its own width.
    pub fn moved(&self) -> bool {
        self.full_width * 3 >= BLOCK_W * 2 && self.full_width <= BLOCK_W * 3 / 2
    }

    /// A blend: the marker is present but nowhere at full strength.
    pub fn ghosted(&self) -> bool {
        self.full_width * 4 < BLOCK_W && self.tinted_width >= BLOCK_W
    }

    /// How far the marker sits from where it belongs.
    pub fn offset_from(&self, want_x: usize) -> i64 {
        self.full_center as i64 - (want_x + BLOCK_W / 2) as i64
    }
}

/// Read the centre row of a packed BGRA picture (what a render target
/// hands back) and say where the marker is.
pub fn read_block_bgra(bgra: &[u8], width: usize, height: usize) -> BlockReading {
    let row = height / 2;
    let mut full: Vec<usize> = Vec::new();
    let mut tinted = 0usize;
    for x in 0..width {
        let at = (row * width + x) * 4;
        if at + 2 >= bgra.len() {
            break;
        }
        let (b, g, r) = (bgra[at] as i32, bgra[at + 1] as i32, bgra[at + 2] as i32);
        let redness = r - (g + b) / 2;
        // The marker is r-((g+b)/2) ~= 200; blended half-and-half with the
        // grey field it is ~100. 150 sits cleanly between the two.
        if redness > 150 {
            full.push(x);
        }
        if redness > 45 {
            tinted += 1;
        }
    }
    let full_width = full.len();
    let full_center = if full.is_empty() {
        0
    } else {
        (full.first().unwrap() + full.last().unwrap() + 1) / 2
    };
    BlockReading { full_width, full_center, tinted_width: tinted }
}

/// The same reading over a packed RGB8 picture, for CPU-side reference
/// pictures that never went near a render target.
pub fn read_block_rgb8(rgb: &[u8], width: usize, height: usize) -> BlockReading {
    let mut bgra = vec![255u8; width * height * 4];
    for i in 0..width * height {
        if i * 3 + 2 >= rgb.len() {
            break;
        }
        bgra[i * 4] = rgb[i * 3 + 2];
        bgra[i * 4 + 1] = rgb[i * 3 + 1];
        bgra[i * 4 + 2] = rgb[i * 3];
    }
    read_block_bgra(&bgra, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blend(a: &[u8], b: &[u8]) -> Vec<u8> {
        a.iter().zip(b).map(|(x, y)| ((*x as u16 + *y as u16) / 2) as u8).collect()
    }

    #[test]
    fn the_ruler_finds_each_endpoint_where_it_is() {
        let (a, b) = gate_pair();
        let ra = read_block_rgb8(&a, GATE_W, GATE_H);
        assert!(ra.moved(), "frame A carries one full-strength marker: {ra:?}");
        assert_eq!(ra.offset_from(BLOCK_A_X), 0);
        let rb = read_block_rgb8(&b, GATE_W, GATE_H);
        assert!(rb.moved());
        assert_eq!(rb.offset_from(BLOCK_B_X), 0);
    }

    #[test]
    fn the_ruler_calls_a_crossfade_a_ghost_and_never_moved() {
        // This is exactly what a dissolve produces at t = 0.5, and it is
        // what the flow tiers must NOT look like.
        let (a, b) = gate_pair();
        let mid = blend(&a, &b);
        let r = read_block_rgb8(&mid, GATE_W, GATE_H);
        assert!(!r.moved(), "a blend never reaches full marker strength: {r:?}");
        assert!(r.ghosted(), "and it tints both places at once: {r:?}");
        // Both endpoints, side by side: twice the block.
        assert!(r.tinted_width >= BLOCK_W * 2 - 8, "{r:?}");
    }

    #[test]
    fn the_ruler_calls_a_true_in_between_moved_and_halfway() {
        // What an honest tween produces: ONE marker, at full strength,
        // exactly between the two endpoint positions.
        let ideal = gate_frame((BLOCK_B_X - BLOCK_A_X) / 2, BLOCK_MID_X);
        let r = read_block_rgb8(&ideal, GATE_W, GATE_H);
        assert!(r.moved(), "{r:?}");
        assert_eq!(r.offset_from(BLOCK_MID_X), 0);
        assert!(!r.ghosted());
    }

    #[test]
    fn a_hard_swap_reads_as_frame_b_and_nothing_else() {
        let (_, b) = gate_pair();
        let r = read_block_rgb8(&b, GATE_W, GATE_H);
        assert!(r.moved());
        assert_eq!(r.offset_from(BLOCK_B_X), 0);
        assert!(r.offset_from(BLOCK_MID_X).abs() > 40, "not halfway: {r:?}");
    }
}
