//! Two-channel normal maps: X and Y are stored, Z is rebuilt in the shader
//! (`z = sqrt(1 - x² - y²)`). A tangent-space normal map is stored as BC5
//! (two independent BC4 channels), encoded once at build time, and goes to
//! the GPU as is wherever BC is sampled. The error that matters is the angle
//! between the rebuilt normal and the true one, measured here against the
//! float normals the map was made from, next to the UASTC paths colour
//! textures take.

use makepad_texcomp::{bc7, rgtc, transcode, uastc::{self, Quality}};

/// A 256x256 tangent-space normal map from a height field with the content
/// real maps carry: soft noise (stone), round rivets, V-grooved panel lines
/// and a bevelled plate. Returns the float normals and the 8-bit RGBA map.
fn normal_map() -> (Vec<[f32; 3]>, Vec<u8>, u32, u32) {
    let (w, h) = (256usize, 256usize);
    let hash = |x: i32, y: i32| { let mut n = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1); n ^= n >> 15; n = n.wrapping_mul(0x85eb_ca6b); n ^= n >> 13; (n & 0xffff) as f32 / 65535.0 };
    let value = |x: f32, y: f32| {
        let (xi, yi) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - xi as f32, y - yi as f32);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let a = hash(xi, yi) + (hash(xi + 1, yi) - hash(xi, yi)) * sx;
        let b = hash(xi, yi + 1) + (hash(xi + 1, yi + 1) - hash(xi, yi + 1)) * sx;
        a + (b - a) * sy
    };
    let height = |x: f32, y: f32| {
        let mut v = 0.0;
        let mut amp = 1.5;
        let mut f = 1.0 / 16.0;
        for _ in 0..4 { v += value(x * f, y * f) * amp; amp *= 0.5; f *= 2.0; }
        // Rivets: hemispheres of radius 5 on a 32-texel grid.
        let (rx, ry) = ((x % 32.0) - 16.0, (y % 32.0) - 16.0);
        let r2 = rx * rx + ry * ry;
        if r2 < 25.0 { v += (25.0 - r2).sqrt() * 0.8; }
        // Panel grooves every 64 texels, 3 wide, V-shaped.
        for d in [x % 64.0, y % 64.0] { if d < 3.0 { v -= (1.5 - (d - 1.5).abs()) * 1.5; } }
        // A bevelled plate in one corner.
        if x > 140.0 && x < 240.0 && y > 140.0 && y < 240.0 {
            let edge = (x - 140.0).min(240.0 - x).min(y - 140.0).min(240.0 - y);
            v += edge.min(6.0) * 0.7;
        }
        v
    };
    let mut normals = Vec::with_capacity(w * h);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h { for x in 0..w {
        let (fx, fy) = (x as f32, y as f32);
        let dx = height(fx + 0.5, fy) - height(fx - 0.5, fy);
        let dy = height(fx, fy + 0.5) - height(fx, fy - 0.5);
        let n = [-dx, -dy, 1.0];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = [n[0] / l, n[1] / l, n[2] / l];
        normals.push(n);
        rgba.extend(n.iter().map(|c| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8));
        rgba.push(255);
    } }
    (normals, rgba, w as u32, h as u32)
}

/// The shader's normal from sampled X and Y (bytes), Z rebuilt.
fn rebuilt(x: u8, y: u8) -> [f32; 3] {
    let (x, y) = (x as f32 / 255.0 * 2.0 - 1.0, y as f32 / 255.0 * 2.0 - 1.0);
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    let l = (x * x + y * y + z * z).sqrt();
    [x / l, y / l, z / l]
}

/// (mean, 99th percentile, max) angle in degrees between the true normals
/// and XY bytes (`stride` bytes per texel, X then Y) with Z rebuilt.
fn angles(truth: &[[f32; 3]], xy: &[u8], stride: usize) -> (f32, f32, f32) {
    let mut a: Vec<f32> = truth.iter().zip(xy.chunks_exact(stride)).map(|(t, p)| {
        let n = rebuilt(p[0], p[1]);
        (t[0] * n[0] + t[1] * n[1] + t[2] * n[2]).clamp(-1.0, 1.0).acos().to_degrees()
    }).collect();
    let mean = a.iter().sum::<f32>() / a.len() as f32;
    a.sort_by(f32::total_cmp);
    (mean, a[a.len() * 99 / 100], *a.last().unwrap())
}

/// Every path's error against the true normals. The stored form is BC5 of
/// X and Y (built once; loads upload it as is where BC is sampled). It is
/// locked to stay within a degree on average, and ahead of the UASTC paths
/// (the colour textures' stored form) that it replaces for normal maps.
#[test]
fn bc5_normals_rebuild_z_within_a_degree() {
    let (truth, rgba, w, h) = normal_map();
    // UASTC of X and Y with Z (blue) zeroed: every mode then fits the two
    // channels the shader reads.
    let xy0: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], 0, 255]).collect();
    let u_rgb = uastc::encode_image(&rgba, w, h, Quality::Fast, 0).unwrap();
    let u_xy = uastc::encode_image(&xy0, w, h, Quality::Fast, 0).unwrap();
    let decode = |u: &[u8]| uastc::decode_image(u, w, h).unwrap();
    let rows = [
        ("8-bit source, Z rebuilt", angles(&truth, &rgba, 4)),
        ("UASTC(XYZ) -> ASTC", angles(&truth, &decode(&u_rgb), 4)),
        ("UASTC(XYZ) -> BC7", angles(&truth, &bc7::decode_image(&transcode::uastc_to_bc7_image(&u_rgb), w, h).unwrap(), 4)),
        ("UASTC(XY0) -> ASTC", angles(&truth, &decode(&u_xy), 4)),
        ("UASTC(XY0) -> BC7", angles(&truth, &bc7::decode_image(&transcode::uastc_to_bc7_image(&u_xy), w, h).unwrap(), 4)),
        ("UASTC(XY0) -> BC5", angles(&truth, &rgtc::decode_image(&rgtc::uastc_to_bc5_image(&u_xy, [0, 1]), w, h, 2), 2)),
        ("BC5 stored", angles(&truth, &rgtc::decode_image(&rgtc::encode_bc5_image(&rgba, w, h, 0), w, h, 2), 2)),
    ];
    for (name, (mean, p99, max)) in &rows { println!("{name:24} mean {mean:.3} p99 {p99:.3} max {max:.2} deg"); }
    let get = |name: &str| rows.iter().find(|r| r.0 == name).unwrap().1;
    let (mean, p99, max) = get("BC5 stored");
    assert!(mean < 1.0 && p99 < 5.0 && max < 20.0, "BC5: mean {mean} p99 {p99} max {max}");
    for name in ["UASTC(XYZ) -> ASTC", "UASTC(XYZ) -> BC7", "UASTC(XY0) -> ASTC"] {
        assert!(mean < get(name).0 && p99 < get(name).1, "BC5 behind {name}");
    }
}

/// Load-time work per 1024x1024 level (65536 blocks): the stored BC5 goes
/// to the GPU untouched where BC is sampled; the decode is the fallback of
/// a GPU without BC. The UASTC remaps are what colour textures pay.
#[test]
fn bc5_load_work_per_megatexel() {
    let (_, rgba, w, h) = normal_map();
    let u = uastc::encode_image(&rgba, w, h, Quality::Fast, 0).unwrap();
    let t = std::time::Instant::now();
    let _ = rgtc::encode_bc5_image(&rgba, w, h, 1);
    let encode = t.elapsed().as_secs_f64() * 1000.0 * 16.0;
    let t = std::time::Instant::now();
    let bc5 = rgtc::encode_bc5_image(&rgba, w, h, 8);
    let encode8 = t.elapsed().as_secs_f64() * 1000.0 * 16.0;
    let (u, bc5): (Vec<u8>, Vec<u8>) = (u.iter().copied().cycle().take(u.len() * 16).collect(), bc5.iter().copied().cycle().take(bc5.len() * 16).collect());
    let best = |f: &dyn Fn() -> usize| (0..3).map(|_| { let t = std::time::Instant::now(); assert!(f() > 0); t.elapsed().as_secs_f64() * 1000.0 }).fold(f64::MAX, f64::min);
    let astc = best(&|| { let mut out = Vec::new(); transcode::uastc_to_astc_into(&u, &mut out); out.len() });
    let bc7 = best(&|| { let mut out = Vec::new(); transcode::uastc_to_bc7_into(&u, &mut out); out.len() });
    let copy = best(&|| bc5.to_vec().len());
    let decode = best(&|| rgtc::decode_image(&bc5, 1024, 1024, 2).len());
    println!("per 1024x1024 level: UASTC->ASTC {astc:.1} ms, UASTC->BC7 {bc7:.1} ms, BC5 upload copy {copy:.2} ms, BC5 decode fallback {decode:.1} ms; BC5 build encode {encode:.0} ms on one core, {encode8:.0} ms on 8");
}
