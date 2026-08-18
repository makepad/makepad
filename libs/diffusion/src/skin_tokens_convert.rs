//! Deterministic, Torch-free conversion of the official SkinTokens Lightning
//! checkpoint to the flat BF16 safetensors form consumed by native inference.
//!
//! Artifact download/cache ownership stays with the caller (ai-content's
//! declarative lifecycle).  This module only converts two explicit paths and
//! exposes progress/cancellation through [`crate::ProgressHook`].

use crate::skin_tokens::{
    SkinTokensWeights, SKIN_TOKENS_ARTIFACTS, SKIN_TOKENS_CHECKPOINT_PARAMS,
    SKIN_TOKENS_CHECKPOINT_TENSORS, SKIN_TOKENS_SOURCE_PATH,
};
use crate::torch_pth::{PthDType, PthStateDict};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkinTokensConversionReport {
    pub source: PathBuf,
    pub output: PathBuf,
    pub tensors: usize,
    pub parameters: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
struct TensorPlan {
    name: String,
    shape: Vec<usize>,
    dtype: PthDType,
    start: u64,
    end: u64,
}

fn io_error(path: &Path, action: &str, error: impl std::fmt::Display) -> DiffusionError {
    DiffusionError::io(path, format!("SkinTokens {action}: {error}"))
}

fn part_path(output: &Path) -> PathBuf {
    let mut value = output.as_os_str().to_os_string();
    value.push(".part");
    PathBuf::from(value)
}

fn dtype_name(dtype: PthDType) -> &'static str {
    match dtype {
        PthDType::F32 => "F32",
        PthDType::F16 => "F16",
        PthDType::BF16 => "BF16",
    }
}

fn push_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < ' ' => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn safetensors_header(plan: &[TensorPlan]) -> Vec<u8> {
    use std::fmt::Write as _;

    // Match safetensors.torch.save_file's compact deterministic JSON so the
    // native conversion is byte-identical to the independent MLX conversion.
    let mut header = String::from("{\"__metadata__\":{\"format\":\"pt\",\"source\":");
    push_json_string(&mut header, SKIN_TOKENS_SOURCE_PATH);
    header.push('}');
    for tensor in plan {
        header.push(',');
        push_json_string(&mut header, &tensor.name);
        header.push_str(":{\"dtype\":\"");
        header.push_str(dtype_name(tensor.dtype));
        header.push_str("\",\"shape\":[");
        for (index, dimension) in tensor.shape.iter().enumerate() {
            if index != 0 {
                header.push(',');
            }
            let _ = write!(header, "{dimension}");
        }
        let _ = write!(
            header,
            "],\"data_offsets\":[{},{}]}}",
            tensor.start, tensor.end
        );
    }
    header.push('}');
    while header.len() % 8 != 0 {
        header.push(' ');
    }
    header.into_bytes()
}

fn build_plan(state: &PthStateDict) -> Result<(Vec<TensorPlan>, u64)> {
    let mut names = state.names().cloned().collect::<Vec<_>>();
    names.sort();
    if names.len() != SKIN_TOKENS_CHECKPOINT_TENSORS {
        return Err(DiffusionError::model(format!(
            "official SkinTokens checkpoint contains {} tensors, expected {}",
            names.len(),
            SKIN_TOKENS_CHECKPOINT_TENSORS,
        )));
    }
    let mut offset = 0u64;
    let mut parameters = 0u64;
    let mut plan = Vec::with_capacity(names.len());
    for name in names {
        let shape = state.shape(&name)?;
        let dtype = state.dtype(&name)?;
        if dtype != PthDType::BF16 {
            return Err(DiffusionError::model(format!(
                "official SkinTokens tensor '{name}' is {dtype:?}, expected BF16",
            )));
        }
        let elements = shape
            .iter()
            .try_fold(1u64, |count, dimension| {
                count.checked_mul(*dimension as u64)
            })
            .ok_or_else(|| {
                DiffusionError::model(format!("SkinTokens tensor '{name}' shape overflows"))
            })?;
        parameters = parameters.checked_add(elements).ok_or_else(|| {
            DiffusionError::model("SkinTokens checkpoint parameter count overflows")
        })?;
        let bytes = elements
            .checked_mul(dtype.element_size() as u64)
            .ok_or_else(|| {
                DiffusionError::model(format!("SkinTokens tensor '{name}' bytes overflow"))
            })?;
        let end = offset.checked_add(bytes).ok_or_else(|| {
            DiffusionError::model("SkinTokens checkpoint byte offsets overflow")
        })?;
        plan.push(TensorPlan {
            name,
            shape,
            dtype,
            start: offset,
            end,
        });
        offset = end;
    }
    if parameters != SKIN_TOKENS_CHECKPOINT_PARAMS {
        return Err(DiffusionError::model(format!(
            "official SkinTokens checkpoint contains {parameters} parameters, expected {SKIN_TOKENS_CHECKPOINT_PARAMS}",
        )));
    }
    Ok((plan, parameters))
}

/// Convert the official artifact at `source` into `output` atomically.
///
/// The progress hook is invoked before parsing and after every tensor write;
/// returning [`DiffusionError::Cancelled`] (or any error) removes the `.part`
/// file and leaves an existing final artifact untouched.
pub fn convert_skin_tokens_checkpoint(
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mut progress: Option<ProgressHook<'_>>,
) -> Result<SkinTokensConversionReport> {
    let source = source.as_ref();
    let output = output.as_ref();
    if output.is_file() {
        let weights = SkinTokensWeights::load(output)?;
        let bytes = fs::metadata(output)
            .map_err(|error| io_error(output, "read converted metadata", error))?
            .len();
        return Ok(SkinTokensConversionReport {
            source: source.to_path_buf(),
            output: output.to_path_buf(),
            tensors: weights.inventory().all.tensors,
            parameters: weights.inventory().all.parameters,
            bytes,
        });
    }
    let source_size = fs::metadata(source)
        .map_err(|error| io_error(source, "read source metadata", error))?
        .len();
    if source_size != SKIN_TOKENS_ARTIFACTS[0].size {
        return Err(DiffusionError::model(format!(
            "{}: official SkinTokens checkpoint is {source_size} bytes, expected {} (download/cache entry may be incomplete)",
            source.display(),
            SKIN_TOKENS_ARTIFACTS[0].size,
        )));
    }
    emit_progress(&mut progress, "convert SkinTokens: parse checkpoint", 0.0)?;
    let mut state = PthStateDict::load(source)?;
    let (plan, parameters) = build_plan(&state)?;
    let header = safetensors_header(&plan);
    let expected_size = 8u64 + header.len() as u64 + plan.last().map_or(0, |item| item.end);
    if expected_size != SKIN_TOKENS_ARTIFACTS[0].converted_size {
        return Err(DiffusionError::model(format!(
            "SkinTokens deterministic conversion planned {expected_size} bytes, expected {}",
            SKIN_TOKENS_ARTIFACTS[0].converted_size,
        )));
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| io_error(parent, "create converted directory", error))?;
    }
    let part = part_path(output);
    let conversion = (|| -> Result<()> {
        let file = File::create(&part)
            .map_err(|error| io_error(&part, "create partial conversion", error))?;
        let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);
        writer
            .write_all(&(header.len() as u64).to_le_bytes())
            .and_then(|_| writer.write_all(&header))
            .map_err(|error| io_error(&part, "write safetensors header", error))?;
        for (index, tensor) in plan.iter().enumerate() {
            let bytes = state.raw_contiguous(&tensor.name)?;
            let expected = (tensor.end - tensor.start) as usize;
            if bytes.len() != expected {
                return Err(DiffusionError::model(format!(
                    "SkinTokens tensor '{}' yielded {} bytes, expected {expected}",
                    tensor.name,
                    bytes.len(),
                )));
            }
            writer
                .write_all(&bytes)
                .map_err(|error| io_error(&part, "write tensor data", error))?;
            emit_progress(
                &mut progress,
                &format!(
                    "convert SkinTokens tensor {}/{}: {}",
                    index + 1,
                    plan.len(),
                    tensor.name,
                ),
                (index + 1) as f64 / plan.len() as f64,
            )?;
        }
        writer
            .flush()
            .map_err(|error| io_error(&part, "flush conversion", error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| io_error(&part, "sync conversion", error))?;
        Ok(())
    })();
    if let Err(error) = conversion {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    fs::rename(&part, output)
        .map_err(|error| io_error(output, "commit converted artifact", error))?;
    let weights = SkinTokensWeights::load(output)?;
    let bytes = fs::metadata(output)
        .map_err(|error| io_error(output, "read converted metadata", error))?
        .len();
    Ok(SkinTokensConversionReport {
        source: source.to_path_buf(),
        output: output.to_path_buf(),
        tensors: weights.inventory().all.tensors,
        parameters,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converted_manifest_is_internally_consistent() {
        assert_eq!(SKIN_TOKENS_ARTIFACTS.len(), 1);
        assert_eq!(
            SKIN_TOKENS_ARTIFACTS[0].converted_size,
            8 + 81_672 + SKIN_TOKENS_CHECKPOINT_PARAMS * 2,
        );
    }
}
