//! Floating action, radial, pill-nav, line and hamburger menus, and the command palette.
//!
//! A family crate of `makepad-widgets`: it builds on `makepad-widgets-core`
//! alone, has no features of its own, and compiles once whatever mix of
//! families an app picks. `makepad-widgets` re-exports it under the module
//! paths it always had and registers it when the app's features ask for it.

// The core's modules and prelude, as `crate::...` paths for the modules
// that moved here from it.
use makepad_widgets_core::*;

pub mod command_palette;
pub mod floating_action;
pub mod hamburger_menu;
pub mod line_menu;
pub mod pill_nav;
pub mod radial_menu;

/// Registers the navigation menus: the line menu after the pill nav whose
/// surface it draws with.
pub fn nav_menus_mod(vm: &mut ScriptVm) {
    crate::pill_nav::script_mod(vm);
    crate::radial_menu::script_mod(vm);
    crate::floating_action::script_mod(vm);
    crate::line_menu::script_mod(vm);
    crate::hamburger_menu::script_mod(vm);
}

/// Registers the command palette.
pub fn command_palette_mod(vm: &mut ScriptVm) {
    crate::command_palette::script_mod(vm);
}

// The pooled test contexts and the whole registration the tests build on:
// the core with the navigation menus and the command palette.
#[cfg(test)]
pub(crate) use makepad_widgets_core::test_cx::PooledCx;

#[cfg(test)]
pub(crate) fn script_mod(vm: &mut ScriptVm) {
    makepad_widgets_core::script_mod_with(
        vm,
        WindowFamilies::default(),
        &[nav_menus_mod, command_palette_mod],
    );
}

#[cfg(test)]
pub(crate) fn checkout_test_cx() -> PooledCx {
    makepad_widgets_core::test_cx::checkout_test_cx(script_mod)
}

#[cfg(test)]
pub(crate) fn on_test_cx(f: impl FnOnce() + Send + 'static) {
    makepad_widgets_core::test_cx::on_test_cx(f)
}

// Registration order, read off the source the way the core's registration
// tests read it. The families register after every widget of the core, so
// the order an app gets is the core's `widgets_mod_with` followed by this
// crate's `nav_menus_mod`, and that is the text searched here.

/// The front crate's lib.rs, whose re-exports put each menu in the prelude.
#[cfg(test)]
const FRONT_LIB: &str = include_str!("../../../src/lib.rs");

/// The core's registration, then this family's, in the order they run.
#[cfg(test)]
fn widgets_mod_source() -> &'static str {
    static SOURCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| {
        let core = include_str!("../../../core/src/lib.rs");
        let start = core.find("pub fn widgets_mod_with(").expect("widgets_mod_with");
        let end = core.find("pub fn script_mod(vm: &mut ScriptVm)").expect("script_mod");
        let family = include_str!("lib.rs");
        let fam_start = family.find("pub fn nav_menus_mod(").expect("nav_menus_mod");
        let fam_end = family.find("pub fn command_palette_mod(").expect("command_palette_mod");
        format!("{}{}", &core[start..end], &family[fam_start..fam_end])
    })
}

/// Asserts `call` is registered after every one of `bases`. When the first
/// base is one of this family's, `call` must also follow it directly, so a
/// menu keeps the slot its bases give it; a base of the core always comes
/// before the whole family, so there is no slot beside it to keep.
#[cfg(test)]
fn assert_registered_after(call: &str, bases: &[&str]) {
    let calls = widgets_mod_source();
    let at = calls.find(call).unwrap_or_else(|| panic!("{call} is not registered"));
    for base in bases {
        let base_at = calls.find(base).unwrap_or_else(|| panic!("{base} is not registered"));
        assert!(base_at < at, "{base} must register before {call}");
    }
    let first = bases[0];
    let family = &calls[calls.find("pub fn nav_menus_mod(").unwrap()..];
    if let Some(first_at) = family.find(first) {
        let next = family[first_at + first.len()..]
            .lines()
            .map(str::trim)
            .find(|line| line.ends_with("::script_mod(vm);"))
            .unwrap_or_default();
        assert_eq!(next, call, "{call} must follow {first} directly");
    }
}

#[cfg(test)]
mod radial_menu_registration_tests {
    /// The ring menu registers after the glass it samples and the badge
    /// whose text measure it uses, with one type default: `PieMenu` and the
    /// other presets derive from it and must not take the default with them.
    /// Neither base is its neighbour, so the order is read here rather than
    /// through `assert_registered_after`, which also pins the slot.
    #[test]
    fn test_radial_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let radial = include_str!("radial_menu.rs");
        assert!(lib.contains("\npub mod radial_menu;"));
        assert!(crate::FRONT_LIB.contains("radial_menu::*"));
        let calls = crate::widgets_mod_source();
        let call = "crate::radial_menu::script_mod(vm);";
        let at = calls.find(call).unwrap_or_else(|| panic!("{call} is not registered"));
        for base in ["crate::gauss_view::script_mod(vm);", "crate::badge::script_mod(vm);"] {
            let base_at = calls.find(base).unwrap_or_else(|| panic!("{base} is not registered"));
            assert!(base_at < at, "{base} must register before {call}");
        }
        assert!(radial.contains("mod.widgets.RadialMenuBase = #(RadialMenu::register_widget(vm))"));
        assert_eq!(radial.matches("set_type_default() do mod.widgets.RadialMenuBase").count(), 1);
        assert!(radial.contains("mod.widgets.PieMenu = mod.widgets.RadialMenu{"), "the ring in its field is a preset, not a type");
    }
}

#[cfg(test)]
mod floating_action_registration_tests {
    /// The floating action registers directly after the toolbar whose
    /// floating slot can host it, after the button its faces are, with one
    /// type default for the action and one for an item.
    #[test]
    fn test_floating_action_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let floating = include_str!("floating_action.rs");
        assert!(lib.contains("\npub mod floating_action;"));
        assert!(crate::FRONT_LIB.contains("floating_action::*"));
        crate::assert_registered_after(
            "crate::floating_action::script_mod(vm);",
            &["crate::toolbar::script_mod(vm);", "crate::button::script_mod(vm);"],
        );
        assert!(floating.contains("mod.widgets.FloatingActionBase = #(FloatingAction::register_widget(vm))"));
        assert!(floating.contains("mod.widgets.FloatingActionItemBase = #(FloatingActionItem::register_widget(vm))"));
        assert_eq!(floating.matches("set_type_default() do mod.widgets.FloatingActionBase").count(), 1);
        assert_eq!(floating.matches("set_type_default() do mod.widgets.FloatingActionItemBase").count(), 1);
    }
}

#[cfg(test)]
mod hamburger_menu_registration_tests {
    /// The menu is composed of a drawer, a popover, a nav list and a burger
    /// button, so it registers after all four, directly after the dialog a
    /// drawer is a preset of, which lands last.
    #[test]
    fn test_hamburger_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let hamburger = include_str!("hamburger_menu.rs");
        assert!(lib.contains("\npub mod hamburger_menu;"));
        assert!(crate::FRONT_LIB.contains("hamburger_menu::*"));
        crate::assert_registered_after(
            "crate::hamburger_menu::script_mod(vm);",
            &[
                "crate::dialog::script_mod(vm);",
                "crate::popover::script_mod(vm);",
                "crate::nav_list::script_mod(vm);",
                "crate::button::script_mod(vm);",
            ],
        );
        assert!(hamburger.contains("mod.widgets.HamburgerMenuBase = #(HamburgerMenu::register_widget(vm))"));
        assert_eq!(hamburger.matches("set_type_default() do mod.widgets.HamburgerMenuBase").count(), 1);
    }
}

#[cfg(test)]
mod pill_nav_registration_tests {
    /// The bar registers directly after the segmented control whose sliding
    /// pill it reuses, and after the popover whose trigger and placement
    /// words it takes, with one type default.
    #[test]
    fn test_pill_nav_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let pill = include_str!("pill_nav.rs");
        assert!(lib.contains("\npub mod pill_nav;"));
        assert!(crate::FRONT_LIB.contains("pill_nav::*"));
        crate::assert_registered_after(
            "crate::pill_nav::script_mod(vm);",
            &["crate::button_group::script_mod(vm);", "crate::popover::script_mod(vm);"],
        );
        assert!(pill.contains("mod.widgets.PillNavBase = #(PillNav::register_widget(vm))"));
        assert_eq!(pill.matches("set_type_default() do mod.widgets.PillNavBase").count(), 1);
    }
}

#[cfg(test)]
mod line_menu_registration_tests {
    /// The stack registers directly after the scroll views it follows, and
    /// after the nav list and pill nav whose ground and surface it draws
    /// with, with one type default.
    #[test]
    fn test_line_menu_is_registered_after_its_bases() {
        let lib = include_str!("lib.rs");
        let line = include_str!("line_menu.rs");
        assert!(lib.contains("\npub mod line_menu;"));
        assert!(crate::FRONT_LIB.contains("line_menu::*"));
        crate::assert_registered_after(
            "crate::line_menu::script_mod(vm);",
            &[
                "crate::scroll_fade::script_mod(vm);",
                "crate::nav_list::script_mod(vm);",
                "crate::pill_nav::script_mod(vm);",
            ],
        );
        assert!(line.contains("mod.widgets.LineMenuBase = #(LineMenu::register_widget(vm))"));
        assert_eq!(line.matches("set_type_default() do mod.widgets.LineMenuBase").count(), 1);
    }
}

