//! Topology and atlas gate vs official C++ xatlas (`oracle/gold/*.txt`).

use makepad_xatlas::parametrize;
use std::fs;
use std::path::PathBuf;

fn f32_from_bits(hex: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(hex, 16).expect(hex))
}

#[derive(Debug)]
struct Gold {
    width: u32,
    height: u32,
    atlas_count: u32,
    chart_count: u32,
    texels_per_unit: f32,
    vertices: Vec<(u32, i32, i32, f32, f32)>,
    faces: Vec<[u32; 3]>,
}

fn load_gold(name: &str) -> Gold {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("oracle/gold")
        .join(format!("{name}.txt"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut gold = Gold {
        width: 0,
        height: 0,
        atlas_count: 0,
        chart_count: 0,
        texels_per_unit: 0.0,
        vertices: Vec::new(),
        faces: Vec::new(),
    };
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(tag) = it.next() else { continue };
        match tag {
            "width" => gold.width = it.next().unwrap().parse().unwrap(),
            "height" => gold.height = it.next().unwrap().parse().unwrap(),
            "atlasCount" => gold.atlas_count = it.next().unwrap().parse().unwrap(),
            "chartCount" => gold.chart_count = it.next().unwrap().parse().unwrap(),
            "texelsPerUnit" => gold.texels_per_unit = f32_from_bits(it.next().unwrap()),
            "v" => gold.vertices.push((
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
                f32_from_bits(it.next().unwrap()),
                f32_from_bits(it.next().unwrap()),
            )),
            "f" => gold.faces.push([
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
            ]),
            _ => {}
        }
    }
    gold
}

fn load_mesh(name: &str) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("oracle/meshes")
        .join(format!("{name}.txt"));
    let text = fs::read_to_string(&path).unwrap();
    let mut toks = text.split_whitespace();
    assert_eq!(toks.next(), Some("v"));
    let nv: usize = toks.next().unwrap().parse().unwrap();
    let mut positions = Vec::with_capacity(nv);
    for _ in 0..nv {
        positions.push([
            toks.next().unwrap().parse().unwrap(),
            toks.next().unwrap().parse().unwrap(),
            toks.next().unwrap().parse().unwrap(),
        ]);
    }
    assert_eq!(toks.next(), Some("f"));
    let nf: usize = toks.next().unwrap().parse().unwrap();
    let mut faces = Vec::with_capacity(nf);
    for _ in 0..nf {
        faces.push([
            toks.next().unwrap().parse().unwrap(),
            toks.next().unwrap().parse().unwrap(),
            toks.next().unwrap().parse().unwrap(),
        ]);
    }
    (positions, faces)
}

fn assert_matches(name: &str) {
    let (positions, faces) = load_mesh(name);
    let gold = load_gold(name);
    let out = parametrize(&positions, &faces).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(out.width, gold.width, "{name} width");
    assert_eq!(out.height, gold.height, "{name} height");
    assert_eq!(out.atlas_count, gold.atlas_count, "{name} atlasCount");
    assert_eq!(out.chart_count, gold.chart_count, "{name} chartCount");
    assert_eq!(
        out.texels_per_unit.to_bits(),
        gold.texels_per_unit.to_bits(),
        "{name} texelsPerUnit"
    );
    assert_eq!(out.vertices.len(), gold.vertices.len(), "{name} vertexCount");
    // Official xatlas publishes float uv[2] in atlas texels (xatlas.h:63).
    // Packing rotates and translates those f32 coordinates, so cancellation
    // near zero can differ by many local ULPs across C++/Rust toolchains.
    // Allow four f32 roundings at the atlas scale, still far below one texel.
    // Counts, chart membership, indices and texel density remain exact.
    let uv_tolerance = 4.0 * f32::EPSILON * gold.width.max(gold.height).max(1) as f32;
    for (i, (got, exp)) in out.vertices.iter().zip(gold.vertices.iter()).enumerate() {
        assert_eq!(got.xref, exp.0, "{name} v{i} xref");
        assert_eq!(got.atlas_index, exp.1, "{name} v{i} atlasIndex");
        assert_eq!(got.chart_index, exp.2, "{name} v{i} chartIndex");
        assert!((got.uv[0] - exp.3).abs() <= uv_tolerance,
            "{name} v{i} uv.x: {} vs {}, tolerance {uv_tolerance}", got.uv[0], exp.3);
        assert!((got.uv[1] - exp.4).abs() <= uv_tolerance,
            "{name} v{i} uv.y: {} vs {}, tolerance {uv_tolerance}", got.uv[1], exp.4);
    }
    assert_eq!(out.indices, gold.faces, "{name} indices");
}

#[test]
fn unit_quad_matches_official_xatlas() {
    assert_matches("unit_quad");
}

#[test]
fn unit_cube_matches_official_xatlas() {
    assert_matches("unit_cube");
}

#[test]
fn tetra_matches_official_xatlas() {
    assert_matches("tetra");
}

#[test]
fn irregular_matches_official_xatlas() {
    assert_matches("irregular");
}
