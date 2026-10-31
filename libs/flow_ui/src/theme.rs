//! Flow chrome names (the graph canvas and its faces), bound to the active Makepad theme.
//!
//! The `flow_*` names are evaluated from `mod.theme` in `script_mod`. Every
//! theme change -- a pick in an app's theme picker, the Tweaker, the window
//! manager's system theme arriving as a `makepad_style` event -- is a style
//! reload, and a style reload runs `App::script_mod` again, so these names
//! and every template that reads them are rebuilt from the theme in force.
//! `App::script_mod` owns the order: after `makepad_widgets::script_mod`.
//!
//! Accent, focus, on-accent ink, and the `color_syntax_*` / `color_map_kind_*`
//! hues are optional. Dark, light, and every desktop sheet define them, and
//! the alias uses that role. The skeleton base does not, so those aliases
//! fall through to a semantic role the skeleton does define. Nothing but
//! `flow_*` is written onto `mod.theme`.
//!
//! Ordinary secondary ink is `color_label_outer`. `color_text_disabled` is
//! only for hints and inactive marks, where a disabled tone is the meaning.
//! `color_elevation_1` is the drop-shadow ink (translucent black on dark,
//! light, and skeleton); `color_shadow` is the opaque bevel step.

use makepad_widgets::makepad_script::trap::NoTrap;
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Status ink for the connection dot and the queue dots.
///
/// Reads the active theme in `vm`. `running` / `ready` / `queued` are
/// `color_info`, `done` / `ok` / `idle` are `color_success`, `failed` /
/// `error` are `color_error`, `waiting` is `color_warning`, and every other
/// state, including `cancelled` and `skipped`, is `color_text_disabled`.
/// Those five roles exist on dark, light, and skeleton. A theme that lacks
/// the one this state needs is logged once and draws nothing, rather than a
/// stand-in gray.
pub fn state_color(vm: &mut ScriptVm, state: &str) -> Vec4f {
    let role = match state {
        "running" | "ready" | "queued" => "color_info",
        "done" | "ok" | "idle" => "color_success",
        "failed" | "error" => "color_error",
        "waiting" => "color_warning",
        _ => "color_text_disabled",
    };
    let Some(packed) = theme_color(vm, role) else {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log!("flow theme: status role {role} is missing");
        }
        return vec4(0.0, 0.0, 0.0, 0.0);
    };
    let channel = |shift: u32| ((packed >> shift) & 0xff) as f32 / 255.0;
    vec4(channel(24), channel(16), channel(8), channel(0))
}

/// A color stored on `mod.theme`, if that key is present and is a color.
/// A missing key is not an error: callers decide whether the absence is a
/// known optional role or a broken theme.
fn theme_color(vm: &mut ScriptVm, role: &str) -> Option<u32> {
    let theme = vm.module(id!(theme));
    vm.bx.heap_ref().value(theme, LiveId::from_str(role).into(), NoTrap)
        .as_color()
}

/// Roles the skeleton base does not define. Dark, light, and every desktop
/// sheet do define them, and then the alias names that role. Skeleton falls
/// through to the second name, which all three bases define. The assignment
/// is still `theme.<role>`, so a theme missing both names reports that
/// lookup instead of receiving an invented color.
fn bind_optional_aliases(vm: &mut ScriptVm) {
    const OPTIONAL: &[(&str, &str, &str)] = &[
        ("flow_text_code", "color_syntax_ident", "color_text"),
        ("flow_text_white", "color_text_on_accent", "color_on_primary"),
        ("flow_accent", "color_ctrl_selected", "color_primary"),
        ("flow_highlight", "color_focus", "color_highlight"),
        ("flow_chat", "color_map_kind_trait", "color_info"),
        ("flow_generation", "color_map_kind_const", "color_warning"),
        ("flow_function", "color_map_kind_fn", "color_makepad"),
        ("flow_http", "color_syntax_keyword", "color_info"),
        ("flow_input", "color_syntax_type", "color_success"),
        ("flow_port_text", "color_syntax_ident", "color_text"),
        ("flow_port_image", "color_syntax_string", "color_warning"),
        ("flow_port_audio", "color_map_kind_trait", "color_info"),
        ("flow_port_video", "color_syntax_macro", "color_makepad"),
        ("flow_port_mesh", "color_map_kind_enum", "color_success"),
        ("flow_port_json", "color_map_kind_fn", "color_warning"),
        ("flow_port_list", "color_map_kind_queue", "color_primary"),
        ("flow_port_bytes", "color_map_kind_file", "color_label_outer"),
    ];
    let mut code = String::from("use mod.prelude.widgets_internal.*\n");
    for (flow, preferred, fallback) in OPTIONAL {
        let source = if theme_color(vm, preferred).is_some() {
            *preferred
        } else {
            *fallback
        };
        code.push_str(&format!("mod.theme.{flow} = theme.{source}\n"));
    }
    // A script that ends on an assignment drops that assignment.
    code.push_str("true\n");
    eval_theme(vm, "flow_theme_optional", code);
}

const REQUIRED_ALIASES: &str = r#"
use mod.prelude.widgets_internal.*
use mod.math.*

// Panels and graph cells are complete surfaces. The stock inset/outset
// roles are translucent overlays and need an opaque widget underneath;
// using them here lets the graph show through the inspector and cards.
mod.theme.flow_window = theme.color_surface
mod.theme.flow_grid_a = theme.color_surface_container_lowest
mod.theme.flow_grid_b = mix(theme.color_surface_container_lowest, theme.color_surface_container_high, 0.15)
mod.theme.flow_surface = theme.color_surface_container
mod.theme.flow_surface_deep = theme.color_surface_container_lowest
mod.theme.flow_surface_translucent = theme.color_surface_container
mod.theme.flow_surface_hover = theme.color_surface_container_high
mod.theme.flow_surface_raised = theme.color_surface_container_high
mod.theme.flow_surface_input = theme.color_surface_container_lowest
mod.theme.flow_edge = theme.color_bevel_outset_1
mod.theme.flow_edge_soft = theme.color_bevel_outset_2
mod.theme.flow_divider = theme.color_bevel_inset_1
mod.theme.flow_shadow = theme.color_elevation_1
mod.theme.flow_clear = #0000
// `color_scrim` is opaque. Scale it so the veil stays a veil.
mod.theme.flow_scrim = theme.color_scrim * 0.8

mod.theme.flow_text = theme.color_text
mod.theme.flow_text_body = theme.color_label_outer
mod.theme.flow_text_chip = theme.color_text
mod.theme.flow_text_port = theme.color_label_outer
mod.theme.flow_text_muted = theme.color_label_outer
mod.theme.flow_text_subtle = theme.color_label_outer
mod.theme.flow_text_empty = theme.color_text_disabled
mod.theme.flow_text_hint = theme.color_text_disabled
mod.theme.flow_text_grip = theme.color_icon_disabled
mod.theme.flow_text_port_connected = theme.color_text
mod.theme.flow_text_port_open = theme.color_text_disabled

mod.theme.flow_accent_hover = theme.color_bg_highlight
mod.theme.flow_success = theme.color_success
mod.theme.flow_error = theme.color_error
mod.theme.flow_waiting = theme.color_warning

mod.theme.flow_state_running = theme.color_info
mod.theme.flow_state_idle = theme.color_text_disabled
mod.theme.flow_badge_input = theme.color_surface_container_lowest
mod.theme.flow_badge_output = theme.color_surface_container_lowest
mod.theme.flow_badge_chat = theme.color_surface_container_lowest
mod.theme.flow_badge_generation = theme.color_surface_container_lowest
mod.theme.flow_badge_http = theme.color_surface_container_lowest
mod.theme.flow_badge_waiting = theme.color_surface_container_lowest
true
"#;

fn eval_theme(vm: &mut ScriptVm, module_path: &str, code: String) {
    vm.eval(ScriptMod::new(&(env!("CARGO_MANIFEST_DIR")), &(module_path), "src/theme.rs", 0, 0, &(code), &[]));
}

pub fn script_mod(vm: &mut ScriptVm) {
    eval_theme(vm, "flow_theme", REQUIRED_ALIASES.into());
    // The house theme declares its window chrome: traffic lights at the caption's left,
    // on every host (the system's own on macOS). A desktop sheet picked (or handed down
    // by the window manager) keeps the host's chrome (`WindowChrome.Follow`).
    if makepad_widgets::desktop_style::current_name(vm).is_none() {
        eval_theme(vm, "flow_theme_chrome", "mod.widgets.Window.chrome = mod.widgets.WindowChrome.Lights".into());
    }
    // After the required script has finished, so this eval does not interrupt it.
    bind_optional_aliases(vm);
}
