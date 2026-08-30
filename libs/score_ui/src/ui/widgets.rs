//! Compact controls shared by the editor chrome and the transient pianist
//! controls. These deliberately restyle the standard widget kit so keyboard,
//! focus, hover, pressed, and disabled behaviour remain native.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.score.*
    use mod.widgets.*

    mod.widgets.ScoreLabel = Label{
        width: Fit
        height: Fit
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        draw_text +: {
            ink_centered: true
            color: score.color_text
            text_style: theme.font_regular{font_size: score.font_ui}
        }
    }

    mod.widgets.ScoreLabelDim = mod.widgets.ScoreLabel{
        draw_text +: {color: score.color_text_dim}
    }

    mod.widgets.ScoreLabelMuted = mod.widgets.ScoreLabel{
        draw_text +: {
            color: score.color_text_muted
            text_style: theme.font_regular{font_size: score.font_small}
        }
    }

    mod.widgets.ScoreHeader = mod.widgets.ScoreLabel{
        draw_text +: {
            color: score.color_text
            text_style: theme.font_bold{font_size: score.font_header}
        }
    }

    mod.widgets.ScoreTitle = mod.widgets.ScoreLabel{
        draw_text +: {
            color: score.color_text
            text_style: theme.font_bold{font_size: score.font_title}
        }
    }

    mod.widgets.ScoreButton = ButtonFlat{
        width: Fit
        height: score.row_height
        align: Align{x: 0.5 y: 0.5}
        margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
        padding: Inset{left: 9 right: 9 top: 0 bottom: 0}
        label_walk: Walk{width: Fit height: Fit}
        draw_bg +: {
            color: score.color_button
            color_hover: score.color_button_hover
            color_down: score.color_button_down
            color_focus: score.color_button
            color_disabled: score.color_panel_alt
            border_radius: score.radius
            border_size: 1.0
            border_color: score.color_border
            border_color_hover: score.color_border_light
            border_color_down: score.color_border
            border_color_focus: score.color_selection
            border_color_disabled: score.color_border
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_text +: {
            ink_centered: true
            color: score.color_text
            color_hover: #xffffff
            color_down: #xffffff
            color_focus: score.color_text
            color_disabled: score.color_text_muted
            text_style: theme.font_regular{font_size: score.font_ui}
        }
    }

    mod.widgets.ScoreButtonAccent = mod.widgets.ScoreButton{
        draw_bg +: {
            color: score.color_accent
            color_hover: score.color_accent_hover
            color_down: score.color_accent_dim
        }
        draw_text +: {color: score.color_text_on_accent}
    }

    mod.widgets.ScoreButtonFlat = mod.widgets.ScoreButton{
        draw_bg +: {
            color: #x00000000
            color_hover: score.color_row_hover
            color_down: score.color_button_down
            border_size: 0.0
        }
    }

    mod.widgets.ScoreMenuButton = mod.widgets.ScoreButtonFlat{
        height: score.menu_height
        padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
    }

    mod.widgets.ScoreToolButton = mod.widgets.ScoreButton{
        width: 31
        height: 28
        padding: Inset{left: 2 right: 2 top: 0 bottom: 0}
        draw_text +: {
            text_style: theme.font_bold{font_size: score.font_header}
        }
    }

    mod.widgets.ScoreTextInput = TextInput{
        width: Fill
        height: 27
        padding: Inset{left: 8 right: 8 top: 5 bottom: 4}
        draw_bg +: {
            color: score.color_input
            color_focus: score.color_input
            border_radius: score.radius
            border_size: 1.0
            border_color: score.color_border
            border_color_focus: score.color_selection
        }
        draw_text +: {
            color: score.color_text
            color_focus: score.color_text
            color_empty: score.color_text_muted
            text_style: theme.font_regular{font_size: score.font_ui}
        }
        draw_selection +: {color: score.color_selection}
        draw_cursor +: {color: score.color_text}
    }

    mod.widgets.ScorePanel = SolidView{
        width: Fill
        height: Fill
        flow: Down
        spacing: 0
        draw_bg +: {color: score.color_panel}
    }

    mod.widgets.ScorePanelHeader = SolidView{
        width: Fill
        height: 30
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 10 right: 7 top: 0 bottom: 0}
        spacing: 6
        draw_bg +: {color: score.color_panel_alt}
    }

    mod.widgets.ScoreRow = SolidView{
        width: Fill
        height: score.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 9 right: 9 top: 0 bottom: 0}
        spacing: 6
        draw_bg +: {color: #x00000000}
    }

    mod.widgets.ScoreSection = mod.widgets.ScoreRow{
        height: 27
        draw_bg +: {color: score.color_panel_alt}
    }

    mod.widgets.ScoreDivider = SolidView{
        width: Fill
        height: 1
        draw_bg +: {color: score.color_border}
    }

    mod.widgets.ScoreFloatingBar = RoundedView{
        width: Fit
        height: 42
        flow: Right
        align: Align{x: 0.5 y: 0.5}
        padding: Inset{left: 7 right: 7 top: 6 bottom: 6}
        spacing: 5
        draw_bg +: {
            color: score.color_float
            border_radius: score.radius_large
            border_size: 1.0
            border_color: score.color_float_border
        }
    }

    mod.widgets.ScorePopup = RoundedView{
        width: 230
        height: Fit
        flow: Down
        spacing: 0
        padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
        draw_bg +: {
            color: score.color_float
            border_radius: score.radius_large
            border_size: 1.0
            border_color: score.color_float_border
        }
    }

    mod.widgets.ScoreMenuRow = mod.widgets.ScoreButtonFlat{
        width: Fill
        height: 25
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
    }

    mod.widgets.ScoreKeyCap = RoundedView{
        width: 92
        height: 23
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            color: score.color_button
            border_radius: score.radius
            border_size: 1.0
            border_color: score.color_border_light
        }
    }

    mod.widgets.ScoreScrub = SliderMinimalFlat{
        width: Fill
        height: 24
        min: 0.0
        max: 1536.0
        step: 0.0
        margin: Inset{left: 5 right: 5 top: 0 bottom: 0}
        label_walk: Walk{width: 0 height: 0}
        text_input +: {width: 0 height: 0}
        draw_bg +: {
            offset_y: 10.0
            handle_size: 9.0
            border_size: 0.0
            color: score.color_input
            color_hover: score.color_input
            color_focus: score.color_input
            color_drag: score.color_input
            color_2: score.color_input
            color_2_hover: score.color_input
            color_2_focus: score.color_input
            color_2_drag: score.color_input
            val_color: score.color_accent
            val_color_hover: score.color_accent_hover
            val_color_focus: score.color_accent_hover
            val_color_drag: score.color_accent_hover
            handle_color: score.color_text
            handle_color_hover: #xffffff
            handle_color_focus: #xffffff
            handle_color_drag: #xffffff
        }
    }
}
