use crate::makepad_platform::*;

script_mod! {
    use mod.math.*
    use mod.pod.*
    use mod.text.*
    use mod.turtle.*
    use mod.res.*

    mod.themes.dark = {
        let theme = me
        // GLOBAL PARAMETERS
        color_contrast: 1.0
        color_tint: #0000ff
        color_tint_amount: 0.0
        space_factor: 6. // Increase for a less dense layout
        corner_radius: 2.5
        beveling: 0.75
        font_size_base: 10.
        font_size_contrast: 2.5 // Greater values = greater font-size steps between font-formats (i.e. from H3 to H2)

        // DIMENSIONS
        space_1: 0.5 * theme.space_factor
        space_2: 1.0 * theme.space_factor
        space_3: 1.5 * theme.space_factor

        mspace_1: Inset{top: theme.space_1, right: theme.space_1, bottom: theme.space_1, left: theme.space_1}
        mspace_h_1: Inset{top: 0., right: theme.space_1, bottom: 0., left: theme.space_1}
        mspace_v_1: Inset{top: theme.space_1, right: 0., bottom: theme.space_1, left: 0.}
        mspace_2: Inset{top: theme.space_2, right: theme.space_2, bottom: theme.space_2, left: theme.space_2}
        mspace_h_2: Inset{top: 0., right: theme.space_2, bottom: 0., left: theme.space_2}
        mspace_v_2: Inset{top: theme.space_2, right: 0., bottom: theme.space_2, left: 0.}
        mspace_3: Inset{top: theme.space_3, right: theme.space_3, bottom: theme.space_3, left: theme.space_3}
        mspace_h_3: Inset{top: 0., right: theme.space_3, bottom: 0., left: theme.space_3}
        mspace_v_3: Inset{top: theme.space_3, right: 0., bottom: theme.space_3, left: 0.}

        data_item_height: 7.75 * theme.space_1
        data_icon_width: 2.6 * theme.space_2
        data_icon_height: 3.6 * theme.space_2

        container_corner_radius: theme.corner_radius * 2.
        textselection_corner_radius: theme.corner_radius * 0.5
        tab_height: 6 * theme.space_factor
        tab_flat_height: 5.5 * theme.space_factor
        splitter_horizontal: 16.0
        splitter_size: 5.0
        splitter_min_horizontal: theme.tab_height
        splitter_max_horizontal: theme.tab_height + theme.splitter_size
        splitter_min_vertical: theme.splitter_horizontal
        splitter_max_vertical: theme.splitter_horizontal + theme.splitter_size
        dock_border_size: 0.0

        // COLOR PALETTE
        color_w: #FFFFFFFF
        color_w_h: #FFFFFF00
        color_b: #000000FF
        color_b_h: #00000000

        color_white: mix(theme.color_w, #FFFFFF00, pow(0.1, theme.color_contrast))
        color_u_6: mix(theme.color_w, theme.color_w_h, pow(0.2, theme.color_contrast))
        color_u_5: mix(theme.color_w, theme.color_w_h, pow(0.35, theme.color_contrast))
        color_u_4: mix(theme.color_w, theme.color_w_h, pow(0.6, theme.color_contrast))
        color_u_3: mix(theme.color_w, theme.color_w_h, pow(0.75, theme.color_contrast))
        color_u_2: mix(theme.color_w, theme.color_w_h, pow(0.85, theme.color_contrast))
        color_u_15: mix(theme.color_w, theme.color_w_h, pow(0.9, theme.color_contrast))

        color_u_1: mix(theme.color_w, theme.color_w_h, pow(0.95, theme.color_contrast))
        color_u_hidden: theme.color_w_h

        color_d_hidden: theme.color_b_h
        color_d_025: mix(theme.color_b, theme.color_b_h, pow(0.95, theme.color_contrast))
        color_d_05: mix(theme.color_b, theme.color_b_h, pow(0.9, theme.color_contrast))
        color_d_1: mix(theme.color_b, theme.color_b_h, pow(0.85, theme.color_contrast))
        color_d_2: mix(theme.color_b, theme.color_b_h, pow(0.75, theme.color_contrast))
        color_d_3: mix(theme.color_b, theme.color_b_h, pow(0.6, theme.color_contrast))
        color_d_4: mix(theme.color_b, theme.color_b_h, pow(0.4, theme.color_contrast))
        color_d_5: mix(theme.color_b, theme.color_b_h, pow(0.25, theme.color_contrast))
        color_black: mix(theme.color_b, theme.color_b_h, pow(0.1, theme.color_contrast))

        color_bg_app: mix(
            theme.color_b * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            theme.color_w * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            pow(0.3, theme.color_contrast))
        color_fg_app: mix(
            theme.color_b * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            theme.color_w * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            pow(0.36, theme.color_contrast))
        color_opaque_u_6: mix(theme.color_fg_app, #F, 0.8)
        color_opaque_u_5: mix(theme.color_fg_app, #F, 0.7)
        color_opaque_u_4: mix(theme.color_fg_app, #F, 0.5)
        color_opaque_u_3: mix(theme.color_fg_app, #F, 0.35)
        color_opaque_u_2: mix(theme.color_fg_app, #F, 0.25)
        color_opaque_u_1: mix(theme.color_fg_app, #F, 0.15)

        color_opaque_d_1: mix(theme.color_fg_app, #0, 0.15)
        color_opaque_d_2: mix(theme.color_fg_app, #0, 0.25)
        color_opaque_d_3: mix(theme.color_fg_app, #0, 0.45)
        color_opaque_d_4: mix(theme.color_fg_app, #0, 0.6)
        color_opaque_d_5: mix(theme.color_fg_app, #0, 0.75)

        // BASICS
        color_makepad: #FF5C39FF

        color_shadow: theme.color_d_3
        color_shadow_focus: theme.color_d_5
        color_shadow_disabled: theme.color_opaque_d_3
        color_shadow_flat: theme.color_d_2
        color_flat_focus: theme.color_u_2
        color_shadow_flat_disabled: theme.color_opaque_d_3
        color_light: theme.color_u_2
        color_light_hover: theme.color_opaque_u_2
        color_light_focus: theme.color_opaque_u_2
        color_light_disabled: theme.color_opaque_u_1

        color_bg_highlight: theme.color_u_1
        color_bg_unfocussed: theme.color_bg_highlight * 0.85
        color_app_caption_bar: theme.color_d_hidden
        color_drag_quad: theme.color_u_5
        color_drag_target_preview: theme.color_u_2

        color_cursor: theme.color_white
        color_cursor_focus: theme.color_white
        color_cursor_empty: theme.color_white
        color_cursor_disabled: theme.color_u_hidden
        color_cursor_border: theme.color_white

        color_highlight: theme.color_u_1
        color_text_cursor: theme.color_white
        color_bg_highlight_inline: theme.color_d_3

        color_text: theme.color_u_5
        color_text_val: theme.color_u_3
        color_text_hl: theme.color_text
        color_text_hover: theme.color_text
        color_text_on_accent: #xffffff
        color_text_active: theme.color_text
        color_focus: #x7aa2f7
        color_ctrl_selected: theme.color_focus
        color_text_focus: theme.color_text
        color_text_down: theme.color_text
        color_text_disabled: theme.color_u_1
        color_text_placeholder: theme.color_u_4
        color_text_placeholder_hover: theme.color_u_4
        color_text_meta: theme.color_u_4

        color_label_inner: theme.color_u_5
        color_label_inner_down: theme.color_u_3
        color_label_inner_drag: theme.color_label_inner_down
        color_label_inner_hover: theme.color_label_inner
        color_label_inner_focus: theme.color_label_inner
        color_label_inner_active: theme.color_label_inner
        color_label_inner_inactive: theme.color_u_4
        color_label_inner_disabled: theme.color_u_2

        color_label_outer: theme.color_u_5
        color_label_outer_off: theme.color_u_3
        color_label_outer_down: theme.color_label_outer

        color_label_outer_drag: theme.color_label_outer
        color_label_outer_hover: theme.color_label_outer
        color_label_outer_focus: theme.color_label_outer
        color_label_outer_active: theme.color_label_outer
        color_label_outer_active_focus: theme.color_label_outer
        color_label_outer_disabled: theme.color_u_2

        color_bg_container: theme.color_d_3 * 0.8
        color_bg_even: theme.color_bg_container * 0.875
        color_bg_odd: theme.color_bg_container * 1.125

        color_bevel: theme.color_shadow_flat
        color_bevel_hover: theme.color_flat_focus
        color_bevel_focus: theme.color_bevel_hover
        color_bevel_active: theme.color_bevel
        color_bevel_empty: theme.color_bevel
        color_bevel_down: theme.color_bevel_hover
        color_bevel_drag: theme.color_bevel_hover
        color_bevel_disabled: theme.color_shadow_flat_disabled

        color_bevel_inset_2: theme.color_light
        color_bevel_inset_2_hover: theme.color_light_focus
        color_bevel_inset_2_focus: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_active: theme.color_bevel_inset_2
        color_bevel_inset_2_empty: theme.color_bevel_inset_2
        color_bevel_inset_2_down: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_drag: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_disabled: theme.color_light_disabled

        color_bevel_inset_1: theme.color_shadow
        color_bevel_inset_1_hover: theme.color_bevel_inset_1
        color_bevel_inset_1_focus: theme.color_bevel_inset_2_hover
        color_bevel_inset_1_active: theme.color_bevel_inset_1
        color_bevel_inset_1_empty: theme.color_bevel_inset_1
        color_bevel_inset_1_down: theme.color_bevel_inset_1
        color_bevel_inset_1_drag: theme.color_bevel_inset_1
        color_bevel_inset_1_disabled: theme.color_shadow_disabled

        color_bevel_outset_1: theme.color_light
        color_bevel_outset_1_hover: theme.color_light_hover
        color_bevel_outset_1_focus: theme.color_bevel_outset_1_hover
        color_bevel_outset_1_active: theme.color_light
        color_bevel_outset_1_down: theme.color_shadow
        color_bevel_outset_1_drag: theme.color_bevel_outset_1_down
        color_bevel_outset_1_disabled: theme.color_light_disabled

        color_bevel_outset_2: theme.color_shadow
        color_bevel_outset_2_hover: theme.color_shadow
        color_bevel_outset_2_focus: theme.color_shadow_focus
        color_bevel_outset_2_active: theme.color_shadow
        color_bevel_outset_2_down: theme.color_light
        color_bevel_outset_2_drag: theme.color_bevel_outset_2_down
        color_bevel_outset_2_disabled: theme.color_shadow_disabled

        // Background of textinputs, radios, checkboxes etc.
        color_inset: theme.color_d_1
        color_inset_hover: theme.color_inset
        color_inset_down: theme.color_inset_hover
        color_inset_active: theme.color_inset_hover
        color_inset_focus: theme.color_inset_hover
        color_inset_drag: theme.color_inset
        color_inset_disabled: theme.color_d_025
        color_inset_empty: theme.color_inset

        color_inset_1: theme.color_d_3
        color_inset_1_hover: theme.color_inset_1
        color_inset_1_down: theme.color_inset_1_hover
        color_inset_1_active: theme.color_inset_1_hover
        color_inset_1_focus: theme.color_inset_1_hover
        color_inset_1_drag: theme.color_inset_1
        color_inset_1_disabled: theme.color_d_025
        color_inset_1_empty: theme.color_inset_1

        color_inset_2: theme.color_d_05
        color_inset_2_hover: theme.color_inset_2
        color_inset_2_down: theme.color_inset_2_hover
        color_inset_2_active: theme.color_inset_2_hover
        color_inset_2_focus: theme.color_inset_2_hover
        color_inset_2_drag: theme.color_inset_2
        color_inset_2_empty: theme.color_d_hidden
        color_inset_2_disabled: theme.color_d_025

        // WIDGET COLORS
        color_outset: theme.color_u_15
        color_outset_down: theme.color_d_1
        color_outset_hover: theme.color_u_2
        color_outset_active: theme.color_u_3
        color_outset_focus: theme.color_outset
        color_outset_drag: theme.color_u_2
        color_outset_disabled: theme.color_u_1
        color_outset_inactive: theme.color_d_hidden

        color_outset_1: theme.color_u_1
        color_outset_1_down: theme.color_d_2
        color_outset_1_drag: theme.color_outset_1_down
        color_outset_1_hover: theme.color_u_2
        color_outset_1_active: theme.color_u_4
        color_outset_1_focus: theme.color_outset_1
        color_outset_1_disabled: theme.color_u_1

        color_outset_2: theme.color_d_1
        color_outset_2_down: theme.color_d_hidden
        color_outset_2_drag: theme.color_outset_2_down
        color_outset_2_hover: theme.color_outset_2
        color_outset_2_active: theme.color_u_1
        color_outset_2_focus: theme.color_outset_2
        color_outset_2_disabled: theme.color_u_1

        color_icon: theme.color_d_2
        color_icon_inactive: theme.color_inset
        color_icon_active: theme.color_u_4
        color_icon_disabled: theme.color_d_1

        color_mark: theme.color_u_5
        color_mark_empty: theme.color_inset
        color_mark_off: theme.color_u_hidden
        color_mark_hover: theme.color_mark
        color_mark_active: theme.color_mark
        color_mark_active_hover: theme.color_mark
        color_mark_focus: theme.color_mark
        color_mark_down: theme.color_u_4
        color_mark_disabled: theme.color_d_hidden

        color_selection: theme.color_d_hidden
        color_selection_hover: theme.color_u_3
        color_selection_down: theme.color_u_3
        color_selection_focus: theme.color_u_3
        color_selection_empty: theme.color_d_hidden
        color_selection_disabled: theme.color_d_hidden

        // Progress bars, slider amounts etc.
        color_val: theme.color_opaque_u_2
        color_val_hover: theme.color_opaque_u_3
        color_val_focus: theme.color_opaque_u_3
        color_val_drag: theme.color_opaque_u_3
        color_val_disabled: theme.color_u_hidden

        color_val_1: theme.color_opaque_u_1
        color_val_1_hover: theme.color_opaque_u_2
        color_val_1_focus: theme.color_opaque_u_2
        color_val_1_drag: theme.color_opaque_u_2
        color_val_1_disabled: theme.color_u_hidden

        color_val_2: theme.color_opaque_u_2
        color_val_2_hover: theme.color_opaque_u_3
        color_val_2_focus: theme.color_opaque_u_3
        color_val_2_drag: theme.color_opaque_u_3
        color_val_2_disabled: theme.color_u_hidden


        // WIDGET SPECIFIC COLORS
        color_handle: theme.color_opaque_u_3
        color_handle_hover: theme.color_opaque_u_4
        color_handle_focus: theme.color_opaque_u_3
        color_handle_disabled: theme.color_u_hidden
        color_handle_drag: theme.color_opaque_u_5

        color_handle_1: theme.color_opaque_u_1
        color_handle_1_hover: theme.color_opaque_u_2
        color_handle_1_focus: theme.color_opaque_u_2
        color_handle_1_disabled: theme.color_u_hidden
        color_handle_1_drag: theme.color_opaque_u_2

        color_handle_2: theme.color_opaque_d_5
        color_handle_2_hover: theme.color_opaque_d_5
        color_handle_2_focus: theme.color_opaque_d_5
        color_handle_2_disabled: theme.color_u_hidden
        color_handle_2_drag: theme.color_opaque_d_5

        color_dock_tab_active: theme.color_fg_app

        // TODO: THESE ARE APPLICATION SPECIFIC COLORS THAT SHOULD BE MOVED FROM THE GENERAL THEME TO THE GIVEN PROJECT
        color_high: #C00
        color_mid: #FA0
        color_low: #8A0
        color_panic: #f0f
        color_icon_wait: theme.color_low
        color_error: theme.color_high
        color_warning: theme.color_mid
        color_icon_panic: theme.color_high

        // The Architecture map's categorical palette (top-level directory ownership), kind stripes and status roles
        color_map_1: #x286cab
        color_map_1_a: #x075d8f
        color_map_1_b: #x177abc
        color_map_1_c: #x0067a9
        color_map_1_d: #x4885be
        color_map_1_e: #x1a548c
        color_map_1_f: #x2d70b9
        color_map_1_g: #x1f5da6
        color_map_1_h: #x4f7bba
        color_map_2: #x00816b
        color_map_2_a: #x126d53
        color_map_2_b: #x0e8f70
        color_map_2_c: #x007b62
        color_map_2_d: #x409882
        color_map_2_e: #x006756
        color_map_2_f: #x008774
        color_map_2_g: #x007365
        color_map_2_h: #x319184
        color_map_3: #x906b00
        color_map_3_a: #x805802
        color_map_3_b: #xa37300
        color_map_3_c: #x8b6400
        color_map_3_d: #xa78339
        color_map_3_e: #x735600
        color_map_3_f: #x947200
        color_map_3_g: #x7e6200
        color_map_3_h: #x998132
        color_map_4: #x78569e
        color_map_4_a: #x5f4a8a
        color_map_4_b: #x8061b3
        color_map_4_c: #x714f9f
        color_map_4_d: #x8d70b3
        color_map_4_e: #x604280
        color_map_4_f: #x8358a7
        color_map_4_g: #x734693
        color_map_4_h: #x8e68a7
        color_map_5: #x04869c
        color_map_5_a: #x007480
        color_map_5_b: #x0094a6
        color_map_5_c: #x008092
        color_map_5_d: #x429eb1
        color_map_5_e: #x006c80
        color_map_5_f: #x008ca7
        color_map_5_g: #x007892
        color_map_5_h: #x4196b0
        color_map_6: #x3b7d3f
        color_map_6_a: #x3c682b
        color_map_6_b: #x4b893d
        color_map_6_c: #x35782e
        color_map_6_d: #x5b945a
        color_map_6_e: #x28642f
        color_map_6_f: #x328544
        color_map_6_g: #x137436
        color_map_6_h: #x488f5e
        color_map_7: #x9c5313
        color_map_7_a: #x874219
        color_map_7_b: #xb0591e
        color_map_7_c: #x9a4900
        color_map_7_d: #xb26d3d
        color_map_7_e: #x7e4000
        color_map_7_f: #xa45800
        color_map_7_g: #x8a4b00
        color_map_7_h: #xa76a2e
        color_map_8: #xa04b6f
        color_map_8_a: #x863f65
        color_map_8_b: #xaf5382
        color_map_8_c: #x9c416e
        color_map_8_d: #xb56787
        color_map_8_e: #x823857
        color_map_8_f: #xac4c71
        color_map_8_g: #x993a5d
        color_map_8_h: #xb16078
        color_map_9: #xb64f4b
        color_map_9_a: #x9c4149
        color_map_9_b: #xc9555b
        color_map_9_c: #xb54246
        color_map_9_d: #xcb6c68
        color_map_9_e: #x963d38
        color_map_9_f: #xc25046
        color_map_9_g: #xae3e31
        color_map_9_h: #xc46857
        color_map_10: #x606cbd
        color_map_10_a: #x485fa3
        color_map_10_b: #x6179d2
        color_map_10_c: #x5466be
        color_map_10_d: #x7786d0
        color_map_10_e: #x4d569c
        color_map_10_f: #x696fca
        color_map_10_g: #x5b5cb6
        color_map_10_h: #x7c7cc8
        color_map_11: #x737c24
        color_map_11_a: #x696715
        color_map_11_b: #x85861d
        color_map_11_c: #x717500
        color_map_11_d: #x8c9348
        color_map_11_e: #x5b6416
        color_map_11_f: #x758421
        color_map_11_g: #x617307
        color_map_11_h: #x7c9049
        color_map_kind_module: #x94bfff
        color_map_kind_file: #xb7c0cc
        color_map_kind_struct: #x74cbb7
        color_map_kind_enum: #x9bcb79
        color_map_kind_trait: #xc1a0e0
        color_map_kind_impl: #x8aace0
        color_map_kind_fn: #xddc27b
        color_map_kind_const: #xdaa37d
        color_map_kind_type_alias: #x84c7d8
        color_map_kind_macro: #xd895b6
        color_map_kind_component: #x94bfff
        color_map_kind_thread: #xddc27b
        color_map_kind_queue: #x9bcb79
        color_map_kind_store: #xdaa37d
        color_map_kind_memory: #xc1a0e0
        color_map_kind_gpu: #x84c7d8
        color_map_kind_io: #xd895b6
        color_success: #x79be93
        color_map_basis_exact: #xb8c2cf
        color_map_basis_inferred: #xd9ae6b
        color_map_basis_candidates: #xb69acf
        color_syntax_keyword: #x90badc
        color_syntax_ident: #xd4d4d4
        color_syntax_type: #xa7d2cb
        color_syntax_fn: #xd6c9ac
        color_syntax_literal: #xb3d1af
        color_syntax_string: #xe0baac
        color_syntax_comment: #xb2c3ae
        color_syntax_macro: #xd8bcde
        color_syntax_attribute: #xe7c8a6
        color_syntax_punctuation: #xd4d4d4
        color_syntax_keyword_branch: #xd8bcde
        color_syntax_keyword_loop: #xe7c8a6
        color_syntax_constant: #xe0baac
        color_search_hit: #xf0c95a
        color_search_rim: #xf0c95a
        color_syntax_bg: #x202125

        // TYPOGRAPHY
        font_size_code: 9.0
        font_wdgt_line_spacing: 1.2
        font_hl_line_spacing: 1.05
        font_longform_line_spacing: 1.2

        font_size_1: theme.font_size_base + 8 * theme.font_size_contrast
        font_size_2: theme.font_size_base + 4 * theme.font_size_contrast
        font_size_3: theme.font_size_base + 2 * theme.font_size_contrast
        font_size_4: theme.font_size_base + 1 * theme.font_size_contrast
        font_size_p: theme.font_size_base

        font_label: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_regular: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_bold: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_italic: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_bold_italic: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_code: TextStyle{
            font_size: theme.font_size_code
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.35
        }
        font_icons: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: crate_resource("self:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
    }
}

#[cfg(test)]
mod crate_tint_role_tests {
    use crate::desktop_style::{install, DesktopStyle, StyleSheet};
    use crate::makepad_platform::*;
    use crate::script_eval;

    const SUFFIXES: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
    /// 8-bit RGB of `color_map_{1..=11}_{a..=h}` from the OKLab derivation (dark, s=+1).
    const DARK_TINTS: [[u32; 8]; 11] = [
        [
            0x075d8f, 0x177abc, 0x0067a9, 0x4885be, 0x1a548c, 0x2d70b9, 0x1f5da6, 0x4f7bba,
        ],
        [
            0x126d53, 0x0e8f70, 0x007b62, 0x409882, 0x006756, 0x008774, 0x007365, 0x319184,
        ],
        [
            0x805802, 0xa37300, 0x8b6400, 0xa78339, 0x735600, 0x947200, 0x7e6200, 0x998132,
        ],
        [
            0x5f4a8a, 0x8061b3, 0x714f9f, 0x8d70b3, 0x604280, 0x8358a7, 0x734693, 0x8e68a7,
        ],
        [
            0x007480, 0x0094a6, 0x008092, 0x429eb1, 0x006c80, 0x008ca7, 0x007892, 0x4196b0,
        ],
        [
            0x3c682b, 0x4b893d, 0x35782e, 0x5b945a, 0x28642f, 0x328544, 0x137436, 0x488f5e,
        ],
        [
            0x874219, 0xb0591e, 0x9a4900, 0xb26d3d, 0x7e4000, 0xa45800, 0x8a4b00, 0xa76a2e,
        ],
        [
            0x863f65, 0xaf5382, 0x9c416e, 0xb56787, 0x823857, 0xac4c71, 0x993a5d, 0xb16078,
        ],
        [
            0x9c4149, 0xc9555b, 0xb54246, 0xcb6c68, 0x963d38, 0xc25046, 0xae3e31, 0xc46857,
        ],
        [
            0x485fa3, 0x6179d2, 0x5466be, 0x7786d0, 0x4d569c, 0x696fca, 0x5b5cb6, 0x7c7cc8,
        ],
        [
            0x696715, 0x85861d, 0x717500, 0x8c9348, 0x5b6416, 0x758421, 0x617307, 0x7c9049,
        ],
    ];
    /// Light appearance (s=-1).
    const LIGHT_TINTS: [[u32; 8]; 11] = [
        [
            0xbae0ff, 0x85c1f5, 0xa5d3ff, 0x87b2df, 0xc6e1ff, 0x94c5fe, 0xb8d7ff, 0x94b6e7,
        ],
        [
            0xa8f0d4, 0x77d2b4, 0x89e6c9, 0x77c2ad, 0xa3f0dc, 0x76d9c4, 0x88edda, 0x79c9bb,
        ],
        [
            0xffd79b, 0xe9bc6e, 0xfcd081, 0xd3b373, 0xf9da99, 0xe8c673, 0xfadb86, 0xd2bc79,
        ],
        [
            0xdfd3ff, 0xc4acf1, 0xd9c1ff, 0xb9a2d8, 0xe8d4ff, 0xd3b0f3, 0xe6c5ff, 0xc6a6da,
        ],
        [
            0xa5ecf7, 0x78d1e1, 0x8ce4f6, 0x7dc1d1, 0xa7ebfd, 0x82d6ee, 0xa0e8ff, 0x86c6dc,
        ],
        [
            0xc3ecb5, 0x9ace8e, 0xaae2a3, 0x90bf8e, 0xbbeebc, 0x95d79d, 0xa6ebb3, 0x8ec79c,
        ],
        [
            0xffcfb6, 0xf3a579, 0xffbd95, 0xdb9e77, 0xffd4b7, 0xf6af78, 0xffc79a, 0xdea778,
        ],
        [
            0xffc8e3, 0xe6a3c3, 0xfbb6d4, 0xd39bb1, 0xffcfdf, 0xf0a9c1, 0xffc0d2, 0xdca1b1,
        ],
        [
            0xffd1d1, 0xf6a5a4, 0xffbebb, 0xdf9e99, 0xffd2cc, 0xfdada1, 0xffc9bf, 0xe5a698,
        ],
        [
            0xd0deff, 0xa8bbf7, 0xc1cfff, 0xa3aedf, 0xd5dcff, 0xb6bffd, 0xcfd4ff, 0xb0b2e5,
        ],
        [
            0xe4e399, 0xc5c76c, 0xd6dc80, 0xb3bb74, 0xdae69e, 0xc0d179, 0xd1e68e, 0xb0c47f,
        ],
    ];

    fn packed(rgb: u32) -> u32 {
        (rgb << 8) | 0xff
    }

    fn assert_tints(vm: &mut ScriptVm, expected: &[[u32; 8]; 11], label: &str) {
        let theme = vm.module(id!(theme));
        for family in 1..=11 {
            for (k, suf) in SUFFIXES.iter().enumerate() {
                let name = format!("color_map_{family}_{suf}");
                let color = vm
                    .bx
                    .heap
                    .value(theme, LiveId::from_str(&name).into(), NoTrap)
                    .as_color();
                assert_eq!(
                    color,
                    Some(packed(expected[family - 1][k])),
                    "{label} {name}"
                );
            }
        }
    }

    const DESIGN_KIND_ROLES: [&str; 7] = [
        "color_map_kind_component",
        "color_map_kind_thread",
        "color_map_kind_queue",
        "color_map_kind_store",
        "color_map_kind_memory",
        "color_map_kind_gpu",
        "color_map_kind_io",
    ];

    fn assert_design_kind_roles(vm: &mut ScriptVm, label: &str) {
        let theme = vm.module(id!(theme));
        for name in DESIGN_KIND_ROLES {
            let color = vm
                .bx
                .heap
                .value(theme, LiveId::from_str(name).into(), NoTrap)
                .as_color();
            assert!(color.is_some(), "{label} {name} does not resolve");
        }
    }

    /// The Design mode's seven subsystem kinds have a colour role in the
    /// defaults and in every shipped theme.
    #[test]
    fn design_kind_roles_resolve_in_every_theme() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            assert_design_kind_roles(vm, "dark-default");
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
            assert_design_kind_roles(vm, "light-default");
            for (style, dark) in [
                (DesktopStyle::Omarchy, false),
                (DesktopStyle::Macos, false),
                (DesktopStyle::Macos, true),
                (DesktopStyle::Windows, false),
                (DesktopStyle::Windows, true),
                (DesktopStyle::Windows2000, false),
                (DesktopStyle::NextStep, false),
                (DesktopStyle::Ios, false),
                (DesktopStyle::Ios, true),
                (DesktopStyle::Android, false),
                (DesktopStyle::Android, true),
            ] {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let errors = vm.take_errors();
                let label = StyleSheet::load_with_appearance(style, dark).name;
                assert!(errors.is_empty(), "{label}: {errors:?}");
                assert_design_kind_roles(vm, &label);
            }
        });
    }

    #[test]
    fn crate_tint_roles_resolve_in_dark_and_light_defaults() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            assert_tints(vm, &DARK_TINTS, "dark-default");
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
            assert_tints(vm, &LIGHT_TINTS, "light-default");
        });
    }

    #[test]
    fn crate_tint_roles_resolve_in_every_shipped_theme() {
        let sheets = [
            (DesktopStyle::Omarchy, false, &DARK_TINTS),
            (DesktopStyle::Macos, false, &LIGHT_TINTS),
            (DesktopStyle::Macos, true, &DARK_TINTS),
            (DesktopStyle::Windows, false, &LIGHT_TINTS),
            (DesktopStyle::Windows, true, &DARK_TINTS),
            (DesktopStyle::Windows2000, false, &LIGHT_TINTS),
            (DesktopStyle::NextStep, false, &LIGHT_TINTS),
            (DesktopStyle::Ios, false, &LIGHT_TINTS),
            (DesktopStyle::Ios, true, &DARK_TINTS),
            (DesktopStyle::Android, false, &LIGHT_TINTS),
            (DesktopStyle::Android, true, &DARK_TINTS),
        ];
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for (style, dark, expected) in sheets {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let errors = vm.take_errors();
                let label = StyleSheet::load_with_appearance(style, dark).name;
                assert!(errors.is_empty(), "{label}: {errors:?}");
                assert_tints(vm, expected, &label);
            }
        });
    }
}
