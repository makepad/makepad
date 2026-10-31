//! Screen spans demo -- exercises `makepad_wm_api::screens()` and
//! `set_fullscreen_span()` hosted (as a WM tile) and standalone (direct
//! mode, no window manager).
//!
//! `ScreenSpanView` draws every screen `wm_api::screens(cx)` reports as a
//! labelled rectangle at that screen's window-local rect (name, size, and
//! whether it is in the current span), or "no screens reported" when the
//! list is empty. Keys: `1` spans the first screen, `2` the first two,
//! `A` all of them, `C` the current one, `N` leaves fullscreen (no span).
//! It redraws on `WmEvent::Screens` (hosted: the WM sent a fresh list or
//! span) and on `Event::WindowGeomChange` (standalone: the window itself
//! moved/resized, e.g. the desktop's own screen list did not change but
//! the demo's idea of "window-local" did).

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ScreenSpanViewBase = #(ScreenSpanView::register_widget(vm))
    mod.widgets.ScreenSpanView = set_type_default() do mod.widgets.ScreenSpanViewBase{
        width: Fill
        height: Fill
        draw_bg +: { color: #x14181d }
        draw_border +: { color: #xe2e8f0 }
        draw_rect +: { color: #x2563ebcc }
        draw_rect_span +: { color: #x16a34acc }
        draw_text +: { color: #xffffff text_style.font_size: 14.0 }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1100, 700)
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down

                    SolidView{
                        width: Fill
                        height: Fit
                        padding: 8
                        draw_bg.color: #x0b0d10
                        status_label := Label{
                            text: "no screens reported"
                            draw_text.color: #xe2e8f0
                        }
                    }
                    screen_span := mod.widgets.ScreenSpanView{}
                }
            }
        }
    }
}

/// A screen's filled rect, in two flavours (spanned / not), with a
/// visible border: plain `DrawColor` has no border fields of its own
/// (`DrawQuad`/`DrawColor` carry none; that needs `RoundedView`'s own
/// shader), so the border is a second, larger quad drawn first
/// (`draw_border`) with the state-colored fill (`draw_rect` /
/// `draw_rect_span`) drawn over it, inset by `BORDER`.
#[derive(Script, ScriptHook, Widget)]
pub struct ScreenSpanView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_border: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_rect_span: DrawColor,
    #[live]
    draw_text: DrawText,
}

/// The border's thickness: the fill quad is inset by this on every side
/// of the border quad drawn under it.
const BORDER: f64 = 3.0;

impl Widget for ScreenSpanView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let pane = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, pane);

        let screens = makepad_wm_api::screens(cx.cx);
        if screens.is_empty() {
            self.draw_text.draw_abs(
                cx,
                dvec2(pane.pos.x + 16.0, pane.pos.y + 16.0),
                "no screens reported",
            );
            return DrawStep::done();
        }

        let span = makepad_wm_api::current_span(cx.cx);
        for screen in &screens {
            let in_span = span
                .as_ref()
                .is_some_and(|names| names.iter().any(|n| n == &screen.name));
            let rect = Rect {
                pos: dvec2(pane.pos.x + screen.x, pane.pos.y + screen.y),
                size: dvec2(screen.w.max(1.0), screen.h.max(1.0)),
            };
            self.draw_border.draw_abs(cx, rect);
            let fill = Rect {
                pos: dvec2(rect.pos.x + BORDER, rect.pos.y + BORDER),
                size: dvec2(
                    (rect.size.x - BORDER * 2.0).max(1.0),
                    (rect.size.y - BORDER * 2.0).max(1.0),
                ),
            };
            if in_span {
                self.draw_rect_span.draw_abs(cx, fill);
            } else {
                self.draw_rect.draw_abs(cx, fill);
            }
            let label = format!(
                "{}  {}x{}{}",
                screen.name,
                screen.w as i64,
                screen.h as i64,
                if in_span { "  [in span]" } else { "" }
            );
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 8.0), &label);
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    /// Recomputes the status line from the current screens/span and
    /// redraws the view: called on startup, on a key press, on a hosted
    /// `WmEvent::Screens`, and on a standalone window geometry change.
    fn refresh(&mut self, cx: &mut Cx) {
        let screens = makepad_wm_api::screens(cx);
        let status = if screens.is_empty() {
            "no screens reported".to_string()
        } else {
            match makepad_wm_api::current_span(cx) {
                Some(names) => format!("{} screen(s) -- span: {}", screens.len(), names.join(", ")),
                None => format!("{} screen(s) -- no span", screens.len()),
            }
        };
        self.ui.label(cx, ids!(status_label)).set_text(cx, &status);
        cx.redraw_all();
    }

    /// `ScreenSpan::Screens` of the first `n` live screens, left to
    /// right; `None` when there are no screens to span at all.
    fn first_n_span(cx: &Cx, n: usize) -> Option<makepad_wm_api::ScreenSpan> {
        let screens = makepad_wm_api::screens(cx);
        if screens.is_empty() {
            return None;
        }
        let names: Vec<String> = screens.iter().take(n).map(|s| s.name.clone()).collect();
        Some(makepad_wm_api::ScreenSpan::Screens(names))
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.refresh(cx);
    }

    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        let request = match e.key_code {
            KeyCode::Key1 => Self::first_n_span(cx, 1).map(Some),
            KeyCode::Key2 => Self::first_n_span(cx, 2).map(Some),
            KeyCode::KeyA => Some(Some(makepad_wm_api::ScreenSpan::All)),
            KeyCode::KeyC => Some(Some(makepad_wm_api::ScreenSpan::Current)),
            KeyCode::KeyN => Some(None),
            _ => None,
        };
        if let Some(span) = request {
            makepad_wm_api::set_fullscreen_span(cx, span);
            self.refresh(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        match event {
            // Hosted: the WM sent a fresh screen list and/or span.
            Event::Custom(json) => {
                if let Some(makepad_wm_api::WmEvent::Screens { .. }) =
                    makepad_wm_api::WmEvent::parse(json)
                {
                    self.refresh(cx);
                }
            }
            // Standalone: the window itself moved or resized.
            Event::WindowGeomChange(_) => self.refresh(cx),
            _ => {}
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
