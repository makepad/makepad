// Parity probe: run the makepad vision tower on a PPM image and compare the
// preprocessed tensor + output embeddings against clip.cpp oracle dumps
// produced by tools/vlm_oracle/clip_dump.
//
// usage: vlm-vision-probe <mmproj.gguf> <image.ppm> [reference_prefix]
//   reference_prefix.preproc.bin / reference_prefix.embd.bin as written by clip_dump

use makepad_ai_llm::{preprocess_rgb8, VisionConfig, VisionTower};

use std::fs;
use std::time::Instant;

fn read_ppm(path: &str) -> (Vec<u8>, usize, usize) {
    let data = fs::read(path).expect("cannot read image");
    let mut fields = Vec::new();
    let mut pos = 0usize;
    // P6 header: magic, width, height, maxval, single whitespace, then pixels
    while fields.len() < 4 {
        while pos < data.len() && (data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        if data[pos] == b'#' {
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < data.len() && !(data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1; // single whitespace after maxval
    assert_eq!(fields[0], "P6", "not a P6 ppm");
    let w: usize = fields[1].parse().unwrap();
    let h: usize = fields[2].parse().unwrap();
    assert_eq!(fields[3], "255", "maxval must be 255");
    let rgb = data[pos..pos + w * h * 3].to_vec();
    (rgb, w, h)
}

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
    let mut max_abs = 0f32;
    let mut max_at = 0usize;
    let mut dot = 0f64;
    let mut na = 0f64;
    let mut nb = 0f64;
    let mut sum_sq = 0f64;
    for (i, (&x, &y)) in ours.iter().zip(theirs).enumerate() {
        let d = (x - y).abs();
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
    println!(
        "{label}: n {} max_abs {:.3e} (at {}: ours {:.5} ref {:.5}) rms {:.3e} cosine {:.8}",
        ours.len(),
        max_abs,
        max_at,
        ours[max_at],
        theirs[max_at],
        rms,
        cos
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <mmproj.gguf> <image.ppm> [reference_prefix]", args[0]);
        std::process::exit(1);
    }
    let mmproj_path = &args[1];
    let image_path = &args[2];
    let ref_prefix = args.get(3);

    let (rgb, w, h) = read_ppm(image_path);
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
    let mut tower =
        VisionTower::load(mmproj_path).expect("load vision tower");
    println!("tower loaded in {:.2} s", t1.elapsed().as_secs_f64());

    let t2 = Instant::now();
    let embd = tower.encode(&prepared).expect("encode");
    println!(
        "encoded {} tokens x {} in {:.1} ms (includes graph compile)",
        prepared.n_tokens(),
        tower.config.proj_dim,
        t2.elapsed().as_secs_f64() * 1000.0
    );
    let t3 = Instant::now();
    let embd2 = tower.encode(&prepared).expect("encode 2");
    println!("second encode {:.1} ms", t3.elapsed().as_secs_f64() * 1000.0);
    assert_eq!(embd.len(), embd2.len());

    let mean = embd.iter().map(|&v| v as f64).sum::<f64>() / embd.len() as f64;
    let rms = (embd.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / embd.len() as f64)
        .sqrt();
    print!("embd mean {mean:.6} rms {rms:.6} first8:");
    for v in &embd[..8] {
        print!(" {v:.5}");
    }
    println!();

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
