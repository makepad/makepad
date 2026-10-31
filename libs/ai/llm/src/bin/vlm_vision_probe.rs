// Parity probe: run the makepad vision tower on a PPM image and compare the
// preprocessed tensor + output embeddings against clip.cpp oracle dumps
// produced by tools/vlm_oracle/clip_dump.
//
// usage: vlm-vision-probe <mmproj.gguf> <image.ppm> [reference_prefix]
//   reference_prefix.preproc.bin / reference_prefix.embd.bin as written by clip_dump

use makepad_ai_llm::{preprocess_rgb8, VisionConfig, VisionExecutionConfig, VisionTower};

use std::fs;
use std::time::Instant;

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn compare(label: &str, ours: &[f32], theirs: &[f32]) {
    assert_eq!(ours.len(), theirs.len(), "{label}: length mismatch");
    assert!(!ours.is_empty(), "{label}: empty output");
    let mut max_abs = 0f32;
    let mut max_rel = 0f32;
    let mut max_ref = 0f32;
    let mut max_at = 0usize;
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut sum_sq = 0f64;
    for (i, (&x, &y)) in ours.iter().zip(theirs).enumerate() {
        assert!(x.is_finite() && y.is_finite(), "{label}: nonfinite value at {i}");
        let d = (x - y).abs();
        max_rel = max_rel.max(d / y.abs().max(1e-6));
        max_ref = max_ref.max(y.abs());
        if d > max_abs {
            max_abs = d;
            max_at = i;
        }
        dot += (x as f64) * (y as f64);
        na += (x as f64) * (x as f64);
        nb += (y as f64) * (y as f64);
        sum_sq += (d as f64) * (d as f64);
    }
    let cos = dot / (na.sqrt() * nb.sqrt()).max(1e-30);
    let rms = (sum_sq / ours.len() as f64).sqrt();
    let relative_l2 = (sum_sq / nb.max(1e-30)).sqrt();
    println!(
        "{label}: n {} max_abs {:.6e} (at {}: ours {:.6} ref {:.6}) rms {:.6e} cosine {:.10} max_rel {:.6e} (denom floor 1e-6) max_abs/ref_peak {:.6e} relative_l2 {:.6e}",
        ours.len(),
        max_abs,
        max_at,
        ours[max_at],
        theirs[max_at],
        rms,
        cos,
        max_rel,
        max_abs / max_ref.max(1e-30),
        relative_l2,
    );
}

// A reproducible RGB fixture when no camera frame is supplied. At 512 x
// 512 this exercises 1024 image patches, including position interpolation.
fn synthetic_image(size: &str) -> (Vec<u8>, usize, usize) {
    let (w, h) = size.split_once('x').expect("synthetic size must be WIDTHxHEIGHT");
    let w: usize = w.parse().expect("synthetic width");
    let h: usize = h.parse().expect("synthetic height");
    assert!(w >= 2 && h >= 2);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let tile = ((x / 13) ^ (y / 17)) & 1;
            rgb.extend_from_slice(&[
                ((x * 255) / (w - 1)) as u8,
                ((y * 255) / (h - 1)) as u8,
                if tile == 0 { 24 } else { 232 },
            ]);
        }
    }
    (rgb, w, h)
}

fn write_f32_dump(path: &str, header: &[u32], data: &[f32]) {
    let mut bytes = Vec::with_capacity(header.len() * 4 + data.len() * 4);
    for &v in header { bytes.extend_from_slice(&v.to_le_bytes()); }
    for &v in data { bytes.extend_from_slice(&v.to_le_bytes()); }
    fs::write(path, bytes).expect("write probe dump");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <mmproj.gguf> <image.ppm|--synthetic=WIDTHxHEIGHT> [reference_prefix] [--dump-prefix prefix] [--repeats N] [--f32-reference]", args[0]);
        std::process::exit(1);
    }
    let mmproj_path = &args[1];
    let image_path = &args[2];
    let mut ref_prefix = None;
    let mut dump_prefix = None;
    let mut repeats = 3usize;
    let mut execution = VisionExecutionConfig::default();
    let mut options = args[3..].iter();
    while let Some(arg) = options.next() {
        match arg.as_str() {
            "--dump-prefix" => dump_prefix = Some(options.next().expect("missing dump prefix")),
            "--repeats" => repeats = options.next().expect("missing repeats").parse().expect("invalid repeats"),
            "--f32-reference" => execution.use_f16_gemm = false,
            other => {
                assert!(!other.starts_with('-') && ref_prefix.is_none(), "unknown or duplicate argument: {other}");
                ref_prefix = Some(arg);
            }
        }
    }
    assert!(repeats > 0, "at least one warm repeat is required");

    let (rgb, w, h) = if let Some(size) = image_path.strip_prefix("--synthetic=") {
        synthetic_image(size)
    } else {
        makepad_ai_llm::read_ppm(image_path).expect("read ppm")
    };
    println!("image: {w} x {h}");

    let gguf = makepad_ai_llm::GgufFile::open(mmproj_path).expect("open mmproj");
    let config = VisionConfig::from_gguf(&gguf).expect("vision config");
    println!(
        "config: {} layers, embd {}, heads {}, proj {}, image {} patch {} merge {}",
        config.n_layers,
        config.n_embd,
        config.n_heads,
        config.proj_dim,
        config.image_size,
        config.patch_size,
        config.n_merge
    );

    let t0 = Instant::now();
    let prepared = preprocess_rgb8(&rgb, w, h, &config).expect("preprocess");
    println!(
        "preprocessed: {} x {} ({} patches, {} tokens) in {:.1} ms",
        prepared.width,
        prepared.height,
        prepared.n_patches(),
        prepared.n_tokens(),
        t0.elapsed().as_secs_f64() * 1000.0
    );

    if let Some(prefix) = ref_prefix {
        let bytes = fs::read(format!("{prefix}.preproc.bin")).expect("read ref preproc");
        let rnx = read_u32(&bytes, 0) as usize;
        let rny = read_u32(&bytes, 4) as usize;
        assert_eq!(
            (rnx, rny),
            (prepared.width, prepared.height),
            "preprocessed size mismatch vs reference"
        );
        let ref_pixels = read_f32s(&bytes[8..]);
        compare("preproc", &prepared.pixels, &ref_pixels);
    }

    let t1 = Instant::now();
    let mut tower = VisionTower::load_with_execution_config(mmproj_path, execution).expect("load vision tower");
    println!("tower loaded in {:.2} s", t1.elapsed().as_secs_f64());
    println!("backend: {:?}; {}; execution: {:?}", tower.backend(), tower.device_description(), execution);

    let t2 = Instant::now();
    let embd = tower.encode(&prepared).expect("encode");
    println!(
        "encoded {} tokens x {} in {:.1} ms (includes graph compile)",
        prepared.n_tokens(),
        tower.config.proj_dim,
        t2.elapsed().as_secs_f64() * 1000.0
    );
    let mut times = Vec::with_capacity(repeats);
    for run in 0..repeats {
        let t3 = Instant::now();
        let next = tower.encode(&prepared).expect("warm encode");
        let ms = t3.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        println!("warm encode {}: {:.3} ms", run + 1, ms);
        compare("replay", &next, &embd);
    }
    times.sort_by(f64::total_cmp);
    let median = (times[(repeats - 1) / 2] + times[repeats / 2]) * 0.5;
    println!("warm encode median {median:.3} ms, min {:.3} max {:.3}, repeats {repeats}", times[0], times[repeats - 1]);

    let mean = embd.iter().map(|&v| v as f64).sum::<f64>() / embd.len() as f64;
    let rms = (embd.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / embd.len() as f64)
        .sqrt();
    print!("embd mean {mean:.6} rms {rms:.6} first8:");
    for v in &embd[..8] {
        print!(" {v:.5}");
    }
    println!();

    if let Some(prefix) = dump_prefix {
        write_f32_dump(&format!("{prefix}.preproc.bin"), &[prepared.width as u32, prepared.height as u32], &prepared.pixels);
        write_f32_dump(&format!("{prefix}.embd.bin"), &[prepared.n_tokens() as u32, tower.config.proj_dim as u32, 0, 0], &embd);
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend_from_slice(&rgb);
        fs::write(format!("{prefix}.ppm"), ppm).expect("write fixture");
        println!("wrote fixture and reference dumps to {prefix}.*");
    }

    if let Some(prefix) = ref_prefix {
        let bytes = fs::read(format!("{prefix}.embd.bin")).expect("read ref embd");
        let n_tokens = read_u32(&bytes, 0) as usize;
        let n_embd = read_u32(&bytes, 4) as usize;
        assert_eq!(n_tokens, prepared.n_tokens(), "token count mismatch");
        assert_eq!(n_embd, tower.config.proj_dim, "embd dim mismatch");
        let ref_embd = read_f32s(&bytes[16..]);
        compare("embd", &embd, &ref_embd);
    }
}
