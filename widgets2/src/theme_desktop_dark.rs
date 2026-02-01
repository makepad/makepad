use crate::makepad_platform::*;

script_mod!{
    use mod.math.*
    use mod.pod.*
    use mod.text.*
    use mod.turtle.*
    use mod.res.*

    mod.themes.dark = {
        mod.theme = me
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
                $latin: FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular2:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular2_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}
                $emoji: FontMember{res: res.crate("makepad_fonts_emoji2:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        } 
        font_regular: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular2:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular2_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}
                $emoji: FontMember{res: res.crate("makepad_fonts_emoji2:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_bold: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_bold2:resources/LXGWWenKaiBold.ttf"
                    "makepad_fonts_chinese_bold2_2:resources/LXGWWenKaiBold.ttf.2"
                ) asc: 0.0 desc: 0.0}
                $emoji: FontMember{res: res.crate("makepad_fonts_emoji2:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_italic: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular2:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular2_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_bold_italic: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_bold2:resources/LXGWWenKaiBold.ttf"
                    "makepad_fonts_chinese_bold2_2:resources/LXGWWenKaiBold.ttf.2"
                ) asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_code: TextStyle{
            font_size: theme.font_size_code
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.35
        }
        font_icons: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: crate_resource("self:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
    }
}
