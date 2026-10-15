//! Read-only layout access for backend emission. Frontends own their type tables.
use crate::{shader_ids::ShaderTypeId, shader_schema::ShaderTypeData};

pub trait ShaderTypes {
    fn type_data(&self, ty: ShaderTypeId) -> &ShaderTypeData;
}
