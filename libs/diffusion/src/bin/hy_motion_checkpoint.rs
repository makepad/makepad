use std::env;

use makepad_diffusion::hy_motion_weights::{hy_motion_tensor_specs, HyMotionCheckpoint};

fn main() {
    if let Err(error) = run() {
        eprintln!("HY-Motion checkpoint validation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let argument = env::args()
        .nth(1)
        .ok_or("usage: hy-motion-checkpoint <latest.ckpt>")?;
    if argument == "--list" {
        for spec in hy_motion_tensor_specs() {
            let shape = spec
                .shape
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join("x");
            println!("{}\t{shape}", spec.name);
        }
        return Ok(());
    }
    let path = argument;
    let mut checkpoint = HyMotionCheckpoint::open(&path)?;
    let report = checkpoint.validate()?;

    // Exercise lazy storage reads without materializing the 4.17 GB model.
    let mean = checkpoint.f32("mean")?;
    let std = checkpoint.f32("std")?;
    let input_weight = checkpoint.f32("motion_transformer.input_encoder.weight")?;
    println!("checkpoint={path}");
    println!("prefix={}", report.checkpoint_prefix);
    println!("archive_tensors={}", report.archive_tensor_count);
    println!("required_tensors={}", report.required_tensor_count);
    println!("required_parameters={}", report.required_parameter_count);
    println!("required_f32_bytes={}", report.required_parameter_count * 4);
    println!("mean_checksum={:.9}", checksum(&mean));
    println!("std_checksum={:.9}", checksum(&std));
    println!("input_weight_checksum={:.9}", checksum(&input_weight));
    Ok(())
}

fn checksum(values: &[f32]) -> f64 {
    values
        .iter()
        .enumerate()
        .map(|(index, &value)| value as f64 * ((index % 251) + 1) as f64)
        .sum()
}
