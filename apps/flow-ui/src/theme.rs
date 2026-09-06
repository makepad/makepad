//! Flow's semantic colors and aliases into the active desktop theme.

use makepad_widgets::*;

pub fn state_color(state: &str) -> Vec4f {
    match state {
        "running" | "ready" | "queued" => vec4(0.35, 0.62, 1.0, 1.0),
        "done" | "ok" | "idle" => vec4(0.30, 0.77, 0.42, 1.0),
        "failed" | "error" => vec4(0.95, 0.43, 0.43, 1.0),
        "waiting" => vec4(0.95, 0.76, 0.3, 1.0),
        "cancelled" | "skipped" => vec4(0.55, 0.55, 0.58, 1.0),
        _ => vec4(0.45, 0.45, 0.5, 1.0),
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.theme.flow_window = mod.theme.color_bg_app
    mod.theme.flow_grid_a = mod.theme.color_bg_app
    mod.theme.flow_grid_b = mod.theme.color_bg_odd
    mod.theme.flow_surface = mod.theme.color_bg_container
    mod.theme.flow_surface_deep = mod.theme.color_fg_app
    mod.theme.flow_surface_translucent = mod.theme.color_bg_container
    mod.theme.flow_surface_hover = mod.theme.color_outset_hover
    mod.theme.flow_surface_raised = mod.theme.color_outset
    mod.theme.flow_surface_input = mod.theme.color_inset
    mod.theme.flow_edge = mod.theme.color_bevel_inset_2
    mod.theme.flow_edge_soft = mod.theme.color_bevel_outset_1
    mod.theme.flow_divider = mod.theme.color_bevel_inset_2
    mod.theme.flow_shadow = #0005
    mod.theme.flow_clear = #0000
    mod.theme.flow_scrim = #000c

    mod.theme.flow_text = mod.theme.color_text
    mod.theme.flow_text_body = mod.theme.color_text
    mod.theme.flow_text_code = mod.theme.color_text
    mod.theme.flow_text_chip = mod.theme.color_text
    mod.theme.flow_text_port = mod.theme.color_text_disabled
    mod.theme.flow_text_muted = mod.theme.color_text_disabled
    mod.theme.flow_text_subtle = mod.theme.color_text_disabled
    mod.theme.flow_text_empty = mod.theme.color_text_disabled
    mod.theme.flow_text_hint = mod.theme.color_text_disabled
    mod.theme.flow_text_grip = mod.theme.color_text_disabled
    mod.theme.flow_text_port_connected = mod.theme.color_text
    mod.theme.flow_text_port_open = mod.theme.color_text_disabled

    mod.theme.flow_accent = mod.theme.color_focus
    mod.theme.flow_accent_hover = mod.theme.color_bg_highlight
    mod.theme.flow_highlight = mod.theme.color_focus
    mod.theme.flow_success = #x4cc46a
    mod.theme.flow_error = #xf26d6d
    mod.theme.flow_waiting = #xf2c14e
    mod.theme.flow_chat = #x8b7cf6
    mod.theme.flow_generation = #xf2994a
    mod.theme.flow_function = #xe6c04a
    mod.theme.flow_http = #x4ac2e6
    mod.theme.flow_input = #x3fb9a8

    mod.theme.flow_port_text = #xd8e6ff
    mod.theme.flow_port_image = #xffe0c8
    mod.theme.flow_port_audio = #xe6d8ff
    mod.theme.flow_port_video = #xffd8e6
    mod.theme.flow_port_mesh = #xd8f2d8
    mod.theme.flow_port_json = #xfff2c8
    mod.theme.flow_port_list = #xcce680
    mod.theme.flow_port_bytes = #xd0d0d0

    mod.theme.flow_state_running = #x599eff
    mod.theme.flow_state_idle = #x737380
    mod.theme.flow_badge_input = #x1f3a37
    mod.theme.flow_badge_output = #x1f3a26
    mod.theme.flow_badge_chat = #x2b2748
    mod.theme.flow_badge_generation = #x40301e
    mod.theme.flow_badge_http = #x1e363d
    mod.theme.flow_badge_waiting = #x3d3620
}
