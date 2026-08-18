//! GLB loader + normalization parity: our f64 scene-graph loader vs the
//! trimesh force='mesh' + normalize + degenerate-filter pipeline captured in
//! the oracle dumps.

mod common;

use common::{faithc_ref_dir, FcDump};
use makepad_remesh::load_glb_normalized;

fn check_asset(name: &str) {
    let Some(dir) = faithc_ref_dir() else {
        eprintln!("SKIP {name}: local/faithc_ref not present");
        return;
    };
    let glb_path = dir.join("faithc/assets/examples").join(format!("{name}.glb"));
    let dump_path = dir.join("dumps").join(format!("{name}_r128.bin"));
    if !glb_path.is_file() || !dump_path.is_file() {
        eprintln!("SKIP {name}: asset or dump missing");
        return;
    }
    let bytes = std::fs::read(&glb_path).unwrap();
    let (mesh, _) = load_glb_normalized(&bytes, 0.05).unwrap();

    let dump = FcDump::load(&dump_path).unwrap();
    let ref_verts = dump.v3s("mesh_vertices");
    let (_, ref_faces) = dump.i64("mesh_faces");

    assert_eq!(mesh.positions.len(), ref_verts.len(), "{name}: vertex count");
    assert_eq!(mesh.faces.len() * 3, ref_faces.len(), "{name}: face count");

    let mut max_d = 0.0f32;
    let mut exact = 0usize;
    for (a, b) in mesh.positions.iter().zip(&ref_verts) {
        let d = (a[0] - b[0])
            .abs()
            .max((a[1] - b[1]).abs())
            .max((a[2] - b[2]).abs());
        max_d = max_d.max(d);
        if d == 0.0 {
            exact += 1;
        }
    }
    let mut faces_equal = true;
    for (i, f) in mesh.faces.iter().enumerate() {
        if [f[0] as i64, f[1] as i64, f[2] as i64]
            != [ref_faces[i * 3], ref_faces[i * 3 + 1], ref_faces[i * 3 + 2]]
        {
            faces_equal = false;
            break;
        }
    }
    println!(
        "{name}: {} verts ({} bit-exact), max|d|={max_d:.3e}, faces equal: {faces_equal}",
        mesh.positions.len(),
        exact
    );
    assert!(faces_equal, "{name}: face arrays differ");
    assert!(max_d <= 1e-6, "{name}: vertex diff {max_d:.3e}");
}

#[test]
fn loader_corgi() {
    check_asset("corgi_traveller");
}

#[test]
fn loader_light_bulb() {
    check_asset("light_bulb");
}

#[test]
fn loader_cloth() {
    check_asset("cloth");
}

#[test]
fn loader_pirateship() {
    check_asset("pirateship");
}
