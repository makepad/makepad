use crate::makepad_platform::*;

script_mod!{
    use mod.math.*
    use mod.pod.*
    
    mod.themes.dark = {
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
        space_1: 0.5 * me.space_factor
        space_2: 1.0 * me.space_factor
        space_3: 1.5 * me.space_factor

        mspace_1: {top: me.space_1, right: me.space_1, bottom: me.space_1, left: me.space_1} 
        mspace_h_1: {top: 0., right: me.space_1, bottom: 0., left: me.space_1}
        mspace_v_1: {top: me.space_1, right: 0., bottom: me.space_1, left: 0.}
        mspace_2: {top: me.space_2, right: me.space_2, bottom: me.space_2, left: me.space_2}
        mspace_h_2: {top: 0., right: me.space_2, bottom: 0., left: me.space_2}
        mspace_v_2: {top: me.space_2, right: 0., bottom: me.space_2, left: 0.}
        mspace_3: {top: me.space_3, right: me.space_3, bottom: me.space_3, left: me.space_3}
        mspace_h_3: {top: 0., right: me.space_3, bottom: 0., left: me.space_3}
        mspace_v_3: {top: me.space_3, right: 0., bottom: me.space_3, left: 0.}

        data_item_height: 7.75 * me.space_1
        data_icon_width: 2.6 * me.space_2
        data_icon_height: 3.6 * me.space_2

        container_corner_radius: me.corner_radius * 2.
        textselection_corner_radius: me.corner_radius * 0.5
        tab_height: 6 * me.space_factor
        tab_flat_height: 5.5 * me.space_factor
        splitter_horizontal: 16.0
        splitter_size: 5.0
        splitter_min_horizontal: me.tab_height
        splitter_max_horizontal: me.tab_height + me.splitter_size
        splitter_min_vertical: me.splitter_horizontal
        splitter_max_vertical: me.splitter_horizontal + me.splitter_size
        dock_border_size: 0.0

        // COLOR PALETTE
        color_w: #FFFFFFFF
        color_w_h: #FFFFFF00
        color_b: #000000FF
        color_b_h: #00000000

        color_white: mix(me.color_w, #FFFFFF00, pow(0.1, me.color_contrast))
        color_u_6: mix(me.color_w, me.color_w_h, pow(0.2, me.color_contrast))
        color_u_5: mix(me.color_w, me.color_w_h, pow(0.35, me.color_contrast))
        color_u_4: mix(me.color_w, me.color_w_h, pow(0.6, me.color_contrast))
        color_u_3: mix(me.color_w, me.color_w_h, pow(0.75, me.color_contrast))
        color_u_2: mix(me.color_w, me.color_w_h, pow(0.85, me.color_contrast))
        color_u_15: mix(me.color_w, me.color_w_h, pow(0.9, me.color_contrast))
        color_u_1: mix(me.color_w, me.color_w_h, pow(0.95, me.color_contrast))
        color_u_hidden: me.color_w_h

        color_d_hidden: me.color_b_h
        color_d_025: mix(me.color_b, me.color_b_h, pow(0.95, me.color_contrast))
        color_d_05: mix(me.color_b, me.color_b_h, pow(0.9, me.color_contrast))
        color_d_1: mix(me.color_b, me.color_b_h, pow(0.85, me.color_contrast))
        color_d_2: mix(me.color_b, me.color_b_h, pow(0.75, me.color_contrast))
        color_d_3: mix(me.color_b, me.color_b_h, pow(0.6, me.color_contrast))
        color_d_4: mix(me.color_b, me.color_b_h, pow(0.4, me.color_contrast))
        color_d_5: mix(me.color_b, me.color_b_h, pow(0.25, me.color_contrast))
        color_black: mix(me.color_b, me.color_b_h, pow(0.1, me.color_contrast))

        color_bg_app: mix(
            me.color_b * mix(#ffffff, me.color_tint, me.color_tint_amount),
            me.color_w * mix(#ffffff, me.color_tint, me.color_tint_amount),
            pow(0.3, me.color_contrast))
        color_fg_app: mix(
            me.color_b * mix(#ffffff, me.color_tint, me.color_tint_amount),
            me.color_w * mix(#ffffff, me.color_tint, me.color_tint_amount),
            pow(0.36, me.color_contrast))
        color_opaque_u_6: mix(me.color_fg_app, #F, 0.8)
        color_opaque_u_5: mix(me.color_fg_app, #F, 0.7)
        color_opaque_u_4: mix(me.color_fg_app, #F, 0.5)
        color_opaque_u_3: mix(me.color_fg_app, #F, 0.35)
        color_opaque_u_2: mix(me.color_fg_app, #F, 0.25)
        color_opaque_u_1: mix(me.color_fg_app, #F, 0.15)

        color_opaque_d_1: mix(me.color_fg_app, #0, 0.15)
        color_opaque_d_2: mix(me.color_fg_app, #0, 0.25)
        color_opaque_d_3: mix(me.color_fg_app, #0, 0.45)
        color_opaque_d_4: mix(me.color_fg_app, #0, 0.6)
        color_opaque_d_5: mix(me.color_fg_app, #0, 0.75)

        // BASICS
        color_makepad: #FF5C39FF

        color_shadow: me.color_d_3
        color_shadow_focus: me.color_d_5
        color_shadow_disabled: me.color_opaque_d_3
        color_shadow_flat: me.color_d_2
        color_flat_focus: me.color_u_2
        color_shadow_flat_disabled: me.color_opaque_d_3
        color_light: me.color_u_2
        color_light_hover: me.color_opaque_u_2
        color_light_focus: me.color_opaque_u_2
        color_light_disabled: me.color_opaque_u_1

        color_bg_highlight: me.color_u_1
        color_bg_unfocussed: me.color_bg_highlight * 0.85
        color_app_caption_bar: me.color_d_hidden
        color_drag_quad: me.color_u_5
        color_drag_target_preview: me.color_u_2

        color_cursor: me.color_white
        color_cursor_focus: me.color_white
        color_cursor_empty: me.color_white
        color_cursor_disabled: me.color_u_hidden
        color_cursor_border: me.color_white

        color_highlight: me.color_u_1
        color_text_cursor: me.color_white
        color_bg_highlight_inline: me.color_d_3

        color_text: me.color_u_5
        color_text_val: me.color_u_3
        color_text_hl: me.color_text
        color_text_hover: me.color_text
        color_text_focus: me.color_text
        color_text_down: me.color_text
        color_text_disabled: me.color_u_1
        color_text_placeholder: me.color_u_4
        color_text_placeholder_hover: me.color_u_4
        color_text_meta: me.color_u_4

        color_label_inner: me.color_u_5
        color_label_inner_down: me.color_u_3
        color_label_inner_drag: me.color_label_inner_down
        color_label_inner_hover: me.color_label_inner
        color_label_inner_focus: me.color_label_inner
        color_label_inner_active: me.color_label_inner
        color_label_inner_inactive: me.color_u_4
        color_label_inner_disabled: me.color_u_2

        color_label_outer: me.color_u_5
        color_label_outer_off: me.color_u_3
        color_label_outer_down: me.color_label_outer

        color_label_outer_drag: me.color_label_outer
        color_label_outer_hover: me.color_label_outer
        color_label_outer_focus: me.color_label_outer
        color_label_outer_active: me.color_label_outer
        color_label_outer_active_focus: me.color_label_outer
        color_label_outer_disabled: me.color_u_2

        color_bg_container: me.color_d_3 * 0.8
        color_bg_even: me.color_bg_container * 0.875
        color_bg_odd: me.color_bg_container * 1.125

        color_bevel: me.color_shadow_flat
        color_bevel_hover: me.color_flat_focus
        color_bevel_focus: me.color_bevel_hover
        color_bevel_active: me.color_bevel
        color_bevel_empty: me.color_bevel
        color_bevel_down: me.color_bevel_hover
        color_bevel_drag: me.color_bevel_hover
        color_bevel_disabled: me.color_shadow_flat_disabled

        color_bevel_inset_2: me.color_light
        color_bevel_inset_2_hover: me.color_light_focus
        color_bevel_inset_2_focus: me.color_bevel_inset_2_hover
        color_bevel_inset_2_active: me.color_bevel_inset_2
        color_bevel_inset_2_empty: me.color_bevel_inset_2
        color_bevel_inset_2_down: me.color_bevel_inset_2_hover
        color_bevel_inset_2_drag: me.color_bevel_inset_2_hover
        color_bevel_inset_2_disabled: me.color_light_disabled

        color_bevel_inset_1: me.color_shadow
        color_bevel_inset_1_hover: me.color_bevel_inset_1
        color_bevel_inset_1_focus: me.color_bevel_inset_2_hover
        color_bevel_inset_1_active: me.color_bevel_inset_1
        color_bevel_inset_1_empty: me.color_bevel_inset_1
        color_bevel_inset_1_down: me.color_bevel_inset_1
        color_bevel_inset_1_drag: me.color_bevel_inset_1
        color_bevel_inset_1_disabled: me.color_shadow_disabled

        color_bevel_outset_1: me.color_light
        color_bevel_outset_1_hover: me.color_light_hover
        color_bevel_outset_1_focus: me.color_bevel_outset_1_hover
        color_bevel_outset_1_active: me.color_light
        color_bevel_outset_1_down: me.color_shadow
        color_bevel_outset_1_drag: me.color_bevel_outset_1_down
        color_bevel_outset_1_disabled: me.color_light_disabled

        color_bevel_outset_2: me.color_shadow
        color_bevel_outset_2_hover: me.color_shadow
        color_bevel_outset_2_focus: me.color_shadow_focus
        color_bevel_outset_2_active: me.color_shadow
        color_bevel_outset_2_down: me.color_light
        color_bevel_outset_2_drag: me.color_bevel_outset_2_down
        color_bevel_outset_2_disabled: me.color_shadow_disabled

        // Background of textinputs, radios, checkboxes etc.
        color_inset: me.color_d_1
        color_inset_hover: me.color_inset
        color_inset_down: me.color_inset_hover
        color_inset_active: me.color_inset_hover
        color_inset_focus: me.color_inset_hover
        color_inset_drag: me.color_inset
        color_inset_disabled: me.color_d_025
        color_inset_empty: me.color_inset

        color_inset_1: me.color_d_3
        color_inset_1_hover: me.color_inset_1
        color_inset_1_down: me.color_inset_1_hover
        color_inset_1_active: me.color_inset_1_hover
        color_inset_1_focus: me.color_inset_1_hover
        color_inset_1_drag: me.color_inset_1
        color_inset_1_disabled: me.color_d_025
        color_inset_1_empty: me.color_inset_1

        color_inset_2: me.color_d_05
        color_inset_2_hover: me.color_inset_2
        color_inset_2_down: me.color_inset_2_hover
        color_inset_2_active: me.color_inset_2_hover
        color_inset_2_focus: me.color_inset_2_hover
        color_inset_2_drag: me.color_inset_2
        color_inset_2_empty: me.color_d_hidden
        color_inset_2_disabled: me.color_d_025

        // WIDGET COLORS
        color_outset: me.color_u_15
        color_outset_down: me.color_d_1
        color_outset_hover: me.color_u_2
        color_outset_active: me.color_u_3
        color_outset_focus: me.color_outset
        color_outset_drag: me.color_u_2
        color_outset_disabled: me.color_u_1
        color_outset_inactive: me.color_d_hidden

        color_outset_1: me.color_u_1
        color_outset_1_down: me.color_d_2
        color_outset_1_drag: me.color_outset_1_down
        color_outset_1_hover: me.color_u_2
        color_outset_1_active: me.color_u_4
        color_outset_1_focus: me.color_outset_1
        color_outset_1_disabled: me.color_u_1

        color_outset_2: me.color_d_1
        color_outset_2_down: me.color_d_hidden
        color_outset_2_drag: me.color_outset_2_down
        color_outset_2_hover: me.color_outset_2
        color_outset_2_active: me.color_u_1
        color_outset_2_focus: me.color_outset_2
        color_outset_2_disabled: me.color_u_1

        color_icon: me.color_d_2
        color_icon_inactive: me.color_inset
        color_icon_active: me.color_u_4
        color_icon_disabled: me.color_d_1

        color_mark: me.color_u_5
        color_mark_empty: me.color_inset
        color_mark_off: me.color_u_hidden
        color_mark_hover: me.color_mark
        color_mark_active: me.color_mark
        color_mark_active_hover: me.color_mark
        color_mark_focus: me.color_mark
        color_mark_down: me.color_u_4
        color_mark_disabled: me.color_d_hidden

        color_selection: me.color_d_hidden
        color_selection_hover: me.color_u_3
        color_selection_down: me.color_u_3
        color_selection_focus: me.color_u_3
        color_selection_empty: me.color_d_hidden
        color_selection_disabled: me.color_d_hidden

        // Progress bars, slider amounts etc.
        color_val: me.color_opaque_u_2
        color_val_hover: me.color_opaque_u_3
        color_val_focus: me.color_opaque_u_3
        color_val_drag: me.color_opaque_u_3
        color_val_disabled: me.color_u_hidden

        color_val_1: me.color_opaque_u_1
        color_val_1_hover: me.color_opaque_u_2
        color_val_1_focus: me.color_opaque_u_2
        color_val_1_drag: me.color_opaque_u_2
        color_val_1_disabled: me.color_u_hidden
        
        color_val_2: me.color_opaque_u_2
        color_val_2_hover: me.color_opaque_u_3
        color_val_2_focus: me.color_opaque_u_3
        color_val_2_drag: me.color_opaque_u_3
        color_val_2_disabled: me.color_u_hidden


        // WIDGET SPECIFIC COLORS
        color_handle: me.color_opaque_u_3
        color_handle_hover: me.color_opaque_u_4
        color_handle_focus: me.color_opaque_u_3
        color_handle_disabled: me.color_u_hidden
        color_handle_drag: me.color_opaque_u_5

        color_handle_1: me.color_opaque_u_1
        color_handle_1_hover: me.color_opaque_u_2
        color_handle_1_focus: me.color_opaque_u_2
        color_handle_1_disabled: me.color_u_hidden
        color_handle_1_drag: me.color_opaque_u_2

        color_handle_2: me.color_opaque_d_5
        color_handle_2_hover: me.color_opaque_d_5
        color_handle_2_focus: me.color_opaque_d_5
        color_handle_2_disabled: me.color_u_hidden
        color_handle_2_drag: me.color_opaque_d_5

        color_dock_tab_active: me.color_fg_app

        // TODO: THESE ARE APPLICATION SPECIFIC COLORS THAT SHOULD BE MOVED FROM THE GENERAL THEME TO THE GIVEN PROJECT
        color_high: #C00
        color_mid: #FA0
        color_low: #8A0
        color_panic: #f0f
        color_icon_wait: me.color_low
        color_error: me.color_high
        color_warning: me.color_mid
        color_icon_panic: me.color_high

        // TYPOGRAPHY
        font_size_code: 9.0
        font_wdgt_line_spacing: 1.2
        font_hl_line_spacing: 1.05
        font_longform_line_spacing: 1.2

        font_size_1: me.font_size_base + 8 * me.font_size_contrast
        font_size_2: me.font_size_base + 4 * me.font_size_contrast
        font_size_3: me.font_size_base + 2 * me.font_size_contrast
        font_size_4: me.font_size_base + 1 * me.font_size_contrast
        font_size_p: me.font_size_base

        font_label: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: res.crate("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
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
                $latin: FontMember{res: res.crate("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
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
                $latin: FontMember{res: res.crate("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
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
                $latin: FontMember{res: res.crate("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular2:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular2_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_bold_italic: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: res.crate("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                /*$chinese: FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_bold2:resources/LXGWWenKaiBold.ttf"
                    "makepad_fonts_chinese_bold2_2:resources/LXGWWenKaiBold.ttf.2"
                ) asc: 0.0 desc: 0.0}*/
            }
            line_spacing: 1.2
        }
        font_code: TextStyle{
            font_size: me.font_size_code
            font_family: FontFamily{
                $latin: FontMember{res: res.crate("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.35
        }
        font_icons: TextStyle{
            font_family: FontFamily{
                $latin: FontMember{res: res.crate("self:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
    }
}
