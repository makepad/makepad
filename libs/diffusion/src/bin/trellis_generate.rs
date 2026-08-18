//! TRELLIS 2 image -> untextured GLB, end to end on our stack:
//! PNG -> preprocess (alpha crop + Lanczos) -> DINOv3 cond (512 + 1024) ->
//! SS flow + conv3d decode -> voxel coords -> LR shape flow -> cascade
//! upsample -> HR shape flow (1024) -> sparse FDG decode -> dual-grid mesh ->
//! GLB. Texture SLAT + bake not wired yet (untextured ships first).
//!
//! Usage:
//!   trellis-generate --image <rgba png> [--pre <preprocessed rgb png>]
//!                    [--noise-dir <t2dump dir>] [--seed 42]
//!                    [--out out.glb] [--stats stats.json]

use makepad_diffusion::backend::{gpu_perf_stats, gpu_pool_cap_override, gpu_pool_clear};
use makepad_diffusion::h3_pipeline::H3NoiseRng;
use makepad_diffusion::trellis::{
    t2_quantize_unique_coords, t2_rope_tables, TrellisWeights, T2_SHAPE_SAMPLER,
    T2_SHAPE_SLAT_MEAN, T2_SHAPE_SLAT_STD, T2_SLAT_CHANNELS, T2_SS_CHANNELS, T2_SS_SAMPLER,
    T2_SS_TOKENS, T2_TEX_IN_CHANNELS, T2_TEX_SAMPLER, T2_TEX_SLAT_MEAN, T2_TEX_SLAT_STD,
};
use makepad_diffusion::trellis_dino::T2Dino;
use makepad_diffusion::trellis_dit::{t2_upload_cond, t2_upload_rope, T2Dit};
use makepad_diffusion::trellis_image::{
    t2_cond_input, t2_pad_black, t2_preprocess_rgba, t2_subject_border, T2Image,
};
use makepad_diffusion::trellis_mesh::{
    t2_dual_grid_to_mesh, t2_fdg_fields, t2_mesh_to_glb_colored,
};
use makepad_diffusion::trellis_pipeline::{
    t2_chw_to_tokens, t2_run_ss, t2_sample_flow, t2_sample_flow_concat,
};
use makepad_diffusion::trellis_slat::T2SparseDec;
use makepad_diffusion::trellis_vae::T2SsDec;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn load_png_rgba(path: &Path) -> Result<T2Image, String> {
    let file = std::fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let reader = BufReader::new(file);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder.decode_headers().map_err(|err| format!("{err:?}"))?;
    let info = decoder.info().cloned().ok_or("png: no info")?;
    let colorspace = decoder.colorspace().ok_or("png: no colorspace")?;
    let pixels = decoder.decode_raw().map_err(|err| format!("{err:?}"))?;
    let components = colorspace.num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        match components {
            4 => rgba[i * 4..i * 4 + 4].copy_from_slice(chunk),
            3 => {
                rgba[i * 4..i * 4 + 3].copy_from_slice(chunk);
                rgba[i * 4 + 3] = 255;
            }
            _ => return Err(format!("png components {components} unsupported")),
        }
    }
    T2Image::from_rgba8(&rgba, w, h).map_err(|err| err.to_string())
}

struct NpyLite;
impl NpyLite {
    fn load_f32(path: &Path) -> Result<Vec<f32>, String> {
        let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
        if &bytes[..6] != b"\x93NUMPY" {
            return Err("not npy".into());
        }
        let (len, start) = if bytes[6] == 1 {
            (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
        } else {
            (
                u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
                12,
            )
        };
        let header = String::from_utf8_lossy(&bytes[start..start + len]).to_string();
        let data = &bytes[start + len..];
        if header.contains("<f4") {
            Ok(data
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        } else {
            Err(format!("npy descr unsupported: {header}"))
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts: HashMap<String, String> = HashMap::new();
    let mut key: Option<String> = None;
    for arg in &args[1..] {
        if let Some(name) = arg.strip_prefix("--") {
            key = Some(name.to_string());
            opts.entry(name.to_string()).or_default();
        } else if let Some(name) = key.take() {
            opts.insert(name, arg.clone());
        }
    }
    if let Err(err) = run(&opts) {
        eprintln!("trellis-generate FAILED: {err}");
        std::process::exit(1);
    }
    println!("TRELLIS-GENERATE-DONE");
}

fn run(opts: &HashMap<String, String>) -> Result<(), String> {
    let models = PathBuf::from(
        opts.get("models")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\ComfyUI\models\microsoft\TRELLIS.2-4B"),
    );
    let dino_path = PathBuf::from(opts.get("dino").map(String::as_str).unwrap_or(
        r"C:\ai\ComfyUI\models\facebook\dinov3-vitl16-pretrain-lvd1689m\model.safetensors",
    ));
    let ssdec_path = PathBuf::from(opts.get("ssdec").map(String::as_str).unwrap_or(
        r"C:\ai\ComfyUI\models\microsoft\TRELLIS-image-large\ckpts\ss_dec_conv3d_16l8_fp16.safetensors",
    ));
    let out_path = opts
        .get("out")
        .map(String::as_str)
        .unwrap_or(r"C:\ai\out\crown_ours_e2e.glb");
    let seed: u64 = opts
        .get("seed")
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);
    let noise_dir = opts.get("noise-dir").map(PathBuf::from);
    let passes: usize = opts
        .get("passes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    // --texture 1: run the tex SLAT flow + decode and write per-voxel base
    // color as COLOR_0 vertex colors. Default OFF to keep the untextured
    // bench numbers (tgen runners) comparable across sessions.
    let texture = opts.get("texture").map(String::as_str) == Some("1");
    // Idle activation-pool cap: on the 24GB box the all-resident model set +
    // the default 6GB idle pool sits past the WDDM residency budget — every
    // stage then pages. --pool-cap-mb overrides (0 = leave the env default).
    let pool_cap_mb: usize = opts
        .get("pool-cap-mb")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if pool_cap_mb > 0 {
        gpu_pool_cap_override(Some(pool_cap_mb * 1024 * 1024));
        println!("pool cap override: {pool_cap_mb} MB");
    }
    // --pool-clear 1: drop the idle pool at every stage boundary so phase
    // working sets never coexist.
    let pool_clear = opts.get("pool-clear").map(String::as_str) == Some("1");
    let mut timings: Vec<(String, f64)> = Vec::new();
    let mut timed = |name: &str, start: Instant| {
        let dt = start.elapsed().as_secs_f64();
        let perf = gpu_perf_stats(true);
        println!("STAGE {name}: {dt:.2}s");
        println!(
            "PERF {name}: evicts={} streams={} ({:.1}MB) pool_oom={} fresh={} ({:.1}MB) \
             overcap_free={:.1}MB vram_used={:.2}/{:.2}GB",
            perf.weight_evict_events,
            perf.weight_stream_count,
            perf.weight_stream_bytes as f64 / (1024.0 * 1024.0),
            perf.pool_oom_clears,
            perf.pool_fresh_alloc_count,
            perf.pool_fresh_alloc_bytes as f64 / (1024.0 * 1024.0),
            perf.pool_overcap_free_bytes as f64 / (1024.0 * 1024.0),
            (perf.mem_total_bytes - perf.mem_free_bytes) as f64 / (1024.0 * 1024.0 * 1024.0),
            perf.mem_total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        );
        if pool_clear {
            gpu_pool_clear();
        }
        timings.push((name.to_string(), dt));
    };

    // 1. Image -> preprocessed RGB.
    let start = Instant::now();
    let pre = if let Some(pre_path) = opts.get("pre") {
        let img = load_png_rgba(Path::new(pre_path))?;
        // Already preprocessed: keep RGB planes.
        let plane = img.width * img.height;
        T2Image {
            channels: 3,
            width: img.width,
            height: img.height,
            data: img.data[..3 * plane].to_vec(),
        }
    } else {
        let image_path = opts
            .get("image")
            .map(String::as_str)
            .unwrap_or(r"C:\ai\test_images\crown.png");
        let img = load_png_rgba(Path::new(image_path))?;
        t2_preprocess_rgba(&img).map_err(|err| err.to_string())?
    };
    let border = t2_subject_border(pre.width.min(pre.height));
    let pre = t2_pad_black(&pre, border).map_err(|err| err.to_string())?;
    println!("preprocessed {}x{} (border {border})", pre.width, pre.height);
    timed("preprocess", start);

    // Prepare every model up front (device-resident / cache-namespace
    // scoped) — pass 1 streams weights lazily inside compute (cold), later
    // passes are warm: the M5 service-residency story.
    let start = Instant::now();
    let dino_weights = TrellisWeights::load(&dino_path).map_err(|err| err.to_string())?;
    let dino = T2Dino::prepare(&dino_weights).map_err(|err| err.to_string())?;
    let ss_path = models.join("ckpts").join("ss_flow_img_dit_1_3B_64_bf16.safetensors");
    let ss_weights = TrellisWeights::load(&ss_path).map_err(|err| err.to_string())?;
    let ss_dit = T2Dit::prepare(ss_weights, "t2ss", T2_SS_CHANNELS, T2_SS_CHANNELS)
        .map_err(|err| err.to_string())?;
    let dec_weights = TrellisWeights::load(&ssdec_path).map_err(|err| err.to_string())?;
    let ss_dec = T2SsDec::prepare(&dec_weights).map_err(|err| err.to_string())?;
    let lr_path = models.join("ckpts").join("slat_flow_img2shape_dit_1_3B_512_bf16.safetensors");
    let lr_weights = TrellisWeights::load(&lr_path).map_err(|err| err.to_string())?;
    let lr_dit = T2Dit::prepare(lr_weights, "t2lr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
        .map_err(|err| err.to_string())?;
    let shape_dec_path = models.join("ckpts").join("shape_dec_next_dc_f16c32_fp16.safetensors");
    let shape_dec_weights = TrellisWeights::load(&shape_dec_path).map_err(|err| err.to_string())?;
    let shape_dec = T2SparseDec::prepare(shape_dec_weights, "t2sdec", 7, true)
        .map_err(|err| err.to_string())?;
    let hr_path = models.join("ckpts").join("slat_flow_img2shape_dit_1_3B_1024_bf16.safetensors");
    let hr_weights = TrellisWeights::load(&hr_path).map_err(|err| err.to_string())?;
    let hr_dit = T2Dit::prepare(hr_weights, "t2hr", T2_SLAT_CHANNELS, T2_SLAT_CHANNELS)
        .map_err(|err| err.to_string())?;
    let tex = if texture {
        let tex_path = models
            .join("ckpts")
            .join("slat_flow_imgshape2tex_dit_1_3B_1024_bf16.safetensors");
        let tex_weights = TrellisWeights::load(&tex_path).map_err(|err| err.to_string())?;
        let tex_dit = T2Dit::prepare(tex_weights, "t2tex", T2_TEX_IN_CHANNELS, T2_SLAT_CHANNELS)
            .map_err(|err| err.to_string())?;
        let tex_dec_path = models
            .join("ckpts")
            .join("tex_dec_next_dc_f16c32_fp16.safetensors");
        let tex_dec_weights =
            TrellisWeights::load(&tex_dec_path).map_err(|err| err.to_string())?;
        let tex_dec = T2SparseDec::prepare(tex_dec_weights, "t2tdec", 6, false)
            .map_err(|err| err.to_string())?;
        Some((tex_dit, tex_dec))
    } else {
        None
    };
    timed("prepare_all", start);

    for pass in 0..passes {
        if passes > 1 {
            println!("=== PASS {pass} ===");
            timed(&format!("pass{pass}"), Instant::now());
        }
        let mut rng = H3NoiseRng::new(seed);
        let mut draw = |count: usize, dump_name: &str| -> Result<Vec<f32>, String> {
            if let Some(dir) = &noise_dir {
                let path = dir.join(dump_name);
                if path.exists() {
                    let values = NpyLite::load_f32(&path)?;
                    if values.len() == count {
                        println!("noise {dump_name}: loaded from dump");
                        return Ok(values);
                    }
                    println!(
                        "noise {dump_name}: dump has {} values, need {count} — own rng",
                        values.len()
                    );
                }
            }
            Ok((0..count).map(|_| rng.next_normal()).collect())
        };

        // 2. DINOv3 cond at 512 + 1024.
        let start = Instant::now();
        let input_512 = t2_cond_input(&pre, 512).map_err(|err| err.to_string())?;
        let cond_512_host = dino.forward_rgb(&input_512, 512).map_err(|err| err.to_string())?;
        let input_1024 = t2_cond_input(&pre, 1024).map_err(|err| err.to_string())?;
        let cond_1024_host = dino.forward_rgb(&input_1024, 1024).map_err(|err| err.to_string())?;
        timed("cond", start);
        let cond_512 = t2_upload_cond(&cond_512_host).map_err(|err| err.to_string())?;
        let neg_512 =
            t2_upload_cond(&vec![0.0; cond_512_host.len()]).map_err(|err| err.to_string())?;
        let cond_1024 = t2_upload_cond(&cond_1024_host).map_err(|err| err.to_string())?;
        let neg_1024 =
            t2_upload_cond(&vec![0.0; cond_1024_host.len()]).map_err(|err| err.to_string())?;

        // 3. Sparse structure stage.
        let start = Instant::now();
        let noise_chw = draw(T2_SS_TOKENS * T2_SS_CHANNELS, "randn_00.npy")?;
        let noise_tokens = t2_chw_to_tokens(&noise_chw, T2_SS_CHANNELS, T2_SS_TOKENS);
        let ss = t2_run_ss(
            &ss_dit,
            &ss_dec,
            &noise_tokens,
            &cond_512,
            &neg_512,
            &T2_SS_SAMPLER,
            32,
            |_, _, _, _| {},
        )
        .map_err(|err| err.to_string())?;
        println!("ss coords: {}", ss.coords.len());
        timed("ss", start);

        // 4. LR shape flow at the SS coords.
        let start = Instant::now();
        let lr_rope = t2_upload_rope(&t2_rope_tables(&ss.coords)).map_err(|err| err.to_string())?;
        let mut lr = draw(ss.coords.len() * T2_SLAT_CHANNELS, "randn_01.npy")?;
        t2_sample_flow(
            &lr_dit,
            &mut lr,
            ss.coords.len(),
            &cond_512,
            &neg_512,
            &lr_rope,
            &T2_SHAPE_SAMPLER,
            false,
            |_, _, _, _| {},
        )
        .map_err(|err| err.to_string())?;
        for row in lr.chunks_exact_mut(T2_SLAT_CHANNELS) {
            for (value, (mean, std)) in row
                .iter_mut()
                .zip(T2_SHAPE_SLAT_MEAN.iter().zip(T2_SHAPE_SLAT_STD.iter()))
            {
                *value = *value * std + mean;
            }
        }
        timed("shape_lr", start);

        // 5. Cascade upsample -> HR token coords.
        let start = Instant::now();
        let hr_grid_coords = shape_dec
            .upsample(&lr, ss.coords.clone(), 4)
            .map_err(|err| err.to_string())?;
        let hr_coords = t2_quantize_unique_coords(&hr_grid_coords, 512, 1024);
        println!("hr tokens: {} (from {} @512)", hr_coords.len(), hr_grid_coords.len());
        timed("upsample", start);

        // 6. HR shape flow at 1024.
        let start = Instant::now();
        let hr_rope = t2_upload_rope(&t2_rope_tables(&hr_coords)).map_err(|err| err.to_string())?;
        let mut hr = draw(hr_coords.len() * T2_SLAT_CHANNELS, "randn_02.npy")?;
        t2_sample_flow(
            &hr_dit,
            &mut hr,
            hr_coords.len(),
            &cond_1024,
            &neg_1024,
            &hr_rope,
            &T2_SHAPE_SAMPLER,
            false,
            |_, _, _, _| {},
        )
        .map_err(|err| err.to_string())?;
        // Tex flow conditions on the NORMALIZED shape slat — snapshot
        // before the in-place denorm.
        let hr_normalized = texture.then(|| hr.clone());
        for row in hr.chunks_exact_mut(T2_SLAT_CHANNELS) {
            for (value, (mean, std)) in row
                .iter_mut()
                .zip(T2_SHAPE_SLAT_MEAN.iter().zip(T2_SHAPE_SLAT_STD.iter()))
            {
                *value = *value * std + mean;
            }
        }
        timed("shape_hr", start);

        // 7. Shape decode + dual-grid mesh + GLB. The decode phase rotates
        // GB-class voxel planes: a LOW cap wins here — gpu_pool_cap_override
        // shrinks the idle pool on entry (largest-first, so the flow stages'
        // small pooled buffers survive for the next pass), decode then runs
        // far from the VRAM ceiling where cudaMallocs are cheap. Measured on
        // the 5.58M-voxel crown: cap 3072 -> 4.66s vs cap 10240 -> 7.26s.
        let start = Instant::now();
        let decode_cap_mb: usize = opts
            .get("pool-cap-decode-mb")
            .and_then(|s| s.parse().ok())
            .unwrap_or(3072);
        // NOTE: no gpu_pool_clear here — measured NET NEGATIVE: it purged the
        // flow stages' pooled sizes, and re-malloc'ing them near the VRAM
        // ceiling costs ~20ms per cudaMalloc (ss went 3.0s -> 8.0s). The
        // largest-first release eviction already recycles decode's own
        // GB-planes while keeping the small flow buffers resident.
        if decode_cap_mb > 0 {
            gpu_pool_cap_override(Some(decode_cap_mb * 1024 * 1024));
        }
        let (feats, voxel_coords, subs) = shape_dec
            .decode(&hr, hr_coords.clone(), None)
            .map_err(|err| err.to_string())?;
        gpu_pool_cap_override((pool_cap_mb > 0).then_some(pool_cap_mb * 1024 * 1024));
        println!("decoded voxels: {}", voxel_coords.len());
        timed("shape_decode", start);

        // Optional tex SLAT flow + decode -> per-voxel PBR (base RGB,
        // metallic, roughness, alpha in 0..=1) at the shape voxel coords
        // (guided by the shape subdivision masks).
        let voxel_rgb: Option<Vec<[f32; 3]>> = match (&tex, hr_normalized) {
            (Some((tex_dit, tex_dec)), Some(concat_cond)) => {
                let start = Instant::now();
                let mut x = draw(hr_coords.len() * T2_SLAT_CHANNELS, "randn_03.npy")?;
                t2_sample_flow_concat(
                    tex_dit,
                    &mut x,
                    hr_coords.len(),
                    Some(&concat_cond),
                    &cond_1024,
                    &cond_1024, // strength 1.0: neg never used
                    &hr_rope,
                    &T2_TEX_SAMPLER,
                    false,
                    |_, _, _, _| {},
                )
                .map_err(|err| err.to_string())?;
                for row in x.chunks_exact_mut(T2_SLAT_CHANNELS) {
                    for (value, (mean, std)) in row
                        .iter_mut()
                        .zip(T2_TEX_SLAT_MEAN.iter().zip(T2_TEX_SLAT_STD.iter()))
                    {
                        *value = *value * std + mean;
                    }
                }
                timed("tex_flow", start);
                let start = Instant::now();
                if decode_cap_mb > 0 {
                    gpu_pool_cap_override(Some(decode_cap_mb * 1024 * 1024));
                }
                let (mut pbr, tex_coords, _) = tex_dec
                    .decode(&x, hr_coords, Some(&subs))
                    .map_err(|err| err.to_string())?;
                gpu_pool_cap_override((pool_cap_mb > 0).then_some(pool_cap_mb * 1024 * 1024));
                for value in &mut pbr {
                    *value = *value * 0.5 + 0.5;
                }
                println!("tex voxels: {}", tex_coords.len());
                timed("tex_decode", start);
                if tex_coords.len() == voxel_coords.len() {
                    Some(pbr.chunks_exact(6).map(|r| [r[0], r[1], r[2]]).collect())
                } else {
                    println!(
                        "tex voxel mismatch ({} vs {}) - untextured",
                        tex_coords.len(),
                        voxel_coords.len()
                    );
                    None
                }
            }
            _ => None,
        };

        let start = Instant::now();
        let fields = t2_fdg_fields(&feats).map_err(|err| err.to_string())?;
        let mesh =
            t2_dual_grid_to_mesh(&voxel_coords, &fields, 1024).map_err(|err| err.to_string())?;
        println!("mesh: {} verts {} faces", mesh.vertices.len(), mesh.faces.len());
        let glb = t2_mesh_to_glb_colored(&mesh, voxel_rgb.as_deref());
        std::fs::write(out_path, &glb).map_err(|err| err.to_string())?;
        println!("wrote {out_path} ({} bytes)", glb.len());
        timed("mesh_glb", start);
    }

    let total: f64 = timings.iter().map(|(_, t)| t).sum();
    println!("TOTAL {total:.2}s");
    if let Some(stats_path) = opts.get("stats") {
        let mut json = String::from("{\n");
        for (name, t) in &timings {
            json.push_str(&format!("  \"{name}\": {t:.3},\n"));
        }
        json.push_str(&format!("  \"total\": {total:.3}\n}}\n"));
        std::fs::write(stats_path, json).map_err(|err| err.to_string())?;
    }
    Ok(())
}
