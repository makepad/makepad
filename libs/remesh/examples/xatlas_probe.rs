//! Time the xatlas unwrap of GLB meshes phase by phase:
//! `cargo run --release -p makepad-remesh --example xatlas_probe -- a.glb b.glb …`
//! Prints V/F, topology audit, and wall time per xatlas phase so a slow or
//! hung unwrap can be reproduced offline from a `pre_unwrap.glb` dump.
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: xatlas_probe <mesh.glb>...");
        std::process::exit(2);
    }
    for path in &args {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: read failed: {e}");
                continue;
            }
        };
        let loaded = match makepad_gltf::load_gltf_from_bytes(&bytes, None) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("{path}: parse failed: {e:?}");
                continue;
            }
        };
        let prim = match makepad_gltf::decode_mesh_primitive(&loaded, 0, 0) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{path}: decode failed: {e:?}");
                continue;
            }
        };
        let (mut positions, mut indices) = (prim.positions, prim.indices);
        // Library GLBs carry seam-split vertices; weld so xatlas sees the
        // real connectivity (the service unwraps welded meshes).
        let welded = makepad_remesh::weld_vertices(&mut positions, &mut indices, 1e-6);
        let audit = makepad_remesh::audit_mesh_topology(&positions, &indices);
        eprintln!("{path}: welded {welded} vertices");
        eprintln!(
            "{path}: V={} F={} boundary={} nonmanifold={} inconsistent={}",
            positions.len(),
            indices.len() / 3,
            audit.boundary_edges,
            audit.nonmanifold_edges,
            audit.inconsistent_edges
        );
        let t0 = Instant::now();
        let mut last_bucket = -1i32;
        let mut last_print = Instant::now();
        let result = makepad_remesh::uv_xatlas_unwrap_ctl(&positions, &indices, &mut |frac| {
            let bucket = (frac * 100.0) as i32;
            if bucket != last_bucket && (last_print.elapsed().as_millis() > 500 || bucket % 10 == 0) {
                eprintln!("  {:6.1}s  {:5.1}%", t0.elapsed().as_secs_f64(), frac * 100.0);
                last_bucket = bucket;
                last_print = Instant::now();
            }
            true
        });
        match result {
            Ok((pos, _uvs, idx, _src)) => eprintln!(
                "  done {:.2}s -> V={} F={}",
                t0.elapsed().as_secs_f64(),
                pos.len(),
                idx.len() / 3
            ),
            Err(e) => eprintln!("  FAILED {:.2}s: {e}", t0.elapsed().as_secs_f64()),
        }
    }
}
