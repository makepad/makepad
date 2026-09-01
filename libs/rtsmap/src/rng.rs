//! The one random stream the generator uses.
//!
//! Two devices must produce the SAME map from the same seed, so this is an
//! integer generator with no float state and no library dependency. The
//! hash is also exposed on its own: cell-indexed noise must not depend on
//! the order cells are visited in, or a refactor that changes iteration
//! order silently changes every map.

/// xorshift32 — the stream the classic converters already used, kept so a
/// generator lifted out of them behaves the same way.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        // 0 is the xorshift fixed point; a caller passing it deserves a map
        // rather than a constant.
        Self(seed.max(1))
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.0 = value;
        value
    }

    /// `0..limit`, and 0 for an empty range rather than a panic.
    pub fn below(&mut self, limit: usize) -> usize {
        if limit == 0 { 0 } else { self.next_u32() as usize % limit }
    }

    /// `first..last` (exclusive), clamped to a non-empty span.
    pub fn range(&mut self, first: usize, last_exclusive: usize) -> usize {
        first + self.below(last_exclusive.saturating_sub(first))
    }

    /// A unit float in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1 << 24) as f32
    }

    /// A signed unit float in `-1.0..1.0`.
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// True with probability `chance` (clamped).
    pub fn chance(&mut self, chance: f32) -> bool {
        self.unit() < chance.clamp(0.0, 1.0)
    }
}

/// Order-independent hash of a seed and two coordinates. Used for every
/// per-cell decision so that painting a cell twice, or in a different order,
/// yields the same answer.
pub fn hash2(seed: u32, x: i32, y: i32) -> u32 {
    let mut value = seed
        .wrapping_add((x as u32).wrapping_mul(0x9e37_79b9))
        .wrapping_add((y as u32).wrapping_mul(0x85eb_ca6b));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value
}

/// `hash2` as a signed unit float — the cell noise the classic generators used.
pub fn noise2(seed: u32, x: i32, y: i32) -> f32 {
    (hash2(seed, x, y) & 0xffff) as f32 / 65535.0 * 2.0 - 1.0
}

/// Smooth (bilinear) value noise over a lattice of `cell` cells, in `-1..1`.
/// Elliptical blobs and dune fields want a field that is continuous, not the
/// per-cell hash, or every shape ends up with a sanded edge.
pub fn value_noise(seed: u32, x: f32, y: f32, cell: f32) -> f32 {
    let cell = cell.max(0.001);
    let (fx, fy) = (x / cell, y / cell);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    // Smoothstep so the lattice does not show as a grid of creases.
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let corner = |ox: f32, oy: f32| noise2(seed, (x0 + ox) as i32, (y0 + oy) as i32);
    let top = corner(0.0, 0.0) * (1.0 - sx) + corner(1.0, 0.0) * sx;
    let bottom = corner(0.0, 1.0) * (1.0 - sx) + corner(1.0, 1.0) * sx;
    top * (1.0 - sy) + bottom * sy
}

/// Two octaves of `value_noise`; enough shape for terrain patches without
/// making a fractal out of a 64-cell map.
pub fn fbm(seed: u32, x: f32, y: f32, cell: f32) -> f32 {
    value_noise(seed, x, y, cell) * 0.65 + value_noise(seed ^ 0x5bf0_3635, x, y, cell * 0.45) * 0.35
}
