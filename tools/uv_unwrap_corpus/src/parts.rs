//! The corpus: generated surfaces at three sizes, CSG and B-rep parts, and the
//! repository's GLB assets.

use crate::Mesh;
use makepad_csg::Solid;
use makepad_csg_brep_model::{self as brep, ops, query, TessOptions};
use std::path::Path;

fn from_rows(name: String, rows: usize, columns: usize, wrap: (bool, bool), f: impl Fn(f64, f64) -> [f64; 3]) -> Mesh {
    let (wu, wv) = wrap;
    let nu = if wu { columns } else { columns + 1 };
    let nv = if wv { rows } else { rows + 1 };
    let mut positions = Vec::with_capacity(nu * nv);
    for v in 0..nv {
        for u in 0..nu {
            positions.push(f(u as f64 / columns as f64, v as f64 / rows as f64));
        }
    }
    let at = |u: usize, v: usize| ((v % nv) * nu + (u % nu)) as u32;
    let mut indices = Vec::with_capacity(rows * columns * 6);
    for v in 0..rows {
        for u in 0..columns {
            indices.extend_from_slice(&[at(u, v), at(u, v + 1), at(u + 1, v + 1), at(u, v), at(u + 1, v + 1), at(u + 1, v)]);
        }
    }
    Mesh { name, positions, indices }
}

/// A hash-based value in -1..1, smooth in nothing: surface roughness.
fn noise(a: f64, b: f64) -> f64 {
    let x = ((a * 12.9898 + b * 78.233).sin() * 43758.5453).fract();
    x * 2.0 - 1.0
}

fn generated(tris: usize) -> Vec<Mesh> {
    let n = ((tris as f64 / 2.0).sqrt()).round() as usize;
    let tau = std::f64::consts::TAU;
    let pi = std::f64::consts::PI;
    let label = if tris >= 1000 { format!("{}k", tris / 1000) } else { tris.to_string() };
    vec![
        from_rows(format!("gen-sphere-{label}"), n, n * 2 / 2, (true, false), |u, v| {
            let (a, b) = (v * pi, u * tau);
            [a.sin() * b.cos(), a.cos(), a.sin() * b.sin()]
        }),
        from_rows(format!("gen-torus-{label}"), n, n, (true, true), |u, v| {
            let (a, b) = (u * tau, v * tau);
            let r = 1.0 + 0.35 * b.cos();
            [r * a.cos(), 0.35 * b.sin(), r * a.sin()]
        }),
        from_rows(format!("gen-terrain-{label}"), n, n, (false, false), |u, v| {
            [u * 10.0, ((u * 11.0).sin() * (v * 7.0).cos() + 0.3 * (u * 29.0 + v * 17.0).sin()) * 0.8, v * 10.0]
        }),
        from_rows(format!("gen-rock-{label}"), n, n, (true, false), |u, v| {
            let (a, b) = (v * pi, u * tau);
            let r = 1.0 + 0.04 * noise((u * 200.0).round(), (v * 200.0).round()) + 0.2 * (b * 3.0).sin() * (a * 4.0).cos();
            [r * a.sin() * b.cos(), r * a.cos(), r * a.sin() * b.sin()]
        }),
        from_rows(format!("gen-cylinder-{label}"), n, n, (true, false), |u, v| {
            let b = u * tau;
            [b.cos(), v * 3.0, b.sin()]
        }),
    ]
}

fn from_csg(name: String, solid: &Solid) -> Mesh {
    let mesh = solid.mesh();
    Mesh {
        name,
        positions: mesh.vertices.iter().map(|v| [v.x, v.y, v.z]).collect(),
        indices: mesh.triangles.iter().flatten().copied().collect(),
    }
}

fn csg_parts(segments: u32, name: &str) -> Vec<Mesh> {
    let bracket = Solid::cube(8.0, 2.0, 5.0, true)
        .difference(&Solid::cylinder(1.2, 4.0, segments, true))
        .difference(&Solid::cylinder(0.5, 4.0, segments / 2, true).translate(2.8, 0.0, 1.2))
        .difference(&Solid::cylinder(0.5, 4.0, segments / 2, true).translate(-2.8, 0.0, -1.2))
        .union(&Solid::sphere(1.3, segments, segments / 2).translate(0.0, 1.0, 0.0));
    let knob = Solid::torus(2.0, 0.6, segments, segments / 2).union(&Solid::cylinder(0.9, 2.0, segments, true));
    vec![from_csg(format!("csg-bracket-{name}"), &bracket), from_csg(format!("csg-knob-{name}"), &knob)]
}

fn brep_part(chord: f64, name: &str) -> Option<Mesh> {
    let mut kernel = ops::new_kernel();
    let k = &mut kernel;
    let plate = ops::make_box(k, [40.0, 10.0, 24.0]).ok()?;
    let bore = ops::make_cylinder(k, 7.0, 30.0).ok()?;
    let mut body = ops::boolean(k, ops::BoolMode::Difference, plate, bore).ok()?;
    let edges = query::edges(k, &mut body).ok()?;
    let straight: Vec<_> = edges.iter().filter(|e| e.length > 9.0).map(|e| e.id).collect();
    let _ = ops::fillet(k, &mut body, &straight, 2.0);
    let tess = brep::tessellate(k, &mut body, &TessOptions { chord, ..TessOptions::default() }).ok()?;
    Some(Mesh {
        name: format!("brep-plate-{name}"),
        positions: tess.mesh.vertices.iter().map(|v| [v.x, v.y, v.z]).collect(),
        indices: tess.mesh.triangles.iter().flatten().copied().collect(),
    })
}

fn glb(path: &Path) -> Option<Mesh> {
    let loaded = makepad_gltf::load_gltf_from_path(path).ok()?;
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for (m, mesh) in loaded.document.meshes_slice().iter().enumerate() {
        for p in 0..mesh.primitives.len() {
            let Ok(primitive) = makepad_gltf::decode_mesh_primitive(&loaded, m, p) else { continue };
            let base = positions.len() as u32;
            positions.extend(primitive.positions.iter().map(|v| [v[0] as f64, v[1] as f64, v[2] as f64]));
            indices.extend(primitive.indices.iter().map(|i| i + base));
        }
    }
    let name = path.file_stem()?.to_string_lossy().into_owned();
    (!indices.is_empty()).then(|| Mesh { name: format!("glb-{name}"), positions, indices })
}

fn find_glbs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                find_glbs(&path, out);
            }
        } else if name.ends_with(".glb") {
            out.push(path);
        }
    }
}

pub fn all(roots: &[String], filter: &str) -> Vec<Mesh> {
    let mut meshes = Vec::new();
    for tris in [1_000, 20_000, 200_000] {
        meshes.extend(generated(tris));
    }
    meshes.extend(csg_parts(24, "s"));
    meshes.extend(csg_parts(96, "m"));
    meshes.extend(csg_parts(320, "l"));
    meshes.extend(brep_part(0.05, "s"));
    meshes.extend(brep_part(0.002, "m"));
    let mut paths = Vec::new();
    for root in roots {
        find_glbs(Path::new(root), &mut paths);
    }
    meshes.extend(paths.iter().filter_map(|p| glb(p)));
    meshes.retain(|m| m.name.contains(filter));
    meshes
}
