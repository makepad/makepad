//! Shared authoring-name checks and inspection value helpers.
use crate::{json::{self,Value}, Error,Limits,Result,Transform};
pub(crate) fn vec_value(v:&[f64])->Value {Value::Arr(v.iter().copied().map(Value::F64).collect())}
pub(crate) fn name(v:&str,limits:&Limits)->Result<()> {
    if v.is_empty()||v.len()>limits.max_name_bytes||v.chars().any(char::is_control) {Err(Error::Invalid("authoring name"))}else{Ok(())}
}
pub(crate) fn transform_value(t:&Transform)->Value {json::obj(vec![
    ("translation",vec_value(&t.translation)),("rotation",vec_value(&t.rotation)),("scale",vec_value(&t.scale))])}
