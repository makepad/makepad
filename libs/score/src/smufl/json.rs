use super::{SmuflError, SmuflResult};
use crate::units::{StaffPoint, StaffSpaces};
use makepad_micro_serde::{DeJson, DeJsonState, DeJsonTok, JsonValue};
use std::collections::HashMap;

pub(super) type Object = HashMap<String, JsonValue>;

pub(super) fn parse(bytes: &[u8]) -> SmuflResult<JsonValue> {
    let input = std::str::from_utf8(bytes).map_err(|_| SmuflError::Utf8)?;
    let mut state = DeJsonState::default();
    let mut chars = input.chars();
    state.next(&mut chars);
    state.next_tok(&mut chars).map_err(json_error)?;
    let value = JsonValue::de_json(&mut state, &mut chars).map_err(json_error)?;
    if state.tok != DeJsonTok::Eof {
        return Err(SmuflError::Json {
            message: "trailing data after the top-level value".to_string(),
            line: state.line + 1,
            column: state.col + 1,
        });
    }
    Ok(value)
}

fn json_error(error: makepad_micro_serde::DeJsonErr) -> SmuflError {
    SmuflError::Json {
        message: error.msg,
        line: error.line + 1,
        column: error.col + 1,
    }
}

pub(super) fn root_object<'a>(
    value: &'a JsonValue,
    document: &str,
) -> SmuflResult<&'a Object> {
    object(value, document)
}

pub(super) fn object<'a>(value: &'a JsonValue, path: &str) -> SmuflResult<&'a Object> {
    match value {
        JsonValue::Object(value) => Ok(value),
        value => Err(wrong_type(path, "an object", value)),
    }
}

pub(super) fn array<'a>(value: &'a JsonValue, path: &str) -> SmuflResult<&'a [JsonValue]> {
    match value {
        JsonValue::Array(value) => Ok(value),
        value => Err(wrong_type(path, "an array", value)),
    }
}

pub(super) fn field<'a>(object: &'a Object, key: &str, parent: &str) -> SmuflResult<&'a JsonValue> {
    object.get(key).ok_or_else(|| SmuflError::MissingField {
        path: child_path(parent, key),
    })
}

pub(super) fn number(value: &JsonValue, path: &str) -> SmuflResult<f64> {
    let number = match value {
        JsonValue::U64(value) => *value as f64,
        JsonValue::U128(value) => *value as f64,
        JsonValue::I64(value) => *value as f64,
        JsonValue::I128(value) => *value as f64,
        JsonValue::F64(value) => *value,
        value => return Err(wrong_type(path, "a number", value)),
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(wrong_type(path, "a finite number", value))
    }
}

pub(super) fn staff_spaces(value: &JsonValue, path: &str) -> SmuflResult<StaffSpaces> {
    number(value, path).map(StaffSpaces::new)
}

pub(super) fn string<'a>(value: &'a JsonValue, path: &str) -> SmuflResult<&'a str> {
    match value {
        JsonValue::String(value) => Ok(value),
        value => Err(wrong_type(path, "a string", value)),
    }
}

pub(super) fn optional_string(
    object: &Object,
    key: &str,
    parent: &str,
) -> SmuflResult<Option<String>> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if matches!(value, JsonValue::Null) {
        return Ok(None);
    }
    Ok(Some(string(value, &child_path(parent, key))?.to_string()))
}

pub(super) fn string_array(value: &JsonValue, path: &str) -> SmuflResult<Vec<String>> {
    array(value, path)?
        .iter()
        .enumerate()
        .map(|(index, value)| string(value, &index_path(path, index)).map(str::to_string))
        .collect()
}

pub(super) fn optional_string_array(
    object: &Object,
    key: &str,
    parent: &str,
) -> SmuflResult<Vec<String>> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    if matches!(value, JsonValue::Null) {
        return Ok(Vec::new());
    }
    string_array(value, &child_path(parent, key))
}

pub(super) fn point(value: &JsonValue, path: &str) -> SmuflResult<StaffPoint> {
    let coordinates = array(value, path)?;
    if coordinates.len() != 2 {
        return Err(SmuflError::WrongType {
            path: path.to_string(),
            expected: "an array of exactly two numbers",
            found: "an array of another length",
        });
    }
    Ok(StaffPoint::new(
        staff_spaces(&coordinates[0], &index_path(path, 0))?,
        staff_spaces(&coordinates[1], &index_path(path, 1))?,
    ))
}

pub(super) fn codepoint(value: &JsonValue, path: &str) -> SmuflResult<char> {
    let text = string(value, path)?;
    let Some(hex) = text.strip_prefix("U+") else {
        return Err(SmuflError::InvalidCodepoint {
            path: path.to_string(),
            value: text.to_string(),
        });
    };
    let parsed = u32::from_str_radix(hex, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| SmuflError::InvalidCodepoint {
            path: path.to_string(),
            value: text.to_string(),
        })?;
    Ok(parsed)
}

pub(super) fn child_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}

pub(super) fn index_path(parent: &str, index: usize) -> String {
    format!("{parent}[{index}]")
}

fn wrong_type(path: &str, expected: &'static str, value: &JsonValue) -> SmuflError {
    SmuflError::WrongType {
        path: path.to_string(),
        expected,
        found: type_name(value),
    }
}

fn type_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::String(_) => "a string",
        JsonValue::Char(_) => "a character",
        JsonValue::U64(_)
        | JsonValue::U128(_)
        | JsonValue::I64(_)
        | JsonValue::I128(_)
        | JsonValue::F64(_) => "a number",
        JsonValue::Bool(_) => "a boolean",
        JsonValue::BareIdent(_) => "an identifier",
        JsonValue::Null => "null",
        JsonValue::Undefined => "undefined",
        JsonValue::Object(_) => "an object",
        JsonValue::Array(_) => "an array",
    }
}
