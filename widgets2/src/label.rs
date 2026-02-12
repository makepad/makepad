use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LabelBase = #(Label::register_widget(vm))
    mod.widgets.Label = set_type_default() do mod.widgets.LabelBase{
        width: Fit
        height: Fit
        padding: theme.mspace_1

        draw_text +: {
            color_dither: uniform(1.0)
            color: theme.color_label_outer
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            gradient_fill_horizontal: uniform(0.0)

            get_color: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let mut color_2 = self.color_2

                let mut gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                if (self.color_2.x < -0.5) {
                    color_2 = self.color
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
            text_style: theme.font_regular{
                line_spacing: theme.font_wdgt_line_spacing
            }
        }
    }

    mod.widgets.Labelbold = mod.widgets.Label{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
    }

    mod.widgets.LabelGradientX = mod.widgets.Label{
        width: Fit
        height: Fit
        draw_text +: {
            color: #f00
            color_2: #ff0
            gradient_fill_horizontal: 1.0
        }
    }

    mod.widgets.LabelGradientY = mod.widgets.Label{
        draw_text +: {
            color: #f00
            color_2: #ff0
        }
    }

    mod.widgets.TextBox = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: Inset{left: 0., right: 0., top: theme.space_1, bottom: 0.}
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{
                line_spacing: theme.font_longform_line_spacing
                font_size: theme.font_size_p
            }
        }
        text: "TextBox"
    }

    mod.widgets.H1 = mod.widgets.Label{
        width: Fill
        padding: 0.
        draw_text +: {
            color: theme.color_text_hl
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1"
    }

    mod.widgets.H1italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1 italic"
    }

    mod.widgets.H2 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2"
    }

    mod.widgets.H2italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2 italic"
    }

    mod.widgets.H3 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3"
    }

    mod.widgets.H3italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3 italic"
    }

    mod.widgets.H4 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4"
    }

    mod.widgets.H4italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4 italic"
    }

    mod.widgets.P = mod.widgets.TextBox{
        text: "Paragraph"
    }

    mod.widgets.Pbold = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pitalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pbolditalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.IconSet = mod.widgets.Label{
        width: Fit
        draw_text +: {
            text_style: theme.font_icons{
                line_spacing: theme.font_wdgt_line_spacing
                font_size: 100.
            }
            color: theme.color_text
        }
        text: "Car"
    }
}

#[derive(Clone, Debug, Default)]
pub enum LabelAction {
    HoverIn(Rect),
    HoverOut,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Label {
    #[uid]
    uid: WidgetUid,
    #[redraw]
    #[live]
    draw_text: DrawText,

    #[walk]
    walk: Walk,
    #[live]
    align: Align,
    #[live(Flow::right_wrap())]
    flow: Flow,
    #[live]
    padding: Inset,

    #[rust]
    area: Area,
    #[live]
    text: ArcStringMut,

    // Indicates if this label responds to hover events
    // It is not turned on by default because it will consume finger events
    // and prevent other widgets from receiving them, if it is not considered with care
    // The primary use case for this kind of emitted actions is for tooltips displaying
    #[live(false)]
    hover_actions_enabled: bool,
}

impl Widget for Label {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let walk = walk.with_add_padding(self.padding);
        cx.begin_turtle(
            walk,
            Layout {
                flow: self.flow,
                ..Default::default()
            },
        );
        // here we need to check if the text is empty, if so we need to set it to a space
        // or the text draw will not work(seems like lazy drawtext bug)
        let _ = self.text.as_ref().is_empty().then(|| {
            let _ = self.set_text(cx, " ");
        });
        self.draw_text
            .draw_walk(cx, walk, self.align, self.text.as_ref());
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.widget_uid();

        match event.hit_designer(cx, self.area) {
            HitDesigner::DesignerPick(_e) => cx.widget_action(uid, WidgetDesignAction::PickedBody),
            _ => (),
        }

        if self.hover_actions_enabled {
            match event.hits_with_capture_overload(cx, self.area, true) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, LabelAction::HoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, LabelAction::HoverOut);
                }
                _ => (),
            }
        }
    }
}

impl LabelRef {
    pub fn hover_in(&self, actions: &Actions) -> Option<Rect> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverIn(rect) => Some(rect),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn hover_out(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverOut => true,
                _ => false,
            }
        } else {
            false
        }
    }

    pub fn set_text_with<F: FnOnce(&mut String)>(&self, f: F) {
        if let Some(mut inner) = self.borrow_mut() {
            f(inner.text.as_mut())
        }
    }
}
