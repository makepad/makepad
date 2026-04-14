use crate::error::{LlamaError, Result};
use crate::gguf::{GgufArray, GgufFile, GgufValue};

pub(super) fn required_u32(gguf: &GgufFile, key: &str) -> Result<u32> {
    let value = gguf.require_value(key)?;
    value_to_u32(value).ok_or_else(|| {
        LlamaError::format(format!(
            "gguf key '{}' has type {}, expected integral scalar",
            key,
            value.value_type().name()
        ))
    })
}

pub(super) fn required_f32(gguf: &GgufFile, key: &str) -> Result<f32> {
    let value = gguf.require_value(key)?;
    value.as_f32().ok_or_else(|| {
        LlamaError::format(format!(
            "gguf key '{}' has type {}, expected f32",
            key,
            value.value_type().name()
        ))
    })
}

pub(super) fn optional_f32(gguf: &GgufFile, key: &str) -> Option<f32> {
    gguf.get_value(key).and_then(GgufValue::as_f32)
}

pub(super) fn required_u32_array(gguf: &GgufFile, key: &str) -> Result<Vec<u32>> {
    let value = gguf.require_value(key)?;
    match value {
        GgufValue::Array(values) => array_to_u32_vec(values).ok_or_else(|| {
            LlamaError::format(format!(
                "gguf key '{}' has type {}, expected integral array",
                key,
                value.value_type().name()
            ))
        }),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected u32 array",
            key,
            other.value_type().name()
        ))),
    }
}

pub(super) fn required_u32_or_repeat_array(
    gguf: &GgufFile,
    key: &str,
    repeat_len: usize,
) -> Result<Vec<u32>> {
    let value = gguf.require_value(key)?;
    if let Some(scalar) = value_to_u32(value) {
        return Ok(vec![scalar; repeat_len]);
    }
    match value {
        GgufValue::Array(values) => {
            let out = array_to_u32_vec(values).ok_or_else(|| {
                LlamaError::format(format!(
                    "gguf key '{}' has type {}, expected integral scalar or array",
                    key,
                    value.value_type().name()
                ))
            })?;
            if out.len() != repeat_len {
                return Err(LlamaError::format(format!(
                    "gguf key '{}' length mismatch: got {}, expected {}",
                    key,
                    out.len(),
                    repeat_len
                )));
            }
            Ok(out)
        }
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected scalar or u32 array",
            key,
            other.value_type().name()
        ))),
    }
}

pub(super) fn required_u32_or_first_array(gguf: &GgufFile, key: &str) -> Result<u32> {
    let value = gguf.require_value(key)?;
    if let Some(scalar) = value_to_u32(value) {
        return Ok(scalar);
    }
    match value {
        GgufValue::Array(values) => array_to_u32_vec(values)
            .and_then(|values| values.into_iter().next())
            .ok_or_else(|| {
                LlamaError::format(format!(
                    "gguf key '{}' has type {}, expected integral scalar or non-empty array",
                    key,
                    value.value_type().name()
                ))
            }),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected scalar or u32 array",
            key,
            other.value_type().name()
        ))),
    }
}

pub(super) fn required_utf8_string(gguf: &GgufFile, key: &str) -> Result<String> {
    match gguf.require_value(key)? {
        GgufValue::String(value) => value.try_utf8().map(|s| s.to_owned()),
        other => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string",
            key,
            other.value_type().name()
        ))),
    }
}

pub(super) fn optional_utf8_string(gguf: &GgufFile, key: &str) -> Result<Option<String>> {
    match gguf.get_value(key) {
        None => Ok(None),
        Some(GgufValue::String(value)) => value.try_utf8().map(|s| Some(s.to_owned())),
        Some(other) => Err(LlamaError::format(format!(
            "gguf key '{}' has type {}, expected string",
            key,
            other.value_type().name()
        ))),
    }
}

pub(super) fn optional_u32(gguf: &GgufFile, key: &str) -> Option<u32> {
    gguf.get_value(key).and_then(value_to_u32)
}

fn value_to_u32(value: &GgufValue) -> Option<u32> {
    match value {
        GgufValue::Uint32(v) => Some(*v),
        GgufValue::Uint64(v) => u32::try_from(*v).ok(),
        GgufValue::Int32(v) => u32::try_from(*v).ok(),
        GgufValue::Int64(v) => u32::try_from(*v).ok(),
        _ => None,
    }
}

fn array_to_u32_vec(value: &GgufArray) -> Option<Vec<u32>> {
    match value {
        GgufArray::Uint8(values) => Some(values.iter().copied().map(u32::from).collect()),
        GgufArray::Int8(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Uint16(values) => Some(values.iter().copied().map(u32::from).collect()),
        GgufArray::Int16(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Uint32(values) => Some(values.clone()),
        GgufArray::Int32(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Uint64(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Int64(values) => values
            .iter()
            .copied()
            .map(u32::try_from)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok(),
        GgufArray::Bool(values) => Some(values.iter().copied().map(u32::from).collect()),
        _ => None,
    }
}
