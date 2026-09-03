//! Flow's application palette. Keeping the design-pass colours here lets
//! every panel, face and canvas shader share one named set without changing
//! the approved appearance.

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

    mod.theme.flow_window = #x0f0f10
    mod.theme.flow_grid_a = #x111111
    mod.theme.flow_grid_b = #x161616
    mod.theme.flow_surface = #x1c1c1f
    mod.theme.flow_surface_deep = #x151517
    mod.theme.flow_surface_translucent = #x161618e8
    mod.theme.flow_surface_hover = #x232327
    mod.theme.flow_surface_raised = #x2a2a30
    mod.theme.flow_surface_input = #x3a3a40
    mod.theme.flow_edge = #x2b2b30
    mod.theme.flow_edge_soft = #x33333a
    mod.theme.flow_divider = #x26262c
    mod.theme.flow_shadow = #0005
    mod.theme.flow_clear = #0000
    mod.theme.flow_scrim = #000c

    mod.theme.flow_text = #xe8e8ec
    mod.theme.flow_text_body = #xd0d0d4
    mod.theme.flow_text_code = #xc8c8cc
    mod.theme.flow_text_chip = #xdddddd
    mod.theme.flow_text_port = #x9a9aa2
    mod.theme.flow_text_muted = #x8a8a92
    mod.theme.flow_text_subtle = #x6e6e76
    mod.theme.flow_text_empty = #x6a6a72
    mod.theme.flow_text_hint = #x5e5e66
    mod.theme.flow_text_grip = #x4a4a52
    mod.theme.flow_text_port_connected = #xc7c7d1
    mod.theme.flow_text_port_open = #x80808c
    mod.theme.flow_text_white = #xffffff

    mod.theme.flow_accent = #xff5c39
    mod.theme.flow_accent_hover = #x6b5148
    mod.theme.flow_highlight = #x5a9cff
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
