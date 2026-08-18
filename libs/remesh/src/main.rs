//! CLI: makepad-remesh <in.glb> <out.glb> [--res 512] [--no-clamp]
//!      [--lambda-d 1e-3] [--tri-mode auto] [--denormalize]
//!      [--postprocess <target_faces> [--tex 1024]]
//!      [--narrow-band [--band 1] [--project 0] [--faces N]]
//!
//! --postprocess skips the FCT remesh entirely and instead runs the
//! game-asset postprocess chain on the input (weld -> fill_small_holes ->
//! drop_small_components -> QEM decimate to the face target -> weld/fill/
//! drop -> chart unwrap + DEBUG bake where texel color = surface position),
//! writing a textured GLB. Chain timing prints per stage — the profiling
//! harness for the trellis bake path.

use makepad_remesh::{remesh_glb, RemeshOptions, TriangulationMode};

fn narrow_band(
    bytes: &[u8],
    resolution: usize,
    band: usize,
    project: f32,
    target: Option<usize>,
    out_path: &str,
) {
    let loaded = match makepad_gltf::load_gltf_from_bytes(bytes, None) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("error: parse: {error:?}");
            std::process::exit(1);
        }
    };
    let prim = match makepad_gltf::decode_mesh_primitive(&loaded, 0, 0) {
        Ok(prim) => prim,
        Err(error) => {
            eprintln!("error: decode: {error:?}");
            std::process::exit(1);
        }
    };
    let mut positions = prim.positions;
    let mut indices = prim.indices;
    eprintln!("in: V {} F {}", positions.len(), indices.len() / 3);
    let mut t = std::time::Instant::now();
    let stage = |name: &str, t: &mut std::time::Instant| {
        eprintln!("  {name}: {:.3}s", t.elapsed().as_secs_f64());
        *t = std::time::Instant::now();
    };

    // Match the preserved TRELLIS post-replay oracle: make the decoded
    // surface coherent before freezing it as the projection/bake source.
    let welded = makepad_remesh::weld_vertices(
        &mut positions,
        &mut indices,
        1.0 / (resolution as f32 * 8.0),
    );
    stage(&format!("weld ({welded} merged)"), &mut t);
    let filled = makepad_remesh::fill_small_holes(&mut indices, 64);
    stage(&format!("fill_small_holes ({filled})"), &mut t);
    let bvh = match makepad_remesh::SurfaceBvh::build(&positions, &indices) {
        Ok(bvh) => bvh,
        Err(error) => {
            eprintln!("error: bvh: {error}");
            std::process::exit(1);
        }
    };
    stage("surface BVH", &mut t);
    let remeshed = match makepad_remesh::remesh_narrow_band_dc(
        &positions,
        &indices,
        &bvh,
        resolution,
        band,
        project,
    ) {
        Ok(mesh) => mesh,
        Err(error) => {
            eprintln!("error: narrow-band remesh: {error}");
            std::process::exit(1);
        }
    };
    let (mut output_positions, mut output_indices) = (remeshed.positions, remeshed.indices);
    stage(
        &format!(
            "narrow-band V {} F {}",
            output_positions.len(),
            output_indices.len() / 3
        ),
        &mut t,
    );
    makepad_remesh::weld_vertices(
        &mut output_positions,
        &mut output_indices,
        1.0 / (resolution as f32 * 8.0),
    );
    makepad_remesh::fill_small_holes(&mut output_indices, 64);
    makepad_remesh::drop_small_components(
        &mut output_positions,
        &mut output_indices,
        0.02,
    );
    stage("remesh cleanup", &mut t);
    if let Some(target) = target {
        let decimated = makepad_remesh::decimate_qem(
            &output_positions,
            &output_indices,
            target,
        );
        output_positions = decimated.0;
        output_indices = decimated.1;
        makepad_remesh::weld_vertices(
            &mut output_positions,
            &mut output_indices,
            1.0 / (resolution as f32 * 8.0),
        );
        makepad_remesh::fill_small_holes(&mut output_indices, 64);
        makepad_remesh::drop_small_components(
            &mut output_positions,
            &mut output_indices,
            0.03,
        );
        stage(
            &format!("decimate/cleanup F {}", output_indices.len() / 3),
            &mut t,
        );
    }
    let before = makepad_remesh::audit_mesh_topology(&output_positions, &output_indices);
    let reoriented =
        makepad_remesh::unify_face_orientations(&output_positions, &mut output_indices);
    let after = makepad_remesh::audit_mesh_topology(&output_positions, &output_indices);
    eprintln!(
        "  topology: boundary {} nonmanifold {} inconsistent {} -> {}; reoriented {} faces; volume {:.6}",
        after.boundary_edges,
        after.nonmanifold_edges,
        before.inconsistent_edges,
        after.inconsistent_edges,
        reoriented,
        after.signed_volume,
    );
    let glb = makepad_gltf::write_glb_mesh(&output_positions, &output_indices);
    if let Err(error) = std::fs::write(out_path, &glb) {
        eprintln!("error: cannot write {out_path}: {error}");
        std::process::exit(1);
    }
    eprintln!(
        "narrow-band -> {out_path}: V {} F {} ({} bytes)",
        output_positions.len(),
        output_indices.len() / 3,
        glb.len()
    );
}

fn postprocess(bytes: &[u8], target: usize, tex: usize, out_path: &str) {
    let loaded = match makepad_gltf::load_gltf_from_bytes(bytes, None) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: parse: {e:?}");
            std::process::exit(1);
        }
    };
    let prim = match makepad_gltf::decode_mesh_primitive(&loaded, 0, 0) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: decode: {e:?}");
            std::process::exit(1);
        }
    };
    let mut positions = prim.positions;
    let mut indices = prim.indices;
    eprintln!("in: V {} F {}", positions.len(), indices.len() / 3);
    let mut t = std::time::Instant::now();
    let mut stage = |name: &str, t: &mut std::time::Instant| {
        eprintln!("  {name}: {:.3}s", t.elapsed().as_secs_f64());
        *t = std::time::Instant::now();
    };
    let welded = makepad_remesh::weld_vertices(&mut positions, &mut indices, 1.0 / 8192.0);
    stage(&format!("weld ({welded} merged)"), &mut t);
    let filled = makepad_remesh::fill_small_holes(&mut indices, 64);
    stage(&format!("fill_small_holes ({filled})"), &mut t);
    let dropped = makepad_remesh::drop_small_components(&mut positions, &mut indices, 0.02);
    stage(&format!("drop_small_components ({dropped})"), &mut t);
    let (mut dp, mut di) = makepad_remesh::decimate_qem(&positions, &indices, target);
    stage(
        &format!("decimate_qem {} -> {}", indices.len() / 3, di.len() / 3),
        &mut t,
    );
    makepad_remesh::weld_vertices(&mut dp, &mut di, 1.0 / 8192.0);
    makepad_remesh::fill_small_holes(&mut di, 64);
    makepad_remesh::drop_small_components(&mut dp, &mut di, 0.03);
    stage("post-decimate cleanup", &mut t);
    // Debug sampler: color = position normalized into the bbox (charts and
    // seams become directly visible).
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &dp {
        for a in 0..3 {
            mn[a] = mn[a].min(p[a]);
            mx[a] = mx[a].max(p[a]);
        }
    }
    let sample = move |p: [f32; 3]| -> Option<[f32; 6]> {
        let mut c = [0f32; 6];
        for a in 0..3 {
            let span = (mx[a] - mn[a]).max(1e-6);
            c[a] = (p[a] - mn[a]) / span;
        }
        c[4] = 0.8;
        c[5] = 1.0;
        Some(c)
    };
    let baked = makepad_remesh::uv_box_bake(&dp, &di, tex, &sample);
    stage(
        &format!(
            "uv_chart_bake (Vo {} Fo {})",
            baked.positions.len(),
            baked.indices.len() / 3
        ),
        &mut t,
    );
    if !baked.ok() {
        eprintln!("error: bake produced no charts");
        std::process::exit(1);
    }
    let base_png = encode_png_rgba(&baked.base_rgba, tex, tex);
    let mr_png = encode_png_rgba(&baked.mr_rgba, tex, tex);
    let glb = makepad_gltf::write_glb_mesh_textured(&makepad_gltf::GlbTexturedMesh {
        positions: &baked.positions,
        normals: None,
        uvs: &baked.uvs,
        indices: &baked.indices,
        base_color_png: &base_png,
        metallic_roughness_png: Some(&mr_png),
        double_sided: true,
        colors: None,
    });
    stage("png encode + glb write", &mut t);
    if let Err(e) = std::fs::write(out_path, &glb) {
        eprintln!("error: cannot write {out_path}: {e}");
        std::process::exit(1);
    }
    eprintln!("postprocessed -> {out_path} ({} bytes)", glb.len());
}

/// Minimal store-only PNG (no external encoder dep in this crate): zlib
/// stored blocks + CRC. Fine for a debug harness.
fn encode_png_rgba(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (n, t) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *t = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        c ^ 0xFFFF_FFFF
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_in = kind.to_vec();
        crc_in.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
    }
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for row in rgba.chunks_exact(w * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    for block in raw.chunks(65535) {
        let last = block.as_ptr() as usize + block.len()
            == raw.as_ptr() as usize + raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn usage() -> ! {
    eprintln!(
        "usage: makepad-remesh <in.glb> <out.glb> [--res 512] [--no-clamp] \
         [--lambda-d 1e-3] [--tri-mode auto|simple_02|simple_13|length|angle|normal|normal_abs] \
         [--denormalize] [--narrow-band [--band 1] [--project 0] [--faces N]]"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths: Vec<&str> = Vec::new();
    let mut res: u32 = 512;
    let mut post_target: Option<usize> = None;
    let mut use_narrow_band = false;
    let mut narrow_band_cells: usize = 1;
    // VisualBruno's production Ovoxel exporter explicitly passes 0. The
    // 0.9 library default is not used by the commercial-quality workflows.
    let mut narrow_project: f32 = 0.0;
    let mut narrow_target: Option<usize> = None;
    let mut tex: usize = 1024;
    let mut opts = RemeshOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--postprocess" => {
                i += 1;
                post_target =
                    Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()));
            }
            "--narrow-band" => use_narrow_band = true,
            "--band" => {
                i += 1;
                narrow_band_cells =
                    args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "--project" => {
                i += 1;
                narrow_project =
                    args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "--faces" => {
                i += 1;
                narrow_target =
                    Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()));
            }
            "--tex" => {
                i += 1;
                tex = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "--res" => {
                i += 1;
                res = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "--no-clamp" => opts.clamp_anchors = false,
            "--denormalize" => opts.denormalize = true,
            "--lambda-d" => {
                i += 1;
                opts.lambda_d = args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
            }
            "--tri-mode" => {
                i += 1;
                opts.tri_mode = match args.get(i).map(|s| s.as_str()) {
                    Some("auto") => TriangulationMode::Auto,
                    Some("simple_02") => TriangulationMode::Simple02,
                    Some("simple_13") => TriangulationMode::Simple13,
                    Some("length") => TriangulationMode::Length,
                    Some("angle") => TriangulationMode::Angle,
                    Some("normal") => TriangulationMode::Normal,
                    Some("normal_abs") => TriangulationMode::NormalAbs,
                    _ => usage(),
                };
            }
            s if !s.starts_with("--") => paths.push(s),
            _ => usage(),
        }
        i += 1;
    }
    if paths.len() != 2 {
        usage();
    }
    if !res.is_power_of_two() || res < 4 {
        eprintln!("error: --res must be a power of two >= 4");
        std::process::exit(2);
    }

    let bytes = match std::fs::read(paths[0]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: cannot read {}: {e}", paths[0]);
            std::process::exit(1);
        }
    };
    if let Some(target) = post_target {
        postprocess(&bytes, target, tex, paths[1]);
        return;
    }
    if use_narrow_band {
        if narrow_band_cells == 0 || !(0.0..=1.0).contains(&narrow_project) {
            eprintln!("error: --band must be positive and --project must be in 0..=1");
            std::process::exit(2);
        }
        narrow_band(
            &bytes,
            res as usize,
            narrow_band_cells,
            narrow_project,
            narrow_target,
            paths[1],
        );
        return;
    }
    let t0 = std::time::Instant::now();
    let out = match remesh_glb(&bytes, res, &opts) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let dt = t0.elapsed();
    if let Err(e) = std::fs::write(paths[1], &out) {
        eprintln!("error: cannot write {}: {e}", paths[1]);
        std::process::exit(1);
    }
    eprintln!(
        "remeshed {} -> {} at {res}^3 in {:.3}s ({} bytes)",
        paths[0],
        paths[1],
        dt.as_secs_f64(),
        out.len()
    );
}
