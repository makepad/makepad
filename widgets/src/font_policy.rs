pub use crate::makepad_platform::{FontAsset, FontChain, FontPolicy, FontRole, FontSet};
use crate::makepad_platform::{
    id, LiveId, NoTrap, ScriptApply, ScriptMod, ScriptValue, ScriptVm, ScriptVmCx,
};

fn family_value(
    vm: &mut ScriptVm,
    chain: FontChain,
    set: FontSet,
    family_proto: ScriptValue,
    member_proto: ScriptValue,
) -> ScriptValue {
    let family = vm.bx.heap.new_with_proto(family_proto);
    vm.bx.heap.set_value(
        family,
        id!(_font_role).into(),
        LiveId::from_str_with_lut(chain.role.as_str())
            .unwrap_or_else(|_| LiveId::from_str(chain.role.as_str()))
            .into(),
        NoTrap,
    );
    vm.bx.heap.set_value(
        family,
        id!(_font_set).into(),
        LiveId::from_str_with_lut(set.as_str())
            .unwrap_or_else(|_| LiveId::from_str(set.as_str()))
            .into(),
        NoTrap,
    );

    for asset in chain.members {
        let member = vm.bx.heap.new_with_proto(member_proto);
        let resource = crate::makepad_platform::script::res::register_crate_resource_path(
            vm,
            asset.resource_path,
        );
        vm.bx
            .heap
            .set_value(member, id!(res).into(), resource, NoTrap);
        vm.bx.heap.set_value(
            member,
            id!(asc).into(),
            (asset.ascender as f64).into(),
            NoTrap,
        );
        vm.bx.heap.set_value(
            member,
            id!(desc).into(),
            (asset.descender as f64).into(),
            NoTrap,
        );
        vm.bx.heap.set_value(
            member,
            id!(weight).into(),
            (asset.weight as f64).into(),
            NoTrap,
        );
        let member_id = LiveId::from_str_with_lut(asset.id)
            .unwrap_or_else(|_| LiveId::from_str(asset.id));
        vm.bx
            .heap
            .vec_push(family, member_id.into(), member.into(), NoTrap);
    }
    family.into()
}

/// Install source-compatible `theme.font_*` styles from the selected policy.
/// Resource handles are constructed while walking that policy, so the
/// unselected branch is never evaluated.
pub(crate) fn install_theme_fonts(vm: &mut ScriptVm) {
    vm.cx_mut().freeze_font_set();
    let set = vm.cx().font_set();
    let policy = set.policy();
    let family_proto = crate::script_eval!(vm, {mod.text.FontFamily});
    let member_proto = crate::script_eval!(vm, {mod.text.FontMember});
    let regular = family_value(vm, policy.regular, set, family_proto, member_proto);
    let bold = family_value(vm, policy.bold, set, family_proto, member_proto);
    let italic = family_value(vm, policy.italic, set, family_proto, member_proto);
    let bold_italic = family_value(vm, policy.bold_italic, set, family_proto, member_proto);
    let monospace = family_value(vm, policy.monospace, set, family_proto, member_proto);
    let icons = family_value(vm, policy.icons, set, family_proto, member_proto);

    crate::script_eval!(vm, {
        use mod.text.*

        mod.themes.dark = mod.themes.dark{
            font_label: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_regular: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            // Deprecated compatibility aliases; use the names above.
            font_regular_i18n: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold_i18n: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic_i18n: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic_i18n: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            font_code: TextStyle{font_size: 9.0 font_family: #(monospace) line_spacing: 1.35}
            font_icons: TextStyle{font_family: #(icons) line_spacing: 1.2}
        }
        mod.themes.light = mod.themes.light{
            font_label: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_regular: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            // Deprecated compatibility aliases; use the names above.
            font_regular_i18n: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold_i18n: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic_i18n: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic_i18n: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            font_code: TextStyle{font_size: 9.0 font_family: #(monospace) line_spacing: 1.35}
            font_icons: TextStyle{font_family: #(icons) line_spacing: 1.2}
        }
        mod.themes.skeleton = mod.themes.skeleton{
            font_label: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_regular: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            // Deprecated compatibility aliases; use the names above.
            font_regular_i18n: TextStyle{font_family: #(regular) line_spacing: 1.2}
            font_bold_i18n: TextStyle{font_family: #(bold) line_spacing: 1.2}
            font_italic_i18n: TextStyle{font_family: #(italic) line_spacing: 1.2}
            font_bold_italic_i18n: TextStyle{font_family: #(bold_italic) line_spacing: 1.2}
            font_code: TextStyle{font_size: 9.0 font_family: #(monospace) line_spacing: 1.35}
            font_icons: TextStyle{font_family: #(icons) line_spacing: 1.2}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_platform::{Cx, ScriptNew};

    fn registered_font_paths(set: FontSet) -> Vec<String> {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert!(cx.set_font_set(set));
        cx.with_vm(crate::theme_mod);
        let mut paths = cx
            .script_data
            .resources
            .resources
            .borrow()
            .iter()
            .filter_map(|resource| resource.dependency_path.clone())
            .filter(|path| path.ends_with(".ttf") || path.ends_with(".otf"))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }

    #[test]
    fn platform_policy_is_the_widgets_theme_contract() {
        assert_eq!(FontSet::Latin.policy().regular.role, FontRole::Regular);
        assert_eq!(FontSet::International.policy().icons.role, FontRole::Icons);
        assert_eq!(FontSet::Latin.policy().regular.members.len(), 2);
        assert_eq!(FontSet::International.policy().regular.members.len(), 3);
        assert_eq!(
            FontSet::International.policy().regular.members[2].id,
            "noto_color_emoji"
        );
    }

    #[test]
    fn application_selection_rejects_late_mutation() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert!(cx.set_font_set(FontSet::Latin));
        cx.freeze_font_set();
        assert!(!cx.set_font_set(FontSet::International));
        assert_eq!(cx.font_set(), FontSet::Latin);
    }

    #[test]
    fn manifest_fixture_is_resolved_from_the_same_policy() {
        for set in [FontSet::Latin, FontSet::International] {
            let manifest = std::str::from_utf8(set.manifest_bytes()).unwrap();
            let assets = manifest
                .lines()
                .filter_map(|line| line.strip_prefix("asset="))
                .collect::<Vec<_>>();
            assert_eq!(
                assets,
                set.policy()
                    .assets
                    .iter()
                    .map(|asset| asset.resource_path)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn theme_registers_only_the_selected_policy_font_handles() {
        for set in [FontSet::Latin, FontSet::International] {
            let mut expected = set
                .policy()
                .assets
                .iter()
                .map(|asset| asset.resource_path.to_string())
                .collect::<Vec<_>>();
            expected.sort();
            assert_eq!(registered_font_paths(set), expected);
        }
    }

    #[test]
    fn international_theme_keeps_desktop_chain_order_and_i18n_aliases() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert!(cx.set_font_set(FontSet::International));
        cx.with_vm(|vm| {
            crate::theme_mod(vm);
            let regular_value = crate::script_eval!(vm, {mod.theme.font_regular});
            let bold_value = crate::script_eval!(vm, {mod.theme.font_bold});
            let italic_value = crate::script_eval!(vm, {mod.theme.font_italic});
            let bold_italic_value = crate::script_eval!(vm, {mod.theme.font_bold_italic});
            let regular_i18n_value = crate::script_eval!(vm, {mod.theme.font_regular_i18n});
            let bold_i18n_value = crate::script_eval!(vm, {mod.theme.font_bold_i18n});
            let italic_i18n_value = crate::script_eval!(vm, {mod.theme.font_italic_i18n});
            let bold_italic_i18n_value =
                crate::script_eval!(vm, {mod.theme.font_bold_italic_i18n});
            let regular = crate::TextStyle::script_from_value(vm, regular_value);
            let bold = crate::TextStyle::script_from_value(vm, bold_value);
            let italic = crate::TextStyle::script_from_value(vm, italic_value);
            let bold_italic = crate::TextStyle::script_from_value(vm, bold_italic_value);
            let regular_i18n = crate::TextStyle::script_from_value(vm, regular_i18n_value);
            let bold_i18n = crate::TextStyle::script_from_value(vm, bold_i18n_value);
            let italic_i18n = crate::TextStyle::script_from_value(vm, italic_i18n_value);
            let bold_italic_i18n =
                crate::TextStyle::script_from_value(vm, bold_italic_i18n_value);

            let regular_ids = regular.font_family.member_ids().collect::<Vec<_>>();
            let bold_ids = bold.font_family.member_ids().collect::<Vec<_>>();
            let italic_ids = italic.font_family.member_ids().collect::<Vec<_>>();
            let bold_italic_ids = bold_italic.font_family.member_ids().collect::<Vec<_>>();
            assert_eq!(
                regular_ids,
                ["ibm_plex_text", "lxgw_wenkai_regular", "noto_color_emoji"]
            );
            assert_eq!(
                bold_ids,
                ["ibm_plex_semibold", "lxgw_wenkai_bold", "noto_color_emoji"]
            );
            assert_eq!(
                italic_ids,
                ["ibm_plex_italic", "lxgw_wenkai_regular", "noto_color_emoji"]
            );
            assert_eq!(
                bold_italic_ids,
                ["ibm_plex_bold_italic", "lxgw_wenkai_bold", "noto_color_emoji"]
            );
            assert_eq!(regular_i18n.font_family.member_ids().collect::<Vec<_>>(), regular_ids);
            assert_eq!(bold_i18n.font_family.member_ids().collect::<Vec<_>>(), bold_ids);
            assert_eq!(
                italic_i18n.font_family.member_ids().collect::<Vec<_>>(),
                italic_ids
            );
            assert_eq!(
                bold_italic_i18n.font_family.member_ids().collect::<Vec<_>>(),
                bold_italic_ids
            );
        });
    }
}
