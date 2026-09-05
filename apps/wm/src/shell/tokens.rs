//! The shell token object — `mod.wm_theme.shell`, `.material` and `.desk`
//! copied into `ShellTokens`'s type default, key for key. Its own module
//! so a live theme switch can evaluate it AGAIN: re-run the theme into the
//! VM (`theme::eval_into`), then this, then read a fresh `ShellTokens` off
//! the type default (`script_new_with_default`) and hand it to every
//! surface.
//!
//! The prelude import is what brings `set_type_default` (a `mod.std`
//! function) into scope. Every section type is registered first: a
//! `#[live]` field of an unregistered struct type has no default object,
//! and merging into it fails with "field ... not found in type-check and
//! has no default". Registering a type twice is harmless: `script_proto`
//! hands back the type already registered, and `set_type_default` re-sets
//! its default.

use makepad_widgets::*;

use super::{
    BarTokens, ControlTokens, DeskTokens, FontTokens, MaterialTokens, MenuTokens,
    NotificationTokens, ShellTokens, SpacingTokens, SurfaceTokens,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(BarTokens::script_component(vm)) {}
    set_type_default() do #(SurfaceTokens::script_component(vm)) {}
    set_type_default() do #(NotificationTokens::script_component(vm)) {}
    set_type_default() do #(MenuTokens::script_component(vm)) {}
    set_type_default() do #(ControlTokens::script_component(vm)) {}
    set_type_default() do #(SpacingTokens::script_component(vm)) {}
    set_type_default() do #(FontTokens::script_component(vm)) {}
    set_type_default() do #(MaterialTokens::script_component(vm)) {}
    set_type_default() do #(DeskTokens::script_component(vm)) {}

    set_type_default() do #(ShellTokens::script_component(vm)) {
        corner_radius: mod.wm_theme.shell.corner_radius
        bar +: {
            background: mod.wm_theme.shell.bar.background
            background_alpha: mod.wm_theme.shell.bar.background_alpha
            text: mod.wm_theme.shell.bar.text
            active: mod.wm_theme.shell.bar.active
            size_horizontal: mod.wm_theme.shell.bar.size_horizontal
            size_vertical: mod.wm_theme.shell.bar.size_vertical
            icon_slot: mod.wm_theme.shell.bar.icon_slot
            icon_canvas: mod.wm_theme.shell.bar.icon_canvas
            icon_font: mod.wm_theme.shell.bar.icon_font
            status_slot: mod.wm_theme.shell.bar.status_slot
        }
        popups +: {
            background: mod.wm_theme.shell.popups.background
            background_alpha: mod.wm_theme.shell.popups.background_alpha
            text: mod.wm_theme.shell.popups.text
            border: mod.wm_theme.shell.popups.border
            border_end: mod.wm_theme.shell.popups.border_end
            border_angle: mod.wm_theme.shell.popups.border_angle
            border_alpha: mod.wm_theme.shell.popups.border_alpha
            border_width: mod.wm_theme.shell.popups.border_width
        }
        tooltip +: {
            background: mod.wm_theme.shell.tooltip.background
            background_alpha: mod.wm_theme.shell.tooltip.background_alpha
            text: mod.wm_theme.shell.tooltip.text
            border: mod.wm_theme.shell.tooltip.border
            border_end: mod.wm_theme.shell.tooltip.border_end
            border_angle: mod.wm_theme.shell.tooltip.border_angle
            border_alpha: mod.wm_theme.shell.tooltip.border_alpha
            border_width: mod.wm_theme.shell.tooltip.border_width
        }
        notifications +: {
            countdown: mod.wm_theme.shell.notifications.countdown
            surface +: {
                background: mod.wm_theme.shell.notifications.background
                background_alpha: mod.wm_theme.shell.notifications.background_alpha
                text: mod.wm_theme.shell.notifications.text
                border: mod.wm_theme.shell.notifications.border
                border_end: mod.wm_theme.shell.notifications.border_end
                border_angle: mod.wm_theme.shell.notifications.border_angle
                border_alpha: mod.wm_theme.shell.notifications.border_alpha
                border_width: mod.wm_theme.shell.notifications.border_width
            }
        }
        menu +: {
            scrim: mod.wm_theme.shell.menu.scrim
            scrim_alpha: mod.wm_theme.shell.menu.scrim_alpha
            selected_background: mod.wm_theme.shell.menu.selected_background
            selected_background_alpha: mod.wm_theme.shell.menu.selected_background_alpha
            selected_text: mod.wm_theme.shell.menu.selected_text
            selected_border: mod.wm_theme.shell.menu.selected_border
            selected_border_alpha: mod.wm_theme.shell.menu.selected_border_alpha
            surface +: {
                background: mod.wm_theme.shell.menu.background
                background_alpha: mod.wm_theme.shell.menu.background_alpha
                text: mod.wm_theme.shell.menu.text
                border: mod.wm_theme.shell.menu.border
                border_end: mod.wm_theme.shell.menu.border_end
                border_angle: mod.wm_theme.shell.menu.border_angle
                border_alpha: mod.wm_theme.shell.menu.border_alpha
                border_width: mod.wm_theme.shell.menu.border_width
            }
        }
        launcher +: {
            scrim: mod.wm_theme.shell.launcher.scrim
            scrim_alpha: mod.wm_theme.shell.launcher.scrim_alpha
            selected_background: mod.wm_theme.shell.launcher.selected_background
            selected_background_alpha: mod.wm_theme.shell.launcher.selected_background_alpha
            selected_text: mod.wm_theme.shell.launcher.selected_text
            selected_border: mod.wm_theme.shell.launcher.selected_border
            selected_border_alpha: mod.wm_theme.shell.launcher.selected_border_alpha
            surface +: {
                background: mod.wm_theme.shell.launcher.background
                background_alpha: mod.wm_theme.shell.launcher.background_alpha
                text: mod.wm_theme.shell.launcher.text
                border: mod.wm_theme.shell.launcher.border
                border_end: mod.wm_theme.shell.launcher.border_end
                border_angle: mod.wm_theme.shell.launcher.border_angle
                border_alpha: mod.wm_theme.shell.launcher.border_alpha
                border_width: mod.wm_theme.shell.launcher.border_width
            }
        }
        controls +: {
            normal_color: mod.wm_theme.shell.controls.normal_color
            normal_fill_alpha: mod.wm_theme.shell.controls.normal_fill_alpha
            normal_border: mod.wm_theme.shell.controls.normal_border
            normal_border_width: mod.wm_theme.shell.controls.normal_border_width
            normal_border_alpha: mod.wm_theme.shell.controls.normal_border_alpha
            hover_color: mod.wm_theme.shell.controls.hover_color
            hover_fill_alpha: mod.wm_theme.shell.controls.hover_fill_alpha
            hover_border: mod.wm_theme.shell.controls.hover_border
            hover_border_width: mod.wm_theme.shell.controls.hover_border_width
            hover_border_alpha: mod.wm_theme.shell.controls.hover_border_alpha
            focus_color: mod.wm_theme.shell.controls.focus_color
            focus_fill_alpha: mod.wm_theme.shell.controls.focus_fill_alpha
            focus_border: mod.wm_theme.shell.controls.focus_border
            focus_border_width: mod.wm_theme.shell.controls.focus_border_width
            focus_border_alpha: mod.wm_theme.shell.controls.focus_border_alpha
            selected_color: mod.wm_theme.shell.controls.selected_color
            selected_fill_alpha: mod.wm_theme.shell.controls.selected_fill_alpha
            selected_border: mod.wm_theme.shell.controls.selected_border
            selected_border_width: mod.wm_theme.shell.controls.selected_border_width
            selected_border_alpha: mod.wm_theme.shell.controls.selected_border_alpha
            pressed_fill_alpha: mod.wm_theme.shell.controls.pressed_fill_alpha
            selection_fill_alpha: mod.wm_theme.shell.controls.selection_fill_alpha
        }
        spacing +: {
            xxs: mod.wm_theme.shell.spacing.xxs
            xs: mod.wm_theme.shell.spacing.xs
            sm: mod.wm_theme.shell.spacing.sm
            md: mod.wm_theme.shell.spacing.md
            lg: mod.wm_theme.shell.spacing.lg
            xl: mod.wm_theme.shell.spacing.xl
            xxl: mod.wm_theme.shell.spacing.xxl
            xxxl: mod.wm_theme.shell.spacing.xxxl
            huge: mod.wm_theme.shell.spacing.huge
            control_gap: mod.wm_theme.shell.spacing.control_gap
            control_padding_x: mod.wm_theme.shell.spacing.control_padding_x
            control_padding_y: mod.wm_theme.shell.spacing.control_padding_y
            input_padding_y: mod.wm_theme.shell.spacing.input_padding_y
            control_height: mod.wm_theme.shell.spacing.control_height
            popup_row_height: mod.wm_theme.shell.spacing.popup_row_height
            dropdown_width: mod.wm_theme.shell.spacing.dropdown_width
            searchable_dropdown_width: mod.wm_theme.shell.spacing.searchable_dropdown_width
            number_field_width: mod.wm_theme.shell.spacing.number_field_width
            searchable_popup_min_height: mod.wm_theme.shell.spacing.searchable_popup_min_height
            row_gap: mod.wm_theme.shell.spacing.row_gap
            row_padding_x: mod.wm_theme.shell.spacing.row_padding_x
            label_gap: mod.wm_theme.shell.spacing.label_gap
            panel_gap: mod.wm_theme.shell.spacing.panel_gap
            panel_padding: mod.wm_theme.shell.spacing.panel_padding
            popup_padding: mod.wm_theme.shell.spacing.popup_padding
            gaps_out: mod.wm_theme.shell.spacing.gaps_out
        }
        font +: {
            base_size: mod.wm_theme.shell.font.base_size
            caption: mod.wm_theme.shell.font.caption
            body_small: mod.wm_theme.shell.font.body_small
            body: mod.wm_theme.shell.font.body
            subtitle: mod.wm_theme.shell.font.subtitle
            title: mod.wm_theme.shell.font.title
            heading: mod.wm_theme.shell.font.heading
            display: mod.wm_theme.shell.font.display
            display_large: mod.wm_theme.shell.font.display_large
            icon_small: mod.wm_theme.shell.font.icon_small
            icon: mod.wm_theme.shell.font.icon
            icon_large: mod.wm_theme.shell.font.icon_large
        }
        material +: {
            glass: mod.wm_theme.material.glass
            corner_radius: mod.wm_theme.material.corner_radius
            control_radius: mod.wm_theme.material.control_radius
            blur_level: mod.wm_theme.material.blur_level
            lensing_effect: mod.wm_theme.material.lensing_effect
            lensing_strength: mod.wm_theme.material.lensing_strength
            lensing_width: mod.wm_theme.material.lensing_width
            diffraction_strength: mod.wm_theme.material.diffraction_strength
            tint_color: mod.wm_theme.material.tint_color
            tint_alpha: mod.wm_theme.material.tint_alpha
            border_color: mod.wm_theme.material.border_color
            border_alpha: mod.wm_theme.material.border_alpha
            border_width: mod.wm_theme.material.border_width
            specular_strength: mod.wm_theme.material.specular_strength
            noise_strength: mod.wm_theme.material.noise_strength
            shadow_color: mod.wm_theme.material.shadow_color
            shadow_alpha: mod.wm_theme.material.shadow_alpha
            shadow_radius: mod.wm_theme.material.shadow_radius
            shadow_offset_y: mod.wm_theme.material.shadow_offset_y
            fallback_color: mod.wm_theme.material.fallback_color
        }
        desk +: {
            gaps_in: mod.wm_theme.desk.gaps_in
            gaps_out: mod.wm_theme.desk.gaps_out
            border_size: mod.wm_theme.desk.border_size
            corner_radius: mod.wm_theme.desk.corner_radius
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::rgb;
    use crate::theme;
    use makepad_widgets::makepad_script::{ScriptVmBase, ScriptVmHost};

    fn test_vm() -> ScriptVm<'static> {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        ScriptVm {
            host,
            bx: Box::new(ScriptVmBase::new()),
        }
    }

    /// Run `f` with the VM's error sink installed: parse and runtime
    /// errors surface here instead of being dropped or logged.
    fn errors_of(vm: &mut ScriptVm, f: impl FnOnce(&mut ScriptVm) -> ScriptValue) -> Vec<String> {
        let prev = vm.bx.captured_errors.replace(Vec::new());
        let value = f(vm);
        let mut errors = vm.take_errors();
        vm.bx.captured_errors = prev;
        if value.is_err() {
            errors.push(format!("{value:?}"));
        }
        errors
    }

    fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: name.to_string(),
            file: format!("{name}.splash"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    }

    #[test]
    fn the_mapping_reads_the_theme_back_into_shell_tokens() {
        let mut vm = test_vm();
        // The block resolves `set_type_default` through the widgets
        // prelude, which a bare VM has not built: a prelude that is just
        // std.
        let seed = "mod.prelude = { widgets_internal: { ..mod.std } }\ntrue\n";
        let errors = errors_of(&mut vm, |vm| eval(vm, "seed", seed));
        assert!(errors.is_empty(), "{errors:?}");
        let src = theme::BUNDLED_TOKYO_NIGHT_SPLASH.replace(
            "mod.wm_theme = {",
            "mod.wm_theme = {\n    material: { glass: 1.0 }\n    desk: { gaps_out: 24.0 }",
        );
        assert!(theme::eval_into(&mut vm, &src));

        let errors = errors_of(&mut vm, script_mod);
        assert!(errors.is_empty(), "{errors:?}");

        let t = ShellTokens::script_new_with_default(&mut vm);
        // Each differs from the Rust default: #101315, 0.0, 10.0.
        assert_eq!(t.bar.background, rgb(0x1a, 0x1b, 0x26));
        assert!(t.material.is_glass());
        assert_eq!(t.desk.gaps_out, 24.0);

        // Run it again: same body slot, new default, no growth.
        let bodies = vm.bx.code.bodies.borrow().len();
        let errors = errors_of(&mut vm, script_mod);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(vm.bx.code.bodies.borrow().len(), bodies);
    }
}
