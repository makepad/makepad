use super::*;

#[test]
fn chunk_cell_floors_negative_coordinates() {
    assert_eq!(chunk_cell(0.0, 0.0), (0, 0));
    assert_eq!(chunk_cell(CHUNK_SIZE - 0.01, CHUNK_SIZE - 0.01), (0, 0));
    assert_eq!(chunk_cell(CHUNK_SIZE, 0.0), (1, 0));
    // Truncation would fold cell -1 onto cell 0 and merge two cells'
    // content bounds across the origin.
    assert_eq!(chunk_cell(-0.01, -0.01), (-1, -1));
    assert_eq!(chunk_cell(-CHUNK_SIZE, -CHUNK_SIZE), (-1, -1));
    assert_eq!(chunk_cell(-CHUNK_SIZE - 0.01, 0.0), (-2, 0));
}

/// The debounce contract: the FIRST build is immediate (nothing stale
/// exists to show), an edit burst coalesces into ONE rebuild once the
/// world has been still for the settle window, and a key that keeps
/// moving keeps the rebuild parked.
#[test]
fn shadow_gate_coalesces_an_edit_burst() {
    let settle = 0.2;
    let t0 = 10.0;
    let mut gate = ShadowRebuildGate::default();
    // First sight builds immediately.
    assert!(gate.should_rebuild((1, 0, 0, 0), t0, settle));
    gate.mark_built((1, 0, 0, 0));
    assert!(!gate.should_rebuild((1, 0, 0, 0), t0, settle));
    // Burst: five mutations in quick succession — no rebuild during it,
    // and the settle clock restarts on every change.
    for i in 2..7u64 {
        let now = t0 + 0.01 * i as f64;
        assert!(!gate.should_rebuild((i, 0, 0, 0), now, settle));
    }
    // Still pending just before the window closes...
    let last_change = t0 + 0.06;
    assert!(!gate.should_rebuild((6, 0, 0, 0), last_change + 0.199, settle));
    // ...and exactly one rebuild once it has.
    let at_rest = last_change + 0.2;
    assert!(gate.should_rebuild((6, 0, 0, 0), at_rest, settle));
    gate.mark_built((6, 0, 0, 0));
    assert!(!gate.should_rebuild((6, 0, 0, 0), at_rest + 1.0, settle));
}

/// Tiling must regroup the terrain mesh, not change it: the union of
/// every tile's triangles is byte-identical (as a multiset of emitted
/// vertices) to the whole-mesh emission.
#[test]
fn terrain_tiles_union_to_the_whole_mesh() {
    let n = 5;
    let terrain = Terrain {
        cells: n,
        cell_size: 30.0,
        origin: -60.0,
        heights: (0..n * n).map(|i| (i as f32 * 0.7).sin() * 3.0).collect(),
        colors: (0..n * n)
            .map(|i| vec4(i as f32 / 25.0, 0.5, 0.25, 1.0))
            .collect(),
        revision: 1,
    };
    let vertex_multiset = |vertices: &[f32]| {
        let mut set: Vec<Vec<u32>> = vertices
            .chunks_exact(16)
            .map(|v| v.iter().map(|f| f.to_bits()).collect())
            .collect();
        set.sort();
        set
    };
    let (whole_verts, whole_idx, ..) = terrain_tile_data(&terrain, None, 0, n - 1, 0, n - 1);
    // 30-unit cells against 48-unit tiles: one cell per tile, 4x4 tiles.
    let cells_per_tile = ((CHUNK_SIZE / terrain.cell_size) as usize).max(1);
    assert_eq!(cells_per_tile, 1);
    let mut tiled_verts = Vec::new();
    let mut tiled_idx_count = 0;
    for gz in (0..n - 1).step_by(cells_per_tile) {
        for gx in (0..n - 1).step_by(cells_per_tile) {
            let (v, i, min, max) = terrain_tile_data(
                &terrain,
                None,
                gx,
                (gx + cells_per_tile).min(n - 1),
                gz,
                (gz + cells_per_tile).min(n - 1),
            );
            // Tile bounds contain the tile's own vertices.
            for p in v.chunks_exact(16) {
                assert!(p[0] >= min.x && p[0] <= max.x);
                assert!(p[1] >= min.y && p[1] <= max.y);
                assert!(p[2] >= min.z && p[2] <= max.z);
            }
            tiled_idx_count += i.len();
            tiled_verts.extend_from_slice(&v);
        }
    }
    assert_eq!(tiled_idx_count, whole_idx.len());
    assert_eq!(vertex_multiset(&tiled_verts), vertex_multiset(&whole_verts));
}
