use {
    crate::{
        animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
        makepad_derive_widget::*,
        makepad_draw::{event::{DigitId, SweepLock, TouchState}, *},
        overlay_place::{place_overlay, span_inboard, PlaceRequest, Placement},
        widget::*,
    },
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawDropDown2LabelBase = #(DrawDropDown2Label::script_component(vm))
    set_type_default() do #(DrawDropDown2Label::script_shader(vm)){
        ..mod.draw.DrawText
    }
    mod.widgets.DrawDropDown2ItemTextBase = #(DrawDropDown2ItemText::script_component(vm))
    set_type_default() do #(DrawDropDown2ItemText::script_shader(vm)){
        ..mod.draw.DrawText
    }
    mod.widgets.DrawDropDown2ItemBgBase = #(DrawDropDown2ItemBg::script_component(vm))
    set_type_default() do #(DrawDropDown2ItemBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawDropDown2ArrowBase = #(DrawDropDown2Arrow::script_component(vm))
    set_type_default() do #(DrawDropDown2Arrow::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DropDown2Base = #(DropDown2::register_widget(vm))

    mod.widgets.DropDown2Flat = set_type_default() do mod.widgets.DropDown2Base{
        width: Fit
        height: Fit
        align: TopLeft
        padding: theme.mspace_1{left: theme.space_2, right: 22.5}
        margin: theme.mspace_v_1{}
        item_height: 22.0
        arrow_height: 16.0
        popup_margin: 8.0
        popup_padding: 3.0
        popup_min_width: 0.0
        item_padding: Inset{left: 10.0 right: 8.0}

        draw_text +: {
            disabled: instance(0.0)
            down: instance(0.0)
            ink_centered: true
            text_overflow: Ellipsis
            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_focus: uniform(theme.color_label_inner_focus)
            color_down: uniform(theme.color_label_inner_down)
            color_disabled: uniform(theme.color_label_inner_disabled)
            text_style: theme.font_regular{ font_size: theme.font_size_p }
            get_color: fn() {
                mix(
                    mix(
                        mix(self.color, self.color_focus, self.focus),
                        mix(self.color_hover, self.color_down, self.down),
                        self.hover
                    ),
                    self.color_disabled,
                    self.disabled
                )
            }
        }

        draw_bg +: {
            hover: instance(0.0)
            focus: instance(0.0)
            down: instance(0.0)
            disabled: instance(0.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)
            color: uniform(theme.color_outset)
            color_hover: uniform(theme.color_outset_hover)
            color_focus: uniform(theme.color_outset_focus)
            color_down: uniform(theme.color_outset_down)
            color_disabled: uniform(theme.color_outset_disabled)
            border_color: uniform(theme.color_bevel)
            border_color_hover: uniform(theme.color_bevel_hover)
            border_color_focus: uniform(theme.color_bevel_focus)
            border_color_down: uniform(theme.color_bevel_down)
            arrow_color: uniform(theme.color_label_inner)
            arrow_color_hover: uniform(theme.color_label_inner_hover)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down * self.hover)
                    .mix(self.color_disabled, self.disabled)
                sdf.fill_keep(fill)
                sdf.stroke(
                    self.border_color
                        .mix(self.border_color_focus, self.focus)
                        .mix(self.border_color_hover, self.hover)
                        .mix(self.border_color_down, self.down * self.hover)
                    self.border_size
                )
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5
                sdf.move_to(c.x - sz, c.y - sz * 0.5)
                sdf.line_to(c.x + sz, c.y - sz * 0.5)
                sdf.line_to(c.x, c.y + sz)
                sdf.close_path()
                sdf.fill(self.arrow_color.mix(self.arrow_color_hover, self.hover))
                sdf.result
            }
        }

        draw_popup_bg +: {
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)
            color: uniform(theme.color_fg_app)
            border_color: uniform(theme.color_bevel)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                sdf.result
            }
        }

        draw_item +: {
            hover: 0.0
            active: 0.0
            color: uniform(theme.color_u_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_active: uniform(theme.color_outset_active)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0., 0., self.rect_size.x, self.rect_size.y)
                sdf.fill(
                    self.color
                        .mix(self.color_active, self.active)
                        .mix(self.color_hover, self.hover)
                )
                sdf.result
            }
        }

        draw_item_text +: {
            hover: 0.0
            active: 0.0
            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)
            text_style: theme.font_regular{ font_size: theme.font_size_p }
            text_overflow: Ellipsis
            get_color: fn() {
                self.color.mix(self.color_active, self.active).mix(self.color_hover, self.hover)
            }
        }

        draw_scroll_arrow +: {
            up: 0.0
            enabled: 1.0
            color: uniform(theme.color_label_inner)
            color_disabled: uniform(theme.color_label_inner_disabled)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0., 0., self.rect_size.x, self.rect_size.y)
                sdf.fill(#0000)
                let c = vec2(self.rect_size.x * 0.5, self.rect_size.y * 0.5)
                let sz = 3.5
                if self.up > 0.5 {
                    sdf.move_to(c.x - sz, c.y + sz * 0.45)
                    sdf.line_to(c.x + sz, c.y + sz * 0.45)
                    sdf.line_to(c.x, c.y - sz * 0.55)
                } else {
                    sdf.move_to(c.x - sz, c.y - sz * 0.45)
                    sdf.line_to(c.x + sz, c.y - sz * 0.45)
                    sdf.line_to(c.x, c.y + sz * 0.55)
                }
                sdf.close_path()
                sdf.fill(mix(self.color_disabled, self.color, self.enabled))
                sdf.result
            }
        }

        selected_item: 0

        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.}}
                    apply: { draw_bg: {disabled: 0.0} draw_text: {disabled: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: { draw_bg: {disabled: 1.0} draw_text: {disabled: 1.0} }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: { all: Forward{duration: 0.1} down: Forward{duration: 0.01} }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}]}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}]}
                    }
                }
                down: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    apply: { draw_bg: {focus: 0.0} draw_text: {focus: 0.0} }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Arrow
                    from: {all: Forward{duration: 0.0}}
                    apply: { draw_bg: {focus: 1.0} draw_text: {focus: 1.0} }
                }
            }
        }
    }

    mod.widgets.DropDown2 = set_type_default() do mod.widgets.DropDown2Flat{}
}

const ARROW_SCROLL_PX_PER_SEC: f64 = 280.0;

/// Covering popup: selected row stays under the trigger; overflow
/// clamps to the pass and shows ▲/▼ scroll arrows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoveringPopupGeom {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub list_y: f64,
    pub list_h: f64,
    pub scroll: f64,
    pub max_scroll: f64,
    pub show_up: bool,
    pub show_down: bool,
    pub item_h: f64,
    pub pad: f64,
}

impl CoveringPopupGeom {
    pub fn popup_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x, self.y),
            size: dvec2(self.width, self.height),
        }
    }

    pub fn list_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x + self.pad, self.list_y),
            size: dvec2((self.width - self.pad * 2.0).max(0.0), self.list_h),
        }
    }

    pub fn up_rect(&self) -> Option<Rect> {
        if self.list_y <= self.y {
            return None;
        }
        Some(Rect {
            pos: dvec2(self.x, self.y),
            size: dvec2(self.width, self.list_y - self.y),
        })
    }

    pub fn down_rect(&self) -> Option<Rect> {
        if self.list_y + self.list_h >= self.y + self.height {
            return None;
        }
        let y = self.list_y + self.list_h;
        Some(Rect {
            pos: dvec2(self.x, y),
            size: dvec2(self.width, (self.y + self.height - y).max(0.0)),
        })
    }

    pub fn item_at(&self, abs: Vec2d, count: usize) -> Option<usize> {
        let list = self.list_rect();
        if !list.contains(abs) {
            return None;
        }
        let local = abs.y - list.pos.y + self.scroll - self.pad;
        if local < 0.0 {
            return None;
        }
        let i = (local / self.item_h.max(1.0)).floor() as usize;
        if i < count {
            Some(i)
        } else {
            None
        }
    }
}

pub fn layout_covering_popup(
    pass: Vec2d,
    trigger: Rect,
    item_count: usize,
    selected: usize,
    item_h: f64,
    content_w: f64,
    pad: f64,
    margin: f64,
    arrow_h: f64,
    scroll: Option<f64>,
) -> CoveringPopupGeom {
    let n = item_count.max(1);
    let selected = selected.min(n.saturating_sub(1));
    let item_h = item_h.max(1.0);
    let margin_x = margin.max(0.0).min(pass.x.max(0.0) * 0.5);
    let margin_y = margin.max(0.0).min(pass.y.max(0.0) * 0.5);
    let available_w = (pass.x - margin_x * 2.0).max(0.0);
    let available_h = (pass.y - margin_y * 2.0).max(0.0);
    let pad = pad.max(0.0).min(available_h * 0.25).min(available_w * 0.5);
    let content_h = pad * 2.0 + n as f64 * item_h;
    let trigger_center = trigger.pos.y + trigger.size.y * 0.5;
    let selected_center = pad + (selected as f64 + 0.5) * item_h;
    let ideal_y = trigger_center - selected_center;
    let height = content_h.min(available_h);
    // The selected row covers the trigger. Share horizontal placement with
    // other overlays while keeping the popup's own vertical alignment.
    let placed = place_overlay(&PlaceRequest {
        anchor: trigger,
        size: dvec2(content_w.max(0.0), height),
        bounds: Rect {
            pos: dvec2(margin_x, 0.0),
            size: dvec2(available_w, 0.0),
        },
        gap: 0.0,
        placement: Placement::BOTTOM_START,
        match_anchor_width: true,
    });
    // The shared helper treats a zero extent as unbounded; this popup has
    // measured its pass, so zero room really means there is no space.
    let x = if available_w > 0.0 { placed.rect.pos.x } else { margin_x };
    let width = placed.rect.size.x.max(0.0).min(available_w);
    let y = if available_h > 0.0 {
        span_inboard(ideal_y, height, margin_y, available_h)
    } else {
        margin_y
    };
    let overflow = content_h > height;
    let arrow_h = if overflow {
        arrow_h.max(0.0).min((height - item_h).max(0.0) * 0.5)
    } else {
        0.0
    };
    let list_h = (height - arrow_h * 2.0).max(0.0);
    let list_y = y + arrow_h;
    let max_scroll = (content_h - list_h).max(0.0);
    let aligned = (list_y + selected_center - trigger_center).clamp(0.0, max_scroll);
    let scroll = scroll.unwrap_or(aligned).clamp(0.0, max_scroll);

    CoveringPopupGeom {
        x,
        y,
        width,
        height,
        list_y,
        list_h,
        scroll,
        max_scroll,
        show_up: overflow && scroll > 0.5,
        show_down: overflow && scroll < max_scroll - 0.5,
        item_h,
        pad,
    }
}

pub fn estimate_label_width(labels: &[String], font_px: f64) -> f64 {
    let n = labels
        .iter()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(1) as f64;
    n * font_px.max(6.0) * 0.62 + 28.0
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawDropDown2Label {
    #[deref]
    draw_super: DrawText,
    #[live]
    focus: f32,
    #[live]
    hover: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawDropDown2ItemText {
    #[deref]
    draw_super: DrawText,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawDropDown2ItemBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawDropDown2Arrow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    up: f32,
    #[live]
    enabled: f32,
}

#[derive(Script, WidgetRegister, WidgetRef, WidgetSet, Animator)]
pub struct DropDown2 {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawDropDown2Label,
    #[live]
    draw_popup_bg: DrawQuad,
    #[live]
    draw_item: DrawDropDown2ItemBg,
    #[live]
    draw_item_text: DrawDropDown2ItemText,
    #[live]
    draw_scroll_arrow: DrawDropDown2Arrow,
    #[live]
    draw_list: DrawList2d,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live(true)]
    visible: bool,
    #[live]
    labels: Vec<String>,
    #[live]
    selected_item: usize,
    #[live(22.0)]
    item_height: f64,
    #[live(16.0)]
    arrow_height: f64,
    #[live(8.0)]
    popup_margin: f64,
    #[live(3.0)]
    popup_padding: f64,
    #[live]
    popup_min_width: f64,
    #[live]
    item_padding: Inset,

    #[rust]
    is_active: bool,
    /// Held while the list is open, so `Escape` belongs to it rather than to
    /// whatever it was opened in front of.
    #[rust]
    cancel_scope: Option<CancelScope>,
    #[rust]
    sweep_lock: Option<SweepLock>,
    #[rust]
    items_before_apply: Option<(Vec<String>, usize)>,
    #[rust]
    pointer: Option<PopupGesture>,
    #[rust]
    search: String,
    #[rust]
    search_timer: Timer,
    #[rust]
    ensure_selected_visible: bool,
    #[rust]
    hover_item: Option<usize>,
    #[rust]
    scroll: Option<f64>,
    #[rust]
    geom: Option<CoveringPopupGeom>,
    /// The field's rect as last seen BETWEEN draws (final, aligned). During
    /// a draw the field's own area still sits at its pre-alignment position
    /// when it follows a Fill sibling, so the popup cannot be placed from it.
    #[rust]
    aligned_rect: Option<Rect>,
    #[rust]
    arrow_dir: Option<f64>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_frame_time: f64,

    #[rust]
    action_data: WidgetActionData,
}

impl ScriptHook for DropDown2 {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !apply.is_animate() {
            self.items_before_apply = Some((self.labels.clone(), self.selected_item));
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_animate() {
            return;
        }
        if self.items_before_apply.take().is_some_and(|(labels, selected)| {
            labels != self.labels || selected != self.selected_item
        }) {
            self.reset_items(vm.cx_mut());
        }
        if !self.visible {
            self.set_closed(vm.cx_mut());
            self.clear_focus(vm.cx_mut());
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PopupPointer {
    Mouse,
    Touch(u64),
}

#[derive(Clone, Copy)]
struct PopupGesture {
    pointer: PopupPointer,
    start: Vec2d,
    last: Vec2d,
    scroll: f64,
    opening: bool,
    moved: bool,
    scrolling: bool,
}

#[derive(Clone, Debug, Default)]
pub enum DropDown2Action {
    Select(usize),
    #[default]
    None,
}

impl DropDown2 {
    fn clamp_selected(&mut self) {
        if self.labels.is_empty() {
            self.selected_item = 0;
        } else {
            self.selected_item = self.selected_item.min(self.labels.len() - 1);
        }
    }

    pub fn set_active(&mut self, cx: &mut Cx) {
        if self.sweep_lock_expired() {
            self.set_closed(cx);
        }
        if self.is_active || !self.visible || self.labels.is_empty() || self.disabled(cx) {
            return;
        }
        let Some(lock) = cx.acquire_sweep_lock(self.draw_bg.area()) else {
            return;
        };
        self.sweep_lock = Some(lock);
        self.clamp_selected();
        self.is_active = true;
        self.cancel_scope = Some(self.begin_cancel_scope(cx));
        self.hover_item = Some(self.selected_item);
        self.scroll = None;
        self.pointer = None;
        self.arrow_dir = None;
        self.geom = None;
        self.clear_search(cx);
        cx.set_key_focus(self.draw_bg.area());
        self.redraw_popup(cx);
    }

    pub fn set_closed(&mut self, cx: &mut Cx) {
        let was_active = self.is_active;
        self.is_active = false;
        if let Some(scope) = self.cancel_scope.take() {
            cx.end_cancel_scope(scope);
        }
        self.pointer = None;
        self.arrow_dir = None;
        self.geom = None;
        self.hover_item = None;
        self.clear_search(cx);
        self.sweep_lock = None;
        if was_active {
            self.redraw_popup(cx);
            self.animator_play(cx, ids!(hover.off));
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_active && !self.sweep_lock_expired()
    }

    fn sweep_lock_expired(&self) -> bool {
        self.is_active && !self.sweep_lock.as_ref().is_some_and(SweepLock::is_active)
    }

    fn redraw_popup(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
    }

    fn clear_focus(&mut self, cx: &mut Cx) {
        if cx.has_key_focus(self.draw_bg.area()) {
            cx.set_key_focus(Area::Empty);
            self.animator_play(cx, ids!(focus.off));
        }
    }

    fn clear_search(&mut self, cx: &mut Cx) {
        self.search.clear();
        cx.stop_timer(self.search_timer);
        self.search_timer = Timer::empty();
    }

    fn reset_items(&mut self, cx: &mut Cx) {
        self.clamp_selected();
        if self.labels.is_empty() {
            self.set_closed(cx);
            self.clear_focus(cx);
        } else if self.is_active {
            self.hover_item = Some(self.selected_item);
            self.pointer = None;
            self.arrow_dir = None;
            self.scroll = None;
            self.geom = None;
            self.ensure_selected_visible = true;
        }
        self.clear_search(cx);
        self.redraw_popup(cx);
    }

    fn emit_select(&mut self, cx: &mut Cx, index: usize) {
        if self.labels.is_empty() || self.disabled(cx) {
            return;
        }
        let index = index.min(self.labels.len() - 1);
        if self.selected_item == index {
            return;
        }
        self.selected_item = index;
        cx.widget_action_with_data(
            &self.action_data,
            self.uid,
            DropDown2Action::Select(self.selected_item),
        );
        self.draw_bg.redraw(cx);
    }

    fn apply_scroll(&mut self, cx: &mut Cx, scroll: f64) {
        let max = self.geom.map(|g| g.max_scroll).unwrap_or(0.0);
        let next = scroll.clamp(0.0, max);
        if self.scroll != Some(next) {
            self.scroll = Some(next);
            if let Some(geom) = self.geom.as_mut() {
                geom.scroll = next;
                geom.show_up = next > 0.5;
                geom.show_down = next < geom.max_scroll - 0.5;
            }
            self.draw_list.redraw(cx);
            self.draw_bg.redraw(cx);
        }
    }

    fn scroll_item_into_view(&mut self, cx: &mut Cx, index: usize) {
        let Some(g) = self.geom else {
            return;
        };
        let top = g.pad + index as f64 * g.item_h;
        let bot = top + g.item_h;
        let mut scroll = self.scroll.unwrap_or(g.scroll);
        if top < scroll {
            scroll = top;
        } else if bot > scroll + g.list_h {
            scroll = bot - g.list_h;
        }
        self.apply_scroll(cx, scroll);
    }

    fn hit_zone(&self, abs: Vec2d) -> PopupHit {
        let Some(g) = self.geom else {
            return PopupHit::Outside;
        };
        if g.up_rect().is_some_and(|r| r.contains(abs)) {
            return PopupHit::UpArrow;
        }
        if g.down_rect().is_some_and(|r| r.contains(abs)) {
            return PopupHit::DownArrow;
        }
        if let Some(i) = g.item_at(abs, self.labels.len()) {
            return PopupHit::Item(i);
        }
        if g.popup_rect().contains(abs) {
            return PopupHit::Chrome;
        }
        PopupHit::Outside
    }

    fn set_hover_item(&mut self, cx: &mut Cx, item: Option<usize>) {
        if self.hover_item != item {
            self.hover_item = item;
            self.draw_list.redraw(cx);
        }
    }

    fn set_arrow_dir(&mut self, cx: &mut Cx, direction: Option<f64>) {
        let direction = direction.filter(|dir| self.geom.is_some_and(|g| {
            if *dir < 0.0 { g.scroll > 0.0 } else { g.scroll < g.max_scroll }
        }));
        if self.arrow_dir != direction {
            self.arrow_dir = direction;
            self.last_frame_time = 0.0;
            if direction.is_some() {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn pointer_down(&mut self, cx: &mut Cx, pointer: PopupPointer, abs: Vec2d, opening: bool) {
        if self.pointer.is_some() {
            return;
        }
        let zone = self.hit_zone(abs);
        if !opening && matches!(zone, PopupHit::Outside) {
            self.set_closed(cx);
            return;
        }
        self.clear_search(cx);
        self.pointer = Some(PopupGesture {
            pointer,
            start: abs,
            last: abs,
            scroll: self.scroll.unwrap_or(0.0),
            opening,
            moved: false,
            scrolling: matches!(zone, PopupHit::UpArrow | PopupHit::DownArrow),
        });
        match zone {
            PopupHit::Item(i) => self.set_hover_item(cx, Some(i)),
            PopupHit::UpArrow => self.set_arrow_dir(cx, Some(-1.0)),
            PopupHit::DownArrow => self.set_arrow_dir(cx, Some(1.0)),
            _ => (),
        }
    }

    fn pointer_move(&mut self, cx: &mut Cx, pointer: PopupPointer, abs: Vec2d) {
        if let Some(mut gesture) = self.pointer {
            if gesture.pointer != pointer {
                return;
            }
            let delta = abs - gesture.start;
            gesture.last = abs;
            gesture.moved |= delta.x.abs() > 5.0 || delta.y.abs() > 5.0;
            if matches!(pointer, PopupPointer::Touch(_)) && gesture.moved
                && self.geom.is_some_and(|g| g.max_scroll > 0.0)
            {
                gesture.scrolling = true;
                self.set_arrow_dir(cx, None);
                self.set_hover_item(cx, None);
                self.apply_scroll(cx, gesture.scroll - delta.y);
                self.pointer = Some(gesture);
                return;
            }
            self.pointer = Some(gesture);
        } else if matches!(pointer, PopupPointer::Touch(_)) {
            return;
        }
        if self.geom.is_none() {
            return;
        }
        match self.hit_zone(abs) {
            PopupHit::Item(i) => {
                self.set_arrow_dir(cx, None);
                self.set_hover_item(cx, Some(i));
            }
            PopupHit::UpArrow => {
                self.set_hover_item(cx, None);
                self.set_arrow_dir(cx, Some(-1.0));
            }
            PopupHit::DownArrow => {
                self.set_hover_item(cx, None);
                self.set_arrow_dir(cx, Some(1.0));
            }
            _ => {
                self.set_arrow_dir(cx, None);
                self.set_hover_item(cx, None);
            }
        }
    }

    fn pointer_up(&mut self, cx: &mut Cx, pointer: PopupPointer, abs: Vec2d) {
        let Some(gesture) = self.pointer.filter(|g| g.pointer == pointer) else {
            return;
        };
        self.pointer = None;
        self.set_arrow_dir(cx, None);
        if gesture.scrolling || (gesture.opening && self.geom.is_none())
            || (gesture.opening && !gesture.moved
                && self.aligned_rect.is_some_and(|rect| rect.contains(abs)))
        {
            return;
        }
        match self.hit_zone(abs) {
            PopupHit::Item(i) => {
                self.emit_select(cx, i);
                self.set_closed(cx);
            }
            PopupHit::Outside => self.set_closed(cx),
            _ => (),
        }
    }

    fn navigate_to(&mut self, cx: &mut Cx, index: usize) {
        if self.labels.is_empty() {
            return;
        }
        let index = index.min(self.labels.len() - 1);
        self.pointer = None;
        self.set_arrow_dir(cx, None);
        if self.is_active {
            self.set_hover_item(cx, Some(index));
            self.ensure_selected_visible = true;
            self.scroll_item_into_view(cx, index);
            self.redraw_popup(cx);
        } else {
            self.emit_select(cx, index);
        }
    }

    fn type_ahead(&mut self, cx: &mut Cx, input: &str) {
        if input.is_empty() || input.chars().any(char::is_control)
            || (input.trim().is_empty() && self.search.is_empty())
        {
            return;
        }
        let input = input.to_lowercase();
        let repeated = self.search == input && input.chars().count() == 1;
        let continuing = !self.search.is_empty() && !repeated;
        if !repeated {
            self.search.push_str(&input);
        }
        cx.stop_timer(self.search_timer);
        self.search_timer = cx.start_timeout(1.0);
        let count = self.labels.len();
        if count == 0 {
            return;
        }
        let current = self.hover_item.unwrap_or(self.selected_item);
        let start = if continuing { current } else { (current + 1) % count };
        let found = (0..count).map(|offset| (start + offset) % count)
            .find(|&index| self.labels[index].to_lowercase().starts_with(&self.search));
        if let Some(index) = found {
            self.navigate_to(cx, index);
        }
    }

    fn draw_field(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_bg.begin(cx, walk, self.layout);
        let label = self
            .labels
            .get(self.selected_item)
            .map(String::as_str)
            .unwrap_or(" ");
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
        if !self.disabled(cx) && !self.labels.is_empty() {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());
        }
    }

    fn draw_popup(&mut self, cx: &mut Cx2d, trigger: Rect) {
        let pass = cx.current_pass_size();
        let first_draw = self.geom.is_none();
        let mut content_w = 0.0_f64;
        let mut item_h = self.item_height;
        for label in &self.labels {
            let text = self.draw_item_text.layout(cx, 0.0, 0.0, None, false, Align::default(), label);
            let scale = self.draw_item_text.font_scale as f64;
            content_w = content_w.max(text.size_in_lpxs.width as f64 * scale
                + self.item_padding.left + self.item_padding.right);
            item_h = item_h.max(text.size_in_lpxs.height as f64 * scale
                + self.item_padding.top + self.item_padding.bottom);
        }
        content_w = (content_w + self.popup_padding.max(0.0) * 2.0).max(self.popup_min_width);
        let mut geom = layout_covering_popup(
            pass,
            trigger,
            self.labels.len(),
            self.selected_item,
            item_h,
            content_w,
            self.popup_padding,
            self.popup_margin,
            self.arrow_height,
            self.scroll,
        );
        if self.ensure_selected_visible {
            let index = self.hover_item.unwrap_or(self.selected_item);
            let top = geom.pad + index as f64 * geom.item_h;
            let bottom = top + geom.item_h;
            geom.scroll = geom.scroll.min(top).max(bottom - geom.list_h).clamp(0.0, geom.max_scroll);
            geom.show_up = geom.scroll > 0.5;
            geom.show_down = geom.scroll < geom.max_scroll - 0.5;
            self.ensure_selected_visible = false;
        }
        if let Some(gesture) = self.pointer.as_mut() {
            if gesture.opening && first_draw {
                gesture.scroll = geom.scroll;
                if gesture.moved && matches!(gesture.pointer, PopupPointer::Touch(_))
                    && geom.max_scroll > 0.0
                {
                    gesture.scrolling = true;
                    geom.scroll = (gesture.scroll - (gesture.last.y - gesture.start.y))
                        .clamp(0.0, geom.max_scroll);
                    self.hover_item = None;
                }
            }
        }
        geom.show_up = geom.scroll > 0.5;
        geom.show_down = geom.scroll < geom.max_scroll - 0.5;
        self.scroll = Some(geom.scroll);
        self.geom = Some(geom);

        self.draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle(pass, Layout::flow_overlay());

        let popup_walk = Walk::fixed(geom.width, geom.height).with_abs_pos(dvec2(geom.x, geom.y));
        self.draw_popup_bg
            .begin(cx, popup_walk, Layout::flow_down().with_padding(Inset {
                left: geom.pad,
                right: geom.pad,
                ..Inset::default()
            }));

        if let Some(arrow) = geom.up_rect() {
            self.draw_scroll_arrow.up = 1.0;
            self.draw_scroll_arrow.enabled = if geom.show_up { 1.0 } else { 0.35 };
            self.draw_scroll_arrow.draw_walk(
                cx,
                Walk::new(Size::fill(), Size::Fixed(arrow.size.y)),
            );
        }

        cx.begin_turtle(
            Walk::new(Size::fill(), Size::Fixed(geom.list_h)),
            Layout::flow_down()
                .with_padding(Inset { top: geom.pad, bottom: geom.pad, ..Inset::default() })
                .with_scroll(dvec2(0.0, geom.scroll)),
        );
        let hover = self.hover_item;
        for (i, label) in self.labels.iter().enumerate() {
            let is_hover = hover == Some(i);
            let is_active = i == self.selected_item;
            self.draw_item.hover = if is_hover && !is_active { 1.0 } else { 0.0 };
            self.draw_item.active = if is_active { 1.0 } else { 0.0 };
            self.draw_item.begin(
                cx,
                Walk::new(Size::fill(), Size::Fixed(geom.item_h)),
                Layout {
                    padding: self.item_padding,
                    align: Align { x: 0.0, y: 0.5 },
                    ..Layout::flow_right()
                },
            );
            self.draw_item_text.hover = self.draw_item.hover;
            self.draw_item_text.active = self.draw_item.active;
            self.draw_item_text
                .draw_walk(cx, Walk::fit(), Align { x: 0.0, y: 0.5 }, label);
            self.draw_item.end(cx);
        }
        cx.end_turtle();

        if let Some(arrow) = geom.down_rect() {
            self.draw_scroll_arrow.up = 0.0;
            self.draw_scroll_arrow.enabled = if geom.show_down { 1.0 } else { 0.35 };
            self.draw_scroll_arrow.draw_walk(
                cx,
                Walk::new(Size::fill(), Size::Fixed(arrow.size.y)),
            );
        }

        self.draw_popup_bg.end(cx);
        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);
    }
}

#[derive(Clone, Copy, Debug)]
enum PopupHit {
    Item(usize),
    UpArrow,
    DownArrow,
    Chrome,
    Outside,
}

impl WidgetNode for DropDown2 {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn area(&self) -> Area {
        self.draw_bg.area()
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.redraw_popup(cx);
    }

    fn set_action_data(&mut self, data: std::sync::Arc<dyn ActionTrait>) {
        self.action_data.set_box(data);
    }

    fn action_data(&self) -> Option<std::sync::Arc<dyn ActionTrait>> {
        self.action_data.clone_data()
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.set_closed(cx);
                self.clear_focus(cx);
            }
            if visible && !self.draw_bg.area().is_valid(cx) {
                cx.redraw_all();
            } else {
                self.redraw_popup(cx);
            }
        }
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        vec![
            ("draw_bg", self.draw_bg.area()),
            ("draw_text", self.draw_text.area()),
            ("draw_popup_bg", self.draw_popup_bg.area()),
            ("draw_item", self.draw_item.area()),
            ("draw_item_text", self.draw_item_text.area()),
            ("draw_scroll_arrow", self.draw_scroll_arrow.area()),
        ]
    }
}

impl Widget for DropDown2 {
    fn text(&self) -> String {
        self.labels.get(self.selected_item).cloned().unwrap_or_default()
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if disabled {
            self.set_closed(cx);
            self.clear_focus(cx);
        }
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
        self.draw_bg.redraw(cx);
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.sweep_lock_expired() {
            self.set_closed(cx);
            self.clear_focus(cx);
            return;
        }
        if self.is_active && cx.sweep_lock_area().is_some_and(|area| area != self.draw_bg.area()) {
            self.set_closed(cx);
            self.clear_focus(cx);
            return;
        }
        self.animator_handle_event(cx, event);
        if self.search_timer.is_event(event).is_some() {
            self.clear_search(cx);
        }
        if !self.visible || self.disabled(cx) || self.labels.is_empty() {
            self.set_closed(cx);
            self.clear_focus(cx);
            return;
        }
        if self.is_active && (crate::modal::ModalAction::is_dismissal(event)
            || matches!(event, Event::Pause | Event::Background | Event::Shutdown
                | Event::WindowLostFocus(_) | Event::WindowClosed(_) | Event::WindowGeomChange(_)))
        {
            self.set_closed(cx);
            return;
        }
        if self.cancel_scope.as_ref().is_some_and(|s| cx.owns_cancel(s))
            && (matches!(event, Event::KeyDown(ke) if ke.key_code == KeyCode::Escape)
                || event.back_pressed())
        {
            self.set_closed(cx);
            return;
        }
        // Alignment finishes after drawing the field, so use its final event-time rect.
        let area = self.draw_bg.area();
        let rect = area.rect(cx);
        let clipped = area.clipped_rect(cx);
        if area.is_valid(cx) && clipped.size.x > 0.0 && clipped.size.y > 0.0 {
            if self.is_active && self.aligned_rect.is_some_and(|old| old != rect) {
                self.set_closed(cx);
            }
            self.aligned_rect = Some(rect);
        } else {
            self.set_closed(cx);
            self.clear_focus(cx);
            return;
        }

        if let Some(ne) = self.next_frame.is_event(event) {
            if let Some(dir) = self.arrow_dir.filter(|_| self.is_active) {
                let dt = if self.last_frame_time > 0.0 {
                    (ne.time - self.last_frame_time).clamp(1.0 / 240.0, 0.05)
                } else {
                    1.0 / 60.0
                };
                self.last_frame_time = ne.time;
                let cur = self.scroll.unwrap_or(0.0);
                self.apply_scroll(cx, cur + dir * ARROW_SCROLL_PX_PER_SEC * dt);
                if self.scroll != Some(cur) {
                    self.next_frame = cx.new_next_frame();
                } else {
                    self.arrow_dir = None;
                }
            }
        }

        if self.is_active {
            if let Event::FingerCancel(cancel) = event {
                let pointer_matches = self.pointer.is_some_and(|gesture| {
                    let digit_id: DigitId = match gesture.pointer {
                        PopupPointer::Mouse => live_id!(mouse).into(),
                        PopupPointer::Touch(uid) => live_id_num!(touch, uid).into(),
                    };
                    digit_id == cancel.digit_id
                });
                let cancelled_capture = matches!(event.hits(cx, area), Hit::FingerUp(fe) if fe.cancelled);
                if pointer_matches && (cx.fingers.press_taken_away(cancel.digit_id) || cancelled_capture) {
                    self.set_closed(cx);
                    return;
                }
            }
            match event {
                Event::Scroll(e) => {
                    if !e.handled_y.get() {
                        if self.geom.is_some_and(|g| g.popup_rect().contains(e.abs)) {
                            self.set_arrow_dir(cx, None);
                            self.apply_scroll(cx, self.scroll.unwrap_or(0.0) + e.scroll.y);
                            self.set_hover_item(cx, None);
                        }
                        e.handled_y.set(true);
                        e.handled_x.set(true);
                    }
                    return;
                }
                Event::MouseMove(e) => {
                    if !e.handled.get().is_empty() && e.handled.get() != area {
                        self.set_closed(cx);
                        return;
                    }
                    e.handled.set(area);
                    self.pointer_move(cx, PopupPointer::Mouse, e.abs);
                    return;
                }
                Event::MouseDown(e) => {
                    if !e.handled.get().is_empty() && e.handled.get() != area {
                        self.set_closed(cx);
                        return;
                    }
                    e.handled.set(area);
                    if e.button.is_primary() {
                        self.pointer_down(cx, PopupPointer::Mouse, e.abs, false);
                    }
                    return;
                }
                Event::MouseUp(e) => {
                    if e.button.is_primary() {
                        self.pointer_up(cx, PopupPointer::Mouse, e.abs);
                    }
                    return;
                }
                Event::MouseLeave(_) => {
                    self.set_arrow_dir(cx, None);
                    self.set_hover_item(cx, None);
                }
                Event::TouchUpdate(e) => {
                    if e.touches.iter().any(|touch| !touch.handled.get().is_empty()
                        && touch.handled.get() != area)
                    {
                        self.set_closed(cx);
                        return;
                    }
                    for touch in &e.touches {
                        touch.handled.set(self.draw_bg.area());
                    }
                    for touch in &e.touches {
                        let pointer = PopupPointer::Touch(touch.uid);
                        match touch.state {
                            TouchState::Start => self.pointer_down(cx, pointer, touch.abs, false),
                            TouchState::Move => self.pointer_move(cx, pointer, touch.abs),
                            TouchState::Stop => self.pointer_up(cx, pointer, touch.abs),
                            TouchState::Stable => (),
                        }
                        if !self.is_active {
                            break;
                        }
                    }
                    return;
                }
                _ => (),
            }
        }

        let hit = event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area());
        if self.sweep_lock_expired() {
            self.set_closed(cx);
            self.clear_focus(cx);
            return;
        }
        match hit {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.set_closed(cx);
                self.clear_search(cx);
            }
            Hit::KeyFocus(_) => self.animator_play(cx, ids!(focus.on)),
            Hit::TextInput(input) if !input.was_paste && input.composition.is_none() => {
                self.type_ahead(cx, &input.input);
            }
            Hit::KeyDown(ke) => {
                if matches!(ke.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown
                    | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown)
                {
                    self.clear_search(cx);
                }
                let current = self.hover_item.unwrap_or(self.selected_item);
                let last = self.labels.len().saturating_sub(1);
                let page = self.geom.map(|g| (g.list_h / g.item_h).floor() as usize).unwrap_or(10).max(1);
                match ke.key_code {
                    KeyCode::ReturnKey | KeyCode::Space
                        if !ke.is_repeat && (ke.key_code != KeyCode::Space || self.search.is_empty()) =>
                    {
                        if self.is_active {
                            if let Some(index) = self.hover_item {
                                self.emit_select(cx, index);
                            }
                            self.set_closed(cx);
                        } else {
                            self.set_active(cx);
                        }
                    }
                    KeyCode::ArrowDown if ke.modifiers.alt && !ke.is_repeat => self.set_active(cx),
                    KeyCode::ArrowUp if ke.modifiers.alt => self.set_closed(cx),
                    KeyCode::ArrowUp => self.navigate_to(cx, current.saturating_sub(1)),
                    KeyCode::ArrowDown => self.navigate_to(cx, current.saturating_add(1)),
                    KeyCode::Home => self.navigate_to(cx, 0),
                    KeyCode::End => self.navigate_to(cx, last),
                    KeyCode::PageUp => self.navigate_to(cx, current.saturating_sub(page)),
                    KeyCode::PageDown => self.navigate_to(cx, current.saturating_add(page)),
                    KeyCode::Tab => self.set_closed(cx),
                    _ => (),
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.animator_play(cx, ids!(hover.down));
                self.set_active(cx);
                if self.is_active {
                    let pointer = match fe.device {
                        DigitDevice::Touch { uid } => PopupPointer::Touch(uid),
                        _ => PopupPointer::Mouse,
                    };
                    self.pointer_down(cx, pointer, fe.abs, true);
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => self.animator_play(cx, ids!(hover.off)),
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.sweep_lock_expired() {
            self.set_closed(cx);
            self.clear_focus(cx);
        }
        if !self.visible {
            self.set_closed(cx);
            return DrawStep::done();
        }
        self.clamp_selected();
        self.draw_field(cx, walk);
        if self.is_active {
            // Turtle alignment is deferred: a field laid out after a Fill
            // sibling (or below a Fill one) is recorded at its pre-shift
            // position and only moved when the ancestor turtle ends, so the
            // rect visible during THIS draw would put the popup at the row's
            // pre-alignment origin. Place it from the rect captured between
            // draws instead (the popup only ever opens from an event).
            let trigger = self
                .aligned_rect
                .unwrap_or_else(|| self.draw_bg.area().rect(cx));
            self.draw_popup(cx, trigger);
        }
        DrawStep::done()
    }
}

impl DropDown2Ref {
    pub fn is_open(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_open())
    }

    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.labels != labels {
                inner.labels = labels;
                inner.reset_items(cx);
            }
        }
    }

    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DropDown2Action::Select(id) = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn changed(&self, actions: &Actions) -> Option<usize> {
        self.selected(actions)
    }

    pub fn set_selected_item(&self, cx: &mut Cx, item: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            let new_selected = if inner.labels.is_empty() {
                0
            } else {
                item.min(inner.labels.len() - 1)
            };
            if new_selected != inner.selected_item {
                inner.selected_item = new_selected;
                inner.reset_items(cx);
            }
        }
    }

    pub fn selected_item(&self) -> usize {
        self.borrow().map(|inner| inner.selected_item).unwrap_or(0)
    }

    pub fn selected_label(&self) -> String {
        self.borrow()
            .map(|inner| {
                inner
                    .labels
                    .get(inner.selected_item)
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    #[test]
    fn short_list_stays_on_screen() {
        let g = layout_covering_popup(
            dvec2(800.0, 600.0),
            trigger(40.0, 40.0, 220.0, 28.0),
            5,
            0,
            22.0,
            220.0,
            3.0,
            8.0,
            16.0,
            None,
        );
        assert!(g.y >= 8.0);
        assert!(g.y + g.height <= 592.0);
        assert!(!g.show_up && !g.show_down);
        assert_eq!(g.scroll, 0.0);
    }

    #[test]
    fn long_list_near_top_clamps_and_scrolls() {
        let g = layout_covering_popup(
            dvec2(800.0, 600.0),
            trigger(40.0, 30.0, 220.0, 28.0),
            46,
            20,
            22.0,
            220.0,
            3.0,
            8.0,
            16.0,
            None,
        );
        assert!(g.y >= 8.0, "{g:?}");
        assert!(g.y + g.height <= 592.0, "{g:?}");
        assert!(g.height <= 584.0);
        assert!(g.max_scroll > 0.0);
        assert!(g.show_up || g.scroll <= 0.5);
        assert!(g.list_h > 0.0);
        let selected_screen =
            g.list_y + g.pad + 20.5 * g.item_h - g.scroll;
        assert!(
            (selected_screen - 44.0).abs() < g.item_h,
            "selected row should stay near the trigger, got {selected_screen} geom={g:?}"
        );
    }

    #[test]
    fn long_list_near_bottom_clamps_above() {
        let g = layout_covering_popup(
            dvec2(800.0, 600.0),
            trigger(40.0, 560.0, 220.0, 28.0),
            46,
            40,
            22.0,
            220.0,
            3.0,
            8.0,
            16.0,
            None,
        );
        assert!(g.y >= 8.0, "{g:?}");
        assert!(g.y + g.height <= 592.0, "{g:?}");
        assert!(g.max_scroll > 0.0);
    }

    #[test]
    fn never_taller_than_pass() {
        let g = layout_covering_popup(
            dvec2(400.0, 300.0),
            trigger(10.0, 150.0, 180.0, 24.0),
            80,
            40,
            22.0,
            180.0,
            3.0,
            8.0,
            16.0,
            None,
        );
        assert!(g.height <= 300.0 - 16.0);
        assert!(g.y >= 8.0);
        assert!(g.y + g.height <= 292.0);
    }
}
