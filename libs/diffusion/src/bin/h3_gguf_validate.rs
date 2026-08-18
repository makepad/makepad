//! Header/inventory validator for native MiniMax-H3 quantized components.
//!
//! This deliberately goes through the same strict GGUF/NVFP4 mapping and
//! validation paths used by generation. It does not need CUDA and is
//! therefore suitable as a download/install admission check on a node.

use makepad_diffusion::h3::H3ShardedWeights;
use makepad_diffusion::h3_quant::{H3GgufWeights, H3Nvfp4Weights, H3QuantComponent};
use std::path::PathBuf;

enum QuantWeights {
    Gguf(H3GgufWeights),
    Nvfp4(H3Nvfp4Weights),
    Bf16(H3ShardedWeights),
}

impl QuantWeights {
    fn tensor_names(&self) -> Vec<String> {
        match self {
            Self::Gguf(weights) => weights.tensor_names(),
            Self::Nvfp4(weights) => weights.tensor_names(),
            Self::Bf16(weights) => weights.tensor_names(),
        }
    }

    fn linear(&self, name: &str) -> Option<&makepad_diffusion::h3_quant::H3QuantLinear> {
        match self {
            Self::Gguf(weights) => weights.linear(name),
            Self::Nvfp4(weights) => weights.linear(name),
            Self::Bf16(_) => None,
        }
    }

    fn linear_payload(&self, name: &str) -> makepad_diffusion::Result<Vec<u8>> {
        match self {
            Self::Gguf(weights) => weights.linear_payload(name),
            Self::Nvfp4(weights) => weights.linear_payload(name),
            Self::Bf16(_) => Err(makepad_diffusion::DiffusionError::model(
                "bf16 comparison source has no quantized linear payload",
            )),
        }
    }

    fn tensor_row_f32(&self, name: &str, row: u64) -> makepad_diffusion::Result<Vec<f32>> {
        match self {
            Self::Gguf(weights) => weights.tensor_row_f32(name, row),
            Self::Nvfp4(weights) => weights.tensor_row_f32(name, row),
            Self::Bf16(weights) => weights.tensor_row_f32(name, row),
        }
    }

    fn total_disk_bytes(&self) -> u64 {
        match self {
            Self::Gguf(weights) => weights.total_disk_bytes(),
            Self::Nvfp4(weights) => weights.total_disk_bytes(),
            Self::Bf16(weights) => weights.total_disk_bytes(),
        }
    }

    fn has_adaln_curve(&self) -> bool {
        match self {
            Self::Gguf(weights) => weights.adaln_curve().is_some(),
            Self::Nvfp4(weights) => weights.adaln_curve().is_some(),
            Self::Bf16(_) => false,
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: h3-gguf-validate [--nvfp4] (--dit|--te) <model> [--linear <canonical-name>] [--row <canonical-name> <index>] [--compare-row <other.gguf|other.safetensors> <canonical-name> <index>] [--compare-bf16-row <bf16.safetensors> <canonical-name> <index>]"
    );
    std::process::exit(2);
}

fn load_weights(
    path: &PathBuf,
    component: H3QuantComponent,
    nvfp4: bool,
) -> makepad_diffusion::Result<QuantWeights> {
    if nvfp4 {
        H3Nvfp4Weights::load(path, component).map(QuantWeights::Nvfp4)
    } else {
        H3GgufWeights::load(path, component).map(QuantWeights::Gguf)
    }
}

fn print_row_comparison(
    a_weights: &QuantWeights,
    a_path: &PathBuf,
    b_weights: &QuantWeights,
    b_path: &PathBuf,
    name: &str,
    index: u64,
) {
    let a = a_weights.tensor_row_f32(name, index).unwrap_or_else(|error| {
        eprintln!("cannot read {} row {index}: {error}", a_path.display());
        std::process::exit(1);
    });
    let b = b_weights.tensor_row_f32(name, index).unwrap_or_else(|error| {
        eprintln!("cannot read {} row {index}: {error}", b_path.display());
        std::process::exit(1);
    });
    if a.len() != b.len() || a.is_empty() {
        eprintln!(
            "comparison row length mismatch: {} has {}, {} has {}",
            a_path.display(),
            a.len(),
            b_path.display(),
            b.len()
        );
        std::process::exit(1);
    }
    let (dot, aa, bb, squared_error, max_abs_error) = a.iter().zip(&b).fold(
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f32),
        |(dot, aa, bb, squared_error, max_abs_error), (&a, &b)| {
            let error = a - b;
            (
                dot + a as f64 * b as f64,
                aa + (a as f64).powi(2),
                bb + (b as f64).powi(2),
                squared_error + (error as f64).powi(2),
                max_abs_error.max(error.abs()),
            )
        },
    );
    let cosine = if aa > 0.0 && bb > 0.0 {
        dot / (aa.sqrt() * bb.sqrt())
    } else {
        f64::NAN
    };
    let rmse = (squared_error / a.len() as f64).sqrt();
    let least_squares_scale = if bb > 0.0 { dot / bb } else { f64::NAN };
    println!(
        "COMPARE_ROW name={name} index={index} values={} cosine={cosine:.9} rmse={rmse:.9} max_abs_error={max_abs_error:.9} a_over_b_scale={least_squares_scale:.9}",
        a.len()
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().unwrap_or_else(|| usage());
    let nvfp4 = first == "--nvfp4";
    let component_arg = if nvfp4 {
        args.next().unwrap_or_else(|| usage())
    } else {
        first
    };
    let component = match component_arg.as_str() {
        "--dit" => H3QuantComponent::Dit,
        "--te" => H3QuantComponent::TextEncoder,
        _ => usage(),
    };
    let path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
    let command = args.next();

    let weights = load_weights(&path, component, nvfp4).unwrap_or_else(|error| {
        eprintln!("INVALID {}: {error}", path.display());
        std::process::exit(1);
    });
    let names = weights.tensor_names();
    let linear_count = names
        .iter()
        .filter(|name| weights.linear(name).is_some())
        .count();
    println!(
        "VALID component={component:?} path={} canonical_tensors={} linears={} payload_bytes={} adaln_curve={}",
        path.display(),
        names.len(),
        linear_count,
        weights.total_disk_bytes(),
        weights.has_adaln_curve()
    );

    match command.as_deref() {
        None => {}
        Some("--linear") => {
            let name = args.next().unwrap_or_else(|| usage());
            if args.next().is_some() {
                usage();
            }
            let linear = weights.linear(&name).unwrap_or_else(|| {
                eprintln!("not a canonical linear: {name}");
                std::process::exit(1);
            });
            let payload = weights.linear_payload(&name).unwrap_or_else(|error| {
                eprintln!("cannot read {name}: {error}");
                std::process::exit(1);
            });
            println!(
                "LINEAR name={name} n={} k={} ggml_type={} bytes={}",
                linear.n,
                linear.k,
                linear.ggml_type,
                payload.len()
            );
        }
        Some("--row") => {
            let name = args.next().unwrap_or_else(|| usage());
            let index: u64 = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .unwrap_or_else(|_| usage());
            if args.next().is_some() {
                usage();
            }
            let row = weights.tensor_row_f32(&name, index).unwrap_or_else(|error| {
                eprintln!("cannot read {name} row {index}: {error}");
                std::process::exit(1);
            });
            let (min, max, sum) = row.iter().copied().fold(
                (f32::INFINITY, f32::NEG_INFINITY, 0.0f64),
                |(min, max, sum), value| {
                    (min.min(value), max.max(value), sum + value as f64)
                },
            );
            println!(
                "ROW name={name} index={index} values={} min={min:.7} max={max:.7} mean={:.7}",
                row.len(),
                sum / row.len().max(1) as f64
            );
        }
        Some("--compare-row") => {
            let other_path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let name = args.next().unwrap_or_else(|| usage());
            let index: u64 = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .unwrap_or_else(|_| usage());
            if args.next().is_some() {
                usage();
            }
            let other_nvfp4 = match other_path.extension().and_then(|ext| ext.to_str()) {
                Some("safetensors") => true,
                Some("gguf") => false,
                _ => {
                    eprintln!(
                        "comparison model must end in .gguf or .safetensors: {}",
                        other_path.display()
                    );
                    std::process::exit(2);
                }
            };
            let other = load_weights(&other_path, component, other_nvfp4).unwrap_or_else(|error| {
                eprintln!("INVALID comparison {}: {error}", other_path.display());
                std::process::exit(1);
            });
            print_row_comparison(&weights, &path, &other, &other_path, &name, index);
        }
        Some("--compare-bf16-row") => {
            let other_path = args.next().map(PathBuf::from).unwrap_or_else(|| usage());
            let name = args.next().unwrap_or_else(|| usage());
            let index: u64 = args
                .next()
                .unwrap_or_else(|| usage())
                .parse()
                .unwrap_or_else(|_| usage());
            if args.next().is_some() {
                usage();
            }
            let other = H3ShardedWeights::load(&other_path)
                .map(QuantWeights::Bf16)
                .unwrap_or_else(|error| {
                    eprintln!("INVALID comparison {}: {error}", other_path.display());
                    std::process::exit(1);
                });
            print_row_comparison(&weights, &path, &other, &other_path, &name, index);
        }
        _ => usage(),
    }
}
