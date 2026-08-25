//! Lane D owns this file.
//!
//! The Fab theme: one token table every lane's `script_mod!` reaches through
//! `use mod.prelude.fab.*` as `fab.<token>`. Fab default-dark grade,
//! Fab density. Nothing in the app hardcodes a color, size or duration
//! that has a token here.
//!
//! Registered before every other lane module (see `main.rs`), so
//! `mod.prelude.fab` (= the widgets prelude + `fab:` alias) is always in
//! scope for them.
//!
//! **Corner radii are `Sdf2d.box` arguments**, and `Sdf2d.box` draws a corner
//! that reads as *twice* the number you give it (memory
//! `sdf-box-corner-radius-convention`). `radius: 2.0` is therefore Fab's
//! 4 px control corner, not an 8 px pill. Same for `border_radius` on the
//! stock shaders — they feed the same `sdf.box`.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.fab = {
        // ---- surfaces (Fab default-dark grade) ----
        color_area: #x303030
        color_editor: #x232323
        color_editor_alt: #x282828
        color_header: #x3d3d3d
        color_panel: #x3d3d3d
        color_panel_sub: #x353535
        color_popover: #x1a1a1a
        color_popover_border: #x545454
        color_topbar: #x181818
        color_statusbar: #x181818
        color_border: #x161616
        color_border_light: #x4a4a4a
        color_row_even: #x282828
        color_row_odd: #x2b2b2b
        color_row_hover: #x3a3a3a
        color_row_active: #x334d80
        color_input: #x1d1d1d
        color_input_hover: #x232323
        color_input_active: #x161616
        color_button: #x545454
        color_button_hover: #x656565
        color_button_down: #x4a4a4a
        color_button_active: #x5680c2
        color_scrollbar: #x424242
        color_scrollbar_hover: #x5a5a5a

        // ---- text ----
        color_text: #xe6e6e6
        color_text_dim: #x9a9a9a
        color_text_muted: #x707070
        color_text_active: #xffffff
        color_text_header: #xd0d0d0
        color_text_on_accent: #xffffff

        // ---- accents ----
        color_accent: #x5680c2
        color_accent_hover: #x6b93d4
        color_accent_dim: #x3c5a8a
        color_selection_bg: #x334d80
        color_focus_ring: #x7aa2e8
        color_warning: #xe0a020
        color_error: #xe04040
        color_ok: #x5cb85c

        // ---- viewport ----
        color_vp_bg_top: #x3f3f3f
        color_vp_bg_bottom: #x2b2b2b
        color_vp_grid: #x505050
        color_vp_grid_major: #x5a5a5a
        color_vp_axis_x: #xff3352
        color_vp_axis_y: #x8bdc00
        color_vp_axis_z: #x2890ff
        color_vp_select: #xe96a2b
        color_vp_select_dim: #xa5461d
        color_vp_hover: #xffffff
        color_vp_text: #xffffff
        color_vp_measure: #xffd05a
        color_vp_section: #x39c5ff
        color_vp_cap: #x8a8a8a
        color_vp_wire: #x000000
        color_vp_ink: #x1a1a1a
        color_vp_paper: #xf4f2ee

        // ---- lane D chrome extras ----
        // Floating chrome over the viewport (T toolbar, N sidebar, HUD).
        color_float: #x2f2f2fe6
        color_float_border: #x14141480
        // Checkboxes / toggles, Fab's inset well and its filled state.
        color_toggle_off: #x1d1d1d
        color_toggle_on: #x5680c2
        color_toggle_mark: #xffffff
        // Drag-numeric field: inset well with an accent progress fill.
        color_num: #x1d1d1d
        color_num_hover: #x2a2a2a
        color_num_fill: #x3c5a8a
        color_num_arrow: #xb0b0b0
        // Menus.
        color_menu_row_hover: #x3d5a8c
        color_menu_sep: #x353535
        color_shadow: #x00000059
        // Pie menu.
        color_pie_bg: #x1a1a1acc
        color_pie_wedge: #x3a3a3add
        color_pie_wedge_hot: #x5680c2ee

        // ---- density (Fab at 1x) ----
        row_height: 20.0
        row_height_sm: 18.0
        header_height: 26.0
        topbar_height: 26.0
        statusbar_height: 22.0
        tab_strip_width: 24.0
        toolbar_width: 34.0
        sidebar_width: 260.0
        prop_label_width: 92.0
        menu_row_height: 22.0
        menu_min_width: 190.0
        corner_zone: 12.0
        splitter_size: 3.0
        pad_1: 4.0
        pad_2: 6.0
        pad_3: 10.0
        // Sdf2d.box arguments — the drawn corner reads as twice these.
        radius: 2.0
        radius_lg: 3.0
        border: 1.0
        icon_size: 16.0
        icon_size_sm: 12.0
        icon_size_lg: 20.0
        // 10 px chevron on the icon grid, 4 px from the label — optical
        // match to the label's cap-height midpoint, not a vertical nudge.
        chevron_size: 10.0
        chevron_gap: 4.0
        gizmo_size: 96.0
        pie_radius: 96.0
        pie_inner: 30.0
        swatch_width: 46.0

        // ---- type (points; px = pt * 4/3, so 8.5pt ≈ 11px like Fab) ----
        font_size_ui: 8.5
        font_size_small: 7.5
        font_size_header: 9.0
        font_size_title: 10.5
        font_size_vp: 9.0

        // ---- motion (seconds), eases only, no bounce ----
        anim_fast: 0.10
        anim_normal: 0.15
        anim_slow: 0.25
        tooltip_delay: 0.5
    }

    mod.prelude.fab = {
        ..mod.prelude.widgets,
        fab: mod.fab
    }
}
