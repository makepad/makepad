//! CUDA-only frozen-tap canary. Unsupported hosts compile a fail-closed stub
//! so workspace `--all-features` validation remains meaningful; the stub
//! exits non-zero and never substitutes CPU or Metal execution.

#[cfg(any(target_os = "linux", target_os = "windows"))]
use makepad_ggml::backend::cuda::{
    gpu_attention_packed_cross, gpu_concat_rows, gpu_device_available, gpu_download, gpu_gated_residual,
    gpu_gelu_erf, gpu_perf_stats, gpu_pool_clear, gpu_rope_interleaved, gpu_slice_cols,
    gpu_slice_rows, gpu_upload, GpuTensor,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use makepad_pbr_paint::cuda_unet::{
    add_rows_broadcast_compat, f16_bytes, resnet_block, Planar,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use makepad_pbr_paint::numerical_fixtures::{
    compare, digest_f32, resnet_section_inputs, resnet_section_reference, AttentionTap, BinaryTap,
    FrozenTensor, RopeTap, TapMismatch, UnaryTap, ADD_ROWS_BROADCAST, CROSS_ATTENTION, GEGLU_ERF,
    MUL, RESNET_SECTION_DIGEST, ROPE_INTERLEAVED,
};

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn upload(tensor: FrozenTensor) -> Result<GpuTensor, String> {
    tensor.validate()?;
    gpu_upload(tensor.values, tensor.rows, tensor.cols)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn mismatch(name: &str, mismatch: TapMismatch) -> String {
    format!(
        "tap {name} mismatch at {}: actual={} expected={} allowed={}",
        mismatch.index, mismatch.actual, mismatch.expected, mismatch.allowed
    )
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
struct TapStats {
    name: String,
    max_abs: f32,
    max_rel: f32,
    values: Vec<f32>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn stats_vs(actual: &[f32], expected: &[f32]) -> (f32, f32) {
    let mut max_abs = 0.0f32;
    let mut max_rel = 0.0f32;
    for (a, e) in actual.iter().zip(expected) {
        let diff = (a - e).abs();
        max_abs = max_abs.max(diff);
        max_rel = max_rel.max(diff / e.abs().max(1e-12));
    }
    (max_abs, max_rel)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn check(
    name: &str,
    tensor: &GpuTensor,
    expected: FrozenTensor,
    atol: f32,
    rtol: f32,
) -> Result<TapStats, String> {
    if tensor.rows() != expected.rows || tensor.cols() != expected.cols {
        return Err(format!(
            "tap {name} shape {}x{}, expected {}x{}",
            tensor.rows(),
            tensor.cols(),
            expected.rows,
            expected.cols
        ));
    }
    let actual = gpu_download(tensor)?;
    compare(expected, &actual, atol, rtol).map_err(|error| mismatch(name, error))?;
    let (max_abs, max_rel) = stats_vs(&actual, expected.values);
    Ok(TapStats {
        name: name.to_string(),
        max_abs,
        max_rel,
        values: actual,
    })
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn write_dump(path: &str, taps: &[TapStats]) -> Result<(), String> {
    let mut json = String::from("{\n  \"source\": \"pbr-cuda-taps\",\n  \"taps\": {\n");
    for (i, tap) in taps.iter().enumerate() {
        json.push_str(&format!(
            "    \"{}\": {{\n      \"max_abs\": {:.9e},\n      \"max_rel\": {:.9e},\n      \"values\": [",
            json_escape(&tap.name),
            tap.max_abs,
            tap.max_rel
        ));
        for (j, v) in tap.values.iter().enumerate() {
            if j > 0 {
                json.push_str(", ");
            }
            json.push_str(&format!("{v:.9e}"));
        }
        json.push_str("]\n    }");
        if i + 1 != taps.len() {
            json.push(',');
        }
        json.push('\n');
    }
    json.push_str("  }\n}\n");
    std::fs::write(path, json).map_err(|e| format!("write dump {path}: {e}"))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn binary(
    tap: BinaryTap,
    op: impl FnOnce(&GpuTensor, &GpuTensor) -> Result<GpuTensor, String>,
) -> Result<TapStats, String> {
    let left = upload(tap.left)?;
    let right = upload(tap.right)?;
    let output = op(&left, &right)?;
    check(tap.name, &output, tap.expected, tap.atol, tap.rtol)
}

/// Correctness-only elementwise multiply assembled from clean-HEAD CUDA APIs.
/// `gpu_gated_residual` multiplies one row by a per-column gate; slicing and
/// concatenating preserves arbitrary 2-D fixture shapes without relying on the
/// uncommitted shared `gpu_mul` helper.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn mul_compat(left: &GpuTensor, right: &GpuTensor) -> Result<GpuTensor, String> {
    if left.rows() != right.rows() || left.cols() != right.cols() {
        return Err("mul compatibility path shape mismatch".to_string());
    }
    let gates = gpu_download(right)?;
    let mut output: Option<GpuTensor> = None;
    for row in 0..left.rows() {
        let update = gpu_slice_rows(left, row, 1)?;
        let zero = gpu_upload(&vec![0.0; left.cols()], 1, left.cols())?;
        let start = row * left.cols();
        let row_output = gpu_gated_residual(
            &zero,
            &update,
            &gates[start..start + left.cols()],
        )?;
        output = Some(match output {
            None => row_output,
            Some(previous) => gpu_concat_rows(&previous, &row_output)?,
        });
    }
    output.ok_or_else(|| "mul compatibility path refuses zero rows".to_string())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn geglu(tap: UnaryTap) -> Result<TapStats, String> {
    let input = upload(tap.input)?;
    let width = input.cols() / 2;
    let value = gpu_slice_cols(&input, 0, width)?;
    let gate = gpu_slice_cols(&input, width, width)?;
    let gate = gpu_gelu_erf(&gate)?;
    let output = mul_compat(&value, &gate)?;
    check(tap.name, &output, tap.expected, tap.atol, tap.rtol)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn rope(tap: RopeTap) -> Result<TapStats, String> {
    let input = upload(tap.input)?;
    let cos = upload(tap.cos)?;
    let sin = upload(tap.sin)?;
    let output = gpu_rope_interleaved(&input, tap.head_count, &cos, &sin)?;
    check(tap.name, &output, tap.expected, tap.atol, tap.rtol)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn attention(tap: AttentionTap) -> Result<TapStats, String> {
    let q = upload(tap.q)?;
    let k = upload(tap.k)?;
    let v = upload(tap.v)?;
    let output = gpu_attention_packed_cross(&q, &k, &v, tap.head_count, tap.scale)?;
    check(tap.name, &output, tap.expected, tap.atol, tap.rtol)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn one_pass(pass: usize, collected: &mut Vec<TapStats>) -> Result<(), String> {
    for result in [
        binary(MUL, mul_compat),
        binary(ADD_ROWS_BROADCAST, add_rows_broadcast_compat),
        geglu(GEGLU_ERF),
        rope(ROPE_INTERLEAVED),
        attention(CROSS_ATTENTION),
    ] {
        let mut stats = result?;
        println!(
            "PBR_CUDA_TAP pass={pass} name={} max_abs={:.9e} max_rel={:.9e}",
            stats.name, stats.max_abs, stats.max_rel
        );
        stats.name = format!("{}_p{pass}", stats.name);
        collected.push(stats);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn resnet_section(pass: usize, collected: &mut Vec<TapStats>) -> Result<(), String> {
    let inputs = resnet_section_inputs();
    let reference = resnet_section_reference();
    if digest_f32(&reference) != RESNET_SECTION_DIGEST {
        return Err("resnet section reference digest drifted on this host".to_string());
    }
    let x = Planar {
        t: gpu_upload(&inputs.x, inputs.cin, inputs.width * inputs.height)?,
        width: inputs.width,
        height: inputs.height,
    };
    let temb = gpu_upload(&inputs.temb, 1, inputs.temb_dim)?;
    let temb_w = f16_bytes(&inputs.temb_w_f16);
    let out = resnet_block("pbr-taps", "resnet0", &x, &temb, &inputs, &temb_w)?;
    if out.t.rows() != inputs.cout || out.t.cols() != inputs.width * inputs.height {
        return Err(format!(
            "resnet section shape {}x{}, expected {}x{}",
            out.t.rows(),
            out.t.cols(),
            inputs.cout,
            inputs.width * inputs.height
        ));
    }
    let actual = gpu_download(&out.t)?;
    let (max_abs, max_rel) = stats_vs(&actual, &reference);
    let mut first_bad = None;
    for (i, (a, e)) in actual.iter().zip(reference.iter()).enumerate() {
        let diff = (a - e).abs();
        if diff > 1e-4 + 1e-3 * e.abs() && first_bad.is_none() {
            first_bad = Some((i, *a, *e, diff));
        }
    }
    println!(
        "PBR_CUDA_SECTION pass={pass} name=resnet_block max_abs={max_abs:.9e} max_rel={max_rel:.9e}"
    );
    if let Some((i, a, e, diff)) = first_bad {
        println!("PBR_CUDA_SECTION_FAIL pass={pass} index={i} actual={a} expected={e} diff={diff}");
    }
    collected.push(TapStats {
        name: format!("resnet_block_p{pass}"),
        max_abs,
        max_rel,
        values: actual,
    });
    if first_bad.is_some() {
        return Err(format!(
            "resnet section exceeded 1e-4+1e-3*|ref| (max_abs={max_abs:.9e})"
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run() -> Result<(), String> {
    std::env::set_var("MAKEPAD_PBR_TAP_PARITY", "1");
    if !gpu_device_available() {
        return Err("CUDA device/runtime unavailable; no fallback is permitted".to_string());
    }
    let dump_path = std::env::var("PBR_CUDA_TAPS_DUMP")
        .unwrap_or_else(|_| "pbr_cuda_taps.json".to_string());
    let mut collected = Vec::new();
    let mut errors = Vec::new();
    let before = gpu_perf_stats(true);
    if let Err(e) = one_pass(1, &mut collected) {
        errors.push(e);
    }
    if let Err(e) = one_pass(2, &mut collected) {
        errors.push(e);
    }
    if let Err(e) = resnet_section(1, &mut collected) {
        errors.push(e);
    }
    if let Err(e) = resnet_section(2, &mut collected) {
        errors.push(e);
    }
    let after = gpu_perf_stats(false);
    if errors.is_empty() {
        println!(
            "PBR_CUDA_TAPS_OK mem_total={} mem_free_before={} mem_free_after={} fresh_allocs={} fresh_bytes={} pool_oom_clears={}",
            after.mem_total_bytes,
            before.mem_free_bytes,
            after.mem_free_bytes,
            after.pool_fresh_alloc_count,
            after.pool_fresh_alloc_bytes,
            after.pool_oom_clears,
        );
    } else {
        println!(
            "PBR_CUDA_TAPS_PARTIAL mem_total={} errors={}",
            after.mem_total_bytes,
            errors.len()
        );
    }
    write_dump(&dump_path, &collected)?;
    println!("PBR_CUDA_TAPS_DUMP {dump_path}");
    gpu_pool_clear();
    if let Some(first) = errors.into_iter().next() {
        return Err(first);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(error) = run() {
        eprintln!("PBR_CUDA_TAPS_FAIL {error}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!(
        "PBR_CUDA_TAPS_FAIL CUDA validation taps are unavailable on this host; no CPU/Metal fallback"
    );
    std::process::exit(1);
}
