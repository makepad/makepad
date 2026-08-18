//! Bounded typed values, shared by spawn recipes, scene component parameters,
//! and typed persistent-state defaults. Arbitrary captured VM locals are
//! exactly what this type refuses to be.

use crate::codec::{canon_enum, CanonReader, CanonWriter};
use crate::error::AssetDataError;
use crate::geom::Vec3;
use crate::id::{AssetRevisionRef, SceneObjectKey};
use crate::limits::MAX_VALUE_STR_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    Bool,
    I64,
    F32,
    Vec3,
    Str,
    /// A stable reference to another authored object.
    Key,
    /// An exact asset revision.
    Asset,
}

canon_enum!(ValueType {
    Bool = 0,
    I64 = 1,
    F32 = 2,
    Vec3 = 3,
    Str = 4,
    Key = 5,
    Asset = 6,
});

impl ValueType {
    pub(crate) fn encode(self, w: &mut CanonWriter) {
        w.u8(self.tag());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    I64(i64),
    F32(f32),
    Vec3(Vec3),
    Str(String),
    Key(SceneObjectKey),
    Asset(AssetRevisionRef),
}

impl Value {
    pub fn value_type(&self) -> ValueType {
        match self {
            Value::Bool(_) => ValueType::Bool,
            Value::I64(_) => ValueType::I64,
            Value::F32(_) => ValueType::F32,
            Value::Vec3(_) => ValueType::Vec3,
            Value::Str(_) => ValueType::Str,
            Value::Key(_) => ValueType::Key,
            Value::Asset(_) => ValueType::Asset,
        }
    }

    pub(crate) fn validate(&self, what: &'static str) -> Result<(), AssetDataError> {
        match self {
            Value::Bool(_) | Value::I64(_) | Value::Key(_) | Value::Asset(_) => Ok(()),
            Value::F32(v) => {
                if v.is_finite() {
                    Ok(())
                } else {
                    Err(AssetDataError::Malformed { what })
                }
            }
            Value::Vec3(v) => v.validate(what),
            Value::Str(s) => {
                if s.len() > MAX_VALUE_STR_BYTES {
                    Err(AssetDataError::OverBudget {
                        what,
                        limit: MAX_VALUE_STR_BYTES as u64,
                        found: s.len() as u64,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    pub(crate) fn encode(&self, w: &mut CanonWriter) {
        w.u8(self.value_type().tag());
        match self {
            Value::Bool(v) => w.bool(*v),
            Value::I64(v) => w.i64(*v),
            Value::F32(v) => w.f32(*v),
            Value::Vec3(v) => v.encode(w),
            Value::Str(s) => w.str(s),
            Value::Key(k) => k.encode(w),
            Value::Asset(a) => a.encode(w),
        }
    }

    pub(crate) fn decode(r: &mut CanonReader) -> Result<Self, AssetDataError> {
        Ok(match ValueType::decode(r)? {
            ValueType::Bool => Value::Bool(r.bool("Value::Bool")?),
            ValueType::I64 => Value::I64(r.i64("Value::I64")?),
            ValueType::F32 => Value::F32(r.f32("Value::F32")?),
            ValueType::Vec3 => Value::Vec3(Vec3::decode(r, "Value::Vec3")?),
            ValueType::Str => Value::Str(r.str("Value::Str", MAX_VALUE_STR_BYTES)?),
            ValueType::Key => Value::Key(SceneObjectKey::decode(r)?),
            ValueType::Asset => Value::Asset(AssetRevisionRef::decode(r)?),
        })
    }
}
