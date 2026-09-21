//! Lane D owns this file.
//!
//! The Fab theme: one token table every lane's `script_mod!` reaches through
//! `use mod.prelude.fab.*` as `fab.<token>`. desktop appearance,
//! Fab density. Nothing in the app hardcodes a color, size or duration
//! that has a token here.
//!
//! Registered before every other lane module (see `main.rs`), so
//! `mod.prelude.fab` (= the widgets prelude + `fab:` alias) is always in
//! scope for them.
//!
//! Surfaces and control geometry follow the shared desktop style; viewport
//! content and semantic axis/status colors remain local to Fab.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.fab = {
        // ---- surfaces ----
        color_area: theme.color_bg_app
        color_editor: theme.color_bg_container
        color_editor_alt: theme.color_bg_odd
        color_header: theme.color_fg_app
        color_panel: theme.color_bg_container
        color_panel_sub: theme.color_inset
        color_popover: theme.color_bg_container
        color_popover_border: theme.color_bevel_outset_2
        color_topbar: theme.color_fg_app
        color_statusbar: theme.color_fg_app
        color_border: theme.color_bevel_inset_2
        color_border_light: theme.color_bevel_outset_1
        color_row_even: theme.color_bg_even
        color_row_odd: theme.color_bg_odd
        color_row_hover: theme.color_outset_hover
        color_row_active: theme.color_bg_highlight
        color_input: theme.color_inset
        color_input_hover: theme.color_inset_hover
        color_input_active: theme.color_inset_active
        color_button: theme.color_outset
        color_button_hover: theme.color_outset_hover
        color_button_down: theme.color_outset_down
        color_button_active: theme.color_ctrl_selected
        color_scrollbar: theme.color_bevel_inset_2
        color_scrollbar_hover: theme.color_text_disabled

        // ---- text ----
        color_text: theme.color_text
        color_text_dim: theme.color_text_disabled
        color_text_muted: theme.color_text_disabled
        color_text_active: theme.color_text_active
        color_text_header: theme.color_text
        color_text_on_accent: theme.color_text_on_accent

        // ---- accents ----
        color_accent: theme.color_focus
        color_accent_hover: theme.color_focus
        color_accent_dim: theme.color_bg_highlight
        color_selection_bg: theme.color_bg_highlight
        color_focus_ring: theme.color_focus
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
        color_float: theme.color_bg_container
        color_float_border: theme.color_bevel_outset_2
        // Checkboxes / toggles, Fab's inset well and its filled state.
        color_toggle_off: theme.color_inset
        color_toggle_on: theme.color_ctrl_selected
        color_toggle_mark: #xffffff
        // Drag-numeric field: inset well with an accent progress fill.
        color_num: theme.color_inset
        color_num_hover: theme.color_inset_hover
        color_num_fill: theme.color_bg_highlight
        color_num_arrow: theme.color_text
        // Menus.
        color_menu_row_hover: theme.color_bg_highlight
        color_menu_sep: theme.color_bevel_inset_2
        color_shadow: #x00000059
        // Pie menu.
        color_pie_bg: theme.color_bg_container
        color_pie_wedge: theme.color_outset
        color_pie_wedge_hot: theme.color_focus

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
        radius: theme.corner_radius
        radius_lg: theme.container_corner_radius
        border: 1.0
        icon_size: 16.0
        icon_size_sm: 12.0
        icon_size_lg: 20.0
        // 10 px chevron on the icon grid, 4 px from the label — optical
        // match to the label's cap-height midpoint, not a vertical nudge.
        chevron_size: 10.0
        chevron_gap: 4.0
        gizmo_size: 96.0
        // Pie menu geometry is a fixed size, not a corner-rounding token:
        // `theme.corner_radius` is 0 under Omarchy, NeXTSTEP and Windows 2000
        // and the pie collapsed. Keep in step with `ui/pie.rs` (outer 96, inner 30).
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
