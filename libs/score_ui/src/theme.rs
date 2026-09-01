//! Score product tokens: quiet warm paper in pianist mode and compact,
//! restrained professional chrome when the editor is disclosed.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.score_theme = {
        color_surround: #x20211f
        color_surround_soft: #x292a27
        color_paper: #xf7f4ec
        color_paper_shadow: #x00000036
        color_ink: #x171713
        color_ink_soft: #x4a4943

        color_chrome: #x1c1d1b
        color_chrome_raised: #x292a27
        color_panel: #x242522
        color_panel_alt: #x20211f
        color_input: #x171815
        color_border: #x0f100e
        color_border_light: #x41423d
        color_row_hover: #x343630
        color_row_active: #x314c62
        color_button: #x343630
        color_button_hover: #x464941
        color_button_down: #x252621
        color_accent: #xc86b4a
        color_accent_hover: #xda7d5a
        color_accent_dim: #x704331
        color_selection: #xff8000
        color_annotation: #xd4913b
        color_ok: #x6da77b
        color_warning: #xd2a445
        color_error: #xd66a62

        color_text: #xe8e5dc
        color_text_dim: #xaaa79e
        color_text_muted: #x74736d
        color_text_on_accent: #xffffff
        color_float: #x181916e8
        color_float_border: #x5a5b54a0

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
        radius: 3.0
        radius_large: 6.0
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

