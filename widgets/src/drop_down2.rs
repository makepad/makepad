use {
    crate::{
        animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
        makepad_derive_widget::*,
        makepad_draw::*,
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

        draw_text +: {
            disabled: instance(0.0)
            down: instance(0.0)
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

/// Apple-style popup: selected row stays under the trigger; overflow
/// clamps to the pass and shows ▲/▼ scroll arrows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApplePopupGeom {
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

impl ApplePopupGeom {
    pub fn popup_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x, self.y),
            size: dvec2(self.width, self.height),
        }
    }

    pub fn list_rect(&self) -> Rect {
        Rect {
            pos: dvec2(self.x, self.list_y),
            size: dvec2(self.width, self.list_h),
        }
    }

    pub fn up_rect(&self) -> Option<Rect> {
        if !self.show_up {
            return None;
        }
        Some(Rect {
            pos: dvec2(self.x, self.y),
            size: dvec2(self.width, self.list_y - self.y),
        })
    }

    pub fn down_rect(&self) -> Option<Rect> {
        if !self.show_down {
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

pub fn layout_apple_popup(
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
) -> ApplePopupGeom {
    let n = item_count.max(1);
    let selected = selected.min(n.saturating_sub(1));
    let item_h = item_h.max(1.0);
    let pad = pad.max(0.0);
    let margin = margin.max(0.0);
    let arrow_h = arrow_h.max(0.0);
    let content_h = pad * 2.0 + n as f64 * item_h;
    let width = content_w
        .max(trigger.size.x)
        .min((pass.x - margin * 2.0).max(40.0))
        .max(40.0);
    let x = trigger.pos.x.clamp(margin, (pass.x - margin - width).max(margin));

    let trigger_center = trigger.pos.y + trigger.size.y * 0.5;
    let selected_center = pad + (selected as f64 + 0.5) * item_h;
    let ideal_y = trigger_center - selected_center;
    let max_h = (pass.y - margin * 2.0).max(item_h + arrow_h * 2.0);
    let overflow = content_h > max_h + 0.5;
    let height = if overflow { max_h } else { content_h };
    let y = ideal_y.clamp(margin, (pass.y - margin - height).max(margin));

    let up_h = if overflow { arrow_h } else { 0.0 };
    let down_h = if overflow { arrow_h } else { 0.0 };
    let list_h = (height - up_h - down_h).max(item_h);
    let list_y = y + up_h;
    let max_scroll = (content_h - list_h).max(0.0);
    let aligned = (list_y + selected_center - trigger_center).clamp(0.0, max_scroll);
    let scroll = scroll.unwrap_or(aligned).clamp(0.0, max_scroll);

    ApplePopupGeom {
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

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct DropDown2 {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
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

    #[rust]
    is_active: bool,
    #[rust]
    opening_click: bool,
    #[rust]
    hover_item: Option<usize>,
    #[rust]
    scroll: Option<f64>,
    #[rust]
    geom: Option<ApplePopupGeom>,
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

    #[action_data]
    #[rust]
    action_data: WidgetActionData,
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
        self.clamp_selected();
        self.is_active = true;
        self.opening_click = true;
        self.hover_item = Some(self.selected_item);
        self.scroll = None;
        self.arrow_dir = None;
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
        cx.sweep_lock(self.draw_bg.area());
    }

    pub fn set_closed(&mut self, cx: &mut Cx) {
        self.is_active = false;
        self.opening_click = false;
        self.arrow_dir = None;
        self.geom = None;
        self.draw_bg.redraw(cx);
        self.draw_list.redraw(cx);
        cx.sweep_unlock(self.draw_bg.area());
    }

    fn emit_select(&mut self, cx: &mut Cx, index: usize) {
        if self.labels.is_empty() {
            return;
        }
        self.selected_item = index.min(self.labels.len() - 1);
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
        cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());
    }

    fn draw_popup(&mut self, cx: &mut Cx2d, trigger: Rect) {
        let pass = cx.current_pass_size();
        let font_px = 9.0;
        let content_w = estimate_label_width(&self.labels, font_px);
        let mut geom = layout_apple_popup(
            pass,
            trigger,
            self.labels.len(),
            self.selected_item,
            self.item_height,
            content_w,
            3.0,
            self.popup_margin,
            self.arrow_height,
            self.scroll,
        );
        self.scroll = Some(geom.scroll);
        self.geom = Some(geom);

        self.draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle(pass, Layout::flow_overlay());

        let popup_walk = Walk::fixed(geom.width, geom.height).with_abs_pos(dvec2(geom.x, geom.y));
        self.draw_popup_bg
            .begin(cx, popup_walk, Layout::flow_down());

        if geom.show_up || geom.max_scroll > 0.5 {
            self.draw_scroll_arrow.up = 1.0;
            self.draw_scroll_arrow.enabled = if geom.show_up { 1.0 } else { 0.35 };
            self.draw_scroll_arrow.draw_walk(
                cx,
                Walk::new(Size::fill(), Size::Fixed(self.arrow_height)),
            );
        }

        cx.begin_turtle(
            Walk::new(Size::fill(), Size::Fixed(geom.list_h)),
            Layout::flow_down().with_scroll(dvec2(0.0, geom.scroll)),
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
                    padding: Inset {
                        left: 10.0,
                        right: 8.0,
                        top: 0.0,
                        bottom: 0.0,
                    },
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

        if geom.show_down || geom.max_scroll > 0.5 {
            self.draw_scroll_arrow.up = 0.0;
            self.draw_scroll_arrow.enabled = if geom.show_down { 1.0 } else { 0.35 };
            self.draw_scroll_arrow.draw_walk(
                cx,
                Walk::new(Size::fill(), Size::Fixed(self.arrow_height)),
            );
        }

        self.draw_popup_bg.end(cx);
        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);

        // Re-read geom after reserved-arrow draw so hit tests match what we
        // actually reserved (both arrow bands whenever the list overflows).
        if geom.max_scroll > 0.5 {
            geom.show_up = true;
            geom.show_down = true;
            geom.list_y = geom.y + self.arrow_height;
            geom.list_h = (geom.height - self.arrow_height * 2.0).max(geom.item_h);
            self.geom = Some(geom);
        }
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

impl Widget for DropDown2 {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        // Between draws every deferred alignment has been applied, so this
        // is the field's true on-screen rect (see `aligned_rect`).
        let rect = self.draw_bg.area().rect(cx);
        if rect.size.x > 0.0 && rect.size.y > 0.0 {
            self.aligned_rect = Some(rect);
        }

        if let Some(ne) = self.next_frame.is_event(event) {
            if self.is_active {
                if let Some(dir) = self.arrow_dir {
                    let dt = if self.last_frame_time > 0.0 {
                        (ne.time - self.last_frame_time).clamp(1.0 / 240.0, 0.05)
                    } else {
                        1.0 / 60.0
                    };
                    self.last_frame_time = ne.time;
                    let cur = self.scroll.or_else(|| self.geom.map(|g| g.scroll)).unwrap_or(0.0);
                    self.apply_scroll(cx, cur + dir * ARROW_SCROLL_PX_PER_SEC * dt);
                    self.next_frame = cx.new_next_frame();
                }
            }
        }

        if self.is_active {
            if let Event::Scroll(e) = event {
                if self
                    .geom
                    .is_some_and(|g| g.popup_rect().contains(e.abs))
                    && !e.handled_y.get()
                {
                    let cur = self.scroll.or_else(|| self.geom.map(|g| g.scroll)).unwrap_or(0.0);
                    self.apply_scroll(cx, cur + e.scroll.y);
                    e.handled_y.set(true);
                }
            }

            if let Event::MouseMove(e) = event {
                match self.hit_zone(e.abs) {
                    PopupHit::Item(i) => {
                        self.arrow_dir = None;
                        if self.hover_item != Some(i) {
                            self.hover_item = Some(i);
                            self.draw_list.redraw(cx);
                        }
                    }
                    PopupHit::UpArrow => {
                        if self.arrow_dir != Some(-1.0) {
                            self.arrow_dir = Some(-1.0);
                            self.last_frame_time = 0.0;
                            self.next_frame = cx.new_next_frame();
                        }
                    }
                    PopupHit::DownArrow => {
                        if self.arrow_dir != Some(1.0) {
                            self.arrow_dir = Some(1.0);
                            self.last_frame_time = 0.0;
                            self.next_frame = cx.new_next_frame();
                        }
                    }
                    _ => {
                        self.arrow_dir = None;
                    }
                }
            }

            if let Event::MouseDown(e) = event {
                match self.hit_zone(e.abs) {
                    PopupHit::Outside => {
                        self.set_closed(cx);
                        self.animator_play(cx, ids!(hover.off));
                        return;
                    }
                    PopupHit::UpArrow | PopupHit::DownArrow | PopupHit::Chrome => return,
                    PopupHit::Item(i) => {
                        self.hover_item = Some(i);
                    }
                }
            }

            if let Event::MouseUp(e) = event {
                match self.hit_zone(e.abs) {
                    PopupHit::Item(i) => {
                        if self.opening_click {
                            self.opening_click = false;
                            if i != self.selected_item {
                                self.emit_select(cx, i);
                                self.set_closed(cx);
                            }
                        } else {
                            self.emit_select(cx, i);
                            self.set_closed(cx);
                        }
                    }
                    PopupHit::Outside => {
                        self.opening_click = false;
                        self.set_closed(cx);
                    }
                    _ => {
                        self.opening_click = false;
                    }
                }
            }
        }

        match event.hits_with_sweep_area(cx, self.draw_bg.area(), self.draw_bg.area()) {
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                self.set_closed(cx);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::Escape => {
                    self.set_closed(cx);
                }
                KeyCode::ReturnKey => {
                    if self.is_active {
                        if let Some(i) = self.hover_item {
                            self.emit_select(cx, i);
                        }
                        self.set_closed(cx);
                    }
                }
                KeyCode::ArrowUp => {
                    if self.is_active {
                        let cur = self.hover_item.unwrap_or(self.selected_item);
                        let next = cur.saturating_sub(1);
                        self.hover_item = Some(next);
                        self.scroll_item_into_view(cx, next);
                        self.draw_list.redraw(cx);
                    } else if self.selected_item > 0 {
                        self.emit_select(cx, self.selected_item - 1);
                    }
                }
                KeyCode::ArrowDown => {
                    if self.is_active {
                        let cur = self.hover_item.unwrap_or(self.selected_item);
                        let last = self.labels.len().saturating_sub(1);
                        let next = (cur + 1).min(last);
                        self.hover_item = Some(next);
                        self.scroll_item_into_view(cx, next);
                        self.draw_list.redraw(cx);
                    } else if !self.labels.is_empty()
                        && self.selected_item < self.labels.len() - 1
                    {
                        self.emit_select(cx, self.selected_item + 1);
                    }
                }
                _ => (),
            },
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.animator_in_state(cx, ids!(disabled.off)) {
                    cx.set_key_focus(self.draw_bg.area());
                    self.animator_play(cx, ids!(hover.down));
                    // Never toggle-close here. The selected row is drawn
                    // under the trigger, so a held drag that leaves the
                    // field and comes back is a sweep re-entry FingerDown
                    // (capture.area was cleared). Treating that as "click
                    // the field again" closes the popup and the next
                    // crossing opens it — flicker. Open on press; dismiss
                    // on mouse-up-on-item, mouse-down-outside, or Escape.
                    if !self.is_active {
                        self.set_active(cx);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
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
    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.labels = labels;
            inner.clamp_selected();
            inner.draw_bg.redraw(cx);
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
                inner.draw_bg.redraw(cx);
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
        let g = layout_apple_popup(
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
        let g = layout_apple_popup(
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
        let g = layout_apple_popup(
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
        let g = layout_apple_popup(
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
