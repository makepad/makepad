//! A procedural office building — the always-available "largest building"
//! benchmark scene: a slab-and-column structure with glazed façades
//! (thin glass, so the sun gets inside), interior partitions and desks,
//! emissive ceiling panels, a ground plane and a few neighbours. Triangle
//! count scales with `floors` and `bays`; `building(8, 10)` is ~100k.

use crate::scene::{push_box, Camera, Material, SceneInput, Sun};
use makepad_draw::*;

pub fn building(floors: usize, bays: usize) -> SceneInput {
    let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
    s.materials = vec![
        Material { albedo: [0.72, 0.70, 0.66], roughness: 0.9, ..Default::default() }, // 0 concrete
        Material { albedo: [0.9, 0.9, 0.88], roughness: 0.7, ..Default::default() },   // 1 plaster
        Material { albedo: [0.55, 0.42, 0.28], roughness: 0.5, ..Default::default() }, // 2 wood
        Material { albedo: [0.9, 0.95, 1.0], roughness: 0.0, transmission: 1.0, ior: 1.5, two_sided: true, ..Default::default() }, // 3 glass
        Material { albedo: [0.6, 0.6, 0.62], roughness: 0.35, metal: 1.0, ..Default::default() }, // 4 steel
        Material { albedo: [1.0, 1.0, 1.0], emission: [4.0, 3.9, 3.6], ..Default::default() },   // 5 panel
        Material { albedo: [0.32, 0.34, 0.3], roughness: 0.95, ..Default::default() },  // 6 ground
        Material { albedo: [0.2, 0.25, 0.5], roughness: 0.4, ..Default::default() },    // 7 accent
    ];
    let bay = 6.0f32;
    let fh = 3.6f32;
    let w = bays as f32 * bay;
    let d = 3.0 * bay;
    let slab = 0.3;
    // Ground.
    push_box(&mut s, vec3f(0.0, -0.3, 0.0), vec3f(w * 6.0, 0.3, d * 6.0), 0.0, 6);
    for f in 0..=floors {
        let y = f as f32 * fh;
        // Slab (the roof is the last one).
        push_box(&mut s, vec3f(0.0, y, 0.0), vec3f(w, slab, d), 0.0, 0);
        if f == floors {
            break;
        }
        // Columns.
        for bx in 0..=bays {
            for bz in 0..=3 {
                let x = -w * 0.5 + bx as f32 * bay;
                let z = -d * 0.5 + bz as f32 * bay;
                push_box(&mut s, vec3f(x, y + slab, z), vec3f(0.4, fh - slab, 0.4), 0.0, 0);
            }
        }
        // Glazed façades: mullions + panes, all four sides.
        let pane_h = fh - slab - 0.6;
        let facade = |s: &mut SceneInput, along_x: bool, sign: f32| {
            let len = if along_x { w } else { d };
            let n = (len / 1.5).round() as usize;
            for i in 0..n {
                let t = -len * 0.5 + (i as f32 + 0.5) * (len / n as f32);
                let (cx, cz, sx, sz) = if along_x {
                    (t, sign * d * 0.5, len / n as f32 - 0.08, 0.04)
                } else {
                    (sign * w * 0.5, t, 0.04, len / n as f32 - 0.08)
                };
                push_box(s, vec3f(cx, y + slab + 0.3, cz), vec3f(sx, pane_h, sz), 0.0, 3);
                let (mx, mz) = if along_x { (t - len / n as f32 * 0.5, cz) } else { (cx, t - len / n as f32 * 0.5) };
                push_box(s, vec3f(mx, y + slab, mz), vec3f(0.08, fh - slab, 0.08), 0.0, 4);
            }
            // Spandrel band.
            let (cx, cz, sx, sz) = if along_x { (0.0, sign * d * 0.5, w, 0.1) } else { (sign * w * 0.5, 0.0, 0.1, d) };
            push_box(s, vec3f(cx, y + slab, cz), vec3f(sx, 0.3, sz), 0.0, 0);
        };
        facade(&mut s, true, 1.0);
        facade(&mut s, true, -1.0);
        facade(&mut s, false, 1.0);
        facade(&mut s, false, -1.0);
        // Balcony railing: balusters every half metre round the slab edge.
        let rail_y = y + slab;
        let post = |s: &mut SceneInput, x: f32, z: f32| {
            push_box(s, vec3f(x, rail_y, z), vec3f(0.04, 1.0, 0.04), 0.0, 4);
        };
        let nx = (w / 0.5) as usize;
        let nz = (d / 0.5) as usize;
        for i in 0..=nx {
            let x = -w * 0.5 - 0.6 + (i as f32 / nx as f32) * (w + 1.2);
            post(&mut s, x, -d * 0.5 - 0.6);
            post(&mut s, x, d * 0.5 + 0.6);
        }
        for i in 1..nz {
            let z = -d * 0.5 - 0.6 + (i as f32 / nz as f32) * (d + 1.2);
            post(&mut s, -w * 0.5 - 0.6, z);
            post(&mut s, w * 0.5 + 0.6, z);
        }
        for (cx, cz, sx, sz) in [(0.0, -d * 0.5 - 0.6, w + 1.2, 0.05), (0.0, d * 0.5 + 0.6, w + 1.2, 0.05), (-w * 0.5 - 0.6, 0.0, 0.05, d + 1.2), (w * 0.5 + 0.6, 0.0, 0.05, d + 1.2)] {
            push_box(&mut s, vec3f(cx, rail_y + 1.0, cz), vec3f(sx, 0.05, sz), 0.0, 4);
            push_box(&mut s, vec3f(cx, rail_y - 0.2, cz), vec3f(sx, 0.2, sz + 0.6), 0.0, 0);
        }
        // Core + partitions.
        push_box(&mut s, vec3f(0.0, y + slab, 0.0), vec3f(bay * 0.8, fh - slab, bay * 0.6), 0.0, 1);
        for bx in 1..bays {
            let x = -w * 0.5 + bx as f32 * bay;
            push_box(&mut s, vec3f(x, y + slab, -d * 0.25), vec3f(0.12, fh - slab, d * 0.45), 0.0, 1);
        }
        // Desks + chairs in every bay, ceiling panels.
        for bx in 0..bays {
            for bz in 0..3 {
                let x = -w * 0.5 + (bx as f32 + 0.5) * bay;
                let z = -d * 0.5 + (bz as f32 + 0.5) * bay;
                if bz == 1 && (bx as f32 - bays as f32 * 0.5).abs() < 1.0 {
                    continue;
                }
                for k in 0..2 {
                    let ox = (k as f32 - 0.5) * 2.4;
                    push_box(&mut s, vec3f(x + ox, y + slab + 0.7, z), vec3f(1.6, 0.05, 0.8), 0.0, 2);
                    for (lx, lz) in [(-0.7, -0.35), (0.7, -0.35), (-0.7, 0.35), (0.7, 0.35)] {
                        push_box(&mut s, vec3f(x + ox + lx, y + slab, z + lz), vec3f(0.05, 0.7, 0.05), 0.0, 4);
                    }
                    push_box(&mut s, vec3f(x + ox, y + slab + 0.45, z + 0.9), vec3f(0.5, 0.05, 0.5), 0.3, 7);
                    push_box(&mut s, vec3f(x + ox, y + slab + 0.5, z + 1.12), vec3f(0.5, 0.45, 0.05), 0.3, 7);
                }
                push_box(&mut s, vec3f(x, y + fh - 0.06, z), vec3f(1.2, 0.02, 0.6), 0.0, 5);
            }
        }
    }
    // Neighbours: plain blocks that catch the light and cast shadows.
    for (x, z, bw, bd, bh) in [(-w * 1.2, 0.0, w * 0.6, d * 1.1, fh * 2.0), (w * 1.3, -d * 0.4, w * 0.7, d * 0.8, fh * 3.5)] {
        push_box(&mut s, vec3f(x, 0.0, z), vec3f(bw, bh, bd), 0.0, 0);
    }
    s.ensure_normals();
    let h = floors as f32 * fh;
    s.camera = Camera {
        pos: vec3f(w * 0.9, h * 0.55 + 2.0, d * 1.45),
        target: vec3f(0.0, h * 0.42, 0.0),
        up: vec3f(0.0, 1.0, 0.0),
        fov_y: 38.0f32.to_radians(),
        focus_dist: (w * w * 0.81 + d * d * 2.1).sqrt(),
        f_stop: 8.0,
        focal_mm: 35.0,
        bokeh_scale: 6.0,
        ..Default::default()
    };
    s.sun = Sun { dir: vec3f(0.55, 0.62, 0.35).normalize(), turbidity: 2.5, sky_strength: 1.0, sun_strength: 4.0 };
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_building_is_about_a_hundred_thousand_triangles() {
        let s = building(8, 10);
        let n = s.tri_count();
        assert!(n > 80_000 && n < 260_000, "tri count {n}");
        assert!(s.materials[3].transmission > 0.0);
    }
}
