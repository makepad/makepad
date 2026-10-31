use super::*;
use crate::formats::gguf_write::value_bytes;
use crate::quant::{dequantize_q4_k, dequantize_q5_k, dequantize_q6_k, dequantize_q8_0};
use std::io::Write;

#[derive(Default, Clone, Copy)]
struct ErrorSums {
    ours: f64,
    reference: f64,
    energy: f64,
    count: u64,
    reference_count: u64,
}

impl ErrorSums {
    fn add(&mut self, x: Self) {
        self.ours += x.ours;
        self.reference += x.reference;
        self.energy += x.energy;
        self.count += x.count;
        self.reference_count += x.reference_count;
    }
    fn relative(error: f64, energy: f64) -> f64 {
        if energy == 0.0 {
            if error == 0.0 {
                0.0
            } else {
                f64::INFINITY
            }
        } else {
            (error / energy).sqrt()
        }
    }
}

fn read_rows(
    file: &mut File,
    gguf: &GgufFile,
    name: &str,
    row: usize,
    rows: usize,
) -> Result<Vec<f32>> {
    let tensor = gguf.require_tensor(name)?;
    let width = tensor.dimensions[0] as usize;
    let elements = block_elements(tensor.tensor_type.ggml_type());
    let bytes = block_size(tensor.tensor_type.ggml_type());
    let row_bytes = width / elements * bytes;
    file.seek(SeekFrom::Start(
        gguf.data_offset + tensor.offset + (row * row_bytes) as u64,
    ))?;
    let mut raw = vec![0u8; rows * row_bytes];
    file.read_exact(&mut raw)?;
    let mut result = vec![0.0; rows * width];
    for (b, out) in raw
        .chunks_exact(bytes)
        .zip(result.chunks_exact_mut(elements))
    {
        match tensor.tensor_type {
            TensorType::F32 => out[0] = f32::from_le_bytes(b.try_into()?),
            TensorType::F16 => out[0] = f16_to_f32(u16::from_le_bytes(b.try_into()?)),
            TensorType::Q8_0 => dequantize_q8_0(b, out),
            TensorType::Q4K => dequantize_q4_k(b, out),
            TensorType::Q5K => dequantize_q5_k(b, out),
            TensorType::Q6K => dequantize_q6_k(b, out),
            ty => return Err(format!("unsupported validation type {ty:?}").into()),
        }
    }
    Ok(result)
}

/// Checks the whole output inventory and every decoded value, with per-tensor
/// errors against the original weights in their runtime layout. Reduction
/// follows file/job order so worker scheduling does not change the report.
pub fn validate(options: &Options, report: &mut dyn Write) -> Result<()> {
    if options.threads == 0 {
        return Err("threads must be positive".into());
    }
    let reference = reference(options)?;
    let plan = make_plan(options, reference.as_ref())?;
    let output = GgufFile::open(&options.output)?;
    if output.tensors.len() != plan.tensors.len() || output.kv.len() != plan.kv.len() {
        return Err("output inventory size differs from conversion plan".into());
    }
    for entry in &plan.kv {
        if output.get_value(&entry.key).map(value_bytes) != Some(value_bytes(&entry.value)) {
            return Err(format!("output metadata mismatch: {}", entry.key).into());
        }
    }
    for tensor in &plan.tensors {
        let actual = output.require_tensor(&tensor.spec.name)?;
        if actual.dimensions != tensor.spec.dimensions
            || actual.tensor_type != tensor.spec.tensor_type
        {
            return Err(format!("output descriptor mismatch: {}", tensor.spec.name).into());
        }
    }
    writeln!(
        report,
        "structure\tPASS\ttensors={}\tmetadata={}",
        output.tensors.len(),
        output.kv.len()
    )?;
    if let Some(reference) = &reference {
        for entry in &reference.kv {
            if output.get_value(&entry.key).map(value_bytes) != Some(value_bytes(&entry.value)) {
                writeln!(report, "metadata_difference\t{}", entry.key)?;
            }
        }
        for entry in &output.kv {
            if reference.get_value(&entry.key).is_none() {
                writeln!(report, "metadata_added\t{}", entry.key)?;
            }
        }
    }
    let jobs = jobs(&plan);
    let next = AtomicUsize::new(0);
    let mut results = std::thread::scope(|scope| -> Result<Vec<(usize, ErrorSums)>> {
        let mut workers = Vec::new();
        for _ in 0..options.threads {
            workers.push(scope.spawn(|| -> Result<Vec<(usize, ErrorSums)>> {
                let mut sources = open_sources(&plan)?;
                let mut ours = File::open(&options.output)?;
                let mut ref_file = reference
                    .as_ref()
                    .map(|r| File::open(&r.path))
                    .transpose()?;
                let mut result = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&job) = jobs.get(index) else {
                        break;
                    };
                    let tensor = &plan.tensors[job.tensor];
                    let source = read_source(&mut sources, tensor, job)?;
                    let decoded =
                        read_rows(&mut ours, &output, &tensor.spec.name, job.row, job.rows)?;
                    let ref_values = match (&reference, &mut ref_file) {
                        (Some(r), Some(f)) if r.get_tensor(&tensor.spec.name).is_some() => {
                            Some(read_rows(f, r, &tensor.spec.name, job.row, job.rows)?)
                        }
                        _ => None,
                    };
                    let mut sums = ErrorSums::default();
                    for (i, &x) in source.iter().enumerate() {
                        let x = x as f64;
                        let y = decoded[i] as f64;
                        if !y.is_finite() {
                            return Err(format!(
                                "non-finite decoded value in {}",
                                tensor.spec.name
                            )
                            .into());
                        }
                        sums.energy += x * x;
                        sums.ours += (x - y) * (x - y);
                        if let Some(reference) = &ref_values {
                            let y = reference[i] as f64;
                            if !y.is_finite() {
                                return Err("non-finite reference value".into());
                            }
                            sums.reference += (x - y) * (x - y);
                        }
                    }
                    sums.count = source.len() as u64;
                    if ref_values.is_some() {
                        sums.reference_count = sums.count;
                    }
                    result.push((index, sums));
                }
                Ok(result)
            }));
        }
        let mut result = Vec::new();
        let mut failure = None;
        for worker in workers {
            match worker
                .join()
                .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                    "validation worker panicked".into()
                })
                .and_then(|r| r)
            {
                Ok(mut rows) => result.append(&mut rows),
                Err(e) => {
                    failure.get_or_insert(e);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(result)
    })?;
    results.sort_by_key(|(index, _)| *index);
    let mut sums = vec![ErrorSums::default(); plan.tensors.len()];
    for (index, error) in results {
        sums[jobs[index].tensor].add(error);
    }
    writeln!(
        report,
        "tensor\ttype\telements\tours_rel_l2\treference_rel_l2\ttransform\tcheckpoint\trow_slice"
    )?;
    let mut means = BTreeMap::<String, (usize, f64, f64)>::new();
    let (mut comparable_n, mut comparable_ours, mut comparable_reference) = (0, 0.0, 0.0);
    for (tensor, sums) in plan.tensors.iter().zip(sums) {
        let ours = ErrorSums::relative(sums.ours, sums.energy);
        let reference_error = ErrorSums::relative(sums.reference, sums.energy);
        writeln!(
            report,
            "{}\t{}\t{}\t{:.9}\t{}\t{:?}/{:?}\t{}\t{:?}",
            tensor.spec.name,
            tensor.spec.tensor_type.name(),
            sums.count,
            ours,
            if sums.reference_count > 0 {
                format!("{reference_error:.9}")
            } else {
                "-".into()
            },
            tensor.transform,
            tensor.order,
            tensor.source_name,
            tensor.row_slice
        )?;
        if sums.reference_count > 0 {
            let entry = means
                .entry(tensor.spec.tensor_type.name().into())
                .or_default();
            entry.0 += 1;
            entry.1 += ours;
            entry.2 += reference_error;
            if reference
                .as_ref()
                .and_then(|r| r.get_tensor(&tensor.spec.name))
                .is_some_and(|r| r.tensor_type == tensor.spec.tensor_type)
            {
                comparable_n += 1;
                comparable_ours += ours;
                comparable_reference += reference_error;
            }
        }
    }
    let (mut n, mut ours, mut reference) = (0, 0.0, 0.0);
    for (ty, (count, a, b)) in means {
        writeln!(
            report,
            "type_mean\t{ty}\t{count}\t{:.9}\t{:.9}",
            a / count as f64,
            b / count as f64
        )?;
        n += count;
        ours += a;
        reference += b;
    }
    if n > 0 {
        writeln!(
            report,
            "overall_mean\t{n}\t{:.9}\t{:.9}",
            ours / n as f64,
            reference / n as f64
        )?;
    }
    if comparable_n > 0 {
        // A precision reduction is intentional. Compare encoder quality only
        // where both files store the same type; report all other errors above.
        writeln!(
            report,
            "same_type_mean\t{comparable_n}\t{:.9}\t{:.9}",
            comparable_ours / comparable_n as f64,
            comparable_reference / comparable_n as f64
        )?;
        if comparable_ours > comparable_reference {
            return Err(
                "same-type mean reconstruction error exceeds reference; see per-tensor report"
                    .into(),
            );
        }
    }
    Ok(())
}
