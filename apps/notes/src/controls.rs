//! Plain content controls: no button face, bevel or independent glass shell.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.NotesTap = set_type_default() do #(NotesTap::register_widget(vm)) {
        width: 44 height: 44 padding: 0 spacing: 0
        show_bg: false
        align: Align{x: 0.5 y: 0.5}
        draw_feedback +: {
            color: theme.color_bg_highlight
            focus_color: uniform(theme.color_focus)
            amount: instance(0.0)
            focused: instance(0.0)
            opacity: instance(1.0)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 3.0)
                sdf.fill(vec4(self.color.rgb, self.amount * 0.65))
                sdf.stroke(vec4(self.focus_color.rgb, self.focused), 1.5)
                return sdf.result * self.opacity
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{from: {all: Play.Forward{duration: 0.1}} ease: Ease.OutCubic apply: {hovered: 0.0}}
                on: AnimatorState{from: {all: Play.Forward{duration: 0.1}} ease: Ease.OutCubic apply: {hovered: 0.4}}
            }
            press: {
                default: @off
                off: AnimatorState{from: {all: Play.Forward{duration: 0.12}} ease: Ease.OutCubic apply: {pressed: 0.0}}
                on: AnimatorState{from: {all: Play.Forward{duration: 0.07}} ease: Ease.OutCubic apply: {pressed: 1.0}}
            }
            select: {
                default: @off
                off: AnimatorState{from: {all: Play.Forward{duration: 0.12}} ease: Ease.OutCubic apply: {selected_fill: 0.0}}
                on: AnimatorState{from: {all: Play.Forward{duration: 0.12}} ease: Ease.OutCubic apply: {selected_fill: 1.0}}
            }
        }
    }
    mod.widgets.NotesSelection = set_type_default() do #(NotesSelection::register_widget(vm)) {
        width: Fill height: Fill visible: false
        margin: Inset{left: 8 right: 8}
        draw_bg +: {
            color: theme.color_bg_highlight
            amount: instance(0.0)
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 4.0)
                sdf.fill(vec4(self.color.rgb, self.color.a * self.amount))
                return sdf.result
            }
        }
        animator: Animator{select:{
            default: @off
            off: AnimatorState{from:{all:Play.Forward{duration:0.12}} ease:Ease.OutCubic apply:{amount:0.0}}
            on: AnimatorState{from:{all:Play.Forward{duration:0.12}} ease:Ease.OutCubic apply:{amount:1.0}}
        }}
    }
    mod.widgets.NotesRule = set_type_default() do #(NotesRule::register_widget(vm)) {
        width: Fill height: 1
        draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.14)}
    }
    mod.widgets.NotesMeta = set_type_default() do #(NotesMeta::register_widget(vm)) {
        padding: 0 max_lines: 1 text_overflow: TextOverflow.Ellipsis
        paper: theme.color_bg_app ink: theme.color_text secondary: theme.color_text_meta
        draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 9}}
    }
}

#[derive(Clone, Debug, Default)]
pub enum TapAction {
    #[default]
    None,
    Activated,
}
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct NotesTap {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_feedback: DrawColor,
    #[apply_default]
    animator: Animator,
    #[live]
    pub nav_name: String,
    #[live(true)]
    pub enabled: bool,
    #[live]
    pressed: f64,
    #[live(1.0)]
    pub opacity: f64,
    #[live]
    hovered: f64,
    #[live]
    selected_fill: f64,
    #[live]
    pub reduced_motion: bool,
    #[rust]
    selected: bool,
    #[rust]
    press_origin: Option<Vec2d>,
    #[rust]
    cancelled: bool,
    #[rust]
    keyboard_focus: bool,
}
impl NotesTap {
    fn feedback(&mut self, cx: &mut Cx, state: &[LiveId; 2]) {
        if self.reduced_motion {
            self.animator_cut(cx, state);
        } else {
            self.animator_play(cx, state);
        }
    }
    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool) {
        if selected == self.selected {
            return;
        }
        self.selected = selected;
        let state = if selected {
            ids!(select.on)
        } else {
            ids!(select.off)
        };
        if self.reduced_motion {
            self.animator_cut(cx, state);
        } else {
            self.animator_play(cx, state);
        }
    }
}
impl Widget for NotesTap {
    fn text(&self) -> String {
        self.nav_name.clone()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.view.visible || !self.enabled {
            return;
        }
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.view.area()) {
            Hit::FingerDown(e) if e.is_primary_hit() => {
                self.press_origin = Some(e.abs);
                self.cancelled = false;
                self.keyboard_focus = false;
                self.feedback(cx, ids!(press.on));
            }
            Hit::FingerMove(e) => {
                if self
                    .press_origin
                    .is_some_and(|p| (e.abs - p).length() > 8.0)
                {
                    self.cancelled = true;
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                if self.press_origin.take().is_some() && !self.cancelled && e.is_over && e.was_tap()
                {
                    cx.widget_action(uid, TapAction::Activated);
                }
                self.feedback(cx, ids!(press.off));
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.feedback(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.feedback(cx, ids!(hover.off));
            }
            Hit::KeyFocus(_) => {
                self.keyboard_focus = true;
                self.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.keyboard_focus = false;
                self.redraw(cx);
            }
            Hit::KeyDown(e) if matches!(e.key_code, KeyCode::ReturnKey | KeyCode::Space) => {
                cx.widget_action(uid, TapAction::Activated);
            }
            _ => {}
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut r = cx.peek_walk_turtle(walk);
        if !r.size.x.is_finite() {
            r.size.x = 0.0;
        }
        if !r.size.y.is_finite() {
            r.size.y = 0.0;
        }
        self.draw_feedback.draw_vars.set_dyn_instance(
            cx,
            live_id!(focused),
            &[if self.keyboard_focus { 1.0 } else { 0.0 }],
        );
        self.draw_feedback.draw_vars.set_dyn_instance(
            cx,
            live_id!(amount),
            &[self.pressed.max(self.selected_fill).max(self.hovered) as f32],
        );
        self.draw_feedback.draw_vars.set_dyn_instance(
            cx,
            live_id!(opacity),
            &[self.opacity as f32],
        );
        self.draw_feedback.draw_abs(cx, r);
        self.view.draw_walk(cx, scope, walk)?;
        let resolved = self.view.area().rect(cx);
        self.draw_feedback.area().set_rect(cx, &resolved);
        if self.enabled {
            cx.add_nav_stop(self.view.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct NotesRule {
    #[source]
    source: ScriptObjectRef,
    #[live(true)]
    #[visible]
    visible: bool,
    #[uid]
    uid: WidgetUid,
    #[live(1.0)]
    pub opacity: f64,
    #[live]
    bottom: bool,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
}
impl Widget for NotesRule {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let parent = cx.turtle().rect();
        let mut r = cx.walk_turtle(walk);
        let px = 1.0 / cx.current_dpi_factor();
        if self.bottom {
            r.pos.y = parent.pos.y + parent.size.y - px;
            r.size.y = px;
        } else if r.size.x <= 1.0 {
            r.size.x = px;
        } else {
            r.size.y = px;
        }
        self.draw_bg.color.w = self.opacity as f32;
        self.draw_bg.draw_abs(cx, r);
        DrawStep::done()
    }
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

pub fn readable_secondary(mut secondary: Vec4f, paper: Vec4f, ink: Vec4f) -> Vec4f {
    fn luminance(c: Vec4f) -> f32 {
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.x) + 0.7152 * linear(c.y) + 0.0722 * linear(c.z)
    }
    secondary.w = 1.0;
    let background = luminance(paper);
    for _ in 0..32 {
        let foreground = luminance(secondary);
        if (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05) >= 4.5 {
            break;
        }
        secondary = secondary * 0.9 + ink * 0.1;
    }
    secondary
}
#[derive(Script, ScriptHook, Widget)]
pub struct NotesMeta {
    #[deref]
    label: Label,
    #[live(1.0)]
    pub opacity: f64,
    #[live]
    paper: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    secondary: Vec4f,
}
impl Widget for NotesMeta {
    fn text(&self) -> String {
        self.label.text()
    }
    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.label.set_text(cx, text);
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.label.handle_event(cx, event, scope);
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut color = readable_secondary(self.secondary, self.paper, self.ink);
        color.w = self.opacity as f32;
        self.label.set_text_color(cx, color);
        self.label.draw_walk(cx, scope, walk)
    }
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct NotesSelection {
    #[source]
    source: ScriptObjectRef,
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[live]
    #[visible]
    visible: bool,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[apply_default]
    animator: Animator,
    #[live]
    amount: f64,
    #[rust]
    selected: bool,
}
impl NotesSelection {
    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool, reduced_motion: bool) {
        if self.selected == selected {
            return;
        }
        self.selected = selected;
        self.visible = true;
        let state = if selected {
            ids!(select.on)
        } else {
            ids!(select.off)
        };
        if reduced_motion {
            self.animator_cut(cx, state);
        } else {
            self.animator_play(cx, state);
        }
    }
}
impl Widget for NotesSelection {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg
            .draw_vars
            .set_dyn_instance(cx, live_id!(amount), &[self.amount as f32]);
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}
