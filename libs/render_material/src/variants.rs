//! The variants a material needs, derived from its hooks and blend, never
//! written by hand: which engine overrides its colour program carries, whether
//! its colour pass may cut pixels, and which shadow caster it casts with.
use makepad_scene::{BaseKind, Blend, LightingModel};
use crate::builtin::Builtin;
use crate::hooks::Hook;
use crate::program::{HookSet, MaterialError};

/// What a host asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialDesc {
    pub kind: BaseKind,
    pub blend: Blend,
    pub lighting: LightingModel,
    /// The frame's environment carries IBL (`Environment::ibl`).
    pub ibl: bool,
}

impl Default for MaterialDesc {
    fn default() -> Self {
        Self { kind: BaseKind::Pbr, blend: Blend::Opaque, lighting: LightingModel::Lit, ibl: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowVariant {
    /// The material casts nothing (blended materials).
    None,
    /// The engine's own caster: the geometry is where the base puts it.
    Stock,
    /// A derived depth-only caster running the `vertex` hook, and for a
    /// masked material `surface` for its alpha only, so displaced geometry
    /// casts a displaced shadow and alpha-tested holes cast holes.
    Derived { vertex: bool, mask: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantPlan {
    pub builtins: Vec<Builtin>,
    /// The colour pass can cut pixels (a surface hook's alpha, or Mask). A
    /// program that cannot draws through the lane's discard-free variant
    /// where the base offers one.
    pub can_discard: bool,
    pub shadow: ShadowVariant,
}

pub fn plan(desc: &MaterialDesc, hooks: &HookSet) -> Result<VariantPlan, MaterialError> {
    let unlit = desc.kind == BaseKind::Unlit || desc.lighting == LightingModel::Unlit;
    let lit_hooks = hooks.has(Hook::Light) || hooks.has(Hook::Lighting);
    if unlit && lit_hooks {
        return Err(MaterialError::new("an Unlit material has no lighting to hook: use a Pbr base for `light` or `lighting`"));
    }
    let mut builtins = Vec::new();
    if unlit {
        // The Unlit kind's params carry its stops; a lit base drawn unlit
        // keeps its params for the hooks.
        builtins.push(if desc.kind == BaseKind::Unlit { Builtin::Unlit } else { Builtin::Flat });
    } else {
        if lit_hooks {
            builtins.push(Builtin::HookedCompose);
        }
        if desc.ibl {
            builtins.push(Builtin::Ibl);
        }
    }
    let mask = desc.blend.can_discard();
    let can_discard = mask || hooks.has(Hook::Surface);
    let shadow = if !desc.blend.is_opaque() {
        ShadowVariant::None
    } else if hooks.has(Hook::Vertex) || mask {
        ShadowVariant::Derived { vertex: hooks.has(Hook::Vertex), mask }
    } else {
        ShadowVariant::Stock
    };
    Ok(VariantPlan { builtins, can_discard, shadow })
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_draw::ScriptObject;

    fn hooks(list: &[Hook]) -> HookSet {
        let mut set = HookSet::new();
        for h in list {
            set.set(*h, ScriptObject::ZERO);
        }
        set
    }

    #[test]
    fn plain_materials_keep_the_stock_caster_and_no_overrides() {
        let p = plan(&MaterialDesc::default(), &HookSet::new()).unwrap();
        assert_eq!(p, VariantPlan { builtins: vec![], can_discard: false, shadow: ShadowVariant::Stock });
        let p = plan(&MaterialDesc::default(), &hooks(&[Hook::Finish, Hook::Emission])).unwrap();
        assert!(!p.can_discard && p.builtins.is_empty(), "finish and emission never cut or relight");
    }

    #[test]
    fn displaced_and_masked_materials_derive_a_caster() {
        let p = plan(&MaterialDesc::default(), &hooks(&[Hook::Vertex])).unwrap();
        assert_eq!(p.shadow, ShadowVariant::Derived { vertex: true, mask: false });
        let masked = MaterialDesc { blend: Blend::Mask { cutoff: 0.3 }, ..Default::default() };
        let p = plan(&masked, &hooks(&[Hook::Surface])).unwrap();
        assert_eq!(p.shadow, ShadowVariant::Derived { vertex: false, mask: true });
        assert!(p.can_discard);
        let over = MaterialDesc { blend: Blend::Over, ..Default::default() };
        assert_eq!(plan(&over, &hooks(&[Hook::Vertex])).unwrap().shadow, ShadowVariant::None);
    }

    #[test]
    fn lighting_hooks_select_the_hooked_composition_and_unlit_refuses_them() {
        let p = plan(&MaterialDesc { ibl: true, ..Default::default() }, &hooks(&[Hook::Light])).unwrap();
        assert_eq!(p.builtins, vec![Builtin::HookedCompose, Builtin::Ibl]);
        let unlit = MaterialDesc { kind: BaseKind::Unlit, ibl: true, ..Default::default() };
        assert_eq!(plan(&unlit, &HookSet::new()).unwrap().builtins, vec![Builtin::Unlit], "unlit takes no IBL");
        assert!(plan(&unlit, &hooks(&[Hook::Lighting])).is_err());
        let flat = MaterialDesc { lighting: LightingModel::Unlit, ..Default::default() };
        assert_eq!(plan(&flat, &hooks(&[Hook::Surface])).unwrap().builtins, vec![Builtin::Flat], "a lit base drawn unlit keeps its params");
    }
}
