//! Software render of real Kenney props, with baked AO on and off.
//!
//!   cargo run -p makepad-game-render --release --example ao_render
//!
//! The GPU path lives in the app, which this crate cannot drive — so this
//! reproduces the shader's shading maths on the CPU and writes a JPEG. That is
//! enough to judge the ONE thing in question: what the baked AO term does to a
//! prop. Anything muddy, striped or banded shows up here exactly as it would
//! on screen, because the arithmetic is the same.
//!
//! Left half of each tile: AO off. Right half: AO on.

use jpeg_encoder::{ColorType, Encoder};
use makepad_draw::makepad_math::Vec3f;
use makepad_game_render::model::{StaticModel, MODEL_VERTEX_FLOATS};

const TILE: usize = 300;
const COLS: usize = 3;

/// Same terms the shader uses, so what this shows is what the GPU would draw.
const SUN_DIR: Vec3f = Vec3f { x: 0.45, y: 0.78, z: 0.44 };
const SUN_COLOR: f32 = 0.72;
const SKY: f32 = 0.40;
const GROUND: f32 = 0.20;

fn main() {
    let root = "apps/arcade/resources/models/kenney";
    // A prop with eaves, an arch, a slatted bench, a barrel, a tree, a wall.
    let wanted = [
        ("house", "fantasy-town-kit"),
        ("arch", "castle-kit"),
        ("bench", "graveyard-kit"),
        ("barrel", "survival-kit"),
        ("tree", "nature-kit"),
        ("wall", "castle-kit"),
    ];

    let mut picks: Vec<(String, std::path::PathBuf)> = Vec::new();
    for (needle, pack) in wanted {
        let dir = std::path::Path::new(root).join(pack);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut best: Option<std::path::PathBuf> = None;
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "glb").unwrap_or(true) {
                continue;
            }
            let name = p.file_stem().unwrap().to_string_lossy().to_lowercase();
            if name.contains(needle) {
                // Shortest matching name = the plainest variant.
                if best.as_ref().map(|b| name.len() < b.to_string_lossy().len()).unwrap_or(true) {
                    best = Some(p);
                }
            }
        }
        if let Some(b) = best {
            picks.push((format!("{pack}/{}", b.file_stem().unwrap().to_string_lossy()), b));
        }
    }
    if picks.is_empty() {
        eprintln!("no models found under {root} — run apps/arcade/download_assets.sh");
        return;
    }

    let rows = picks.len().div_ceil(COLS);
    let (w, h) = (TILE * COLS, TILE * rows);
    let mut fb = vec![0u8; w * h * 3];
    // Mid grey, so both over- and under-darkening are visible against it.
    for p in fb.iter_mut() {
        *p = 96;
    }

    for (i, (name, path)) in picks.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Ok(m) = StaticModel::parse_glb(&bytes) else { continue };
        let ox = (i % COLS) * TILE;
        let oy = (i / COLS) * TILE;
        // Left half without AO, right half with — one model, one camera, so
        // the only difference in the image is the term being judged.
        draw(&mut fb, w, ox, oy, TILE / 2, TILE, &m, false);
        draw(&mut fb, w, ox + TILE / 2, oy, TILE / 2, TILE, &m, true);
        println!("{i}: {name}  ({} verts)", m.vertex_count());
    }

    let out = std::env::args().nth(1).unwrap_or_else(|| {
        "/private/tmp/claude-501/-Users-admin-makepad-makepad/99a6fb4f-a075-40cd-8cda-fdb93c19da1d/scratchpad/ao_compare.jpg".into()
    });
    let enc = Encoder::new_file(&out, 92).expect("encoder");
    enc.encode(&fb, w as u16, h as u16, ColorType::Rgb).expect("encode");
    println!("wrote {out} ({w}x{h})");

    contact_sheet(&picks);
}

/// Second image: props standing on ground, contact skirt off vs on. This is
/// the half that decides whether the skirt reads as grounding or as a stain,
/// and it cannot be judged from the model alone.
fn contact_sheet(picks: &[(String, std::path::PathBuf)]) {
    use makepad_game_render::shadow_mesh::{build_contact_ao, Receiver, ShadowMeshBuilder, SHADOW_VERTEX_FLOATS};

    // Two props at 4x the tile size. The skirt is a subtle ground effect and
    // simply cannot be judged at thumbnail scale — the first attempt at this
    // sheet rendered six props small and showed nothing but specks.
    let big = TILE * 2;
    let picks: Vec<_> = picks.iter().take(2).collect();
    let cols = 2usize;
    let rows = 1usize;
    let (w, h) = (big * cols, big * rows);
    let mut fb = vec![0u8; w * h * 3];

    for (i, (_, path)) in picks.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Ok(m) = StaticModel::parse_glb(&bytes) else { continue };
        let ox = (i % cols) * big;
        let oy = (i / cols) * big;
        for (half, skirt) in [(0usize, false), (big / 2, true)] {
            let mut mesh = ShadowMeshBuilder::default();
            if skirt {
                let hx = (m.max.x - m.min.x) * 0.5;
                let hz = (m.max.z - m.min.z) * 0.5;
                let cx = (m.min.x + m.max.x) * 0.5;
                let cz = (m.min.z + m.max.z) * 0.5;
                build_contact_ao(
                    Vec3f { x: cx, y: m.min.y, z: cz },
                    hx,
                    hz,
                    &Receiver { base_y: m.min.y, terrain: None },
                    &mut mesh,
                );
            }
            let tris: Vec<([Vec3f; 3], [f32; 3])> = (0..mesh.indices.len() / 3)
                .map(|t| {
                    let g = |k: usize| {
                        let vi = mesh.indices[t * 3 + k] as usize * SHADOW_VERTEX_FLOATS;
                        let a = ((mesh.vertices[vi + 5].to_bits() >> 24) & 0xff) as f32 / 255.0;
                        (
                            Vec3f {
                                x: mesh.vertices[vi],
                                y: mesh.vertices[vi + 1],
                                z: mesh.vertices[vi + 2],
                            },
                            a,
                        )
                    };
                    let (p0, a0) = g(0);
                    let (p1, a1) = g(1);
                    let (p2, a2) = g(2);
                    ([p0, p1, p2], [a0, a1, a2])
                })
                .collect();
            draw_grounded(&mut fb, w, ox + half, oy, big / 2, big, &m, &tris);
        }
    }

    let out = "/private/tmp/claude-501/-Users-admin-makepad-makepad/99a6fb4f-a075-40cd-8cda-fdb93c19da1d/scratchpad/ao_contact.jpg";
    let enc = Encoder::new_file(out, 92).expect("encoder");
    enc.encode(&fb, w as u16, h as u16, ColorType::Rgb).expect("encode");
    println!("wrote {out} ({w}x{h})");
}

/// Rasterise the model into a sub-rect, with a z-buffer and the shader's
/// lighting. `ao` selects whether the baked term is applied.
#[allow(clippy::too_many_arguments)]
fn draw(
    fb: &mut [u8],
    fb_w: usize,
    ox: usize,
    oy: usize,
    vw: usize,
    vh: usize,
    m: &StaticModel,
    ao: bool,
) {
    let mut zbuf = vec![f32::MAX; vw * vh];
    let centre = Vec3f {
        x: (m.min.x + m.max.x) * 0.5,
        y: (m.min.y + m.max.y) * 0.5,
        z: (m.min.z + m.max.z) * 0.5,
    };
    let span = (m.max.x - m.min.x)
        .max(m.max.y - m.min.y)
        .max(m.max.z - m.min.z)
        .max(1.0e-4);

    // Fixed three-quarter view: high enough to see the ground-facing crevices
    // that AO is mostly about.
    let yaw = 0.7f32;
    let pitch = 0.45f32;
    let (sy, cy) = (yaw.sin(), yaw.cos());
    let (sp, cp) = (pitch.sin(), pitch.cos());
    let scale = vw.min(vh) as f32 / (span * 1.45);

    let project = |p: Vec3f| -> (f32, f32, f32) {
        let x = p.x - centre.x;
        let y = p.y - centre.y;
        let z = p.z - centre.z;
        let rx = x * cy + z * sy;
        let rz = -x * sy + z * cy;
        let ry = y * cp + rz * sp;
        let depth = -y * sp + rz * cp;
        (
            vw as f32 * 0.5 + rx * scale,
            vh as f32 * 0.5 - ry * scale,
            depth,
        )
    };

    let vert = |i: usize| -> (Vec3f, Vec3f, f32) {
        let b = i * MODEL_VERTEX_FLOATS;
        let p = Vec3f {
            x: m.vertices[b],
            y: m.vertices[b + 1],
            z: m.vertices[b + 2],
        };
        let n = oct_decode(m.vertices[b + 3]);
        let packed = m.vertices[b + 5].to_bits();
        let unorm = |shift: u32| ((packed >> shift) & 0xff) as f32 / 255.0;
        // rgb = material tint, w = baked AO (see model.rs).
        let tint = (unorm(0) + unorm(8) + unorm(16)) / 3.0;
        let a = unorm(24);
        (p, n, if ao { a } else { 1.0 } * (0.25 + 0.75 * tint))
    };

    for t in 0..m.indices.len() / 3 {
        let (p0, n0, a0) = vert(m.indices[t * 3] as usize);
        let (p1, n1, a1) = vert(m.indices[t * 3 + 1] as usize);
        let (p2, n2, a2) = vert(m.indices[t * 3 + 2] as usize);
        let s0 = project(p0);
        let s1 = project(p1);
        let s2 = project(p2);

        let area = (s1.0 - s0.0) * (s2.1 - s0.1) - (s2.0 - s0.0) * (s1.1 - s0.1);
        if area.abs() < 1.0e-6 {
            continue;
        }
        let minx = s0.0.min(s1.0).min(s2.0).floor().max(0.0) as usize;
        let maxx = (s0.0.max(s1.0).max(s2.0).ceil() as usize).min(vw.saturating_sub(1));
        let miny = s0.1.min(s1.1).min(s2.1).floor().max(0.0) as usize;
        let maxy = (s0.1.max(s1.1).max(s2.1).ceil() as usize).min(vh.saturating_sub(1));

        for py in miny..=maxy {
            for px in minx..=maxx {
                let fx = px as f32 + 0.5;
                let fy = py as f32 + 0.5;
                let w0 = ((s1.0 - fx) * (s2.1 - fy) - (s2.0 - fx) * (s1.1 - fy)) / area;
                let w1 = ((s2.0 - fx) * (s0.1 - fy) - (s0.0 - fx) * (s2.1 - fy)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let depth = s0.2 * w0 + s1.2 * w1 + s2.2 * w2;
                let zi = py * vw + px;
                if depth >= zbuf[zi] {
                    continue;
                }
                zbuf[zi] = depth;

                let n = Vec3f {
                    x: n0.x * w0 + n1.x * w1 + n2.x * w2,
                    y: n0.y * w0 + n1.y * w1 + n2.y * w2,
                    z: n0.z * w0 + n1.z * w1 + n2.z * w2,
                };
                let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1.0e-6);
                let ny = n.y / l;
                let dp = ((n.x * SUN_DIR.x + n.y * SUN_DIR.y + n.z * SUN_DIR.z) / l).max(0.0);
                let shade = a0 * w0 + a1 * w1 + a2 * w2;
                // The shader's arithmetic: AO scales AMBIENT only, direct is
                // untouched. Reproduced exactly so this image is predictive.
                let hemi = (ny * 0.5 + 0.5).clamp(0.0, 1.0);
                let ambient = GROUND + (SKY - GROUND) * hemi;
                let lit = (ambient * shade + SUN_COLOR * dp).clamp(0.0, 1.4);
                let v = (lit * 210.0).clamp(0.0, 255.0) as u8;
                let o = ((oy + py) * fb_w + ox + px) * 3;
                fb[o] = v;
                fb[o + 1] = v;
                fb[o + 2] = v;
            }
        }
    }
}

/// Inverse of skin.rs's oct_encode — the same fold the shader does.
fn oct_decode(packed: f32) -> Vec3f {
    let bits = packed.to_bits();
    let f16 = |h: u32| -> f32 {
        let s = ((h >> 15) & 1) as i32;
        let e = ((h >> 10) & 0x1f) as i32;
        let m = (h & 0x3ff) as i32;
        let v = if e == 0 {
            (m as f32) * 2.0f32.powi(-24)
        } else {
            (1.0 + m as f32 / 1024.0) * 2.0f32.powi(e - 15)
        };
        if s == 1 {
            -v
        } else {
            v
        }
    };
    let ex = f16(bits & 0xffff);
    let ey = f16((bits >> 16) & 0xffff);
    let nz = 1.0 - ex.abs() - ey.abs();
    let t = (-nz).max(0.0);
    let sx = if ex >= 0.0 { 1.0 } else { -1.0 };
    let sy = if ey >= 0.0 { 1.0 } else { -1.0 };
    let v = Vec3f {
        x: ex - t * sx,
        y: ey - t * sy,
        z: nz,
    };
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1.0e-6);
    Vec3f {
        x: v.x / l,
        y: v.y / l,
        z: v.z / l,
    }
}

/// Prop standing on a lit ground plane, with the contact skirt composited on
/// top of the ground exactly as the alpha-blended shadow pass would.
#[allow(clippy::too_many_arguments)]
fn draw_grounded(
    fb: &mut [u8],
    fb_w: usize,
    ox: usize,
    oy: usize,
    vw: usize,
    vh: usize,
    m: &StaticModel,
    skirt: &[([Vec3f; 3], [f32; 3])],
) {
    let centre = Vec3f {
        x: (m.min.x + m.max.x) * 0.5,
        y: (m.min.y + m.max.y) * 0.5,
        z: (m.min.z + m.max.z) * 0.5,
    };
    let span = (m.max.x - m.min.x)
        .max(m.max.y - m.min.y)
        .max(m.max.z - m.min.z)
        .max(1.0e-4);
    let yaw = 0.7f32;
    let pitch = 0.45f32;
    let (sy, cy) = (yaw.sin(), yaw.cos());
    let (sp, cp) = (pitch.sin(), pitch.cos());
    let scale = vw.min(vh) as f32 / (span * 1.25);
    let project = |p: Vec3f| -> (f32, f32, f32) {
        let (x, y, z) = (p.x - centre.x, p.y - centre.y, p.z - centre.z);
        let rx = x * cy + z * sy;
        let rz = -x * sy + z * cy;
        // Rotate about x by `pitch` to look DOWN on the scene: y' = y·cos+z·sin,
        // z' = -y·sin+z·cos. Getting these signs backwards puts the camera under
        // the floor, and a ground plane then projects up over the prop it holds.
        let ry = y * cp + rz * sp;
        (vw as f32 * 0.5 + rx * scale, vh as f32 * 0.55 - ry * scale, -y * sp + rz * cp)
    };

    let mut zbuf = vec![f32::MAX; vw * vh];
    let mut col = vec![[0.30f32; 3]; vw * vh];

    // Ground first, as a big quad at the prop's base.
    let g = span * 1.1;
    let gy = m.min.y;
    let quad = [
        Vec3f { x: centre.x - g, y: gy, z: centre.z - g },
        Vec3f { x: centre.x + g, y: gy, z: centre.z - g },
        Vec3f { x: centre.x + g, y: gy, z: centre.z + g },
        Vec3f { x: centre.x - g, y: gy, z: centre.z + g },
    ];
    for t in [[0usize, 1, 2], [0, 2, 3]] {
        raster(&mut zbuf, &mut col, vw, vh, &project, [quad[t[0]], quad[t[1]], quad[t[2]]],
            |_| [0.62, 0.66, 0.55], false);
    }
    // Skirt composited onto the ground: premultiplied black, exactly the
    // shader's blend.
    for (tri, alpha) in skirt {
        raster(&mut zbuf, &mut col, vw, vh, &project, *tri, |w| {
            let a = alpha[0] * w[0] + alpha[1] * w[1] + alpha[2] * w[2];
            [a, a, a]
        }, true);
    }
    // Then the prop, with its baked AO.
    for t in 0..m.indices.len() / 3 {
        let vp = |k: usize| {
            let b = m.indices[t * 3 + k] as usize * MODEL_VERTEX_FLOATS;
            let p = Vec3f { x: m.vertices[b], y: m.vertices[b + 1], z: m.vertices[b + 2] };
            let n = oct_decode(m.vertices[b + 3]);
            let packed = m.vertices[b + 5].to_bits();
            let ao = ((packed >> 24) & 0xff) as f32 / 255.0;
            (p, n, ao)
        };
        let (p0, n0, a0) = vp(0);
        let (p1, n1, a1) = vp(1);
        let (p2, n2, a2) = vp(2);
        raster(&mut zbuf, &mut col, vw, vh, &project, [p0, p1, p2], |w| {
            let n = Vec3f {
                x: n0.x * w[0] + n1.x * w[1] + n2.x * w[2],
                y: n0.y * w[0] + n1.y * w[1] + n2.y * w[2],
                z: n0.z * w[0] + n1.z * w[1] + n2.z * w[2],
            };
            let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1.0e-6);
            let dp = ((n.x * SUN_DIR.x + n.y * SUN_DIR.y + n.z * SUN_DIR.z) / l).max(0.0);
            let hemi = (n.y / l * 0.5 + 0.5).clamp(0.0, 1.0);
            let ao = a0 * w[0] + a1 * w[1] + a2 * w[2];
            let lit = (GROUND + (SKY - GROUND) * hemi) * ao + SUN_COLOR * dp;
            [lit * 0.8, lit * 0.78, lit * 0.72]
        }, false);
    }

    for y in 0..vh {
        for x in 0..vw {
            let c = col[y * vw + x];
            let o = ((oy + y) * fb_w + ox + x) * 3;
            for k in 0..3 {
                fb[o + k] = (c[k] * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Shared rasteriser. `blend` composites premultiplied black by the shaded
/// value's alpha instead of replacing — that is the shadow pass's blend.
fn raster(
    zbuf: &mut [f32],
    col: &mut [[f32; 3]],
    vw: usize,
    vh: usize,
    project: &impl Fn(Vec3f) -> (f32, f32, f32),
    tri: [Vec3f; 3],
    shade: impl Fn([f32; 3]) -> [f32; 3],
    blend: bool,
) {
    let s: Vec<(f32, f32, f32)> = tri.iter().map(|p| project(*p)).collect();
    let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
    if area.abs() < 1.0e-6 {
        return;
    }
    let minx = s.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = (s.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as usize).min(vw.saturating_sub(1));
    let miny = s.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = (s.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as usize).min(vh.saturating_sub(1));
    for py in miny..=maxy {
        for px in minx..=maxx {
            let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
            let w0 = ((s[1].0 - fx) * (s[2].1 - fy) - (s[2].0 - fx) * (s[1].1 - fy)) / area;
            let w1 = ((s[2].0 - fx) * (s[0].1 - fy) - (s[0].0 - fx) * (s[2].1 - fy)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let depth = s[0].2 * w0 + s[1].2 * w1 + s[2].2 * w2;
            let zi = py * vw + px;
            let v = shade([w0, w1, w2]);
            if blend {
                // Shadow geometry: depth-test but no depth-write, premultiplied
                // black — the ground keeps (1 - a) of its colour.
                if depth > zbuf[zi] {
                    continue;
                }
                for k in 0..3 {
                    col[zi][k] *= 1.0 - v[0];
                }
            } else {
                if depth >= zbuf[zi] {
                    continue;
                }
                zbuf[zi] = depth;
                col[zi] = v;
            }
        }
    }
}
