use crate::{
    animator::{Animator, AnimatorImpl},
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::span_inboard,
    scroll_bars::ScrollBars,
    widget::*,
    widget_async::CxSplashVmExt,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PopupMenuItemBase = #(PopupMenuItem::register_widget(vm))
    mod.widgets.PopupMenuBase = #(PopupMenu::script_component(vm))

    mod.widgets.PopupMenuItem = set_type_default() do mod.widgets.PopupMenuItemBase{
        width: Fill
        height: Fit
        align: Align{y: 0.5}
        padding: theme.mspace_1{left: 15.}

        draw_text +: {
            active: instance(0.0)
            hover: instance(0.0)
            disabled: instance(0.0)

            ink_centered: true

            color: theme.color_label_inner
            color_hover: uniform(theme.color_label_inner_hover)
            color_active: uniform(theme.color_label_inner_active)
            color_disabled: uniform(theme.color_label_inner_disabled)

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }

            get_color: fn() {
                return self.color
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_disabled, self.disabled)
            }
        }

        /** The per-item icon ink. Flat rather than state-mixed like `draw_text`,
        because a chrome glyph has to stay readable against the row's active
        fill; the row itself already carries the state. */
        draw_icon +: {
            color: theme.color_label_inner
        }

        /** The icon sits in front of the label with a small gap. This is only
        walked when the dropdown handed this row an icon, so a menu without
        icons keeps exactly its old metrics. */
        icon_walk: Walk{
            width: 10
            height: Fit
            margin: Inset{right: 5.0}
        }

        draw_bg +: {
            active: instance(0.0)
            hover: instance(0.0)
            disabled: instance(0.0)

            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color_dither: uniform(1.0)
            border_size: uniform(theme.beveling)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_u_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_active: uniform(theme.color_outset_active)
            color_disabled: uniform(theme.color_outset_disabled)

            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_2_hover: uniform(theme.color_outset_2_hover)
            color_2_active: uniform(theme.color_outset_2_active)
            color_2_disabled: uniform(theme.color_outset_2_disabled)

            border_color: uniform(theme.color_u_hidden)
            border_color_hover: uniform(theme.color_u_hidden)
            border_color_active: uniform(theme.color_u_hidden)
            border_color_disabled: uniform(theme.color_u_hidden)

            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color_2_hover: uniform(theme.color_u_hidden)
            border_color_2_active: uniform(theme.color_u_hidden)
            border_color_2_disabled: uniform(theme.color_u_hidden)

            mark_color: uniform(theme.color_u_hidden)
            mark_color_active: uniform(theme.color_mark_active)
            mark_color_disabled: uniform(theme.color_mark_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                let mut color_fill = self.color
                let mut color_fill_hover = self.color_hover
                let mut color_fill_active = self.color_active
                let mut color_fill_disabled = self.color_disabled

                if self.color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_fill = vec2(
                        self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                        self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                    )
                    let dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y
                    color_fill = mix(self.color, self.color_2, dir)
                    color_fill_hover = mix(self.color_hover, self.color_2_hover, dir)
                    color_fill_active = mix(self.color_active, self.color_2_active, dir)
                    color_fill_disabled = mix(self.color_disabled, self.color_2_disabled, dir)
                }

                let mut color_stroke = self.border_color
                let mut color_stroke_hover = self.border_color_hover
                let mut color_stroke_active = self.border_color_active
                let mut color_stroke_disabled = self.border_color_disabled

                if self.border_color_2.x > -0.5 {
                    let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                    let gradient_border = vec2(
                        self.pos.x + dither
                        self.pos.y + dither
                    )
                    let dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y
                    color_stroke = mix(self.border_color, self.border_color_2, dir)
                    color_stroke_hover = mix(self.border_color_hover, self.border_color_2_hover, dir)
                    color_stroke_active = mix(self.border_color_active, self.border_color_2_active, dir)
                    color_stroke_disabled = mix(self.border_color_disabled, self.border_color_2_disabled, dir)
                }

                let fill = color_fill
                    .mix(color_fill_active, self.active)
                    .mix(color_fill_hover, self.hover)
                    .mix(color_fill_disabled, self.disabled)

                let stroke = color_stroke
                    .mix(color_stroke_active, self.active)
                    .mix(color_stroke_hover, self.hover)
                    .mix(color_stroke_disabled, self.disabled)

                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                // Mark
                let sz = 3.
                let dx = 2.0
                let c = vec2(8.0, 0.5 * self.rect_size.y)
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx)
                sdf.line_to(c.x, c.y + sz)
                sdf.line_to(c.x + sz, c.y - sz)

                sdf.stroke(
                    self.mark_color
                        .mix(self.mark_color_active, self.active)
                        .mix(self.mark_color_disabled, self.disabled)
                    1.)

                return sdf.result
            }
        }

        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                    }
                }
                on: AnimatorState{
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                    }
                }
            }

            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
        }
        indent_width: 10.0
    }

    mod.widgets.PopupMenuItemGradientX = mod.widgets.PopupMenuItem{
        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 1.0

            color: theme.color_u_hidden
            color_hover: theme.color_outset_1_hover
            color_active: theme.color_outset_1_active
            color_disabled: theme.color_outset_1_disabled

            color_2: theme.color_u_hidden
            color_2_hover: theme.color_outset_2_hover
            color_2_active: theme.color_outset_2_active
            color_2_disabled: theme.color_outset_2_disabled
        }
    }

    mod.widgets.PopupMenuItemGradientY = mod.widgets.PopupMenuItemGradientX{
        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 0.0
        }
    }

    mod.widgets.PopupMenuFlat = set_type_default() do mod.widgets.PopupMenuBase{
        width: 150.
        height: Fit
        flow: Flow.Down
        padding: theme.mspace_1

        menu_item: mod.widgets.PopupMenuItem{}

        /** The scroll area of a menu whose owner caps its height (the
        dropdown's `popup_max_height`). A menu without a cap never draws it. */
        scroll_bars: mod.widgets.ScrollBars{show_scroll_x: false show_scroll_y: true}

        draw_bg +: {
            border_size: uniform(theme.beveling)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)
            border_radius: uniform(theme.corner_radius)

            color: uniform(theme.color_fg_app)
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            border_color: uniform(theme.color_bevel)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            color_dither: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither

                let color_2 = if self.color_2.x > -0.5 self.color_2 else self.color
                let border_color_2 = if self.border_color_2.x > -0.5 self.border_color_2 else self.border_color

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x
                    self.border_size / self.rect_size.y
                )

                let gradient_border = vec2(
                    self.pos.x + dither
                    self.pos.y + dither
                )

                let gradient_border_dir = if self.gradient_border_horizontal > 0.5 gradient_border.x else gradient_border.y

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                )

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x
                    self.rect_size.y / sz_inner_px.y
                )

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                let gradient_fill_dir = if self.gradient_fill_horizontal > 0.5 gradient_fill.x else gradient_fill.y

                sdf.box(
                    self.border_size
                    self.border_size
                    self.rect_size.x - self.border_size * 2.
                    self.rect_size.y - self.border_size * 2.
                    self.border_radius
                )

                sdf.fill_keep(mix(self.color, color_2, gradient_fill_dir))

                if self.border_size > 0.0 {
                    sdf.stroke(
                        mix(self.border_color, border_color_2, gradient_border_dir)
                        self.border_size
                    )
                }

                return sdf.result
            }
        }
    }

    mod.widgets.PopupMenu = mod.widgets.PopupMenuFlat{
        menu_item: mod.widgets.PopupMenuItem{}
        draw_bg +: {
            border_color: theme.color_bevel_outset_1
            border_color_2: theme.color_bevel_outset_2
        }
    }

    mod.widgets.PopupMenuGradientY = mod.widgets.PopupMenu{
        menu_item: mod.widgets.PopupMenuItemGradientY{}

        draw_bg +: {
            color: theme.color_fg_app
            color_2: theme.color_fg_app * 1.2
        }
    }

    mod.widgets.PopupMenuGradientX = mod.widgets.PopupMenuGradientY{
        menu_item: mod.widgets.PopupMenuItemGradientY{}

        draw_bg +: {
            gradient_border_horizontal: 0.0
            gradient_fill_horizontal: 1.0
        }
    }
}

/// A menu item is a real widget: it has a uid and sits in the widget tree
/// under the dropdown that opened the menu, so the design tweaker can pick it
/// (in 2D and on its own plane in the exploded view) and style its template.
/// The menu still drives it directly — `handle_event_with` for actions,
/// `draw_item` for the label — the widget seams only add identity.
#[derive(Script, ScriptHook, Animator, Widget)]
pub struct PopupMenuItem {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    /// The label the menu hands in before each draw.
    #[rust]
    pub label: String,
    #[rust]
    disabled: bool,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    /// The row's icon, seated by the menu before each draw the same way `label`
    /// is. Left empty by every caller that does not pass one, and an empty
    /// `DrawSvg` is never walked, so an icon-less menu keeps its old metrics.
    #[live]
    draw_icon: DrawSvg,
    /// Hold the icon slot open on a row that has no icon of its own, so a list
    /// where only some items carry one still reads as a column of labels. Off
    /// unless the menu says otherwise, which is what keeps an icon-less menu on
    /// its old metrics.
    #[rust]
    reserve_icon: bool,

    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,
    #[walk]
    walk: Walk,

    #[live]
    indent_width: f32,
    #[live]
    icon_walk: Walk,

    #[live]
    opened: f32,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script)]
pub struct PopupMenu {
    #[source]
    source: ScriptObjectRef,

    #[live]
    draw_list: DrawList2d,
    #[live]
    menu_item: ScriptValue,

    #[live]
    draw_bg: DrawQuad,
    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
    #[live]
    items: Vec<String>,
    #[rust]
    first_tap: bool,
    /// Each item is a widget of its own, registered in the widget tree under
    /// `tree_parent` by its item id (the design tweaker picks them).
    #[rust]
    menu_items: ComponentMap<PopupMenuItemId, WidgetRef>,
    /// The widget-tree node the items hang under. The menu itself is not a
    /// tree node (a `DropDown` owns it by value through a global), so the
    /// dropdown hands in its own uid and the items read as `drop_down.<n>`.
    #[rust]
    pub tree_parent: WidgetUid,
    #[rust]
    init_select_item: Option<PopupMenuItemId>,
    /// Whether this pass draws an icon column. Set per open by whoever fills
    /// the menu, alongside `tree_parent`, since one menu instance is shared by
    /// every dropdown on the same template and only some of them have icons.
    #[rust]
    pub icon_column: bool,
    /// The tallest the menu may be, in points; 0 is no cap, and a menu
    /// without one draws every item exactly as it always did. Past the cap
    /// the items scroll inside the menu. Set per open by whoever fills the
    /// menu, alongside `icon_column`, for the same reason: one menu instance
    /// serves every dropdown on the same template.
    #[rust]
    pub max_height: f64,
    /// The scroll area a capped menu holds its items in.
    #[live]
    scroll_bars: ScrollBars,
    /// Whether the last draw put the items in the scroll area, for the
    /// event side.
    #[rust]
    scrolling: bool,
    /// While a capped menu draws: where its items begin in the align list,
    /// so the end of the draw can move them as one.
    #[rust]
    scroll_content_start: Option<usize>,
    /// The item to bring into view on the next draw: the chosen one, as the
    /// menu opens.
    #[rust]
    reveal: Option<PopupMenuItemId>,
    /// That item's rect this draw, from the top of the scrolled content.
    #[rust]
    reveal_rect: Option<Rect>,
    /// The offset the menu opened at. A menu placed by one of its items
    /// (`end_at_item`) is placed as if still at this offset, so it stays
    /// where it opened while its items scroll.
    #[rust]
    opened_scroll: f64,

    #[rust]
    count: usize,
}

/// The least a menu keeps clear of the pass's edges.
const MENU_MARGIN: f64 = 4.0;

impl ScriptHook for PopupMenu {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Apply menu_item template to existing items
        if !self.menu_item.is_nil() {
            for (_, node) in self.menu_items.iter_mut() {
                let Some(mut node) = node.borrow_mut::<PopupMenuItem>() else {
                    continue;
                };
                node.script_apply(vm, apply, scope, self.menu_item);
            }
        }
        self.draw_list.redraw(vm.cx_mut());
    }
}

pub enum PopupMenuItemAction {
    WasSweeped,
    WasSelected,
    MightBeSelected,
    None,
}

#[derive(Clone, Default)]
pub enum PopupMenuAction {
    WasSweeped(PopupMenuItemId),
    WasSelected(PopupMenuItemId),
    #[default]
    None,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct PopupMenuItemId(pub LiveId);

impl PopupMenuItem {
    /// Point the row's icon at `svg`, or clear it. Only writes on an actual
    /// change: a `ScriptHandleRef` is a GC root, and re-seating one every frame
    /// churns the root table and makes `DrawSvg` think the document moved.
    pub fn set_icon(&mut self, svg: Option<&ScriptHandleRef>) {
        let want = svg.map(|s| s.as_handle());
        if self.draw_icon.svg.as_ref().map(|s| s.as_handle()) != want {
            self.draw_icon.svg = svg.cloned();
        }
    }

    pub fn draw_item(&mut self, cx: &mut Cx2d, label: &str) {
        self.draw_bg.begin(cx, self.walk, self.layout);
        // Guarded rather than left to `DrawSvg`'s own empty-document bail, so
        // the turtle provably never sees the icon walk when there is no icon
        // and no column to hold open.
        if self.draw_icon.svg.is_some() {
            self.draw_icon.draw_walk(cx, self.icon_walk);
        } else if self.reserve_icon {
            cx.walk_turtle(self.icon_walk);
        }
        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), label);
        self.draw_bg.end(cx);
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuItemAction),
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.area().redraw(cx);
        }

        let hit = event.hits_with_options(
            cx,
            self.draw_bg.area(),
            HitOptions::new().with_sweep_area(sweep_area),
        );
        // Consume hits on an unavailable row without selecting or hovering it.
        if self.disabled {
            return;
        }
        match hit {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                dispatch_action(cx, PopupMenuItemAction::WasSweeped);
                self.animator_play(cx, ids!(hover.on));
                self.animator_play(cx, ids!(active.on));
            }
            Hit::FingerUp(se) if se.is_primary_hit() => {
                if !se.is_sweep && !se.cancelled {
                    dispatch_action(cx, PopupMenuItemAction::WasSelected);
                } else {
                    self.animator_play(cx, ids!(hover.off));
                    self.animator_play(cx, ids!(active.off));
                }
            }
            _ => {}
        }
    }
}

impl Widget for PopupMenuItem {
    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
        // Driven by `PopupMenu::handle_event_with`, which needs the menu's
        // sweep area and action sink; nothing to do on the plain seam.
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        let label = std::mem::take(&mut self.label);
        self.draw_item(cx, &label);
        self.label = label;
        DrawStep::done()
    }
}

impl PopupMenu {
    fn item(&mut self, cx: &mut Cx, item_id: PopupMenuItemId) -> WidgetRef {
        let menu_item = self.menu_item;
        // `menu_item` is an object index in the popup's heap. A popup owned by a
        // Splash isolate must install that VM on Cx before the template is applied.
        let Some(vm_id) = cx.script_ref_vm_id(&self.source) else {
            return WidgetRef::empty();
        };
        self.menu_items
            .get_or_insert(cx, item_id, |cx| {
                cx.with_script_vm_id(vm_id, |vm| {
                    WidgetRef::script_from_value(vm, menu_item)
                })
            })
            .clone()
    }

    pub fn menu_contains_pos(&self, cx: &mut Cx, pos: Vec2d) -> bool {
        self.draw_bg.area().clipped_rect(cx).contains(pos)
    }

    #[cfg(test)]
    pub(crate) fn area(&self) -> Area {
        self.draw_bg.area()
    }

    #[cfg(test)]
    pub(crate) fn scroll_bars(&self) -> &ScrollBars {
        &self.scroll_bars
    }

    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.draw_list.begin_popup_overlay(cx);

        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        self.draw_bg.begin(cx, self.walk, self.layout);
        self.count = 0;

        // A capped menu holds its items in a scroll area no taller than the
        // cap, nor than the pass: a menu taller than the window would leave
        // its last rows past an edge no scroll could bring them over.
        self.scrolling = self.max_height > 0.0;
        self.reveal_rect = None;
        if self.scrolling {
            let room = self.max_height.min(size.y - MENU_MARGIN * 2.0);
            let padding = self.layout.padding;
            let walk = Walk::new(
                if self.walk.width.is_fit() { Size::fit() } else { Size::fill() },
                Size::Fit {
                    min: None,
                    max: Some(FitBound::Abs((room - padding.top - padding.bottom).max(0.0))),
                },
            );
            let layout = Layout {
                flow: self.layout.flow,
                spacing: self.layout.spacing,
                align: self.layout.align,
                ..Layout::default()
            };
            self.scroll_bars.begin(cx, walk, layout);
            self.scroll_content_start = Some(cx.align_list_len());
        }
    }

    /// Close a capped menu's scroll area. The items were laid out at the
    /// offset the last draw left, before this draw's total was known; now it
    /// is, so the offset is settled here (kept inside the new total, and
    /// moved just far enough to show the item asked for, if one was) and the
    /// items already drawn are moved to it within this same draw, so the menu
    /// never shows a frame at the old offset. Returns how far the offset the
    /// items were laid out at lies past the one the menu opened at, which is
    /// what `end_at_item` corrects the item's place by.
    fn end_scroll(&mut self, cx: &mut Cx2d) -> f64 {
        let Some(start) = self.scroll_content_start.take() else {
            return 0.0;
        };
        let total = cx.turtle().used().y;
        let visible = cx.current_turtle_max_height().map_or(total, |max| total.min(max));
        let end = (total - visible).max(0.0);
        let was = cx.turtle().scroll().y;
        let mut to = was.min(end).max(0.0);
        if let Some(rect) = self.reveal_rect.take() {
            if rect.pos.y < to {
                to = rect.pos.y;
            } else if rect.pos.y + rect.size.y > to + visible {
                to = rect.pos.y + rect.size.y - visible;
            }
            to = to.min(end).max(0.0);
        }
        if to != was {
            let items = TurtleAlignRange { start, end: cx.align_list_len() };
            cx.shift_align_range(&items, dvec2(0.0, was - to));
            self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, to));
        }
        self.scroll_bars.end(cx);
        // The draw after `init_select_item` is the opening one.
        if self.reveal.is_some() {
            self.opened_scroll = to;
        }
        was - self.opened_scroll
    }

    /// Seed a Fit menu's content width before its Fill rows are drawn, so
    /// every row has the width of the widest label, padding and icon included.
    /// Fixed-width menus keep their declared size.
    pub fn fit_width_to_items<'a>(
        &mut self,
        cx: &mut Cx2d,
        items: impl Iterator<Item = (PopupMenuItemId, &'a str, Option<&'a ScriptHandleRef>)>,
    ) {
        if !self.walk.width.is_fit() {
            return;
        }
        let mut widest = 0.0_f64;
        for (item_id, label, icon) in items {
            let item = self.item(cx, item_id);
            let Some(mut item) = item.borrow_mut::<PopupMenuItem>() else { continue };
            let text = item.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), label);
            let mut width = (text.size_in_lpxs.width * item.draw_text.font_scale) as f64;
            if self.icon_column || icon.is_some() {
                item.set_icon(icon);
                let walk = item.icon_walk;
                let icon_width = item.draw_icon.measure(cx, walk)
                    .map(|size| size.x)
                    .unwrap_or_else(|| walk.width.to_fixed().unwrap_or(0.0))
                    + walk.margin.width();
                width = match item.layout.flow {
                    Flow::Right { .. } => width + icon_width + item.layout.spacing,
                    Flow::Down | Flow::Overlay => width.max(icon_width),
                };
            }
            widest = widest.max(width.ceil() + item.layout.padding.width() + item.walk.margin.width());
        }
        let height = cx.turtle().used_height();
        cx.turtle_mut().set_used(widest + self.layout.padding.left, height);
    }

    pub fn end(&mut self, cx: &mut Cx2d, shift_area: Area, shift: Vec2d) {
        self.end_placed(cx, shift_area, shift, None)
    }

    /// As `end`, for a menu placed by one of its own items: `item_pos` is
    /// where the turtle stood as that item began, and the menu moves by
    /// `shift - item_pos`, which puts that item at `shift`. A capped menu is
    /// placed by where the item stood at the offset the menu opened at (the
    /// one that brought the chosen item into view), so the menu stays put
    /// while its items scroll.
    pub fn end_at_item(&mut self, cx: &mut Cx2d, shift_area: Area, shift: Vec2d, item_pos: Vec2d) {
        self.end_placed(cx, shift_area, shift, Some(item_pos))
    }

    fn end_placed(&mut self, cx: &mut Cx2d, shift_area: Area, shift: Vec2d, item_pos: Option<Vec2d>) {
        let past_opened = self.end_scroll(cx);
        self.draw_bg.end(cx);
        self.reveal = None;
        let shift = match item_pos {
            Some(item_pos) => shift - item_pos - dvec2(0.0, past_opened),
            None => shift,
        };

        // The caller's shift is a WANT, not an answer. The turtle translates
        // the menu by trigger.pos + shift and never asks whether the result
        // is still on screen, so a dropdown low in a window opened a menu
        // whose last rows fell past the bottom edge — not merely clipped,
        // unreachable, since a menu without a cap has no scroll and never
        // flips. Pull the span back inside the pass on both axes first.
        let menu = self.draw_bg.area().rect(cx);
        let trigger = shift_area.rect(cx);
        let pass = cx.current_pass_size();
        let want = menu.pos + trigger.pos + shift;
        let placed = dvec2(
            span_inboard(want.x, menu.size.x, MENU_MARGIN, pass.x - MENU_MARGIN * 2.0),
            span_inboard(want.y, menu.size.y, MENU_MARGIN, pass.y - MENU_MARGIN * 2.0),
        );
        let shift = shift + (placed - want);

        cx.end_pass_sized_turtle_with_shift(shift_area, shift);
        self.draw_list.end(cx);
        self.menu_items.retain_visible();
        if let Some(init_select_item) = self.init_select_item.take() {
            self.select_item_state(cx, init_select_item);
        }
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
    }

    pub fn draw_item(&mut self, cx: &mut Cx2d, item_id: PopupMenuItemId, label: &str) {
        self.draw_item_full(cx, item_id, label, None, false, false)
    }

    /// As `draw_item`, plus the icon to draw in front of the label.
    pub fn draw_item_with_icon(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
        icon: Option<&ScriptHandleRef>,
    ) {
        self.draw_item_full(cx, item_id, label, icon, false, false)
    }

    /// Draw a selectable item using the disabled palette. This is visual
    /// metadata only: dimmed entries still receive pointer and keyboard input.
    pub fn draw_item_dimmed(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
        dimmed: bool,
    ) {
        self.draw_item_full(cx, item_id, label, None, dimmed, false)
    }

    /// An optional icon, dimmed palette, and an independent input-disabled
    /// state. Disabled rows always use the disabled palette.
    ///
    /// The three above are kept as their own entry points so no caller had
    /// to change: each names the one thing it cares about and takes the
    /// default for the other.
    pub fn draw_item_full(
        &mut self,
        cx: &mut Cx2d,
        item_id: PopupMenuItemId,
        label: &str,
        icon: Option<&ScriptHandleRef>,
        dimmed: bool,
        disabled: bool,
    ) {
        self.count += 1;

        let item = self.item(cx, item_id);
        if let Some(mut menu_item) = item.borrow_mut::<PopupMenuItem>() {
            menu_item.label = label.to_string();
            menu_item.disabled = disabled;
            menu_item.set_icon(icon);
            menu_item.reserve_icon = self.icon_column;
            menu_item.animator_cut(
                cx,
                if dimmed || disabled {
                    ids!(disabled.on)
                } else {
                    ids!(disabled.off)
                },
            );
            if disabled {
                menu_item.animator_cut(cx, ids!(active.off));
                menu_item.animator_cut(cx, ids!(hover.off));
            }
        }
        // Re-seat on every draw: one menu instance is shared by every dropdown
        // that uses the same template, so the items belong to whichever
        // dropdown has the menu open right now.
        cx.widget_tree_insert_child(self.tree_parent, item_id.0, item.clone());
        // Through the widget seam, so the item counts as its own nesting level
        // and the design tweaker's plane pick lands on it.
        item.draw_all(cx, &mut Scope::empty());
        if self.scroll_content_start.is_some() && self.reveal == Some(item_id) {
            let rect = item.area().rect(cx);
            let origin = cx.turtle().origin();
            self.reveal_rect = Some(Rect { pos: rect.pos - origin, size: rect.size });
        }
    }

    /// The items as widgets, for whoever enumerates children (the DropDown).
    pub fn item_refs(&self) -> impl Iterator<Item = (LiveId, WidgetRef)> + '_ {
        self.menu_items
            .iter()
            .map(|(id, item)| (id.0, item.clone()))
    }

    pub fn init_select_item(&mut self, which_id: PopupMenuItemId) {
        self.init_select_item = Some(which_id);
        self.reveal = Some(which_id);
        self.first_tap = true;
    }

    fn select_item_state(&mut self, cx: &mut Cx, which_id: PopupMenuItemId) {
        for (id, item) in &mut *self.menu_items {
            let Some(mut item) = item.borrow_mut::<PopupMenuItem>() else {
                continue;
            };
            if *id == which_id && !item.disabled {
                item.animator_cut(cx, ids!(active.on));
                item.animator_cut(cx, ids!(hover.on));
            } else {
                item.animator_cut(cx, ids!(active.off));
                item.animator_cut(cx, ids!(hover.off));
            }
        }
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        sweep_area: Area,
        dispatch_action: &mut dyn FnMut(&mut Cx, PopupMenuAction),
    ) {
        if self.scrolling {
            // The bar hit-tests with plain `hits`, which the owner's lock on
            // `sweep_area` (held while the menu is open) turns away, so it is
            // lifted around the bar, as a Popover lifts its own around its
            // content. Only a lock on top is the owner's to lift; the stored
            // handle follows the owner's area through every redraw. The bar
            // goes first so a press on it is the bar's, not the row it lies
            // over.
            let lifted = cx.sweep_lock_area() == Some(sweep_area);
            if lifted {
                cx.sweep_unlock(sweep_area);
            }
            self.scroll_bars.handle_event(cx, event, &mut Scope::empty());
            if lifted {
                cx.sweep_lock(sweep_area);
            }
        }
        let mut actions = Vec::new();
        for (item_id, node) in self.menu_items.iter_mut() {
            let Some(mut node) = node.borrow_mut::<PopupMenuItem>() else {
                continue;
            };
            node.handle_event_with(cx, event, sweep_area, &mut |_, e| {
                actions.push((*item_id, e))
            });
        }

        for (node_id, action) in actions {
            match action {
                PopupMenuItemAction::MightBeSelected => {
                    if self.first_tap {
                        self.first_tap = false;
                    } else {
                        self.select_item_state(cx, node_id);
                        dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                    }
                }
                PopupMenuItemAction::WasSweeped => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSweeped(node_id));
                }
                PopupMenuItemAction::WasSelected => {
                    self.select_item_state(cx, node_id);
                    dispatch_action(cx, PopupMenuAction::WasSelected(node_id));
                }
                _ => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_item_is_instantiated_in_the_popup_owners_vm() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let vm_id = cx.alloc_splash_vm();
        let mut popup = cx.with_script_vm_id(vm_id, PopupMenu::script_new_with_default);
        let popup_heap = popup.source.heap_key();

        let item = popup.item(&mut cx, PopupMenuItemId(LiveId(1)));
        let item = item
            .borrow::<PopupMenuItem>()
            .expect("popup template should instantiate a PopupMenuItem");

        assert_eq!(item.source.heap_key(), popup_heap);
    }
}
