//! MapAnything parity validator.
//!
//! `mapanything-validate <model.safetensors> <reference-case-dir> <f32|bf16> <inputs-case-dir> [warm-runs] [band-case-dir]`
//!
//! A reference case dir holds `index.json` (per tensor: name, kind `full` or
//! `rows`, shape, offset and count into `tensors.bin`, the sampled row indices
//! of the flattened `[-1, last]` matrix for `rows`, and full-tensor stats) and
//! `tensors.bin` (little-endian f32). The inputs case dir supplies the
//! `forward_input[i].*` tensors. Data only: no Python, no network.
//!
//! Every native tap with a reference tensor of the same name (and occurrence)
//! prints one `TAP` line: the relative L2 error over the compared values, the
//! largest absolute error, and the reference's absolute maximum.
//!
//! The band case is the reference run of the same inputs at the other
//! precision. Each output then prints a `BAND` line: our error and the band
//! reference's error against the reference, on the same values, so the port
//! is judged against the reference's own precision noise. `VALID` lines give
//! our depth error in mm on the nearer half of the pixels a full reference
//! marks unambiguous (the far half holds the background, where depth is
//! barely conditioned).

use makepad_ai_common::json::Json;
use makepad_ai_vision::mapanything::{MapAnything, MapAnythingPrecision, MapAnythingView};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

struct Entry {
    name: String,
    full: bool,
    offset: usize,
    count: usize,
    shape: Vec<usize>,
    rows: Vec<usize>,
}

impl Entry {
    /// The values of a full tensor of this entry's shape at this entry's
    /// sampled rows (all of them for a full entry).
    fn pick(&self, values: &[f32]) -> Vec<f32> {
        if self.full {
            return values.to_vec();
        }
        let last = *self.shape.last().unwrap_or(&1);
        self.rows.iter().flat_map(|&row| values[row * last..(row + 1) * last].iter().copied()).collect()
    }
}

struct Case {
    entries: Vec<Entry>,
    data: Vec<u8>,
}

impl Case {
    fn load(dir: &Path) -> Result<Self, String> {
        let index = fs::read_to_string(dir.join("index.json")).map_err(|err| format!("{}: {err}", dir.display()))?;
        let index = Json::parse(&index)?;
        let data = fs::read(dir.join("tensors.bin")).map_err(|err| format!("{}: {err}", dir.display()))?;
        let numbers = |value: Option<&Json>| -> Vec<usize> {
            value
                .and_then(Json::as_arr)
                .map(|items| items.iter().filter_map(|item| item.as_f64().map(|n| n as usize)).collect())
                .unwrap_or_default()
        };
        let mut entries = Vec::new();
        for item in index.as_arr().ok_or("index.json is not a list")? {
            entries.push(Entry {
                name: item.get("name").and_then(Json::as_str).ok_or("entry without a name")?.to_string(),
                full: item.get("kind").and_then(Json::as_str) == Some("full"),
                offset: item.get("offset").and_then(Json::as_f64).ok_or("entry without an offset")? as usize,
                count: item.get("count").and_then(Json::as_f64).ok_or("entry without a count")? as usize,
                shape: numbers(item.get("shape")),
                rows: numbers(item.get("rows")),
            });
        }
        Ok(Self { entries, data })
    }

    fn values(&self, entry: &Entry) -> Vec<f32> {
        self.data[entry.offset..entry.offset + 4 * entry.count]
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect()
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    fn named(&self, name: &str) -> Option<Vec<f32>> {
        self.entry(name).map(|entry| self.values(entry))
    }

    /// A tensor this case holds in full, as values.
    fn full(&self, name: &str) -> Option<Vec<f32>> {
        self.entry(name).filter(|entry| entry.full).map(|entry| self.values(entry))
    }
}

struct Stats {
    rel: f64,
    max_abs: f64,
    ref_max: f64,
    nonfinite: usize,
}

fn stats(ours: &[f32], reference: &[f32]) -> Stats {
    let (mut diff, mut norm, mut max_abs, mut ref_max) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut nonfinite = 0usize;
    for (&a, &b) in ours.iter().zip(reference) {
        if !a.is_finite() {
            nonfinite += 1;
            continue;
        }
        let d = f64::from(a) - f64::from(b);
        diff += d * d;
        norm += f64::from(b) * f64::from(b);
        max_abs = max_abs.max(d.abs());
        ref_max = ref_max.max(f64::from(b).abs());
    }
    Stats {
        rel: (diff / norm.max(1e-30)).sqrt(),
        max_abs,
        ref_max,
        nonfinite,
    }
}

/// Mean, 99th percentile and largest of absolute errors in mm.
fn millimetres(mut errors: Vec<f64>) -> (f64, f64, f64) {
    if errors.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    errors.sort_by(f64::total_cmp);
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let p99 = errors[((errors.len() - 1) as f64 * 0.99).round() as usize];
    (mean, p99, errors[errors.len() - 1])
}

struct Compare<'a> {
    reference: &'a Case,
    seen: HashMap<String, usize>,
    compared: Vec<usize>,
    worst_output: f64,
    /// Our output taps in full, in tap order, for the band and valid-pixel tables.
    outputs: Vec<(String, Vec<f32>)>,
}

impl Compare<'_> {
    fn tap(&mut self, name: &str, ours: Vec<f32>) {
        let occurrence = {
            let seen = self.seen.entry(name.to_string()).or_insert(0);
            *seen += 1;
            *seen - 1
        };
        let Some((position, entry)) = self
            .reference
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.name == name)
            .nth(occurrence)
        else {
            println!("EXTRA {name}#{occurrence} n={} (no reference tensor)", ours.len());
            return;
        };
        self.compared.push(position);
        let expected_len: usize = entry.shape.iter().product();
        if ours.len() != expected_len {
            println!("SHAPE {name}#{occurrence} ours n={} reference {:?}", ours.len(), entry.shape);
            return;
        }
        let picked = entry.pick(&ours);
        let found = stats(&picked, &self.reference.values(entry));
        if name.starts_with("output[") {
            self.worst_output = self.worst_output.max(found.rel);
            if occurrence == 0 {
                self.outputs.push((name.to_string(), ours));
            }
        }
        println!(
            "TAP {name}#{occurrence} {} n={} rel_l2={:.3e} max_abs={:.3e} ref_absmax={:.4e}{}",
            if entry.full { "full" } else { "rows" },
            picked.len(),
            found.rel,
            found.max_abs,
            found.ref_max,
            if found.nonfinite > 0 { format!(" NONFINITE={}", found.nonfinite) } else { String::new() }
        );
    }
}

/// `BAND` lines: per output, our error and the band reference's error
/// against the reference on the same values (the rows whichever of the two
/// references sampled).
fn band_table(outputs: &[(String, Vec<f32>)], reference: &Case, band: &Case) {
    let mut worst_ratio = 0.0f64;
    for (name, ours) in outputs {
        let (Some(at_reference), Some(at_band)) = (reference.entry(name), band.entry(name)) else {
            continue;
        };
        if !at_reference.full && !at_band.full && at_reference.rows != at_band.rows {
            println!("BAND {name} skipped: the two references sample different rows");
            continue;
        }
        let sampler = if at_reference.full { at_band } else { at_reference };
        let sampled = |entry: &Entry, values: Vec<f32>| if entry.full { sampler.pick(&values) } else { values };
        let reference_values = sampled(at_reference, reference.values(at_reference));
        let band_values = sampled(at_band, band.values(at_band));
        let ours_found = stats(&sampler.pick(ours), &reference_values);
        let band_found = stats(&band_values, &reference_values);
        let ratio = ours_found.rel / band_found.rel.max(1e-30);
        if !name.ends_with(".non_ambiguous_mask") {
            worst_ratio = worst_ratio.max(ratio);
        }
        println!(
            "BAND {name} n={} ours_rel={:.3e} band_rel={:.3e} ratio={ratio:.2} ours_max={:.3e} band_max={:.3e}",
            reference_values.len(),
            ours_found.rel,
            band_found.rel,
            ours_found.max_abs,
            band_found.max_abs
        );
    }
    println!("BAND WORST ratio={worst_ratio:.2} (the thresholded mask left out)");
}

/// `VALID` lines: per view, our depth error in mm on the nearer half of the
/// pixels a full reference (the reference or the band case) marks
/// unambiguous, and on the other case's sampled pixels both errors.
fn valid_table(outputs: &[(String, Vec<f32>)], reference: &Case, band: Option<&Case>) {
    let mut cases = vec![("reference", reference)];
    cases.extend(band.map(|band| ("band", band)));
    for view in 0.. {
        let depth_name = format!("output[{view}].depth_z");
        let mask_name = format!("output[{view}].non_ambiguous_mask");
        let Some((_, ours)) = outputs.iter().find(|(name, _)| *name == depth_name) else {
            break;
        };
        let Some((label, full_case, depth, mask)) = cases.iter().find_map(|&(label, case)| {
            Some((label, case, case.full(&depth_name)?, case.full(&mask_name)?))
        }) else {
            println!("VALID {depth_name} skipped: no case holds its depth and mask in full");
            continue;
        };
        let unambiguous = |pixel: usize| mask[pixel] > 0.5 && depth[pixel] > 0.0;
        let mut depths: Vec<f32> = (0..depth.len()).filter(|&pixel| unambiguous(pixel)).map(|pixel| depth[pixel]).collect();
        depths.sort_by(f32::total_cmp);
        let Some(&median) = depths.get(depths.len() / 2) else {
            println!("VALID {depth_name} skipped: no unambiguous pixel");
            continue;
        };
        let valid = |pixel: usize| unambiguous(pixel) && depth[pixel] <= median;
        let error_mm = |values: &[f32], pixel: usize| 1e3 * (f64::from(values[pixel]) - f64::from(depth[pixel])).abs();
        let pixels: Vec<usize> = (0..depth.len()).filter(|&pixel| valid(pixel)).collect();
        let (mean, p99, max) = millimetres(pixels.iter().map(|&pixel| error_mm(ours, pixel)).collect());
        println!(
            "VALID {depth_name} vs {label} px={} depth<={median:.4} m mean_mm={mean:.4} p99_mm={p99:.4} max_mm={max:.4}",
            pixels.len()
        );
        // The other case's depth sampled per pixel ([.., 1] rows): both errors on its valid samples.
        for &(other_label, other) in &cases {
            if std::ptr::eq(other, full_case) {
                continue;
            }
            let Some(entry) = other.entry(&depth_name).filter(|entry| !entry.full && entry.shape.last() == Some(&1)) else {
                continue;
            };
            let other_values = other.values(entry);
            let samples: Vec<(usize, f32)> =
                entry.rows.iter().copied().zip(other_values).filter(|&(pixel, _)| valid(pixel)).collect();
            let (ours_mean, _, ours_max) = millimetres(samples.iter().map(|&(pixel, _)| error_mm(ours, pixel)).collect());
            let (other_mean, _, other_max) = millimetres(
                samples
                    .iter()
                    .map(|&(pixel, value)| 1e3 * (f64::from(value) - f64::from(depth[pixel])).abs())
                    .collect(),
            );
            println!(
                "VALID {depth_name} sampled px={} ours mean_mm={ours_mean:.4} max_mm={ours_max:.4} | {other_label} mean_mm={other_mean:.4} max_mm={other_max:.4}",
                samples.len()
            );
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        return Err("usage: mapanything-validate <model.safetensors> <reference-case-dir> <f32|bf16> <inputs-case-dir> [warm-runs] [band-case-dir]".into());
    }
    let precision = match args[3].as_str() {
        "f32" => MapAnythingPrecision::StrictF32,
        "bf16" => MapAnythingPrecision::Bf16,
        other => return Err(format!("precision {other:?}: f32 or bf16")),
    };
    let warm_runs: usize = args.get(5).map(|text| text.parse().map_err(|_| "warm-runs is a count")).transpose()?.unwrap_or(0);
    let reference = Case::load(Path::new(&args[2]))?;
    let inputs = Case::load(Path::new(&args[4]))?;
    let band = args.get(6).map(|dir| Case::load(Path::new(dir))).transpose()?;

    let mut images = Vec::new();
    while let Some(image) = inputs.named(&format!("forward_input[{}].img", images.len())) {
        images.push(image);
    }
    if images.is_empty() {
        return Err("the inputs case has no forward_input[0].img".into());
    }
    let input = |view: usize, key: &str| inputs.named(&format!("forward_input[{view}].{key}"));
    let rays: Vec<Option<Vec<f32>>> = (0..images.len()).map(|view| input(view, "ray_directions_cam")).collect();
    let depth: Vec<Option<Vec<f32>>> = (0..images.len()).map(|view| input(view, "depth_along_ray")).collect();
    let mut poses = Vec::new();
    for view in 0..images.len() {
        poses.push(match (input(view, "camera_pose_quats"), input(view, "camera_pose_trans")) {
            (Some(quat), Some(trans)) if quat.len() == 4 && trans.len() == 3 => {
                Some(([quat[0], quat[1], quat[2], quat[3]], [trans[0], trans[1], trans[2]]))
            }
            _ => None,
        });
    }
    let views: Vec<MapAnythingView> = (0..images.len())
        .map(|view| MapAnythingView {
            image: &images[view],
            rays: rays[view].as_deref(),
            depth_along_ray: depth[view].as_deref(),
            pose: poses[view],
            metric: true,
        })
        .collect();
    println!(
        "CASE {} views={} rays={} depth={} poses={} precision={}",
        args[2],
        views.len(),
        rays.iter().filter(|r| r.is_some()).count(),
        depth.iter().filter(|d| d.is_some()).count(),
        poses.iter().filter(|p| p.is_some()).count(),
        args[3]
    );

    let started = Instant::now();
    let model = MapAnything::load(&args[1], "mapanything", precision, None).map_err(|err| err.to_string())?;
    println!("LOAD {:.2} s", started.elapsed().as_secs_f64());

    let mut compare = Compare {
        reference: &reference,
        seen: HashMap::new(),
        compared: Vec::new(),
        worst_output: 0.0,
        outputs: Vec::new(),
    };
    let started = Instant::now();
    let mut sink = |name: &str, values: Vec<f32>| compare.tap(name, values);
    let output = model.forward(&views, None, Some(&mut sink)).map_err(|err| err.to_string())?;
    println!("TAPPED RUN {:.2} s (taps download and compare inline)", started.elapsed().as_secs_f64());
    println!("METRIC SCALE {:.6}", output.metric_scale);
    for (position, entry) in reference.entries.iter().enumerate() {
        if !entry.name.starts_with("forward_input") && !compare.compared.contains(&position) {
            println!("MISSING {} {:?}", entry.name, entry.shape);
        }
    }
    println!("WORST OUTPUT rel_l2={:.3e}", compare.worst_output);
    if let Some(band) = &band {
        band_table(&compare.outputs, &reference, band);
    }
    valid_table(&compare.outputs, &reference, band.as_ref());

    for run in 0..warm_runs {
        let started = Instant::now();
        model.forward(&views, None, None).map_err(|err| err.to_string())?;
        println!("WARM RUN {run} {:.3} s", started.elapsed().as_secs_f64());
    }
    Ok(())
}

fn main() {
    makepad_ai_common::backend::metal::run_model_worker("mapanything_validate", model_main);
}

fn model_main() {
    if let Err(err) = run() {
        eprintln!("mapanything-validate: {err}");
        std::process::exit(1);
    }
}
