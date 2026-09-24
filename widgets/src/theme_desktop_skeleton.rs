use crate::makepad_platform::*;

script_mod! {
    use mod.math.*
    use mod.pod.*
    use mod.text.*
    use mod.turtle.*
    use mod.res.*
    use mod.animator.*

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

        mspace_1: Inset{top: 3., right: 3., bottom: 3., left: 3.}
        mspace_h_1: Inset{top: 0., right: 3., bottom: 0., left: 3.}
        mspace_v_1: Inset{top: 3., right: 0., bottom: 3., left: 0.}
        mspace_2: Inset{top: 6., right: 6., bottom: 6., left: 6.}
        mspace_h_2: Inset{top: 0., right: 6., bottom: 0., left: 6.}
        mspace_v_2: Inset{top: 6., right: 0., bottom: 6., left: 0.}
        mspace_3: Inset{top: 9., right: 9., bottom: 9., left: 9.}
        mspace_h_3: Inset{top: 0., right: 9., bottom: 0., left: 9.}
        mspace_v_3: Inset{top: 9., right: 0., bottom: 9., left: 0.}

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

        // DESIGN TOKENS
        // Shape: corner radii in the unit border_radius takes.
        radius_none: 0.
        radius_xs: 2.
        radius_s: 4.
        radius_m: 6.
        radius_l: 8.
        radius_xl: 14.
        radius_full: 999.
        // Elevation: blur radius and vertical offset for a RoundedShadowView.
        elevation_1_radius: 4.
        elevation_1_offset_y: 1.
        elevation_2_radius: 8.
        elevation_2_offset_y: 2.
        elevation_3_radius: 12.
        elevation_3_offset_y: 4.
        elevation_4_radius: 16.
        elevation_4_offset_y: 6.
        elevation_5_radius: 24.
        elevation_5_offset_y: 8.
        // Motion: durations in seconds, easings as animator values.
        motion_short_1: 0.05
        motion_short_2: 0.1
        motion_short_3: 0.15
        motion_short_4: 0.2
        motion_medium_1: 0.25
        motion_medium_2: 0.3
        motion_medium_3: 0.35
        motion_medium_4: 0.4
        motion_long_1: 0.45
        motion_long_2: 0.5
        motion_long_3: 0.55
        motion_long_4: 0.6
        motion_extra_long_1: 0.7
        motion_extra_long_2: 0.8
        motion_extra_long_3: 0.9
        motion_extra_long_4: 1.0
        motion_ease_standard: Ease.Bezier{cp0: 0.2 cp1: 0.0 cp2: 0.0 cp3: 1.0}
        motion_ease_standard_decelerate: Ease.Bezier{cp0: 0.0 cp1: 0.0 cp2: 0.0 cp3: 1.0}
        motion_ease_standard_accelerate: Ease.Bezier{cp0: 0.3 cp1: 0.0 cp2: 1.0 cp3: 1.0}
        motion_ease_emphasized_decelerate: Ease.Bezier{cp0: 0.05 cp1: 0.7 cp2: 0.1 cp3: 1.0}
        motion_ease_emphasized_accelerate: Ease.Bezier{cp0: 0.3 cp1: 0.0 cp2: 0.8 cp3: 0.15}
        motion_ease_linear: Ease.Linear
        // Overshoots and settles, which is what a spring does and what the
        // exponential decay that used to be here did not: that one only
        // crawled up to its value from below, so nothing on screen ever
        // read as sprung.
        motion_ease_spring: Ease.OutElastic
        // Arrives, rebounds off its own value, and comes back, twice more,
        // smaller. It never goes past what it is animating to, which is
        // the whole difference from the spring above.
        motion_ease_bounce: Ease.OutBounce
        // State layer opacities.
        state_hover_opacity: 0.08
        state_focus_opacity: 0.10
        state_press_opacity: 0.10
        state_drag_opacity: 0.16
        state_disabled_content_opacity: 0.38
        state_disabled_container_opacity: 0.12
        state_scrim_opacity: 0.32
        // Type scale on the skeleton's own ladder: paragraph 10 with a step
        // of 2 (font_size_1..4 are 26/18/14/12), so the titles equal font_size_3
        // and font_size_4 here as they do in dark and light.
        type_title_l_size: 14.
        type_title_m_size: 12.
        type_title_s_size: 11.
        type_body_l_size: 10.8
        type_body_m_size: 10.
        type_body_s_size: 9.2
        type_label_l_size: 10.
        type_label_m_size: 9.2
        type_label_s_size: 8.4
        font_size_5: 11.32
        font_size_6: 10.66
        // Sizes and the upper spacing rungs.
        size_control_s: 24.
        size_control_m: 30.
        size_control_l: 36.
        size_icon_s: 12.
        size_icon_m: 16.
        size_icon_l: 24.
        size_touch_target: 44.
        size_border: 1.
        size_focus_ring: 2.
        size_divider: 1.
        space_4: 12.
        space_5: 18.
        space_6: 24.
        // END DESIGN TOKENS

        // COLOR PALETTE
        color_u_hidden: #FFFFFF00
        color_d_hidden: #00000000

        // The tint ladders every other theme derives from its contrast knob,
        // spelled out at contrast 1 so the widgets that read them evaluate
        // here too (the code view, the text flow, the file tree's inactive
        // face).
        color_white: #xFFFFFFE6
        color_u_6: #xFFFFFFCC
        color_u_5: #xFFFFFFA6
        color_u_4: #xFFFFFF66
        color_u_3: #xFFFFFF33
        color_u_2: #xFFFFFF26
        color_u_15: #xFFFFFF1A
        color_u_1: #xFFFFFF0D
        color_d_025: #x0000000D
        color_d_05: #x0000001A
        color_d_1: #x00000026
        color_d_2: #x00000040
        color_d_3: #x00000066
        color_d_4: #x00000099
        color_d_5: #x000000BF
        color_black: #x000000E6
        color_outset_inactive: #x00000000

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
        color_text_meta: #00000088

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

        // DESIGN TOKENS
        // The opaque ladder: color_fg_app mixed toward white and black by the
        // amounts dark and light use, generated by theme_tokens::skeleton_ladder.
        color_opaque_u_6: #xFCFCFCFF
        color_opaque_u_5: #xFAFAFAFF
        color_opaque_u_4: #xF7F7F7FF
        color_opaque_u_3: #xF4F4F4FF
        color_opaque_u_2: #xF2F2F2FF
        color_opaque_u_1: #xF1F1F1FF
        color_opaque_d_1: #xCACACAFF
        color_opaque_d_2: #xB3B3B3FF
        color_opaque_d_3: #x838383FF
        color_opaque_d_4: #x5F5F5FFF
        color_opaque_d_5: #x3C3C3CFF
        // The status colours the light theme carries, so every widget
        // evaluates under this theme too.
        color_high: #xCC0000FF
        color_mid: #xFFAA00FF
        color_low: #x88AA00FF
        color_panic: #xFF00FFFF
        color_icon_wait: #x88AA00FF
        color_error: #xCC0000FF
        color_warning: #xFFAA00FF
        color_icon_panic: #xCC0000FF
        // Accent roles, generated by theme_tokens::roles_for(Scheme::Skeleton):
        // the brand families lose their hue, the intents keep a little.
        color_primary: #x595959FF
        color_on_primary: #xFFFFFFFF
        color_primary_container: #xCCCCCCFF
        color_on_primary_container: #x1A1A1AFF
        color_secondary: #x595959FF
        color_on_secondary: #xFFFFFFFF
        color_secondary_container: #xCCCCCCFF
        color_on_secondary_container: #x1A1A1AFF
        color_tertiary: #x595959FF
        color_on_tertiary: #xFFFFFFFF
        color_tertiary_container: #xCCCCCCFF
        color_on_tertiary_container: #x1A1A1AFF
        color_on_error: #xFFFFFFFF
        color_error_container: #xF2D9D9FF
        color_on_error_container: #x2C1111FF
        color_on_warning: #x000000FF
        color_warning_container: #xF2EBD9FF
        color_on_warning_container: #x2C2411FF
        color_success: #x389457FF
        color_on_success: #x000000FF
        color_success_container: #xD9F2E1FF
        color_on_success_container: #x112C1AFF
        color_info: #x386394FF
        color_on_info: #xFFFFFFFF
        color_info_container: #xD9E5F2FF
        color_on_info_container: #x111E2CFF
        color_inverse_primary: #xBBBBBBFF
        color_scrim: #x000000FF
        color_elevation_shadow: #x000000FF

        // MATERIAL
        // Off. The moulded stylesheets raise material_level; every value
        // below is the default they start from, not a look this theme wears.
        material_level: 0.
        material_light_x: -0.35
        material_light_y: -0.55
        material_light_z: 0.76
        material_light_intensity: 1.0
        material_led_radius: 24.
        material_led_intensity: 1.0
        material_bevel_width: 3.
        material_bevel_curve: 0.35
        material_specular: 0.25
        material_roughness: 0.55
        material_ao: 0.35
        material_rim: 0.5
        material_gloss: 0.
        material_glow: 0.
        material_ink_glow: 0.
        material_ink_lift: 1.6
        material_face_gradient: 0.
        material_hairline: 0.
        material_ao_reach: 1.
        material_inner_shadow: 0.
        material_inner_radius: 6.
        material_shadow: 0.
        material_shadow_blur: 8.
        material_shadow_falloff: 1.
        material_contact_ao: 0.
        material_ground_lip: 0.
        material_press_invert: 0.
        material_raise: 3.
        material_sink: 3.
        material_press_depth: -6.
        color_material_light: #xFFFFFFFF
        color_material_shadow: #x404040FF
        color_material_glow: #x00A0A0FF
        // Surface and outline roles, the light mapping over the ladder above.
        color_surface: #xDDDDDDFF
        color_surface_container: #xEEEEEEFF
        color_surface_container_low: #xE6E6E6FF
        color_surface_container_high: #xCACACAFF
        color_surface_container_highest: #xB3B3B3FF
        color_surface_container_lowest: #xFFFFFFFF
        color_surface_dim: #xCACACAFF
        color_surface_bright: #xF4F4F4FF
        color_on_surface: #x000000AA
        color_on_surface_variant: #x00000099
        color_outline: #x00000040
        color_outline_variant: #x00000020
        color_inverse_surface: #x3C3C3CFF
        color_inverse_on_surface: #xFCFCFCFF
        // Elevation shadow colours, one per step.
        color_elevation_1: #x00000020
        color_elevation_2: #x0000002A
        color_elevation_3: #x00000033
        color_elevation_4: #x0000003D
        color_elevation_5: #x00000047
        // Presence and placeholder colours.
        color_presence_online: #x2FB344FF
        color_presence_away: #xE8A317FF
        color_presence_busy: #xD83B3BFF
        color_presence_offline: #x8A8A8AFF
        color_placeholder: #xCACACAFF
        color_placeholder_hl: #xB3B3B3FF
        // END DESIGN TOKENS

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
        // DESIGN TOKENS
        font_title_l: theme.font_bold{font_size: theme.type_title_l_size line_spacing: 1.2}
        font_title_m: theme.font_bold{font_size: theme.type_title_m_size line_spacing: 1.2}
        font_title_s: theme.font_bold{font_size: theme.type_title_s_size line_spacing: 1.2}
        font_body_l: theme.font_regular{font_size: theme.type_body_l_size line_spacing: 1.35}
        font_body_m: theme.font_regular{font_size: theme.type_body_m_size line_spacing: 1.35}
        font_body_s: theme.font_regular{font_size: theme.type_body_s_size line_spacing: 1.35}
        font_label_l: theme.font_bold{font_size: theme.type_label_l_size line_spacing: 1.2}
        font_label_m: theme.font_bold{font_size: theme.type_label_m_size line_spacing: 1.2}
        font_label_s: theme.font_bold{font_size: theme.type_label_s_size line_spacing: 1.2}
        // END DESIGN TOKENS
    }
}
