use crate::makepad_platform::*;

script_mod!{
    use mod.math.*
    use mod.pod.*
    
    mod.themes.skeleton = {
        let theme = me
        // GLOBAL PARAMETERS
        space_factor: 10. // Increase for a less dense layout
        corner_radius: 2.5
        beveling: 0.75
        font_size_base: 15. // TODO: can this be removed? this is used somewhere

        // DIMENSIONS
        space_1: 3.
        space_2: 6.
        space_3: 9.

        mspace_1: {top: 3., right: 3., bottom: 3., left: 3.} 
        mspace_h_1: {top: 0., right: 3., bottom: 0., left: 3.}
        mspace_v_1: {top: 3., right: 0., bottom: 3., left: 0.}
        mspace_2: {top: 6., right: 6., bottom: 6., left: 6.}
        mspace_h_2: {top: 0., right: 6., bottom: 0., left: 6.}
        mspace_v_2: {top: 6., right: 0., bottom: 6., left: 0.}
        mspace_3: {top: 9., right: 9., bottom: 9., left: 9.}
        mspace_h_3: {top: 0., right: 9., bottom: 0., left: 9.}
        mspace_v_3: {top: 9., right: 0., bottom: 9., left: 0.}

        data_item_height: 23.25
        data_icon_width: 15.5
        data_icon_height: 21.5

        container_corner_radius: 5. 
        textselection_corner_radius: 12.5
        tab_height: 38.
        tab_flat_height: 33.
        splitter_min_horizontal: 36.
        splitter_max_horizontal: 46.
        splitter_min_vertical: 16.0
        splitter_max_vertical: 26.
        splitter_size: 5.0
        dock_border_size: 0.0

        // COLOR PALETTE
        color_u_hidden: #FFFFFF00
        color_d_hidden: #00000000

        color_bg_app: #D
        color_fg_app: #E

        // BASICS
        color_makepad: #FF5C39FF

        color_shadow: #00000011

        color_bg_highlight: #FFFFFF22
        color_app_caption_bar: #00000000
        color_drag_target_preview: #FFFFFF66

        color_cursor: #FFFFFF
        color_cursor_border: #FFFFFF

        color_highlight: #f00
        color_text_cursor: #FFFFFF
        color_bg_highlight_inline: #00000011

        color_text: #000000AA
        color_text_val: #00000044
        color_text_hl: #000000AA
        color_text_hover: #000000AA
        color_text_focus: #000000AA
        color_text_down: #000000AA
        color_text_disabled: #00000022
        color_text_placeholder: #00000088
        color_text_placeholder_hover: #000000AA

        color_label_inner: #000000AA
        color_label_inner_down: #000000CC
        color_label_inner_hover: #000000AA
        color_label_inner_focus: #000000AA
        color_label_inner_active: #000000AA
        color_label_inner_inactive: #00000088
        color_label_inner_disabled: #00000022

        color_label_outer: #000000CC
        color_label_outer_off: #00000088
        color_label_outer_down: #000000AA

        color_label_outer_drag: #000000AA
        color_label_outer_hover: #000000AA
        color_label_outer_focus: #000000AA
        color_label_outer_active: #000000AA
        color_label_outer_disabled: #00000044

        color_bg_container: #00000011
        color_bg_even: #ffffff44
        color_bg_odd: #ffffff00

        color_bevel: #00000011
        color_bevel_hover: #00000022
        color_bevel_focus: #00000022
        color_bevel_active: #00000011
        color_bevel_empty: #00000011
        color_bevel_down: #00000022
        color_bevel_drag: #00000022
        color_bevel_disabled: #3

        color_bevel_inset_1: #FFFFFFAA
        color_bevel_inset_1_hover: #FFFFFFDD
        color_bevel_inset_1_focus: #FFFFFFDD
        color_bevel_inset_1_active: #FFFFFFAA
        color_bevel_inset_1_empty: #FFFFFFAA
        color_bevel_inset_1_down: #FFFFFFDD
        color_bevel_inset_1_drag: #FFFFFFDD
        color_bevel_inset_1_disabled: #00000008

        color_bevel_inset_2: #00000011
        color_bevel_inset_2_hover: #00000011
        color_bevel_inset_2_focus: #00000022
        color_bevel_inset_2_active: #00000011
        color_bevel_inset_2_empty: #00000011
        color_bevel_inset_2_down: #00000011
        color_bevel_inset_2_drag: #00000011
        color_bevel_inset_2_disabled: #00000008

        color_bevel_outset_1: #FFFFFFAA
        color_bevel_outset_1_hover: #FFFFFFDD
        color_bevel_outset_1_focus: #FFFFFFDD
        color_bevel_outset_1_active: #FFFFFFAA
        color_bevel_outset_1_down: #00000011
        color_bevel_outset_1_disabled: #00000008

        color_bevel_outset_2: #00000011
        color_bevel_outset_2_hover: #00000011
        color_bevel_outset_2_active: #00000011
        color_bevel_outset_2_down: #FFFFFFAA
        color_bevel_outset_2_focus: #00000022
        color_bevel_outset_2_disabled: #00000008

        // Background of textinputs, radios, checkboxes etc.
        color_inset: #0000000A
        color_inset_hover: #00000008
        color_inset_down: #00000008
        color_inset_active: #00000008
        color_inset_focus: #00000008
        color_inset_drag: #00000008
        color_inset_disabled: #FFFFFF22
        color_inset_empty: #00000008

        color_inset_1: #00000008
        color_inset_1_hover: #00000011
        color_inset_1_down: #00000011
        color_inset_1_active: #00000011
        color_inset_1_focus: #00000011
        color_inset_1_drag: #00000008
        color_inset_1_disabled: #FFFFFF22
        color_inset_1_empty: #00000008

        color_inset_2: #00000011
        color_inset_2_hover: #00000022
        color_inset_2_down: #00000022
        color_inset_2_active: #00000022
        color_inset_2_focus: #00000022
        color_inset_2_drag: #00000011
        color_inset_2_empty: #FFFFFF00
        color_inset_2_disabled: #00000011

        // WIDGET COLORS
        color_outset: #FFFFFF88
        color_outset_down: #FFFFFF22
        color_outset_hover: #FFFFFFAA
        color_outset_active: #FFFFFFDD
        color_outset_focus: #FFFFFF88
        color_outset_drag: #FFFFFFAA
        color_outset_disabled: #FFFFFF88

        color_outset_1: #FFFFFFAA
        color_outset_1_down: #00000022
        color_outset_1_hover: #FFFFFFAA
        color_outset_1_active: #FFFFFFEE
        color_outset_1_focus: #FFFFFFAA
        color_outset_1_disabled: #FFFFFF22

        color_outset_2: #FFFFFF00
        color_outset_2_down: #00000000
        color_outset_2_hover: #FFFFFF66
        color_outset_2_active: #FFFFFFAA
        color_outset_2_focus: #FFFFFF66
        color_outset_2_disabled: #FFFFFF00

        color_icon: #FFFFFF66
        color_icon_inactive: #0000000A
        color_icon_active: #00000066
        color_icon_disabled: #FFFFFFAA

        color_mark_empty: #00000008
        color_mark_off: #00000000
        color_mark_active: #00000066
        color_mark_active_hover: #00000066
        color_mark_focus: #00000066
        color_mark_disabled: #00000022

        color_selection: #FFFFFF00
        color_selection_hover: #00000044
        color_selection_down: #00000044
        color_selection_focus: #00000044
        color_selection_empty: #FFFFFF00
        color_selection_disabled: #FFFFFF00

        // Progress bars, slider amounts etc.
        color_val: #9
        color_val_hover: #A
        color_val_focus: #A
        color_val_drag: #A
        color_val_disabled: #00000000

        color_val_1: #4
        color_val_1_hover: #6
        color_val_1_focus: #6
        color_val_1_drag: #6
        color_val_1_disabled: #00000000
        
        color_val_2: #3
        color_val_2_hover: #4
        color_val_2_focus: #4
        color_val_2_drag: #4
        color_val_2_disabled: #00000000


        // WIDGET SPECIFIC COLORS
        color_handle: #6
        color_handle_hover: #6
        color_handle_focus: #6
        color_handle_disabled: #2
        color_handle_drag: #6

        color_handle_1: #FFFFFF
        color_handle_1_hover: #FFFFFF
        color_handle_1_focus: #FFFFFF
        color_handle_1_disabled: #1
        color_handle_1_drag: #FFFFFF

        color_handle_2: #8
        color_handle_2_hover: #8
        color_handle_2_focus: #8
        color_handle_2_disabled: #A
        color_handle_2_drag: #8

        // TYPOGRAPHY
        font_size_code: 9.0
        font_wdgt_line_spacing: 1.2
        font_hl_line_spacing: 1.05
        font_longform_line_spacing: 1.2

        font_size_1: 26.
        font_size_2: 18.
        font_size_3: 14.
        font_size_4: 12.
        font_size_p: 10.

        font_label: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                chinese := FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}
                emoji := FontMember{res: res.crate("makepad_fonts_emoji:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        } // TODO: LEGACY, REMOVE. REQUIRED BY RUN LIST IN STUDIO ATM
        font_regular: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/IBMPlexSans-Text.ttf") asc: -0.1 desc: 0.0}
                chinese := FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}
                emoji := FontMember{res: res.crate("makepad_fonts_emoji:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_bold: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/IBMPlexSans-SemiBold.ttf") asc: -0.1 desc: 0.0}
                chinese := FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_bold:resources/LXGWWenKaiBold.ttf"
                    "makepad_fonts_chinese_bold_2:resources/LXGWWenKaiBold.ttf.2"
                ) asc: 0.0 desc: 0.0}
                emoji := FontMember{res: res.crate("makepad_fonts_emoji:resources/NotoColorEmoji.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_italic: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/IBMPlexSans-Italic.ttf") asc: -0.1 desc: 0.0}
                chinese := FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_regular:resources/LXGWWenKaiRegular.ttf"
                    "makepad_fonts_chinese_regular_2:resources/LXGWWenKaiRegular.ttf.2"
                ) asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_bold_italic: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/IBMPlexSans-BoldItalic.ttf") asc: -0.1 desc: 0.0}
                chinese := FontMember{res: res.split_crate(
                    "makepad_fonts_chinese_bold:resources/LXGWWenKaiBold.ttf"
                    "makepad_fonts_chinese_bold_2:resources/LXGWWenKaiBold.ttf.2"
                ) asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
        font_code: TextStyle{
            font_size: theme.font_size_code
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/LiberationMono-Regular.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.35
        }
        font_icons: TextStyle{
            font_family: FontFamily{
                latin := FontMember{res: res.crate("self:resources/fa-solid-900.ttf") asc: 0.0 desc: 0.0}
            }
            line_spacing: 1.2
        }
    }
}
