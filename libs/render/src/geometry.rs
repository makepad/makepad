//! Unit shape geometries, moved verbatim from gamemaker's game_view.rs.
//!
//! CubeVertex POD layout (12 floats): pos3, id, normal3, pad, uv2, pad2 — what
//! the DrawCube shader family samples. All shapes span [-0.5, 0.5]. The opaque
//! pass backface-culls, so triangles are wound with cross(b-a, c-a) pointing
//! OUTWARD — the same convention the terrain mesh uses.

use makepad_draw::*;
use makepad_game_sim::Shape;

fn pod_vertex(vertices: &mut Vec<f32>, p: Vec3f, n: Vec3f) {
    vertices.extend_from_slice(&[
        p.x, p.y, p.z, 0.0, n.x, n.y, n.z, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]);
}

/// Flat-shaded triangle, normal from winding.
fn pod_tri(vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f) {
    let n = Vec3f::cross(b - a, c - a).normalize();
    for p in [a, b, c] {
        indices.push((vertices.len() / 12) as u32);
        pod_vertex(vertices, p, n);
    }
}

pub fn shape_geometry_data(shape: Shape) -> (Vec<f32>, Vec<u32>) {
    let mut vertices: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    match shape {
        Shape::Box => {
            // Hand-rolled unit cube, matching the outward-winding rule.
            let corner = |x: i32, y: i32, z: i32| {
                vec3f(x as f32 - 0.5, y as f32 - 0.5, z as f32 - 0.5)
            };
            // (axis-aligned quads: a, b, c, d counter-clockwise seen from outside)
            let faces = [
                // +x
                [corner(1, 0, 0), corner(1, 0, 1), corner(1, 1, 1), corner(1, 1, 0)],
                // -x
                [corner(0, 0, 1), corner(0, 0, 0), corner(0, 1, 0), corner(0, 1, 1)],
                // +y
                [corner(0, 1, 0), corner(1, 1, 0), corner(1, 1, 1), corner(0, 1, 1)],
                // -y
                [corner(0, 0, 1), corner(1, 0, 1), corner(1, 0, 0), corner(0, 0, 0)],
                // +z
                [corner(1, 0, 1), corner(0, 0, 1), corner(0, 1, 1), corner(1, 1, 1)],
                // -z
                [corner(0, 0, 0), corner(1, 0, 0), corner(1, 1, 0), corner(0, 1, 0)],
            ];
            for [a, b, c, d] in faces {
                pod_tri(&mut vertices, &mut indices, a, c, b);
                pod_tri(&mut vertices, &mut indices, a, d, c);
            }
        }
        Shape::Sphere => {
            // UV sphere, smooth normals (position direction).
            const RINGS: usize = 10;
            const SEGS: usize = 16;
            let point = |r: usize, s: usize| {
                let theta = std::f32::consts::PI * r as f32 / RINGS as f32;
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(
                    0.5 * theta.sin() * phi.cos(),
                    0.5 * theta.cos(),
                    0.5 * theta.sin() * phi.sin(),
                )
            };
            let push_smooth = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, p: Vec3f| {
                indices.push((vertices.len() / 12) as u32);
                pod_vertex(vertices, p, p.normalize());
            };
            for r in 0..RINGS {
                for s in 0..SEGS {
                    let (a, b) = (point(r, s), point(r + 1, s));
                    let (c, d) = (point(r + 1, s + 1), point(r, s + 1));
                    // Wound so cross points outward (verified by the test
                    // below); pole rows drop their degenerate half-quad.
                    if r + 1 < RINGS {
                        for p in [a, c, b] {
                            push_smooth(&mut vertices, &mut indices, p);
                        }
                    }
                    if r > 0 {
                        for p in [a, d, c] {
                            push_smooth(&mut vertices, &mut indices, p);
                        }
                    }
                }
            }
        }
        Shape::Cylinder => {
            const SEGS: usize = 20;
            let rim = |y: f32, s: usize| {
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(0.5 * phi.cos(), y, 0.5 * phi.sin())
            };
            for s in 0..SEGS {
                let (a, b) = (rim(-0.5, s), rim(-0.5, s + 1));
                let (c, d) = (rim(0.5, s + 1), rim(0.5, s));
                // Side (smooth radial normals).
                let side = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, p: Vec3f| {
                    indices.push((vertices.len() / 12) as u32);
                    pod_vertex(vertices, p, vec3f(p.x, 0.0, p.z).normalize());
                };
                for p in [a, c, b] {
                    side(&mut vertices, &mut indices, p);
                }
                for p in [a, d, c] {
                    side(&mut vertices, &mut indices, p);
                }
                // Caps (flat).
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, 0.5, 0.0), rim(0.5, s + 1), rim(0.5, s));
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, -0.5, 0.0), rim(-0.5, s), rim(-0.5, s + 1));
            }
        }
        Shape::Cone => {
            const SEGS: usize = 20;
            let rim = |s: usize| {
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(0.5 * phi.cos(), -0.5, 0.5 * phi.sin())
            };
            let apex = vec3f(0.0, 0.5, 0.0);
            for s in 0..SEGS {
                // Side (flat per-face) + base cap.
                pod_tri(&mut vertices, &mut indices, apex, rim(s + 1), rim(s));
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, -0.5, 0.0), rim(s), rim(s + 1));
            }
        }
        Shape::Wedge => {
            // A ramp: full box footprint, sloping from the top back edge
            // (+z) down to the bottom front edge (-z). Front = -z like parts.
            let p = |x: f32, y: f32, z: f32| vec3f(x, y, z);
            let (l, r, bo, t, f, ba) = (-0.5, 0.5, -0.5, 0.5, -0.5, 0.5);
            // Bottom.
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, bo, f), p(r, bo, ba));
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, bo, ba), p(l, bo, ba));
            // Back (+z, full height).
            pod_tri(&mut vertices, &mut indices, p(r, bo, ba), p(r, t, ba), p(l, t, ba));
            pod_tri(&mut vertices, &mut indices, p(r, bo, ba), p(l, t, ba), p(l, bo, ba));
            // Slope (front bottom edge to back top edge).
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, t, ba), p(r, bo, f));
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(l, t, ba), p(r, t, ba));
            // Side triangles.
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(l, bo, ba), p(l, t, ba));
            pod_tri(&mut vertices, &mut indices, p(r, bo, f), p(r, t, ba), p(r, bo, ba));
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// Every triangle of every shape must wind so its face normal points away
    /// from the shape's interior — the opaque pass backface-culls.
    #[test]
    fn shape_windings_face_outward() {
        for shape in Shape::ALL {
            let (vertices, indices) = shape_geometry_data(shape);
            assert_eq!(indices.len() % 3, 0);
            for tri in indices.chunks_exact(3) {
                let v = |i: u32| {
                    let base = i as usize * 12;
                    vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
                };
                let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
                let n = Vec3f::cross(b - a, c - a);
                let centroid = (a + b + c) * (1.0 / 3.0);
                // A strictly-interior point: outward face normals of a convex
                // solid satisfy n · (face_point - interior) > 0. The wedge is
                // not origin-centred, so it gets its own interior point.
                let interior = match shape {
                    Shape::Wedge => vec3f(0.0, -0.25, 0.25),
                    _ => vec3f(0.0, 0.0, 0.0),
                };
                if n.length() < 1.0e-6 {
                    panic!("{:?} has a degenerate triangle", shape);
                }
                let d = n.dot(centroid - interior);
                assert!(
                    d > 1.0e-6,
                    "{:?} triangle winds inward (n·(centroid-interior) = {})",
                    shape,
                    d
                );
            }
        }
    }


    /// Culling is only safe where winding is consistent, so the two facts are
    /// asserted together: every shader that culls draws geometry whose winding
    /// this crate controls and has verified.
    ///
    /// `DrawSceneCube` culls (it draws solid slabs, crates, ground and rigid
    /// bodies, all from `shape_geometry_data`, which the test above proves
    /// winds outward). Turning it on was worth doing — the platform default is
    /// OFF (`draw_shader.rs:41`) and `DrawCube` does not override it, so every
    /// one of those surfaces was rasterising its hidden back face too. A tiler
    /// pays per fragment and a headset pays for two eyes.
    ///
    /// The shaders that do NOT cull each have a reason recorded at their
    /// declaration in `shaders.rs`; this test exists so that a future audit
    /// changing one has to change the stated intent as well.
    #[test]
    fn culling_choices_match_the_geometry_they_draw() {
        let src = include_str!("shaders.rs");
        let setting = |shader: &str| -> Option<bool> {
            let at = src.find(&format!("mod.draw.{shader} = "))?;
            let tail = &src[at..];
            let end = tail.find("\n    mod.draw.").unwrap_or(tail.len());
            let decl = &tail[..end];
            // Only the declaration's OWN line decides — scanning to the
            // block's end made any later identifier containing "true"
            // (e.g. a `sun_true` field) read as culling enabled.
            decl.rfind("backface_culling:")
                .map(|i| decl[i..].lines().next().unwrap_or("").contains("true"))
        };

        // Culls: closed solids whose winding this crate generates and asserts.
        assert_eq!(setting("DrawSceneCube"), Some(true), "solid slabs must cull");
        assert_eq!(setting("DrawSceneTerrain"), Some(true), "terrain must cull");
        assert_eq!(setting("DrawSceneSkinnedGpu"), Some(true), "characters must cull");

        // Does not cull, deliberately — each documented at its declaration.
        // The sky is a cube seen from INSIDE, so every visible face is a back
        // face and culling erases it. Foliage is two-sided cards. The alpha
        // batch carries flat single-sided blobs and water where culling changes
        // the composite rather than hiding a hidden face.
        assert_eq!(
            setting("DrawSceneSkinned"),
            Some(false),
            "imported model sheets and roof undersides are two-sided"
        );
        assert_eq!(setting("DrawSceneSky"), Some(false), "sky is viewed from inside");
        assert_eq!(
            setting("DrawSceneSkyAnalytic"),
            Some(false),
            "analytic sky is viewed from inside"
        );
        assert_eq!(
            setting("DrawSceneSkyMap"),
            Some(false),
            "classic map sky portals are viewed from their underside"
        );
        assert_eq!(setting("DrawSceneFoliage"), Some(false), "foliage is two-sided");
        assert_eq!(setting("DrawSceneAlpha"), Some(false), "alpha is flat/one-sided");
        assert_eq!(
            setting("DrawSceneWater"),
            Some(false),
            "a wave trough seen flat-on shows the sheet's underside"
        );
    }

    #[test]
    fn skinned_albedo_uses_repeat_not_fract() {
        // Tiling world textures (Q3) share DrawSceneSkinned with Kenney atlases.
        // `fract(uv)` wraps in software but wrecks mip LOD at every tile seam.
        let src = include_str!("shaders.rs");
        let at = src
            .find("mod.draw.DrawSceneSkinned = ")
            .expect("DrawSceneSkinned");
        let tail = &src[at..];
        let end = tail.find("\n    mod.draw.").unwrap_or(tail.len());
        let decl = &tail[..end];
        assert!(
            decl.contains("sample_as_bgra_repeat(self.v_uv)"),
            "world albedo must REPEAT-sample raw UVs"
        );
        assert!(
            !decl.contains("fract(self.v_uv)"),
            "fract(uv) aliases distant tiles once mips exist"
        );
    }

    /// The imported-model material is both a CSM receiver and an analytic
    /// daylight surface. Pin the actual shader expressions: a regression to
    /// the CAD flat path would remove N.L or the shadow multiplier while the
    /// Rust-side sun configuration continued to look perfectly healthy.
    #[test]
    fn imported_models_receive_direct_sun_ambient_ao_and_csm() {
        let src = include_str!("shaders.rs");
        let at = src
            .find("mod.draw.DrawSceneSkinned = ")
            .expect("DrawSceneSkinned");
        let tail = &src[at..];
        let end = tail.find("\n    mod.draw.").unwrap_or(tail.len());
        let decl = &tail[..end];
        for expression in [
            "let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)",
            "self.v_ambient = mix(self.sun_ground, self.sun_sky, hemi)",
            "self.v_direct = self.sun_color * dp",
            "self.csm_vis(self.v_csm.xyz, self.v_csm_n, self.v_csm.w)",
            // Baked AO and screen-space AO both gate the AMBIENT fill only;
            // the direct sun term must never carry the screen-space factor.
            "self.v_ambient * (ao * sao)",
            "self.v_direct * (ao_direct * sun_lit)",
        ] {
            assert!(decl.contains(expression), "model lighting lost `{expression}`");
        }
        assert!(
            decl.contains("let world_normal = raw_world_normal * face_sign"),
            "two-sided receiver normals must face the visible side"
        );
    }
}
