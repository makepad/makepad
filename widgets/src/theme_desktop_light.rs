use crate::makepad_platform::*;

script_mod! {
    use mod.math.*
    use mod.pod.*
    use mod.text.*
    use mod.turtle.*
    use mod.res.*


    mod.themes.light = {
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
        color_u_3: mix(theme.color_w, theme.color_w_h, pow(0.8, theme.color_contrast))
        color_u_2: mix(theme.color_w, theme.color_w_h, pow(0.85, theme.color_contrast))
        color_u_15: mix(theme.color_w, theme.color_w_h, pow(0.9, theme.color_contrast))
        color_u_1: mix(theme.color_w, theme.color_w_h, pow(0.95, theme.color_contrast))
        color_u_hidden: theme.color_w_h

        color_d_hidden: theme.color_b_h
        color_d_025: mix(theme.color_b, theme.color_b_h, pow(0.965, theme.color_contrast))
        color_d_05: mix(theme.color_b, theme.color_b_h, pow(0.925, theme.color_contrast))
        color_d_075: mix(theme.color_b, theme.color_b_h, pow(0.9, theme.color_contrast))
        color_d_1: mix(theme.color_b, theme.color_b_h, pow(0.875, theme.color_contrast))
        color_d_2: mix(theme.color_b, theme.color_b_h, pow(0.75, theme.color_contrast))
        color_d_3: mix(theme.color_b, theme.color_b_h, pow(0.6, theme.color_contrast))
        color_d_4: mix(theme.color_b, theme.color_b_h, pow(0.4, theme.color_contrast))
        color_d_5: mix(theme.color_b, theme.color_b_h, pow(0.25, theme.color_contrast))
        color_black: mix(theme.color_b, theme.color_b_h, pow(0.1, theme.color_contrast))

        color_bg_app: mix(
            theme.color_w * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            theme.color_b * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            pow(0.15, theme.color_contrast))
        color_fg_app: mix(
            theme.color_w * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            theme.color_b * mix(#ffffff, theme.color_tint, theme.color_tint_amount),
            pow(0.175, theme.color_contrast))
        color_opaque_u_6: mix(theme.color_fg_app, #F, 0.8)
        color_opaque_u_5: mix(theme.color_fg_app, #F, 0.7)
        color_opaque_u_4: mix(theme.color_fg_app, #F, 0.5)
        color_opaque_u_3: mix(theme.color_fg_app, #F, 0.35)
        color_opaque_u_2: mix(theme.color_fg_app, #F, 0.25)
        color_opaque_u_1: mix(theme.color_fg_app, #F, 0.15)

        color_opaque_d_05: mix(theme.color_fg_app, #0, 0.05)
        color_opaque_d_1: mix(theme.color_fg_app, #0, 0.15)
        color_opaque_d_2: mix(theme.color_fg_app, #0, 0.25)
        color_opaque_d_3: mix(theme.color_fg_app, #0, 0.45)
        color_opaque_d_4: mix(theme.color_fg_app, #0, 0.6)
        color_opaque_d_5: mix(theme.color_fg_app, #0, 0.75)

        // BASICS
        color_makepad: #FF5C39FF

        color_shadow: theme.color_d_1
        color_shadow_focus: theme.color_d_2
        color_shadow_disabled: theme.color_d_1
        color_shadow_flat: theme.color_d_1
        color_shadow_flat_disabled: theme.color_opaque_d_1
        color_light: theme.color_u_4
        color_light_hover: theme.color_u_5
        color_light_focus: theme.color_u_5
        color_light_disabled: theme.color_u_4

        color_flat_focus: theme.color_d_2

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

        color_highlight: theme.color_d_1
        color_text_cursor: theme.color_white
        color_bg_highlight_inline: theme.color_d_1

        color_text: theme.color_d_4
        color_text_val: theme.color_d_2
        color_text_hl: theme.color_text
        color_text_hover: theme.color_text
        color_text_on_accent: #xffffff
        color_text_active: theme.color_text
        color_focus: #x0067c0
        color_ctrl_selected: theme.color_focus
        color_text_focus: theme.color_text
        color_text_down: theme.color_text
        color_text_disabled: theme.color_d_1
        color_text_placeholder: theme.color_d_3
        color_text_placeholder_hover: theme.color_d_3
        color_text_meta: theme.color_d_3

        color_label_inner: theme.color_d_4
        color_label_inner_down: theme.color_d_5
        color_label_inner_drag: theme.color_label_inner_down
        color_label_inner_hover: theme.color_label_inner
        color_label_inner_focus: theme.color_label_inner
        color_label_inner_active: theme.color_label_inner
        color_label_inner_inactive: theme.color_d_3
        color_label_inner_disabled: theme.color_d_1

        color_label_outer: theme.color_d_5
        color_label_outer_off: theme.color_d_3
        color_label_outer_down: theme.color_label_outer

        color_label_outer_drag: theme.color_label_outer
        color_label_outer_hover: theme.color_label_outer
        color_label_outer_focus: theme.color_label_outer
        color_label_outer_active: theme.color_label_outer
        color_label_outer_active_focus: theme.color_label_outer
        color_label_outer_disabled: theme.color_d_1

        color_bg_container: theme.color_u_3 * 0.8
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

        color_bevel_inset_1: theme.color_shadow
        color_bevel_inset_1_hover: theme.color_bevel_inset_1
        color_bevel_inset_1_focus: theme.color_shadow_focus
        color_bevel_inset_1_active: theme.color_bevel_inset_1
        color_bevel_inset_1_empty: theme.color_bevel_inset_1
        color_bevel_inset_1_down: theme.color_bevel_inset_1
        color_bevel_inset_1_drag: theme.color_bevel_inset_1
        color_bevel_inset_1_disabled: theme.color_shadow_disabled

        color_bevel_inset_2: theme.color_light
        color_bevel_inset_2_hover: theme.color_light_focus
        color_bevel_inset_2_focus: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_active: theme.color_bevel_inset_2
        color_bevel_inset_2_empty: theme.color_bevel_inset_2
        color_bevel_inset_2_down: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_drag: theme.color_bevel_inset_2_hover
        color_bevel_inset_2_disabled: theme.color_light_disabled

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
        color_inset: theme.color_d_075
        color_inset_hover: theme.color_inset
        color_inset_down: theme.color_inset_hover
        color_inset_active: theme.color_inset_hover
        color_inset_focus: theme.color_inset_hover
        color_inset_drag: theme.color_inset
        color_inset_disabled: theme.color_d_025
        color_inset_empty: theme.color_inset

        color_inset_1: theme.color_d_1
        color_inset_1_hover: theme.color_d_2
        color_inset_1_down: theme.color_inset_1_hover
        color_inset_1_active: theme.color_inset_1_hover
        color_inset_1_focus: theme.color_inset_1_hover
        color_inset_1_drag: theme.color_inset_1_hover
        color_inset_1_empty: theme.color_u_hidden
        color_inset_1_disabled: theme.color_d_025

        color_inset_2: theme.color_d_05
        color_inset_2_hover: theme.color_d_1
        color_inset_2_down: theme.color_inset_2_hover
        color_inset_2_active: theme.color_inset_2_hover
        color_inset_2_focus: theme.color_inset_2_hover
        color_inset_2_drag: theme.color_inset_2_hover
        color_inset_2_disabled: theme.color_d_025
        color_inset_2_empty: theme.color_inset_2

        // WIDGET COLORS
        color_outset: theme.color_u_3
        color_outset_down: theme.color_u_1
        color_outset_hover: theme.color_u_4
        color_outset_active: theme.color_u_5
        color_outset_focus: theme.color_outset
        color_outset_drag: theme.color_outset_hover
        color_outset_disabled: theme.color_u_1
        color_outset_inactive: theme.color_d_hidden

        color_outset_1: theme.color_u_4
        color_outset_1_down: theme.color_d_2
        color_outset_1_drag: theme.color_outset_1_down
        color_outset_1_hover: theme.color_outset_1
        color_outset_1_active: theme.color_u_6
        color_outset_1_focus: theme.color_outset_1
        color_outset_1_disabled: theme.color_u_1

        color_outset_2: theme.color_u_hidden
        color_outset_2_down: theme.color_d_hidden
        color_outset_2_hover: theme.color_u_2
        color_outset_2_drag: theme.color_outset_2_down
        color_outset_2_active: theme.color_u_4
        color_outset_2_focus: theme.color_outset_2_hover
        color_outset_2_disabled: theme.color_u_1

        color_icon: theme.color_u_2
        color_icon_inactive: theme.color_inset
        color_icon_active: theme.color_d_4
        color_icon_disabled: theme.color_u_1

        color_mark: theme.color_d_4
        color_mark_empty: theme.color_inset
        color_mark_off: theme.color_d_hidden
        color_mark_hover: theme.color_mark
        color_mark_active: theme.color_mark
        color_mark_active_hover: theme.color_mark
        color_mark_focus: theme.color_mark
        color_mark_down: theme.color_d_3
        color_mark_disabled: theme.color_d_hidden

        color_selection: theme.color_u_hidden
        color_selection_hover: theme.color_d_2
        color_selection_down: theme.color_d_2
        color_selection_focus: theme.color_d_2
        color_selection_empty: theme.color_u_hidden
        color_selection_disabled: theme.color_u_hidden

        // Progress bars, slider amounts etc.
        color_val: theme.color_opaque_d_3
        color_val_hover: theme.color_opaque_d_4
        color_val_focus: theme.color_val_hover
        color_val_drag: theme.color_val_hover
        color_val_disabled: theme.color_d_hidden

        color_val_1: theme.color_opaque_d_3
        color_val_1_hover: theme.color_opaque_d_4
        color_val_1_focus: theme.color_val_1_hover
        color_val_1_drag: theme.color_val_1_hover
        color_val_1_disabled: theme.color_d_hidden

        color_val_2: theme.color_opaque_d_3
        color_val_2_hover: theme.color_opaque_d_4
        color_val_2_focus: theme.color_val_2_hover
        color_val_2_drag: theme.color_val_2_hover
        color_val_2_disabled: theme.color_d_hidden


        // WIDGET SPECIFIC COLORS
        color_handle: theme.color_opaque_u_4
        color_handle_hover: theme.color_handle
        color_handle_focus: theme.color_handle
        color_handle_disabled: theme.color_opaque_u_1
        color_handle_drag: theme.color_handle

        color_handle_1: theme.color_opaque_u_6
        color_handle_1_hover: theme.color_w
        color_handle_1_focus: theme.color_handle_1_hover
        color_handle_1_disabled: theme.color_opaque_u_1
        color_handle_1_drag: theme.color_handle_1_hover

        color_handle_2: theme.color_opaque_u_1
        color_handle_2_hover: theme.color_opaque_u_1
        color_handle_2_focus: theme.color_handle_2_hover
        color_handle_2_disabled: theme.color_opaque_u_1
        color_handle_2_drag: theme.color_handle_2_hover

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
        color_map_1: #x9cccff
        color_map_1_a: #xbae0ff
        color_map_1_b: #x85c1f5
        color_map_1_c: #xa5d3ff
        color_map_1_d: #x87b2df
        color_map_1_e: #xc6e1ff
        color_map_1_f: #x94c5fe
        color_map_1_g: #xb8d7ff
        color_map_1_h: #x94b6e7
        color_map_2: #x88dec7
        color_map_2_a: #xa8f0d4
        color_map_2_b: #x77d2b4
        color_map_2_c: #x89e6c9
        color_map_2_d: #x77c2ad
        color_map_2_e: #xa3f0dc
        color_map_2_f: #x76d9c4
        color_map_2_g: #x88edda
        color_map_2_h: #x79c9bb
        color_map_3: #xefcc83
        color_map_3_a: #xffd79b
        color_map_3_b: #xe9bc6e
        color_map_3_c: #xfcd081
        color_map_3_d: #xd3b373
        color_map_3_e: #xf9da99
        color_map_3_f: #xe8c673
        color_map_3_g: #xfadb86
        color_map_3_h: #xd2bc79
        color_map_4: #xd5b9f7
        color_map_4_a: #xdfd3ff
        color_map_4_b: #xc4acf1
        color_map_4_c: #xd9c1ff
        color_map_4_d: #xb9a2d8
        color_map_4_e: #xe8d4ff
        color_map_4_f: #xd3b0f3
        color_map_4_g: #xe6c5ff
        color_map_4_h: #xc6a6da
        color_map_5: #x8fdcef
        color_map_5_a: #xa5ecf7
        color_map_5_b: #x78d1e1
        color_map_5_c: #x8ce4f6
        color_map_5_d: #x7dc1d1
        color_map_5_e: #xa7ebfd
        color_map_5_f: #x82d6ee
        color_map_5_g: #xa0e8ff
        color_map_5_h: #x86c6dc
        color_map_6: #xa4dba4
        color_map_6_a: #xc3ecb5
        color_map_6_b: #x9ace8e
        color_map_6_c: #xaae2a3
        color_map_6_d: #x90bf8e
        color_map_6_e: #xbbeebc
        color_map_6_f: #x95d79d
        color_map_6_g: #xa6ebb3
        color_map_6_h: #x8ec79c
        color_map_7: #xfab688
        color_map_7_a: #xffcfb6
        color_map_7_b: #xf3a579
        color_map_7_c: #xffbd95
        color_map_7_d: #xdb9e77
        color_map_7_e: #xffd4b7
        color_map_7_f: #xf6af78
        color_map_7_g: #xffc79a
        color_map_7_h: #xdea778
        color_map_8: #xf2b2ca
        color_map_8_a: #xffc8e3
        color_map_8_b: #xe6a3c3
        color_map_8_c: #xfbb6d4
        color_map_8_d: #xd39bb1
        color_map_8_e: #xffcfdf
        color_map_8_f: #xf0a9c1
        color_map_8_g: #xffc0d2
        color_map_8_h: #xdca1b1
        color_map_9: #xffb5ae
        color_map_9_a: #xffd1d1
        color_map_9_b: #xf6a5a4
        color_map_9_c: #xffbebb
        color_map_9_d: #xdf9e99
        color_map_9_e: #xffd2cc
        color_map_9_f: #xfdada1
        color_map_9_g: #xffc9bf
        color_map_9_h: #xe5a698
        color_map_10: #xbbc7ff
        color_map_10_a: #xd0deff
        color_map_10_b: #xa8bbf7
        color_map_10_c: #xc1cfff
        color_map_10_d: #xa3aedf
        color_map_10_e: #xd5dcff
        color_map_10_f: #xb6bffd
        color_map_10_g: #xcfd4ff
        color_map_10_h: #xb0b2e5
        color_map_11: #xcbd685
        color_map_11_a: #xe4e399
        color_map_11_b: #xc5c76c
        color_map_11_c: #xd6dc80
        color_map_11_d: #xb3bb74
        color_map_11_e: #xdae69e
        color_map_11_f: #xc0d179
        color_map_11_g: #xd1e68e
        color_map_11_h: #xb0c47f
        color_map_kind_module: #x315e96
        color_map_kind_file: #x566273
        color_map_kind_struct: #x286b5d
        color_map_kind_enum: #x4d702d
        color_map_kind_trait: #x70508f
        color_map_kind_impl: #x465f91
        color_map_kind_fn: #x79601f
        color_map_kind_const: #x865332
        color_map_kind_type_alias: #x326b7c
        color_map_kind_macro: #x8a4667
        color_map_kind_component: #x315e96
        color_map_kind_thread: #x79601f
        color_map_kind_queue: #x4d702d
        color_map_kind_store: #x865332
        color_map_kind_memory: #x70508f
        color_map_kind_gpu: #x326b7c
        color_map_kind_io: #x8a4667
        color_success: #x2f7d4b
        color_map_basis_exact: #x4c5968
        color_map_basis_inferred: #x7c5b27
        color_map_basis_candidates: #x72558c
        color_syntax_keyword: #x3d688a
        color_syntax_ident: #x242426
        color_syntax_type: #x36615a
        color_syntax_fn: #x504326
        color_syntax_literal: #x425f3e
        color_syntax_string: #x6e483b
        color_syntax_comment: #x4b5c47
        color_syntax_macro: #x674b6d
        color_syntax_attribute: #x755635
        color_syntax_punctuation: #x242426
        color_syntax_keyword_branch: #x674b6d
        color_syntax_keyword_loop: #x755635
        color_syntax_constant: #x6e483b
        color_search_hit: #xf0c95a
        color_search_rim: #xf0c95a
        color_syntax_bg: #xffffff

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
        } // TODO: LEGACY, REMOVE. REQUIRED BY RUN LIST IN STUDIO ATM
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
