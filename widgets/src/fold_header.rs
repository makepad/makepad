use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    fold_button::FoldButtonAction,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FoldHeaderBase = #(FoldHeader::register_widget(vm))

    mod.widgets.FoldHeader = set_type_default() do mod.widgets.FoldHeaderBase{
        width: Fill
        height: Fit
        body_walk: Walk{width: Fill, height: Fit}

        flow: Down

        animator: Animator{
            active: {
                default: @on
                off: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        opened: 0.0
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        opened: 1.0
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
enum DrawState {
    DrawHeader,
    DrawBody,
}

#[derive(Clone, Default)]
pub enum FoldHeaderAction {
    Opening,
    Closing,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct FoldHeader {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    rect_size: f64,
    /// Set by the header step when the body is drawn unconstrained (fully
    /// open or first draw) — the only draw whose used height is the truth.
    #[rust]
    measure_body: bool,
    #[rust]
    area: Area,
    #[find]
    #[redraw]
    #[live]
    header: WidgetRef,
    #[find]
    #[redraw]
    #[live]
    body: WidgetRef,
    #[apply_default]
    animator: Animator,

    #[live]
    opened: f64,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
    #[live]
    body_walk: Walk,
}

impl Widget for FoldHeader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.area.redraw(cx);
        }

        self.header.handle_event(cx, event, scope);
        self.body.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            let button = self.header.widget(cx, ids!(fold_button));
            if button.is_empty() {
                return;
            }
            match actions
                .find_widget_action(button.widget_uid())
                .map(|action| action.cast::<FoldButtonAction>())
            {
                Some(FoldButtonAction::Opening) => {
                    self.animator_play(cx, ids!(active.on));
                }
                Some(FoldButtonAction::Closing) => {
                    self.animator_play(cx, ids!(active.off));
                }
                _ => (),
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::DrawHeader) {
            cx.begin_turtle(walk, self.layout);
        }
        if let Some(DrawState::DrawHeader) = self.draw_state.get() {
            let walk = self.header.walk(cx);
            self.header.draw_walk(cx, scope, walk)?;

            // Fully open (or never measured): draw the body with its own walk,
            // so a Fill body keeps tracking the window and we can measure its
            // unconstrained height. While folding/unfolding, clamp the body to
            // `opened * rect_size` and scroll the hidden part away.
            let unconstrained = self.rect_size == 0.0 || self.opened >= 1.0;
            let (body_walk, scroll_y) = if unconstrained {
                (self.body_walk, 0.0)
            } else {
                let body_walk = Walk {
                    height: Size::Fixed(self.rect_size * self.opened),
                    ..self.body_walk
                };
                let scroll_y = self.rect_size * (1.0 - self.opened);
                (body_walk, scroll_y)
            };

            cx.begin_turtle(
                body_walk,
                Layout::flow_down().with_scroll(dvec2(0.0, scroll_y)),
            );
            self.draw_state.set(DrawState::DrawBody);
            self.measure_body = unconstrained;
        }
        if let Some(DrawState::DrawBody) = self.draw_state.get() {
            let walk = self.body.walk(cx);
            self.body.draw_walk(cx, scope, walk)?;
            // Remember the content height ONLY from an unconstrained draw. A
            // folded or animating body is drawn into a shorter turtle, and
            // measuring it there would shrink the remembered size: a Fill body
            // collapses to its Fit children while closed, and re-opening then
            // gave the body that collapsed height (the chat text box never
            // came back).
            if self.measure_body {
                let used_y = cx.turtle().used().y;
                if used_y > 0.0 {
                    self.rect_size = used_y;
                }
            }
            cx.end_turtle();
            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl FoldHeader {
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, ids!(active.on), ids!(active.off));
        // Also toggle the fold button if it exists
        if let Some(mut fold_button) = self
            .header
            .widget(cx, ids!(fold_button))
            .borrow_mut::<crate::fold_button::FoldButton>()
        {
            fold_button.set_is_open(cx, is_open, animate);
        }
    }

    pub fn is_open(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }

    pub fn opened(&self) -> f64 {
        self.opened
    }
}

impl FoldHeaderRef {
    pub fn set_is_open(&self, cx: &mut Cx, is_open: bool, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_is_open(cx, is_open, animate);
        }
    }

    pub fn is_open(&self, cx: &Cx) -> bool {
        self.borrow().map_or(true, |inner| inner.is_open(cx))
    }

    pub fn opened(&self) -> f64 {
        self.borrow().map_or(1.0, |inner| inner.opened())
    }
}
