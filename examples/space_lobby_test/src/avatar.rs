//! A simplified Avatar widget extracted from Robrix.
//! Holds either an image thumbnail or a single-character text label,
//! masked by a circle.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.Avatar = #(Avatar::register_widget(vm)) {
        width: 36.0,
        height: 36.0,
        align: Align{ x: 0.5, y: 0.5 }
        flow: Overlay,

        text_view := CircleView {
            visible: true,
            align: Align { x: 0.5, y: 0.5 }
            show_bg: true,
            draw_bg.color: (COLOR_AVATAR_BG)

            text := Label {
                padding: 0,
                margin: 0,
                width: Fit, height: Fit,
                flow: Right,
                align: Align{ x: 0.5, y: 0.5 }
                draw_text +: {
                    text_style: TITLE_TEXT { font_size: 15. }
                    color: #f,
                }
                text: "?"
            }
        }

        img_view := CircleView {
            visible: false,
            align: Align { x: 0.5, y: 0.5 }
            img := Image {
                fit: ImageFit.Stretch,
                width: Fill, height: Fill,
                draw_bg +: {
                    pixel: fn() {
                        let maxed = max(self.rect_size.x, self.rect_size.y);
                        let sdf = Sdf2d.viewport(self.pos * vec2(maxed, maxed));
                        let r = maxed * 0.5;
                        sdf.circle(r, r, r);
                        sdf.fill_keep(self.get_color());
                        return sdf.result
                    }
                }
            }
        }
    }
}


#[derive(ScriptHook, Script, Widget)]
pub struct Avatar {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
}

impl Widget for Avatar {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let f = user_name_first_letter(v)
            .unwrap_or("?").to_uppercase();
        self.label(cx, ids!(text_view.text)).set_text(cx, &f);
        self.view(cx, ids!(img_view)).set_visible(cx, false);
        self.view(cx, ids!(text_view)).set_visible(cx, true);
    }
}

impl Avatar {
    /// Sets the text content of this avatar.
    pub fn show_text(&mut self, cx: &mut Cx, bg_color: Option<Vec4>, text: &str) {
        self.set_text(cx, text);
        if let Some(bgc) = bg_color {
            let mut text_view = self.view(cx, ids!(text_view));
            script_apply_eval!(cx, text_view, {
                draw_bg.color: #(bgc)
            });
        }
    }
}

impl AvatarRef {
    pub fn show_text(&self, cx: &mut Cx, bg_color: Option<Vec4>, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_text(cx, bg_color, text);
        }
    }
}

/// Returns the first non-`@` Unicode grapheme of the given string.
pub fn user_name_first_letter(s: &str) -> Option<&str> {
    let s = s.trim().trim_start_matches('@');
    if s.is_empty() {
        return None;
    }
    // Simple: just take the first char
    s.chars().next().map(|c| &s[..c.len_utf8()])
}
