//! About Synth: what it is and who made what it plays, from CREDITS.md
//! (the same text the repository carries), so a download's credit shows in
//! the app as well as the file.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let stage = mod.stage
    mod.widgets.SynthAboutPanel = Modal{
        content +: {width: 640 height: Fit flow: Down spacing: 10 padding: 18 show_bg: true
            draw_bg +: {
                color: stage.panel
                pixel: fn(){
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0., 0., self.rect_size.x, self.rect_size.y, 8.)
                    sdf.fill(vec4(self.color.xyz, 1.))
                    return sdf.result
                }
            }
            View{width: Fill height: Fit flow: Right spacing: 8
                Label{width: Fill text: "About Makepad Synth" draw_text +: {color: stage.text_bright text_style: theme.font_bold{font_size: 14}}}
                about_close := StageButton{width: 56 height: 24 text: "Close"}
            }
            ScrollYView{width: Fill height: 520 show_bg: true draw_bg +: {color: stage.well}
                about_text := Label{width: Fill height: Fit padding: 10 text: ""
                    draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 9.5}}}}
        }
    }
}

/// The credits shown.
pub const CREDITS: &str = include_str!("../CREDITS.md");

pub fn open(cx: &mut Cx, ui: &WidgetRef) {
    ui.label(cx, ids!(about_text)).set_text(cx, CREDITS);
    ui.modal(cx, ids!(about_panel)).open(cx);
}

pub fn handle_actions(cx: &mut Cx, ui: &WidgetRef, actions: &Actions) {
    if ui.button(cx, ids!(about_close)).clicked(actions) {
        ui.modal(cx, ids!(about_panel)).close(cx);
    }
    if ui.button(cx, ids!(score_about)).clicked(actions) {
        open(cx, ui);
    }
}
