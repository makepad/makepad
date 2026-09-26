//! The knob's revolve profile, baked the way the Material Bench bakes it.
//!
//! The cross-section of a turned knob is a Bezier drawn from the centre
//! (x = 0) to the rim (x = 1), height 1 at the top. What tilts a normal is
//! its slope, so it is baked to -dh/dr per normalised radius, alongside the
//! height and, for the cast shadow, the solid sliced by height: for height
//! i/255 the outermost radius that is still that high.
use makepad_widgets::*;

const TAPS: usize = 256;

/// The bench's dark "glossy" preset: a flat crown, a crease stepping down to
/// a skirt, and a roll at the rim. Anchors are [x, y, type]: 0 point,
/// 1 smooth, 2 corner.
const GLOSSY_PROFILE: [[f64; 3]; 5] = [
    [0.0, 1.0, 1.0],
    [0.42, 0.95, 1.0],
    [0.50, 0.56, 2.0],
    [0.90, 0.50, 1.0],
    [1.0, 0.0, 2.0],
];
const GLOSSY_SMOOTH: usize = 3;

struct Anchor {
    x: f64,
    y: f64,
    ty: u8,
    ix: f64,
    iy: f64,
    ox: f64,
    oy: f64,
}

/// fixTangents: automatic tangents, monotone at a local extremum
/// (Fritsch-Carlson), clamped so height stays a function of x.
fn fix_tangents(p: &mut [Anchor]) {
    let n = p.len();
    for i in 0..n {
        if p[i].ty == 0 {
            p[i].ix = 0.0;
            p[i].iy = 0.0;
            p[i].ox = 0.0;
            p[i].oy = 0.0;
            continue;
        }
        let (ax, ay) = (p[i.saturating_sub(1)].x, p[i.saturating_sub(1)].y);
        let (bx, by) = (p[(i + 1).min(n - 1)].x, p[(i + 1).min(n - 1)].y);
        let mut ty = (by - ay) / 6.0;
        if i > 0 && i < n - 1 && (p[i].y - ay) * (by - p[i].y) <= 0.0 {
            ty = 0.0;
        }
        p[i].ox = (bx - ax) / 6.0;
        p[i].oy = ty;
        p[i].ix = -p[i].ox;
        p[i].iy = -ty;
        let back = if i > 0 { (p[i].x - p[i - 1].x) / 3.0 } else { 1.0 };
        let fwd = if i < n - 1 { (p[i + 1].x - p[i].x) / 3.0 } else { 1.0 };
        if p[i].ty == 1 {
            if p[i].ox < 0.0 {
                p[i].ox = 0.0;
                p[i].oy = 0.0;
            }
            let lim = back.min(fwd);
            if p[i].ox > lim {
                let s = lim / p[i].ox;
                p[i].ox *= s;
                p[i].oy *= s;
            }
            p[i].ix = -p[i].ox;
            p[i].iy = -p[i].oy;
        } else {
            p[i].ox = p[i].ox.min(fwd).max(0.0);
            p[i].ix = p[i].ix.max(-back).min(0.0);
        }
    }
}

fn polyline(p: &[Anchor]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for w in p.windows(2) {
        let (p0, p1) = (&w[0], &w[1]);
        let (c1x, c1y) = (p0.x + p0.ox, p0.y + p0.oy);
        let (c2x, c2y) = (p1.x + p1.ix, p1.y + p1.iy);
        for k in 0..=24 {
            let t = k as f64 / 24.0;
            let u = 1.0 - t;
            out.push((
                u * u * u * p0.x + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * p1.x,
                u * u * u * p0.y + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * p1.y,
            ));
        }
    }
    out
}

fn resample(p: &[Anchor], n: usize) -> Vec<f64> {
    let pts = polyline(p);
    (0..n)
        .map(|i| {
            let r = i as f64 / (n - 1) as f64;
            if r <= pts[0].0 {
                return pts[0].1;
            }
            for k in 0..pts.len() - 1 {
                if r >= pts[k].0 && r <= pts[k + 1].0 {
                    let t = (r - pts[k].0) / (pts[k + 1].0 - pts[k].0).max(1e-6);
                    return pts[k].1 + t * (pts[k + 1].1 - pts[k].1);
                }
            }
            pts[pts.len() - 1].1
        })
        .collect()
}

/// Slope, height and the height-sliced radius, one RGBA float texel each.
pub fn bake_profile(anchors: &[[f64; 3]], smooth: usize) -> Vec<f32> {
    let mut p: Vec<Anchor> = anchors
        .iter()
        .map(|a| Anchor {
            x: a[0],
            y: a[1],
            ty: a[2] as u8,
            ix: 0.0,
            iy: 0.0,
            ox: 0.0,
            oy: 0.0,
        })
        .collect();
    fix_tangents(&mut p);
    let n = TAPS;
    let h = resample(&p, n);
    let mut sl: Vec<f64> = (0..n)
        .map(|i| -(h[(i + 1).min(n - 1)] - h[i.saturating_sub(1)]) * (n - 1) as f64 / 2.0)
        .collect();
    if smooth > 0 {
        let w = smooth as isize;
        sl = (0..n as isize)
            .map(|i| {
                let mut acc = 0.0;
                for k in -w..=w {
                    acc += sl[(i + k).clamp(0, n as isize - 1) as usize];
                }
                acc / (2 * w + 1) as f64
            })
            .collect();
    }
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let z = i as f64 / (n - 1) as f64;
        let mut rmax = 0.0;
        for k in (0..n).rev() {
            if h[k] >= z - 1e-6 {
                rmax = k as f64 / (n - 1) as f64;
                break;
            }
        }
        data.push(sl[i].clamp(-8.0, 8.0) as f32);
        data.push(h[i].clamp(0.0, 1.0) as f32);
        data.push(rmax as f32);
        data.push(1.0);
    }
    data
}

pub fn knob_profile_texture(cx: &mut Cx) -> Texture {
    Texture::new_with_format(
        cx,
        TextureFormat::VecRGBAf32 {
            width: TAPS,
            height: 1,
            data: Some(bake_profile(&GLOSSY_PROFILE, GLOSSY_SMOOTH)),
            updated: TextureUpdated::Full,
        },
    )
}
