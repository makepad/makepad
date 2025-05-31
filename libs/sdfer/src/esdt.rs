//! Rust port of the ESDT ("Euclidean Subpixel Distance Transform") algorithm.
//!
//!
//! This algorithm was originally published as the [`@use-gpu/glyph`](https://www.npmjs.com/package/@use-gpu/glyph)
//! `npm` package, and was described in <https://acko.net/blog/subpixel-distance-transform/>.

use crate::img::{Bitmap, Image2d, NDCursor, NDCursorExt as _, Unorm8};

// HACK(eddyb) only exists to allow toggling precision for testing purposes.
#[cfg(sdfer_use_f64_instead_of_f32)]
type f32 = f64;

#[derive(Copy, Clone, Debug)]
pub struct Params {
    pub pad: usize,
    pub radius: f32,
    pub cutoff: f32,
    pub solidify: bool,
    pub preprocess: bool,
    // FIXME(eddyb) implement.
    // pub postprocess: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            pad: 4,
            radius: 3.0,
            cutoff: 0.25,
            solidify: true,
            preprocess: false,
            // FIXME(eddyb) implement.
            // postprocess: false,
        }
    }
}

/// Opaque `struct` allowing buffer reuse between SDF computations, instead of
/// reallocating all the buffers every time.
#[derive(Default)]
pub struct ReusableBuffers(ReusableBuffers2d, ReusableBuffers1d);

// Convert grayscale glyph to SDF
pub fn glyph_to_sdf(
    glyph: &mut Image2d<Unorm8, impl AsMut<[Unorm8]> + AsRef<[Unorm8]>>,
    params: Params,
    reuse_bufs: Option<ReusableBuffers>,
) -> (Image2d<Unorm8>, ReusableBuffers) {
    
    
    if params.solidify {
        solidify_alpha(glyph.reborrow_mut());
    }
    glyph_to_esdt(glyph.reborrow_mut(), params, reuse_bufs)
}

// Solidify semi-transparent areas
fn solidify_alpha(mut glyph: Image2d<Unorm8, &mut [Unorm8]>) {
    let (w, h) = (glyph.width(), glyph.height());

    let mut mask: Image2d<u8> = Image2d::new(w, h);

    let get_data = |x: isize, y: isize| {
        if x >= 0 && (x as usize) < w && y >= 0 && (y as usize) < h {
            glyph[(x as usize, y as usize)]
        } else {
            Unorm8::MIN
        }
    };

    let mut masked = 0;

    // Mask pixels whose alpha matches their 4 adjacent neighbors (within 16 steps)
    // and who don't have black or white neighbors.
    for y in 0..(h as isize) {
        for x in 0..(w as isize) {
            let a = get_data(x, y);
            // FIXME(eddyb) audit all comparisons with `254` and try removing them.
            if a == Unorm8::MIN || a >= Unorm8::from_bits(254) {
                continue;
            }

            let l = get_data(x - 1, y);
            let r = get_data(x + 1, y);
            let t = get_data(x, y - 1);
            let b = get_data(x, y + 1);

            let (min, max) = [a, l, r, t, b]
                .into_iter()
                .map(|x| (x, x))
                .reduce(|(a_min, a_max), (b_min, b_max)| (a_min.min(b_min), a_max.max(b_max)))
                .unwrap();

            let [a, min, max] = [a, min, max].map(Unorm8::to_bits);

            // FIXME(eddyb) audit all comparisons with `254` and try removing them.
            if (max - min) < 16 && min > 0 && max < 254 {
                // NOTE(eddyb) `min > 0` guarantees all neighbors are in-bounds.
                let (x, y) = (x as usize, y as usize);

                // Spread to 4 neighbors with max
                mask[(x - 1, y)] = mask[(x - 1, y)].max(a);
                mask[(x, y - 1)] = mask[(x, y - 1)].max(a);
                mask[(x, y)] = a;
                mask[(x + 1, y)] = mask[(x + 1, y)].max(a);
                mask[(x, y + 1)] = mask[(x, y + 1)].max(a);
                masked += 1;
            }
        }
    }

    if masked == 0 {
        return;
    }

    let get_mask = |x: isize, y: isize| {
        if x >= 0 && (x as usize) < w && y >= 0 && (y as usize) < h {
            mask[(x as usize, y as usize)]
        } else {
            0
        }
    };

    // Sample 3x3 area for alpha normalization factor
    for y in 0..(h as isize) {
        for x in 0..(w as isize) {
            let a = &mut glyph[(x as usize, y as usize)];
            // FIXME(eddyb) audit all comparisons with `254` and try removing them.
            if *a == Unorm8::MIN || *a >= Unorm8::from_bits(254) {
                continue;
            }

            let c = get_mask(x, y);

            let l = get_mask(x - 1, y);
            let r = get_mask(x + 1, y);
            let t = get_mask(x, y - 1);
            let b = get_mask(x, y + 1);

            let tl = get_mask(x - 1, y - 1);
            let tr = get_mask(x + 1, y - 1);
            let bl = get_mask(x - 1, y + 1);
            let br = get_mask(x + 1, y + 1);

            if let Some(m) = [c, l, r, t, b, tl, tr, bl, br]
                .into_iter()
                .find(|&x| x != 0)
            {
                *a = Unorm8::from_bits((a.to_bits() as f32 / m as f32 * 255.0) as u8);
            }
        }
    }
}

// Convert grayscale or color glyph to SDF using subpixel distance transform
fn glyph_to_esdt(
    mut glyph: Image2d<Unorm8, &mut [Unorm8]>,
    params: Params,
    reuse_bufs: Option<ReusableBuffers>,
) -> (Image2d<Unorm8>, ReusableBuffers) {
    // FIXME(eddyb) use `Params` itself directly in more places.
    let Params {
        pad,
        radius,
        cutoff,
        solidify: _,
        preprocess,
    } = params;

    let wp = glyph.width() + pad * 2;
    let hp = glyph.height() + pad * 2;

    let mut state = State::from_glyph(glyph.reborrow_mut(), params, reuse_bufs);

    state.esdt_outer_and_inner(wp, hp);

    // FIXME(eddyb) implement.
    // if postprocess { state.relax_subpixel_offsets(glyph, pad); }

    let mut sdf = Image2d::from_fn(wp, hp, |x, y| {
        let i = y * wp + x;
        let ReusableBuffers2d { xo, yo, xi, yi, .. } = &state.bufs_2d;
        let outer = ((xo[i].powi(2) + yo[i].powi(2)).sqrt() - 0.5).max(0.0);
        let inner = ((xi[i].powi(2) + yi[i].powi(2)).sqrt() - 0.5).max(0.0);
        let d = if outer >= inner { outer } else { -inner };
        Unorm8::encode(1.0 - (d / radius + cutoff))
    });

    if !preprocess {
        paint_into_distance_field(&mut sdf, glyph.reborrow(), params);
    }

    (sdf, ReusableBuffers(state.bufs_2d, state.reuse_bufs_1d))
}

// Helpers
fn is_black(x: f32) -> bool {
    x == 0.0
}
fn is_white(x: f32) -> bool {
    x == 1.0
}
fn is_solid(x: f32) -> bool {
    x == 0.0 || x == 1.0
}

// Paint original alpha channel into final SDF when gray
fn paint_into_distance_field(
    sdf: &mut Image2d<Unorm8>,
    glyph: Image2d<Unorm8, &[Unorm8]>,
    params: Params,
) {
    let Params {
        pad,
        radius,
        cutoff,
        ..
    } = params;

    for y in 0..glyph.height() {
        for x in 0..glyph.width() {
            let a = glyph[(x, y)].decode();
            if !is_solid(a) {
                let d = 0.5 - a;
                sdf[(x + pad, y + pad)] = Unorm8::encode(1.0 - (d / radius + cutoff));
            }
        }
    }
}

/// 2D buffers, which get reused (see also `ReusableBuffers` itself).
#[derive(Default)]
struct ReusableBuffers2d {
    // FIXME(eddyb) group `outer` with `{x,y}o`.
    outer: Bitmap,
    // FIXME(eddyb) group `inner` with `{x,y}i``.
    inner: Bitmap,

    xo: Vec<f32>,
    yo: Vec<f32>,
    xi: Vec<f32>,
    yi: Vec<f32>,
}

struct State {
    // FIXME(eddyb) do the grouping suggested in `ReusableBuffers2d`, to have
    // `outer` and `inner` fields in here, to use instead of `ReusableBuffers2d`.
    bufs_2d: ReusableBuffers2d,
    reuse_bufs_1d: ReusableBuffers1d,
}

impl State {
    fn from_glyph(
        mut glyph: Image2d<Unorm8, &mut [Unorm8]>,
        params: Params,
        reuse_bufs: Option<ReusableBuffers>,
    ) -> Self {
        let Params {
            pad,
            // FIXME(eddyb) should this still be taken as a separate `bool`?
            preprocess: relax,
            ..
        } = params;

        let wp = glyph.width() + pad * 2;
        let hp = glyph.height() + pad * 2;
        let np = wp * hp;

        let ReusableBuffers(bufs_2d, reuse_bufs_1d) = reuse_bufs.unwrap_or_default();
        let mut state = Self {
            bufs_2d,
            reuse_bufs_1d,
        };
        let ReusableBuffers2d {
            outer,
            inner,
            xo,
            yo,
            xi,
            yi,
        } = &mut state.bufs_2d;

        outer.resize_and_fill_with(wp, hp, true);
        inner.resize_and_fill_with(wp, hp, false);
        for buf2d in [&mut *xo, yo, xi, yi] {
            buf2d.clear();
            buf2d.resize(np, 0.0);
        }

        for y in 0..glyph.height() {
            for x in 0..glyph.width() {
                let a = &mut glyph[(x, y)];
                if *a == Unorm8::MIN {
                    continue;
                }

                // FIXME(eddyb) audit all comparisons with `254` and try removing them,
                // especially this step that modifies the `glyph` itself.
                if *a >= Unorm8::from_bits(254) {
                    // Fix for bad rasterizer rounding
                    *a = Unorm8::MAX;

                    outer.at(x + pad, y + pad).set(false);
                    inner.at(x + pad, y + pad).set(true);
                } else {
                    outer.at(x + pad, y + pad).set(false);
                    inner.at(x + pad, y + pad).set(false);
                }
            }
        }

        //
        // Generate subpixel offsets for all border pixels
        //

        let get_data = |x: isize, y: isize| {
            if x >= 0 && (x as usize) < glyph.width() && y >= 0 && (y as usize) < glyph.height() {
                glyph[(x as usize, y as usize)].decode()
            } else {
                0.0
            }
        };

        // Make vector from pixel center to nearest boundary
        for y in 0..(glyph.height() as isize) {
            for x in 0..(glyph.width() as isize) {
                let c = get_data(x, y);
                // NOTE(eddyb) `j - 1` (X-) / `j - wp` (Y-) positive (`pad >= 1`).
                let j = ((y as usize) + pad) * wp + (x as usize) + pad;

                if !is_solid(c) {
                    let dc = c - 0.5;

                    // NOTE(eddyb) l(eft) r(ight) t(op) b(ottom)
                    let l = get_data(x - 1, y);
                    let r = get_data(x + 1, y);
                    let t = get_data(x, y - 1);
                    let b = get_data(x, y + 1);

                    let tl = get_data(x - 1, y - 1);
                    let tr = get_data(x + 1, y - 1);
                    let bl = get_data(x - 1, y + 1);
                    let br = get_data(x + 1, y + 1);

                    let ll = (tl + l * 2.0 + bl) / 4.0;
                    let rr = (tr + r * 2.0 + br) / 4.0;
                    let tt = (tl + t * 2.0 + tr) / 4.0;
                    let bb = (bl + b * 2.0 + br) / 4.0;

                    let (min, max) = [l, r, t, b, tl, tr, bl, br]
                        .into_iter()
                        .map(|x| (x, x))
                        .reduce(|(a_min, a_max), (b_min, b_max)| {
                            (a_min.min(b_min), a_max.max(b_max))
                        })
                        .unwrap();

                    if min > 0.0 {
                        // Interior creases
                        inner.at(x as usize + pad, y as usize + pad).set(true);
                        continue;
                    }
                    if max < 1.0 {
                        // Exterior creases
                        outer.at(x as usize + pad, y as usize + pad).set(true);
                        continue;
                    }

                    let mut dx = rr - ll;
                    let mut dy = bb - tt;
                    let dl = 1.0 / (dx.powi(2) + dy.powi(2)).sqrt();
                    dx *= dl;
                    dy *= dl;

                    xo[j] = -dc * dx;
                    yo[j] = -dc * dy;
                } else if is_white(c) {
                    // NOTE(eddyb) l(eft) r(ight) t(op) b(ottom)
                    let l = get_data(x - 1, y);
                    let r = get_data(x + 1, y);
                    let t = get_data(x, y - 1);
                    let b = get_data(x, y + 1);

                    if is_black(l) {
                        xo[j - 1] = 0.4999;
                        outer.at(x as usize + pad - 1, y as usize + pad).set(false);
                        inner.at(x as usize + pad - 1, y as usize + pad).set(false);
                    }
                    if is_black(r) {
                        xo[j + 1] = -0.4999;
                        outer.at(x as usize + pad + 1, y as usize + pad).set(false);
                        inner.at(x as usize + pad + 1, y as usize + pad).set(false);
                    }

                    if is_black(t) {
                        yo[j - wp] = 0.4999;
                        outer.at(x as usize + pad, y as usize + pad - 1).set(false);
                        inner.at(x as usize + pad, y as usize + pad - 1).set(false);
                    }
                    if is_black(b) {
                        yo[j + wp] = -0.4999;
                        outer.at(x as usize + pad, y as usize + pad + 1).set(false);
                        inner.at(x as usize + pad, y as usize + pad + 1).set(false);
                    }
                }
            }
        }

        // Blend neighboring offsets but preserve normal direction
        // Uses xo as input, xi as output
        // Improves quality slightly, but slows things down.
        if relax {
            let check_cross = |nx, ny, dc, dl, dr, dxl, dyl, dxr, dyr| {
                ((dxl * nx + dyl * ny) * (dc * dl) > 0.0)
                    && ((dxr * nx + dyr * ny) * (dc * dr) > 0.0)
                    && ((dxl * dxr + dyl * dyr) * (dl * dr) > 0.0)
            };

            for y in 0..(glyph.height() as isize) {
                for x in 0..(glyph.width() as isize) {
                    // NOTE(eddyb) `j - 1` (X-) / `j - wp` (Y-) positive (`pad >= 1`).
                    let j = ((y as usize) + pad) * wp + (x as usize) + pad;

                    let nx = xo[j];
                    let ny = yo[j];
                    if nx == 0.0 && ny == 0.0 {
                        continue;
                    }

                    // NOTE(eddyb) c(enter) l(eft) r(ight) t(op) b(ottom)
                    let c = get_data(x, y);
                    let l = get_data(x - 1, y);
                    let r = get_data(x + 1, y);
                    let t = get_data(x, y - 1);
                    let b = get_data(x, y + 1);

                    let dxl = xo[j - 1];
                    let dxr = xo[j + 1];
                    let dxt = xo[j - wp];
                    let dxb = xo[j + wp];

                    let dyl = yo[j - 1];
                    let dyr = yo[j + 1];
                    let dyt = yo[j - wp];
                    let dyb = yo[j + wp];

                    let mut dx = nx;
                    let mut dy = ny;
                    let mut dw = 1.0;

                    let dc = c - 0.5;
                    let dl = l - 0.5;
                    let dr = r - 0.5;
                    let dt = t - 0.5;
                    let db = b - 0.5;

                    if !is_solid(l) && !is_solid(r) {
                        if check_cross(nx, ny, dc, dl, dr, dxl, dyl, dxr, dyr) {
                            dx += (dxl + dxr) / 2.0;
                            dy += (dyl + dyr) / 2.0;
                            dw += 1.0;
                        }
                    }

                    if !is_solid(t) && !is_solid(b) {
                        if check_cross(nx, ny, dc, dt, db, dxt, dyt, dxb, dyb) {
                            dx += (dxt + dxb) / 2.0;
                            dy += (dyt + dyb) / 2.0;
                            dw += 1.0;
                        }
                    }

                    if !is_solid(l) && !is_solid(t) {
                        if check_cross(nx, ny, dc, dl, dt, dxl, dyl, dxt, dyt) {
                            dx += (dxl + dxt - 1.0) / 2.0;
                            dy += (dyl + dyt - 1.0) / 2.0;
                            dw += 1.0;
                        }
                    }

                    if !is_solid(r) && !is_solid(t) {
                        if check_cross(nx, ny, dc, dr, dt, dxr, dyr, dxt, dyt) {
                            dx += (dxr + dxt + 1.0) / 2.0;
                            dy += (dyr + dyt - 1.0) / 2.0;
                            dw += 1.0;
                        }
                    }

                    if !is_solid(l) && !is_solid(b) {
                        if check_cross(nx, ny, dc, dl, db, dxl, dyl, dxb, dyb) {
                            dx += (dxl + dxb - 1.0) / 2.0;
                            dy += (dyl + dyb + 1.0) / 2.0;
                            dw += 1.0;
                        }
                    }

                    if !is_solid(r) && !is_solid(b) {
                        if check_cross(nx, ny, dc, dr, db, dxr, dyr, dxb, dyb) {
                            dx += (dxr + dxb + 1.0) / 2.0;
                            dy += (dyr + dyb + 1.0) / 2.0;
                            dw += 1.0;
                        }
                    }

                    let nn = (nx * nx + ny * ny).sqrt();
                    let ll = (dx * nx + dy * ny) / nn;

                    dx = nx * ll / dw / nn;
                    dy = ny * ll / dw / nn;

                    xi[j] = dx;
                    yi[j] = dy;
                }
            }
        }

        // Produce zero points for positive and negative DF, at +0.5 / -0.5.
        // Splits xs into xo/xi
        for y in 0..(glyph.height() as isize) {
            for x in 0..(glyph.width() as isize) {
                // NOTE(eddyb) `j - 1` (X-) / `j - wp` (Y-) positive (`pad >= 1`).
                let j = ((y as usize) + pad) * wp + (x as usize) + pad;

                // NOTE(eddyb) `if relax` above changed `xs`/`ys` in the original.
                let (nx, ny) = if relax {
                    (xi[j], yi[j])
                } else {
                    (xo[j], yo[j])
                };
                if nx == 0.0 && ny == 0.0 {
                    continue;
                }

                let nn = (nx.powi(2) + ny.powi(2)).sqrt();

                let sx = if ((nx / nn).abs() - 0.5) > 0.0 {
                    nx.signum() as isize
                } else {
                    0
                };
                let sy = if ((ny / nn).abs() - 0.5) > 0.0 {
                    ny.signum() as isize
                } else {
                    0
                };

                let c = get_data(x, y);
                let d = get_data(x + sx, y + sy);
                // FIXME(eddyb) is this inefficient? (was `Math.sign(d - c)`)
                let s = (d - c).total_cmp(&0.0) as i8 as f32;

                let dlo = (nn + 0.4999 * s) / nn;
                let dli = (nn - 0.4999 * s) / nn;

                xo[j] = nx * dlo;
                yo[j] = ny * dlo;
                xi[j] = nx * dli;
                yi[j] = ny * dli;
            }
        }

        state
    }

    fn esdt_outer_and_inner(&mut self, w: usize, h: usize) {
        {
            let Self {
                bufs_2d:
                    ReusableBuffers2d {
                        outer,
                        inner,
                        xo,
                        yo,
                        xi,
                        yi,
                    },
                reuse_bufs_1d,
            } = self;
            esdt(outer, xo, yo, w, h, reuse_bufs_1d);
            esdt(inner, xi, yi, w, h, reuse_bufs_1d);
        }
    }
}

// 2D subpixel distance transform by unconed
// extended from Felzenszwalb & Huttenlocher https://cs.brown.edu/~pff/papers/dt-final.pdf
fn esdt(
    mask: &mut Bitmap,
    xs: &mut [f32],
    ys: &mut [f32],
    w: usize,
    h: usize,
    reuse_bufs_1d: &mut ReusableBuffers1d,
) {
    reuse_bufs_1d.critical_minima.clear();
    reuse_bufs_1d.critical_minima.reserve(w.max(h));

    let mut xs = Image2d::from_storage(w, h, xs);
    let mut ys = Image2d::from_storage(w, h, ys);

    for x in 0..w {
        let mut mask_xy_cursor = mask
            .cursor_at(0, 0)
            .zip(
                // FIXME(eddyb) combine `xs` and `ys` into the same `Image2d`.
                ys.cursor_at(0, 0).zip(xs.cursor_at(0, 0)),
            )
            .map_abs_and_rel(move |y| (x, y), |dy| (0, dy));
        mask_xy_cursor.reset(0);

        esdt1d(mask_xy_cursor, h, reuse_bufs_1d)
    }
    for y in 0..h {
        let mut mask_xy_cursor = mask
            .cursor_at(0, 0)
            .zip(
                // FIXME(eddyb) combine `xs` and `ys` into the same `Image2d`.
                xs.cursor_at(0, 0).zip(ys.cursor_at(0, 0)),
            )
            .map_abs_and_rel(move |x| (x, y), |dx| (dx, 0));
        mask_xy_cursor.reset(0);

        esdt1d(mask_xy_cursor, w, reuse_bufs_1d)
    }
}

/// 1D buffers (for `esdt1d`), which get reused between calls.
//
// FIXME(eddyb) the name is outdated now that there's only one buffer.
#[derive(Default)]
struct ReusableBuffers1d {
    critical_minima: Vec<CriticalMinimum>,
}

// FIXME(eddyb) clean up the names after all the refactors.
struct CriticalMinimum {
    // FIXME(eddyb) this is really just a position, since it's not used to
    // index anything indirectly anymore, but rather indicates the original `q`,
    // and is used to compare against it in the second iteration of `esdt1d`.
    v: usize, // Array index

    z: f32, // Voronoi threshold
    f: f32, // Squared distance
    b: f32, // Subpixel offset parallel
    t: f32, // Subpixel offset perpendicular
}

// 1D subpixel distance transform
fn esdt1d(
    mut mask_xy_cursor: impl for<'a> NDCursor<
        'a,
        usize,
        RefMut = (crate::img::BitmapEntry<'a>, (&'a mut f32, &'a mut f32)),
    >,
    // FIXME(eddyb) provide this through the cursor, maybe?
    length: usize,
    reuse_bufs_1d: &mut ReusableBuffers1d,
) {
    // FIXME(eddyb) this is a pretty misleading name.
    const INF: f32 = 1e10;

    let cm = &mut reuse_bufs_1d.critical_minima;
    cm.clear();

    {
        let (mask, (&mut dx, &mut dy)) = mask_xy_cursor.get_mut();
        cm.push(CriticalMinimum {
            v: 0,
            z: -INF,
            f: if mask.get() { INF } else { dy.powi(2) },

            b: dx,
            t: dy,
        });
        mask_xy_cursor.advance(1);
    }

    // Scan along array and build list of critical minima
    for q in 1..length {
        // Perpendicular
        let (mask, (&mut dx, &mut dy)) = mask_xy_cursor.get_mut();
        let fq = if mask.get() { INF } else { dy.powi(2) };
        mask_xy_cursor.advance(1);

        // Parallel
        let qs = q as f32 + dx;
        let q2 = qs.powi(2);

        // Remove any minima eclipsed by this one
        let mut s;
        loop {
            let r = &cm[cm.len() - 1];

            s = (fq - r.f + q2 - r.b.powi(2)) / (qs - r.b) / 2.0;

            if !(s <= r.z) {
                break;
            }

            cm.pop();
            if cm.len() == 0 {
                break;
            }
        }

        // Add to minima list
        cm.push(CriticalMinimum {
            v: q,
            z: s,
            f: fq,
            b: qs,
            t: dy,
        });
    }

    mask_xy_cursor.reset(0);

    // Resample array based on critical minima
    {
        let mut k = 0;
        for q in 0..length {
            // Skip eclipsed minima
            while k + 1 < cm.len() && cm[k + 1].z < q as f32 {
                k += 1;
            }

            let r = &cm[k];

            // Distance from integer index to subpixel location of minimum
            let rq = r.b - q as f32;

            let (mut mask, (dx, dy)) = mask_xy_cursor.get_mut();
            *dx = rq;
            *dy = r.t;
            // Mark cell as having propagated
            if r.v != q {
                mask.set(false);
            }
            mask_xy_cursor.advance(1);
        }
    }
}
