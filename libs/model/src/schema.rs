//! Shared strict schemas for authoring domains and their versioned source data.
use crate::{canon::{Reader,Writer}, json::{self,Value}, service::*, Error,Limits,Result,Transform};
pub(crate) fn vec_value(v:&[f64])->Value {Value::Arr(v.iter().copied().map(Value::F64).collect())}
pub(crate) fn name(v:&str,limits:&Limits)->Result<()> {
    if v.is_empty()||v.len()>limits.max_name_bytes||v.chars().any(char::is_control) {Err(Error::Invalid("authoring name"))}else{Ok(())}
}
pub(crate) fn transform(v:&Value)->Result<Transform> {
    fields(v,&["translation","rotation","scale"])?;
    let t=Transform{translation:v.get("translation").map(array).transpose()?.unwrap_or([0.;3]),
        rotation:v.get("rotation").map(array).transpose()?.unwrap_or([0.,0.,0.,1.]),
        scale:v.get("scale").map(array).transpose()?.unwrap_or([1.;3])};t.validate()?;Ok(t)
}
pub(crate) fn transform_value(t:&Transform)->Value {json::obj(vec![
    ("translation",vec_value(&t.translation)),("rotation",vec_value(&t.rotation)),("scale",vec_value(&t.scale))])}
pub(crate) fn rows(v:&Value,max:usize)->Result<&[Value]> {
    let rows=v.as_arr().ok_or(Error::Invalid("expected authoring array"))?;
    if rows.len()>max {Err(Error::Budget("authoring array"))}else{Ok(rows)}
}
pub(crate) fn boolean(v:&Value)->Result<bool> {v.as_bool().ok_or(Error::Invalid("expected boolean"))}
pub(crate) fn write_value(w:&mut Writer,v:Value)->Result<()> {w.blob(v.to_json().as_bytes())}
pub(crate) fn read_value(r:&mut Reader,limits:&Limits)->Result<Value> {
    json::parse_depth(r.blob(limits.max_source_bytes)?,24).map_err(Error::Corrupt)
}
