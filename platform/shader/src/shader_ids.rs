//! Registry-owned shader identities. Zero is the absent declaration/function/scope.
use makepad_relax_api::relax_api;

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderDef(pub u32);

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderFnId(pub u32);

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderScopeId(pub u32);

#[relax_api(plain)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShaderTypeId(pub u32);

impl ShaderTypeId {
    pub const VOID: Self = Self(0);
}
