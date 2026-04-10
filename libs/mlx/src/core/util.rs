fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn f32_to_bf16_word(value: f32) -> u16 {
    (bf16_round_to_f32(value).to_bits() >> 16) as u16
}

fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF0000;
    f32::from_bits(rounded)
}

pub fn gemma4_qproj_case_input_bf16_words(len: usize) -> Vec<u16> {
    gemma4_qproj_case_input_bf16_words_with_phase(len, 0)
}

pub fn gemma4_qproj_case_input_bf16_words_with_phase(len: usize, phase: usize) -> Vec<u16> {
    (0..len)
        .map(|index| {
            f32_to_bf16_word(
                GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN
                    [(index + phase) % GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN.len()],
            )
        })
        .collect()
}

pub fn gemma4_qproj_case_input_f32_values_with_phase(len: usize, phase: usize) -> Vec<f32> {
    (0..len)
        .map(|index| {
            GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN
                [(index + phase) % GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN.len()]
        })
        .collect()
}

pub fn fnv1a64_u32_words(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for word in words {
        for byte in word.to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn json_object<'a>(
    path: &Path,
    context: &str,
    value: &'a JsonValue,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected object, got {:?}", context, other),
        }),
    }
}

fn json_string(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(other) => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected string, got {:?}", context, other),
        }),
        None => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} missing string field", context),
        }),
    }
}

fn json_u64(path: &Path, context: &str, value: &JsonValue) -> Result<u64> {
    match value {
        JsonValue::U64(number) => Ok(*number),
        JsonValue::U128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} does not fit in u64", context, number),
            })
        }
        JsonValue::I64(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative", context, number),
            })
        }
        JsonValue::I128(number) => {
            u64::try_from(*number).map_err(|_| MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} value {} is negative or too large", context, number),
            })
        }
        other => Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected integer, got {:?}", context, other),
        }),
    }
}

fn json_u64_array(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<Vec<u64>> {
    let array = match value {
        Some(JsonValue::Array(array)) => array,
        Some(other) => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} expected integer array, got {:?}", context, other),
            });
        }
        None => {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.to_path_buf(),
                message: format!("{} missing integer array", context),
            });
        }
    };
    let mut out = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        out.push(json_u64(path, &format!("{}[{}]", context, index), item)?);
    }
    Ok(out)
}

fn json_two_u64s(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<[u64; 2]> {
    let values = json_u64_array(path, context, value)?;
    if values.len() != 2 {
        return Err(MlxRtError::InvalidSafetensors {
            path: path.to_path_buf(),
            message: format!("{} expected two integers, got {}", context, values.len()),
        });
    }
    Ok([values[0], values[1]])
}

fn json_string_map(
    path: &Path,
    context: &str,
    value: &JsonValue,
) -> Result<HashMap<String, String>> {
    let object = json_object(path, context, value)?;
    let mut out = HashMap::with_capacity(object.len());
    for (key, value) in object {
        out.insert(
            key.clone(),
            json_string(path, &format!("{}.{}", context, key), Some(value))?,
        );
    }
    Ok(out)
}

fn json_dtype(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<MlxDType> {
    let dtype_str = json_string(path, &format!("{}.dtype", context), value)?;
    MlxDType::from_safetensors_str(&dtype_str).map_err(|_| MlxRtError::InvalidSafetensors {
        path: path.to_path_buf(),
        message: format!("{} unsupported dtype {}", context, dtype_str),
    })
}

fn tokenizer_object<'a>(
    path: &Path,
    context: &str,
    value: Option<&'a JsonValue>,
) -> Result<&'a HashMap<String, JsonValue>> {
    match value {
        Some(JsonValue::Object(object)) => Ok(object),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected object, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing object", context),
        }),
    }
}

fn tokenizer_array<'a>(
    path: &Path,
    context: &str,
    value: Option<&'a JsonValue>,
) -> Result<&'a Vec<JsonValue>> {
    match value {
        Some(JsonValue::Array(array)) => Ok(array),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected array, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing array", context),
        }),
    }
}

fn tokenizer_string(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(JsonValue::String(text)) => Ok(text.clone()),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected string, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing string", context),
        }),
    }
}

fn tokenizer_bool(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<bool> {
    match value {
        Some(JsonValue::Bool(flag)) => Ok(*flag),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected bool, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing bool", context),
        }),
    }
}

fn tokenizer_u32(path: &Path, context: &str, value: Option<&JsonValue>) -> Result<u32> {
    match value {
        Some(JsonValue::U64(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} does not fit in u32", context, number),
        }),
        Some(JsonValue::U128(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} does not fit in u32", context, number),
        }),
        Some(JsonValue::I64(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} is negative or too large", context, number),
        }),
        Some(JsonValue::I128(number)) => u32::try_from(*number).map_err(|_| MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} value {} is negative or too large", context, number),
        }),
        Some(other) => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected integer, got {:?}", context, other),
        }),
        None => Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} missing integer", context),
        }),
    }
}

fn tokenizer_pattern_string(
    path: &Path,
    context: &str,
    value: Option<&JsonValue>,
) -> Result<String> {
    let object = tokenizer_object(path, context, value)?;
    tokenizer_string(path, &format!("{}.String", context), object.get("String"))
}

fn tokenizer_string_pair(
    path: &Path,
    context: &str,
    value: &JsonValue,
) -> Result<(String, String)> {
    let array = match value {
        JsonValue::Array(array) => array,
        other => {
            return Err(MlxRtError::Json {
                path: path.to_path_buf(),
                message: format!("{} expected [string, string], got {:?}", context, other),
            });
        }
    };
    if array.len() != 2 {
        return Err(MlxRtError::Json {
            path: path.to_path_buf(),
            message: format!("{} expected two strings, got {}", context, array.len()),
        });
    }
    Ok((
        tokenizer_string(path, &format!("{}[0]", context), array.first())?,
        tokenizer_string(path, &format!("{}[1]", context), array.get(1))?,
    ))
}

fn parse_byte_fallback_token(token: &str) -> Option<u8> {
    if !token.starts_with("<0x") || !token.ends_with('>') || token.len() != 6 {
        return None;
    }
    u8::from_str_radix(&token[3..5], 16).ok()
}

fn flush_pending_bytes(out: &mut String, pending_bytes: &mut Vec<u8>) {
    if pending_bytes.is_empty() {
        return;
    }
    out.push_str(&String::from_utf8_lossy(pending_bytes));
    pending_bytes.clear();
}

