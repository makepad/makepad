//! MilkDrop's noise textures, generated once (deterministically) and
//! uploaded.
//!
//! | name | size | zoom |
//! |---|---|---|
//! | `noise_lq` | 256² | 1 |
//! | `noise_lq_lite` | 32² | 1 |
//! | `noise_mq` | 256² | 4 |
//! | `noise_hq` | 256² | 8 |
//! | `noisevol_lq` | 32³ | 1 |
//! | `noisevol_hq` | 32³ | 4 |
//!
//! Each channel is independent uniform noise; a zoom above 1 places random
//! values every `zoom` texels and fills between them with cubic
//! interpolation, as MilkDrop 2 does, so the "hq" textures are smooth.
//! Everything wraps. Volumes are stored as their 32 slices in an 8×4 atlas
//! (makepad has no 3D textures); `DrawMilkBase::noisevol_*` sample it.

/// BGRA pixels, row-major.
pub struct NoiseImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Catmull-Rom through four samples at t in [0, 1).
fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

fn pack(c: [f32; 4]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (b(c[3]) << 24) | (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2])
}

/// Square 2D noise of `size` with interpolation `zoom`.
pub fn noise_2d(size: usize, zoom: usize, seed: u64) -> NoiseImage {
    let zoom = zoom.max(1);
    let cells = (size / zoom).max(1);
    let mut rng = Rng(seed | 1);
    let lattice: Vec<[f32; 4]> = (0..cells * cells).map(|_| [rng.next(), rng.next(), rng.next(), rng.next()]).collect();
    let at = |x: isize, y: isize| -> [f32; 4] {
        let c = cells as isize;
        lattice[(y.rem_euclid(c) * c + x.rem_euclid(c)) as usize]
    };
    let mut data = vec![0u32; size * size];
    for y in 0..size {
        for x in 0..size {
            if zoom == 1 {
                data[y * size + x] = pack(at(x as isize, y as isize));
                continue;
            }
            let cx = (x / zoom) as isize;
            let cy = (y / zoom) as isize;
            let tx = (x % zoom) as f32 / zoom as f32;
            let ty = (y % zoom) as f32 / zoom as f32;
            let mut out = [0.0f32; 4];
            for (ch, o) in out.iter_mut().enumerate() {
                let row = |dy: isize| {
                    cubic(at(cx - 1, cy + dy)[ch], at(cx, cy + dy)[ch], at(cx + 1, cy + dy)[ch], at(cx + 2, cy + dy)[ch], tx)
                };
                *o = cubic(row(-1), row(0), row(1), row(2), ty);
            }
            data[y * size + x] = pack(out);
        }
    }
    NoiseImage { width: size, height: size, data }
}

/// 32³ volume noise with interpolation `zoom`, as an 8×4 atlas of slices
/// (256×128).
pub fn noise_volume(zoom: usize, seed: u64) -> NoiseImage {
    const N: usize = 32;
    let zoom = zoom.max(1);
    let cells = (N / zoom).max(1);
    let mut rng = Rng(seed | 1);
    let lattice: Vec<[f32; 4]> =
        (0..cells * cells * cells).map(|_| [rng.next(), rng.next(), rng.next(), rng.next()]).collect();
    let at = |x: isize, y: isize, z: isize| -> [f32; 4] {
        let c = cells as isize;
        lattice[((z.rem_euclid(c) * c + y.rem_euclid(c)) * c + x.rem_euclid(c)) as usize]
    };
    let mut data = vec![0u32; 256 * 128];
    for z in 0..N {
        let tile_x = (z % 8) * N;
        let tile_y = (z / 8) * N;
        for y in 0..N {
            for x in 0..N {
                let value = if zoom == 1 {
                    at(x as isize, y as isize, z as isize)
                } else {
                    let (cx, cy, cz) = ((x / zoom) as isize, (y / zoom) as isize, (z / zoom) as isize);
                    let (tx, ty, tz) = (
                        (x % zoom) as f32 / zoom as f32,
                        (y % zoom) as f32 / zoom as f32,
                        (z % zoom) as f32 / zoom as f32,
                    );
                    let mut out = [0.0f32; 4];
                    for (ch, o) in out.iter_mut().enumerate() {
                        let row = |dy: isize, dz: isize| {
                            cubic(
                                at(cx - 1, cy + dy, cz + dz)[ch],
                                at(cx, cy + dy, cz + dz)[ch],
                                at(cx + 1, cy + dy, cz + dz)[ch],
                                at(cx + 2, cy + dy, cz + dz)[ch],
                                tx,
                            )
                        };
                        let plane = |dz: isize| cubic(row(-1, dz), row(0, dz), row(1, dz), row(2, dz), ty);
                        *o = cubic(plane(-1), plane(0), plane(1), plane(2), tz);
                    }
                    out
                };
                data[(tile_y + y) * 256 + tile_x + x] = pack(value);
            }
        }
    }
    NoiseImage { width: 256, height: 128, data }
}

/// A stand-in for a named image texture a preset samples but the host does
/// not supply (`sampler_worms`, `sampler_clouds`, ...): smooth coloured
/// noise, different per name, so a preset still gets texture-like detail
/// (projectM substitutes a random texture the same way).
pub fn stand_in(name: &str) -> NoiseImage {
    let seed = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x1000_0000_01b3));
    let zoom = 4 << (seed % 3) as usize;
    noise_2d(256, zoom, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_determinism() {
        let a = noise_2d(256, 8, 3);
        let b = noise_2d(256, 8, 3);
        assert_eq!((a.width, a.height, a.data.len()), (256, 256, 65536));
        assert_eq!(a.data, b.data);
        let v = noise_volume(4, 5);
        assert_eq!((v.width, v.height), (256, 128));
        assert_ne!(stand_in("worms").data, stand_in("clouds").data);
    }

    #[test]
    fn zoomed_noise_is_smooth_and_wraps() {
        let img = noise_2d(64, 8, 9);
        let red = |x: usize, y: usize| ((img.data[y * 64 + x] >> 16) & 255) as i32;
        let mut max_step = 0;
        for y in 0..64 {
            for x in 0..64 {
                max_step = max_step.max((red(x, y) - red((x + 1) % 64, y)).abs());
            }
        }
        // Neighbouring texels differ by a fraction of the range, including
        // across the wrap seam.
        assert!(max_step < 80, "{max_step}");
    }
}
