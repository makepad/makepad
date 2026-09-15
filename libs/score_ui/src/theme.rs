//! Score product tokens: quiet warm paper in pianist mode and compact,
//! restrained professional chrome when the editor is disclosed.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.score_theme = {
        color_surround: theme.color_bg_app
        color_surround_soft: theme.color_bg_container
        color_paper: #xf7f4ec
        color_paper_shadow: #x00000036
        color_ink: #x171713
        color_ink_soft: #x4a4943

        color_chrome: theme.color_fg_app
        color_chrome_raised: theme.color_outset
        color_panel: theme.color_bg_container
        color_panel_alt: theme.color_bg_odd
        color_input: theme.color_inset
        color_border: theme.color_bevel_inset_2
        color_border_light: theme.color_bevel_outset_1
        color_row_hover: theme.color_outset_hover
        color_row_active: theme.color_bg_highlight
        color_button: theme.color_outset
        color_button_hover: theme.color_outset_hover
        color_button_down: theme.color_outset_down
        color_accent: theme.color_focus
        color_accent_hover: theme.color_focus
        color_accent_dim: theme.color_bg_highlight
        color_selection: #xff8000
        color_annotation: #xd4913b
        color_ok: #x6da77b
        color_warning: #xd2a445
        color_error: #xd66a62

        color_text: theme.color_text
        color_text_dim: theme.color_text_disabled
        color_text_muted: theme.color_text_disabled
        color_text_on_accent: theme.color_text_on_accent
        color_float: theme.color_bg_container
        color_float_border: theme.color_bevel_outset_2

        menu_height: 27.0
        toolbar_height: 38.0
        transport_height: 34.0
        panel_width: 260.0
        inspector_width: 292.0
        row_height: 24.0
        row_height_small: 20.0
        pad_1: 4.0
        pad_2: 7.0
        pad_3: 11.0
        pad_4: 16.0
        radius: theme.corner_radius
        radius_large: theme.container_corner_radius
        font_ui: 8.5
        font_small: 7.5
        font_header: 9.2
        font_title: 12.0
        anim_fast: 0.10
        anim_normal: 0.16
        anim_page: 0.24
    }

    mod.prelude.score = {
        ..mod.prelude.widgets,
        score: mod.score_theme
    }
}

