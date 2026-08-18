//! Native DA3METRIC-LARGE release validator.
//!
//! `da3-validate <model.safetensors> <input.png> <output-dir> [warm-runs]`
//!
//! The harness is deliberately data-only: it never invokes Python, Torch, or
//! a network service. It writes packed little-endian f32 prediction maps so a
//! frozen oracle can compare the exact native result offline.

use makepad_diffusion::backend::gpu_perf_stats;
use makepad_diffusion::da3::{preprocess_rgb8, Da3MetricLarge, Da3Precision};
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn read_png_rgb(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let file = fs::File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(BufReader::new(file), options);
    decoder
        .decode_headers()
        .map_err(|err| format!("{} png header: {err:?}", path.display()))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| format!("{} png has no info", path.display()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| format!("{} png has no colorspace", path.display()))?;
    let pixels = decoder
        .decode_raw()
        .map_err(|err| format!("{} png decode: {err:?}", path.display()))?;
    let components = colorspace.num_components();
    if components < 3 {
        return Err(format!(
            "{} png has {components} components; RGB/RGBA required",
            path.display()
        ));
    }
    let pixel_count = info.width as usize * info.height as usize;
    let mut rgb = vec![0u8; pixel_count * 3];
    for (source, target) in pixels
        .chunks_exact(components)
        .zip(rgb.chunks_exact_mut(3))
    {
        target.copy_from_slice(&source[..3]);
    }
    Ok((rgb, info.width as usize, info.height as usize))
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn stats(name: &str, values: &[f32]) {
    let mut finite = 0usize;
    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    let mut sum = 0.0f64;
    for &value in values {
        if value.is_finite() {
            finite += 1;
            minimum = minimum.min(value);
            maximum = maximum.max(value);
            sum += f64::from(value);
        }
    }
    println!(
        "STATS {name} n={} finite={} min={minimum:.9} max={maximum:.9} mean={:.9}",
        values.len(),
        finite,
        sum / finite.max(1) as f64
    );
}

fn run(
    model_path: &Path,
    input_path: &Path,
    output_dir: &Path,
    warm_runs: usize,
    precision: Da3Precision,
) -> Result<(), String> {
    fs::create_dir_all(output_dir)
        .map_err(|err| format!("create {}: {err}", output_dir.display()))?;

    let decode_start = Instant::now();
    let (rgb, original_width, original_height) = read_png_rgb(input_path)?;
    let preprocessed =
        preprocess_rgb8(&rgb, original_width, original_height).map_err(|err| err.to_string())?;
    println!(
        "INPUT original={}x{} processed={}x{} decode_preprocess_ms={:.3}",
        original_width,
        original_height,
        preprocessed.width,
        preprocessed.height,
        decode_start.elapsed().as_secs_f64() * 1000.0
    );

    let vram = gpu_perf_stats(false);
    let baseline_free = vram.mem_free_bytes;
    println!(
        "VRAM_BASELINE free_mb={} total_mb={}",
        baseline_free / (1 << 20),
        vram.mem_total_bytes / (1 << 20)
    );
    let load_start = Instant::now();
    let model = Da3MetricLarge::load_with_precision(model_path, precision)
        .map_err(|err| err.to_string())?;
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
    let execution = model.execution_info();
    println!("MODE {precision:?}");
    println!(
        "EXECUTION backend={} weights={} activations={} accumulation={}",
        execution.compute_backend,
        execution.weight_precision,
        execution.activation_precision,
        execution.accumulation_precision
    );
    println!("LOAD_MS {load_ms:.3}");

    // First run is tapped: it dumps every oracle-boundary activation and is
    // excluded from warm timing (tap downloads serialize the GPU, mirroring
    // the oracle's slower hooked forward).
    let tap_start = Instant::now();
    let mut tap_names = Vec::new();
    {
        let mut sink = |name: &'static str, values: Vec<f32>| {
            let path = output_dir.join(format!("tap_{name}_f32.bin"));
            if let Err(err) = fs::write(&path, f32_bytes(&values)) {
                eprintln!("TAP-WRITE-FAILED {name}: {err}");
            } else {
                tap_names.push((name, values.len()));
            }
        };
        model
            .forward_normalized_tapped(
                &preprocessed.pixels,
                preprocessed.width,
                preprocessed.height,
                None,
                Some(&mut sink),
            )
            .map_err(|err| err.to_string())?;
    }
    println!("TAPPED_SECONDS {:.9}", tap_start.elapsed().as_secs_f64());
    for (name, len) in &tap_names {
        println!("TAP {name} values={len}");
    }

    let run_count = warm_runs.max(1);
    let mut durations = Vec::with_capacity(run_count);
    let mut final_prediction = None;
    for index in 0..run_count {
        let start = Instant::now();
        let prediction = model
            .forward_normalized(
                &preprocessed.pixels,
                preprocessed.width,
                preprocessed.height,
                None,
            )
            .map_err(|err| err.to_string())?;
        let seconds = start.elapsed().as_secs_f64();
        println!("RUN index={index} seconds={seconds:.9}");
        durations.push(seconds);
        final_prediction = Some(prediction);
    }
    let prediction = final_prediction.expect("at least one native DA3 run");
    let vram = gpu_perf_stats(false);
    println!(
        "VRAM_AFTER free_mb={} used_delta_mb={}",
        vram.mem_free_bytes / (1 << 20),
        baseline_free.saturating_sub(vram.mem_free_bytes) / (1 << 20)
    );
    let mut measured = if durations.len() > 1 {
        durations[1..].to_vec()
    } else {
        durations.clone()
    };
    measured.sort_by(f64::total_cmp);
    let median = measured[measured.len() / 2];
    println!("WARM_MEDIAN_SECONDS {median:.9}");
    let p95_index = ((measured.len() - 1) as f64 * 0.95).ceil() as usize;
    println!("WARM_P95_SECONDS {:.9}", measured[p95_index]);

    // Cancellation and recovery: a progress hook abort must fail the request
    // cleanly, and the very next request must produce byte-identical output.
    let cancelled = model.forward_normalized(
        &preprocessed.pixels,
        preprocessed.width,
        preprocessed.height,
        Some(&mut |label: &str, _fraction: f64| {
            if label.contains("block 13/") {
                Err(makepad_diffusion::DiffusionError::workflow(
                    "da3-validate cancellation test",
                ))
            } else {
                Ok(())
            }
        }),
    );
    match cancelled {
        Err(err) if err.to_string().contains("cancellation test") => {
            println!("CANCEL_ABORT=ok");
        }
        Err(err) => return Err(format!("cancel test failed with wrong error: {err}")),
        Ok(_) => return Err("cancel test did not abort".to_string()),
    }
    let recovered = model
        .forward_normalized(
            &preprocessed.pixels,
            preprocessed.width,
            preprocessed.height,
            None,
        )
        .map_err(|err| format!("post-cancel recovery run failed: {err}"))?;
    if recovered.depth == prediction.depth && recovered.sky == prediction.sky {
        println!("CANCEL_RECOVERY=ok byte_identical=true");
    } else {
        return Err("post-cancel output differs from pre-cancel output".to_string());
    }
    stats("depth", &prediction.depth);
    stats("sky", &prediction.sky);

    fs::write(output_dir.join("depth_native_f32.bin"), f32_bytes(&prediction.depth))
        .map_err(|err| format!("write native depth: {err}"))?;
    fs::write(output_dir.join("sky_native_f32.bin"), f32_bytes(&prediction.sky))
        .map_err(|err| format!("write native sky: {err}"))?;
    fs::write(
        output_dir.join("input_normalized_native_f32.bin"),
        f32_bytes(&preprocessed.pixels),
    )
    .map_err(|err| format!("write native normalized input: {err}"))?;
    fs::write(
        output_dir.join("native_shape.txt"),
        format!("{} {}\n", prediction.width, prediction.height),
    )
    .map_err(|err| format!("write native shape: {err}"))?;
    fs::write(
        output_dir.join("native_execution.txt"),
        format!(
            "backend={}\nweights={}\nactivations={}\naccumulation={}\nload_ms={load_ms:.3}\nwarm_median_seconds={median:.9}\n",
            execution.compute_backend,
            execution.weight_precision,
            execution.activation_precision,
            execution.accumulation_precision,
        ),
    )
    .map_err(|err| format!("write native execution: {err}"))?;
    println!("OUTPUT {}", output_dir.display());
    Ok(())
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(model_path) = args.next() else {
        eprintln!("usage: da3-validate <model.safetensors> <input.png> <output-dir> [warm-runs]");
        std::process::exit(2);
    };
    let Some(input_path) = args.next() else {
        eprintln!("usage: da3-validate <model.safetensors> <input.png> <output-dir> [warm-runs]");
        std::process::exit(2);
    };
    let Some(output_dir) = args.next() else {
        eprintln!("usage: da3-validate <model.safetensors> <input.png> <output-dir> [warm-runs]");
        std::process::exit(2);
    };
    let warm_runs = args
        .next()
        .and_then(|value| value.to_string_lossy().parse::<usize>().ok())
        .unwrap_or(6);
    let precision = match args.next().map(|value| value.to_string_lossy().to_string()) {
        None => Da3Precision::FullBf16,
        Some(mode) if mode == "bf16" => Da3Precision::FullBf16,
        Some(mode) if mode == "f32" => Da3Precision::StrictF32,
        Some(other) => {
            eprintln!("unknown mode {other:?}: expected bf16 or f32");
            std::process::exit(2);
        }
    };
    if args.next().is_some() {
        eprintln!(
            "usage: da3-validate <model.safetensors> <input.png> <output-dir> [warm-runs] [bf16|f32]"
        );
        std::process::exit(2);
    }
    if let Err(err) = run(
        &PathBuf::from(model_path),
        &PathBuf::from(input_path),
        &PathBuf::from(output_dir),
        warm_runs,
        precision,
    ) {
        eprintln!("DA3-VALIDATE-FAILED: {err}");
        std::process::exit(1);
    }
    println!("DA3-VALIDATE-DONE");
}
