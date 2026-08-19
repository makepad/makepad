// Included by obj/splat_pack.rs under #[cfg(test)].

fn lcg(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 40) as f32) / (1u64 << 24) as f32
}

fn random_unit_quaternion(seed: &mut u64) -> [f32; 4] {
    let q = [
        lcg(seed) * 2.0 - 1.0,
        lcg(seed) * 2.0 - 1.0,
        lcg(seed) * 2.0 - 1.0,
        lcg(seed) * 2.0 - 1.0,
    ];
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-6);
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

fn random_records(count: usize, extent: f32, seed: &mut u64) -> Vec<SplatRecord> {
    (0..count)
        .map(|_| SplatRecord {
            center: [
                (lcg(seed) - 0.5) * extent,
                (lcg(seed) - 0.5) * extent * 0.3,
                (lcg(seed) - 0.5) * extent,
            ],
            scales: [
                (lcg(seed) * -8.0).exp() + 0.0015,
                (lcg(seed) * -8.0).exp() + 0.0015,
                (lcg(seed) * -8.0).exp() + 0.0015,
            ],
            rotation: random_unit_quaternion(seed),
            color: [lcg(seed), lcg(seed), lcg(seed), lcg(seed)],
        })
        .collect()
}

fn quat_angle_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * dot.acos().to_degrees()
}

#[test]
fn record_round_trip_error_bounds() {
    let mut seed = 7u64;
    // Scene-sized spread (the renderer normalizes the largest axis to 2.2).
    let records = random_records(20_000, 2.2, &mut seed);
    let (order, codes) = morton_sorted(&records);
    let ordered: Vec<SplatRecord> = order.iter().map(|&i| records[i as usize]).collect();
    let scale_range = ScaleRange::from_records(&ordered);
    let scale_step = scale_range.ln_range / 255.0;

    let mut max_pos_err = 0.0f32;
    let mut max_scale_rel_err = 0.0f32;
    let mut max_quat_deg = 0.0f32;
    let mut max_color_err = 0.0f32;
    for (first, len) in chunk_runs(&codes) {
        let chunk = &ordered[first..first + len];
        let bounds = ChunkBounds::from_records(chunk);
        for record in chunk {
            let words = pack_record(record, &bounds, &scale_range);
            let back = unpack_record(words, &bounds, &scale_range);
            for axis in 0..3 {
                max_pos_err = max_pos_err.max((back.center[axis] - record.center[axis]).abs());
                let rel = (back.scales[axis] / record.scales[axis] - 1.0).abs();
                max_scale_rel_err = max_scale_rel_err.max(rel);
                assert!(
                    bounds.min[axis] <= record.center[axis] + 1e-6
                        && record.center[axis] <= bounds.min[axis] + bounds.extent[axis] + 1e-6
                );
            }
            max_quat_deg = max_quat_deg.max(quat_angle_deg(back.rotation, record.rotation));
            for c in 0..4 {
                max_color_err = max_color_err.max((back.color[c] - record.color[c]).abs());
            }
        }
    }
    // Position: half a 14-bit step of the chunk extent. Chunks are at most
    // scene/8 wide (CHUNK_SPLIT_LEVEL), so worst case 2.2/8/16383/2 = 8.4e-6
    // units; dense regions are far tighter. The renderer's min splat radius
    // is 1.5e-3.
    println!(
        "pack errors: pos {:.2e} units, scale {:.3}% (step {:.4} ln), quat {:.3} deg, color {:.4}",
        max_pos_err,
        max_scale_rel_err * 100.0,
        scale_step,
        max_quat_deg,
        max_color_err
    );
    assert!(max_pos_err < 1.0e-5, "pos err {max_pos_err}");
    // 8-bit log scale over the scene range: ±half a step.
    assert!(max_scale_rel_err <= (scale_step * 0.5).exp() - 1.0 + 1e-4);
    // Three 9-bit components over [-1/√2, 1/√2]; the reconstructed largest
    // component amplifies the error near equal magnitudes: worst 0.55 deg.
    assert!(max_quat_deg < 0.6, "quat err {max_quat_deg} deg");
    assert!(max_color_err <= 0.5 / 255.0 + 1e-6);
}

#[test]
fn quaternion_sign_and_largest_component_round_trip() {
    for q in [
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0, -1.0],
        [1.0, 0.0, 0.0, 0.0],
        [-0.5, 0.5, -0.5, 0.5],
        [0.7071, 0.7071, 0.0, 0.0],
    ] {
        let (c0, c1, c2, i) = encode_quaternion(q);
        let back = decode_quaternion(c0, c1, c2, i);
        // Equal-magnitude components are the worst case (0.55 deg analytic).
        assert!(quat_angle_deg(q, back) < 0.6, "{q:?} -> {back:?}");
    }
}

#[test]
fn morton_order_is_a_permutation_with_bounded_chunks() {
    let mut seed = 3u64;
    let records = random_records(10_000, 2.2, &mut seed);
    let (order, codes) = morton_sorted(&records);
    let mut seen = vec![false; records.len()];
    for &i in &order {
        assert!(!seen[i as usize]);
        seen[i as usize] = true;
    }
    assert!(codes.windows(2).all(|w| w[0] <= w[1]));
    let ordered: Vec<SplatRecord> = order.iter().map(|&i| records[i as usize]).collect();
    let runs = chunk_runs(&codes);
    assert_eq!(runs.iter().map(|r| r.1).sum::<usize>(), records.len());
    assert!(runs.iter().all(|r| r.1 >= 1 && r.1 <= CHUNK_SPLATS));
    let scene = PackedScene::build(&ordered, &codes);
    assert_eq!(scene.records, records.len());
    assert_eq!(scene.count, runs.len() * CHUNK_SPLATS);
    assert_eq!(scene.centers.len(), scene.count);
    // Padding slots are marked invisible; real slots carry their radius.
    let padding = scene.radius_bound.iter().filter(|r| **r < 0.0).count();
    assert_eq!(padding, scene.count - scene.records);
    // Chunks never cross a level-3 cell: extent <= scene/8 (+ the cell's
    // own quantization slop), and splitting wastes little: random uniform
    // points are the worst case for early closes.
    let mut max_extent = 0.0f32;
    for chunk in 0..runs.len() {
        let base = chunk * 8;
        for axis in 0..3 {
            max_extent = max_extent.max(scene.chunk_texels[base + 4 + axis]);
        }
    }
    assert!(max_extent <= 2.2 / 8.0 * 1.01, "chunk extent {max_extent}");
    // Early closes cost at most one partial chunk per level-3 cell: a fixed
    // 8^3 * 256 slots (2 MB of texture) however sparse the scene. A 10k
    // uniform cloud is the worst case (one cell = one 20-record chunk).
    let fill = scene.records as f32 / scene.count as f32;
    println!("chunk fill {:.1}% over {} chunks (max extent {:.3})", fill * 100.0, runs.len(), max_extent);
    assert!(scene.count <= scene.records + 512 * CHUNK_SPLATS);
    // Row/texel geometry.
    assert_eq!(scene.words.len(), scene.rows * RECORDS_PER_ROW * 4);
    assert_eq!(scene.chunk_texels.len(), scene.chunk_rows * CHUNKS_PER_ROW * 8);
}

#[test]
fn packed_words_match_unpack_field_layout() {
    let chunk = ChunkBounds {
        min: [0.0, 0.0, 0.0],
        extent: [1.0, 1.0, 1.0],
    };
    let scales = ScaleRange {
        ln_min: -5.0,
        ln_range: 5.0,
    };
    let record = SplatRecord {
        center: [1.0, 0.0, 1.0],
        scales: [1.0, (-5.0f32).exp(), 1.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        color: [1.0, 0.0, 1.0, 0.0],
    };
    let [w0, w1, w2, w3] = pack_record(&record, &chunk, &scales);
    assert_eq!(w0, 0x00ff00ff);
    // px = 16383 (14 ones), py = 0, pz = 16383 -> low 4 bits in word1 top.
    assert_eq!(w1 & 0x3fff, 16383);
    assert_eq!((w1 >> 14) & 0x3fff, 0);
    assert_eq!(w1 >> 28, 0xf);
    assert_eq!(w2 & 0x3ff, 16383 >> 4);
    assert_eq!((w2 >> 10) & 0xff, 255); // sx
    assert_eq!((w2 >> 18) & 0xff, 0); // sy
    assert_eq!(((w2 >> 26) & 0x3f) | ((w3 & 0x3) << 6), 255); // sz
    assert_eq!((w3 >> 29) & 0x3, 3); // largest quaternion component = w
    assert_eq!(w3 >> 31, 0); // spare bit
    let back = unpack_record([w0, w1, w2, w3], &chunk, &scales);
    assert_eq!(back.center, record.center);
    assert_eq!(back.color, record.color);
}
