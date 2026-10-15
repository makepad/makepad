//! The shader compiler's reflection types: what a script value is to the
//! shader front end (`ShaderType`), and the compile output (`ShaderOutput`,
//! re-exported). The front end itself lowers bytecode to the shader IR
//! (shader_lower*.rs, shader_ir*.rs).

use crate::mod_pod::*;
use crate::value::*;
use makepad_live_id::*;

pub use crate::shader_output::*;

#[derive(Debug, PartialEq, Clone)]
pub enum ShaderType {
    None,
    IoSelf(ScriptObject),
    PodType(ScriptPodType),
    Pod(ScriptPodType),
    PodPtr(ScriptPodType), // Pointer to pod type (used for uniform buffers in Metal)
    CooperativeMatrix(crate::shader_ir::TyId),
    CooperativeMatrixType(crate::shader_ir::TyId),
    Texture(TextureType),  // Texture type for method calls like .size()
    Id(LiveId),
    AbstractInt,
    AbstractFloat,
    Range {
        start: String,
        end: String,
        ty: ScriptPodType,
    },
    Error(ScriptValue),
    /// A script scope object that we're accessing properties from.
    /// Properties are flattened into ScopeUniforms.
    ScopeObject(ScriptObject),
    /// A uniform buffer defined in the script scope (e.g., `let buf = shader.uniform_buffer(...)`)
    /// Contains the uniform buffer object and its pod type.
    ScopeUniformBuffer {
        obj: ScriptObject,
        pod_ty: ScriptPodType,
    },
    /// A texture defined in the script scope (e.g., `let tex = shader.texture_2d(float)`)
    /// Contains the texture object and its type.
    ScopeTexture {
        obj: ScriptObject,
        tex_type: TextureType,
        shader_name: LiveId,
    },
}

impl ShaderType {
    pub fn make_concrete(&self, builtins: &ScriptPodBuiltins) -> Option<ScriptPodType> {
        match self {
            Self::Pod(ty) => Some(*ty),
            Self::PodPtr(ty) => Some(*ty),
            Self::Texture(_) => None, // Textures don't have a concrete pod type
            Self::ScopeTexture { .. } => None, // Scope textures don't have a concrete pod type
            Self::Id(_id) => None,
            Self::None => None,
            Self::IoSelf(_) => None,
            Self::ScopeObject(_) => None, // Scope objects don't have a concrete pod type
            Self::ScopeUniformBuffer { pod_ty, .. } => Some(*pod_ty),
            Self::PodType(_) => None,
            Self::AbstractInt => Some(builtins.pod_i32),
            Self::AbstractFloat => Some(builtins.pod_f32),
            Self::Range { ty, .. } => Some(*ty),
            Self::Error(_e) => None,
            Self::CooperativeMatrix(_) | Self::CooperativeMatrixType(_) => None,
        }
    }
}
