//! Property choices: a folding section, dropdown, connected segments and checkbox.
//! Row presets place each choice in PropRow's value slot and reserve its trail.
use crate::{
    animator::Animate,
    fold_header::{FoldHeader, FoldHeaderAction},
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let on_accent = theme.color_text_on_accent ?? theme.color_on_primary

    mod.widgets.PropSectionBase = #(PropSection::register_widget(vm))
    mod.widgets.PropSection = set_type_default() do mod.widgets.PropSectionBase{
        width: Fill
        height: Fit
        flow: Down
        margin: Inset{bottom: theme.prop_section_gap}
        body_walk: Walk{width: Fill height: Fit}
        opened: 1.0
        grip: false
        draw_bg +: {
            color: uniform(theme.prop_section_bg)
            radius: uniform(theme.prop_radius_section)
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        animator: Animator{
            active: {
                default: @on
                off: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    ease: ExpDecay{d1: 0.96 d2: 0.97}
                    redraw: true
                    apply: {opened: 0.0}
                }
                on: AnimatorState{
                    from: {all: Forward{duration: 0.2}}
                    ease: ExpDecay{d1: 0.98 d2: 0.95}
                    redraw: true
                    apply: {opened: 1.0}
                }
            }
        }
        header: View{
            width: Fill
            height: Fit
            flow: Down
            // FoldHeader exposes the wrapper's children, so hdr has a stable path.
            hdr := View{
                width: Fill
                height: theme.prop_section_height
                flow: Right
                align: Align{x: 0.0 y: 0.5}
                padding: Inset{left: theme.prop_pad right: theme.prop_pad}
                spacing: theme.prop_label_gap
                show_bg: true
                draw_bg +: {
                    hover: instance(0.0)
                    opened: instance(1.0)
                    color: uniform(theme.prop_section_bg)
                    color_hover: uniform(theme.color_surface_container_highest)
                    radius: uniform(theme.prop_radius_section)
                    pixel: fn() {
                        let dpi = self.draw_pass.dpi_factor
                        let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                        let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.box_y(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.radius, self.radius * (1.0 - self.opened))
                        sdf.fill(self.color.mix(self.color_hover, self.hover))
                        return sdf.result
                    }
                }
                chevron := View{
                    width: theme.prop_chevron
                    height: theme.prop_chevron
                    show_bg: true
                    draw_bg +: {
                        opened: instance(1.0)
                        color: uniform(theme.prop_ink_dim)
                        pixel: fn() {
                            let dpi = self.draw_pass.dpi_factor
                            let c = self.rect_size * 0.5
                            let r = (self.rect_size.y - 1.25) * 0.5
                            let a = self.opened * PI * 0.5
                            let cs = cos(a)
                            let sn = sin(a)
                            let p0 = c + vec2(-r * 0.5 * cs + r * sn, -r * 0.5 * sn - r * cs)
                            let p1 = c + vec2(r * 0.5 * cs, r * 0.5 * sn)
                            let p2 = c + vec2(-r * 0.5 * cs - r * sn, -r * 0.5 * sn + r * cs)
                            let q0 = (floor((self.rect_pos + p0) * dpi) + 0.5) / dpi - self.rect_pos
                            let q1 = (floor((self.rect_pos + p1) * dpi) + 0.5) / dpi - self.rect_pos
                            let q2 = (floor((self.rect_pos + p2) * dpi) + 0.5) / dpi - self.rect_pos
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.move_to(q0.x, q0.y)
                            sdf.line_to(q1.x, q1.y)
                            sdf.line_to(q2.x, q2.y)
                            sdf.stroke(self.color, 0.625)
                            return sdf.result
                        }
                    }
                }
                title := Label{
                    width: Fill
                    height: Fit
                    padding: Inset{}
                    text: "Section"
                    max_lines: 1
                    text_overflow: TextOverflow.Ellipsis
                    draw_text +: {
                        ink_centered: true
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: theme.font_size_p}
                    }
                }
                // Shown by the owner once it has a count to say: an empty one would
                // still take a slot in the row and the semantic tree.
                count := Label{
                    visible: false
                    height: Fit
                    padding: Inset{}
                    text: ""
                    draw_text +: {
                        ink_centered: true
                        color: theme.prop_ink_dim
                        text_style: theme.font_regular{font_size: theme.type_label_s_size}
                    }
                }
                trail := View{width: Fit height: Fit flow: Right}
                grip := View{
                    visible: false
                    width: theme.prop_chevron
                    height: 12
                    show_bg: true
                    draw_bg +: {
                        color: uniform(theme.prop_ink_dim)
                        pixel: fn() {
                            let dpi = self.draw_pass.dpi_factor
                            let c = floor((self.rect_pos + self.rect_size * 0.5) * dpi) / dpi - self.rect_pos
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            sdf.circle(c.x - 2.0, c.y - 4.5, 0.75)
                            sdf.circle(c.x + 2.0, c.y - 4.5, 0.75)
                            sdf.circle(c.x - 2.0, c.y - 1.5, 0.75)
                            sdf.circle(c.x + 2.0, c.y - 1.5, 0.75)
                            sdf.circle(c.x - 2.0, c.y + 1.5, 0.75)
                            sdf.circle(c.x + 2.0, c.y + 1.5, 0.75)
                            sdf.circle(c.x - 2.0, c.y + 4.5, 0.75)
                            sdf.circle(c.x + 2.0, c.y + 4.5, 0.75)
                            sdf.fill(self.color)
                            return sdf.result
                        }
                    }
                }
            }
        }
        body: View{
            width: Fill
            height: Fit
            flow: Down
            spacing: theme.prop_row_gap
            padding: Inset{top: 2 bottom: 6 left: theme.prop_pad right: theme.prop_pad}
        }
    }

    mod.widgets.PropDropDown = DropDown{
        width: Fill
        height: theme.prop_row_height
        margin: Inset{}
        padding: Inset{left: 6 right: theme.prop_chevron + 12}
        align: Align{x: 0.0 y: 0.5}
        marked: false
        draw_text +: {
            ink_centered: true
            color: theme.color_text
            color_disabled: theme.prop_ink_dim
            color_dim: uniform(theme.prop_ink_dim)
            mixed: instance(0.0)
            text_style: theme.font_regular{font_size: theme.font_size_p}
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            get_color: fn() {return self.color.mix(self.color_dim, self.mixed).mix(self.color_disabled, self.disabled)}
        }
        draw_bg +: {
            marked: instance(0.0)
            keyboard_focus: instance(0.0)
            color: uniform(theme.prop_choice)
            color_hover: uniform(theme.prop_choice_hover)
            color_disabled: uniform(theme.color_outset_disabled)
            accent: uniform(theme.color_ctrl_selected)
            focus_color: uniform(theme.color_focus)
            arrow_color: uniform(theme.prop_ink_dim)
            arrow_size: uniform(theme.prop_chevron)
            border_radius: uniform(theme.prop_radius)
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let px = 1.0 / dpi
                let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let face = self.color.mix(self.color_hover, self.hover).mix(self.color_disabled, self.disabled)
                sdf.box(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.border_radius)
                sdf.fill(face.mix(self.accent, self.marked * 0.18))
                if self.keyboard_focus > 0.5 {
                    sdf.box(lo.x + px * 0.5, lo.y + px * 0.5, hi.x - lo.x - px, hi.y - lo.y - px, self.border_radius)
                    sdf.stroke(self.focus_color, px * 0.5)
                }
                let c = vec2(hi.x - 6.0 - self.arrow_size * 0.5, (lo.y + hi.y) * 0.5)
                let r = self.arrow_size * 0.5 - px * 0.5
                let p0 = (floor((self.rect_pos + c + vec2(-r, -r * 0.5)) * dpi) + 0.5) / dpi - self.rect_pos
                let p1 = (floor((self.rect_pos + c + vec2(0.0, r * 0.5)) * dpi) + 0.5) / dpi - self.rect_pos
                let p2 = (floor((self.rect_pos + c + vec2(r, -r * 0.5)) * dpi) + 0.5) / dpi - self.rect_pos
                sdf.move_to(p0.x, p0.y)
                sdf.line_to(p1.x, p1.y)
                sdf.line_to(p2.x, p2.y)
                sdf.stroke(self.arrow_color, px * 0.5)
                return sdf.result
            }
        }
        animator +: {
            hover: {
                default: @off
                off: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 0.0 down: 0.0} draw_text: {hover: 0.0 down: 0.0}}}
                on: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 1.0 down: 0.0} draw_text: {hover: 1.0 down: 0.0}}}
                down: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 1.0 down: 1.0} draw_text: {hover: 1.0 down: 1.0}}}
            }
        }
    }

    mod.widgets.PropSegmented = SegmentedControl{
        radio: true
        width: Fill
        height: theme.prop_row_height
        segment_height: theme.prop_row_height
        segment_padding: theme.prop_pad
        equal_width: true
        join: Connected
        inset: 0.0
        glide_secs: 0.0
        marked: false
        draw_bg +: {
            color: theme.prop_choice
            accent: uniform(theme.color_ctrl_selected)
            radius: theme.prop_radius
            border_size: 0.0
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.radius)
                sdf.fill(self.color.mix(self.accent, self.marked * 0.18))
                return sdf.result
            }
        }
        draw_pill +: {
            color: theme.prop_choice
            color_hover: uniform(theme.prop_choice_hover)
            color_selected: uniform(theme.color_ctrl_selected)
            separator_color: uniform(theme.color_bevel_inset_2)
            radius: theme.prop_radius
            border_size: 0.0
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let px = 1.0 / dpi
                let lo = floor(self.rect_pos * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((self.rect_pos + self.rect_size) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                if self.vertical > 0.5 {
                    sdf.box_y(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.radius * self.first, self.radius * self.last)
                } else {
                    sdf.box_x(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.radius * self.first, self.radius * self.last)
                }
                let face = self.color.mix(self.color_hover, self.hover).mix(self.color_selected, self.active)
                sdf.fill(face.mix(self.color_selected, self.marked * 0.18))
                if self.separator > 0.5 {
                    if self.vertical > 0.5 {
                        sdf.rect(lo.x, lo.y, hi.x - lo.x, px)
                    } else {
                        sdf.rect(lo.x, lo.y, px, hi.y - lo.y)
                    }
                    sdf.fill(self.separator_color)
                }
                return sdf.result
            }
        }
        draw_text +: {
            ink_centered: true
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
        }
        draw_text_selected +: {
            ink_centered: true
            color: on_accent
            text_style: theme.font_regular{font_size: theme.font_size_p}
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
        }
    }

    mod.widgets.PropCheckBox = CheckBox{
        width: Fill
        height: theme.prop_row_height
        margin: Inset{}
        padding: Inset{}
        align: Align{x: 0.0 y: 0.5}
        label_walk: Walk{width: Fill height: Fit margin: Inset{left: 18}}
        draw_text +: {
            ink_centered: true
            color: theme.color_text
            color_disabled: theme.prop_ink_dim
            text_style: theme.font_regular{font_size: theme.font_size_p}
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
            get_color: fn() {return self.color.mix(self.color_disabled, self.disabled)}
        }
        draw_bg +: {
            marked: instance(0.0)
            size: uniform(12.0)
            border_radius: uniform(theme.prop_radius)
            color: uniform(theme.prop_choice)
            color_hover: uniform(theme.prop_choice_hover)
            hairline_color: uniform(theme.color_bevel_inset_2)
            color_active: uniform(theme.color_ctrl_selected)
            mark_color_active: uniform(on_accent)
            focus_color: uniform(theme.color_focus)
            pixel: fn() {
                let dpi = self.draw_pass.dpi_factor
                let px = 1.0 / dpi
                let origin = self.rect_pos + vec2(0.0, (self.rect_size.y - self.size) * 0.5)
                let lo = floor(origin * dpi + 0.5) / dpi - self.rect_pos
                let hi = floor((origin + vec2(self.size)) * dpi + 0.5) / dpi - self.rect_pos
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let on = max(self.active, self.mixed)
                let face = self.color.mix(self.color_hover, self.hover).mix(self.color_active, on).mix(self.color_disabled, self.disabled)
                sdf.box(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y, self.border_radius)
                sdf.fill(face.mix(self.focus_color, self.marked * 0.18))
                if on < 0.5 {
                    sdf.box(lo.x + px * 0.5, lo.y + px * 0.5, hi.x - lo.x - px, hi.y - lo.y - px, self.border_radius)
                    sdf.stroke(self.hairline_color, px * 0.5)
                }
                let c = (lo + hi) * 0.5
                let ink = self.mark_color_active.mix(self.mark_color_disabled, self.disabled)
                if self.mixed > 0.5 {
                    let y = (floor((self.rect_pos.y + c.y) * dpi) + 0.5) / dpi - self.rect_pos.y
                    sdf.move_to(lo.x + 3.0, y)
                    sdf.line_to(hi.x - 3.0, y)
                    sdf.stroke(ink, px * 0.5)
                } else if self.active > 0.5 {
                    let p0 = (floor((self.rect_pos + c + vec2(-3.0, 0.0)) * dpi) + 0.5) / dpi - self.rect_pos
                    let p1 = (floor((self.rect_pos + c + vec2(-1.0, 2.0)) * dpi) + 0.5) / dpi - self.rect_pos
                    let p2 = (floor((self.rect_pos + c + vec2(3.0, -2.0)) * dpi) + 0.5) / dpi - self.rect_pos
                    sdf.move_to(p0.x, p0.y)
                    sdf.line_to(p1.x, p1.y)
                    sdf.line_to(p2.x, p2.y)
                    sdf.stroke(ink, px * 0.5)
                }
                if self.focus > 0.5 {
                    sdf.box(lo.x + px * 0.5, lo.y + px * 0.5, hi.x - lo.x - px, hi.y - lo.y - px, self.border_radius)
                    sdf.stroke(self.focus_color, px * 0.5)
                }
                return sdf.result
            }
        }
        animator +: {
            hover: {
                default: @off
                off: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 0.0 down: 0.0} draw_text: {hover: 0.0 down: 0.0}}}
                on: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 1.0 down: 0.0} draw_text: {hover: 1.0 down: 0.0}}}
                down: AnimatorState{from: {all: Snap} apply: {draw_bg: {hover: 1.0 down: 1.0} draw_text: {hover: 1.0 down: 1.0}}}
            }
        }
    }

    mod.widgets.PropDropDownRow = PropRow{value +: {pick := mod.widgets.PropDropDown{}}}
    mod.widgets.PropSegmentedRow = PropRow{value +: {seg := mod.widgets.PropSegmented{}}}
    mod.widgets.PropCheckRow = PropRow{value +: {check := mod.widgets.PropCheckBox{}}}
}

#[derive(Script, ScriptHook, Widget)]
pub struct PropSection {
    #[deref]
    fold: FoldHeader,
    #[live(true)]
    #[visible]
    visible: bool,
    #[live]
    draw_bg: DrawQuad,
    #[live]
    grip: bool,
    #[rust]
    hovered: bool,
    #[rust]
    drawing: DrawStateWrap<()>,
}

impl Widget for PropSection {
    fn visit_cancel(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) -> bool {
        self.visible && self.fold.visit_cancel(visit)
    }
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        self.fold.semantics(cx, node);
        node.name = self.fold.widget(cx, ids!(hdr.title)).text();
    }

    fn handle_action(
        &mut self,
        cx: &mut Cx,
        request: &a11y_tree::ActionRequest,
        scope: &mut Scope,
    ) -> a11y_tree::ActionResult {
        let result = self.fold.handle_action(cx, request, scope);
        if matches!(result, a11y_tree::ActionResult::Handled) {
            cx.widget_action(self.widget_uid(), if self.fold.is_open(cx) {
                FoldHeaderAction::Opening
            } else { FoldHeaderAction::Closing });
        }
        result
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible { return; }
        self.fold.handle_event(cx, event, scope);
        let header = self.fold.widget(cx, ids!(hdr));
        match event.hits(cx, header.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                let open = !self.fold.is_open(cx);
                self.fold.set_is_open(cx, open, Animate::Yes);
                cx.set_key_focus(header.area());
                cx.widget_action(
                    self.widget_uid(),
                    if open { FoldHeaderAction::Opening } else { FoldHeaderAction::Closing },
                );
            }
            Hit::KeyDown(ke) if matches!(ke.key_code, KeyCode::Space | KeyCode::ReturnKey) => {
                let open = !self.fold.is_open(cx);
                self.fold.set_is_open(cx, open, Animate::Yes);
                cx.widget_action(
                    self.widget_uid(),
                    if open { FoldHeaderAction::Opening } else { FoldHeaderAction::Closing },
                );
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.hovered = true;
                self.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.redraw(cx);
            }
            _ => {}
        }
        if matches!(event, Event::ClearHover) && self.hovered {
            self.hovered = false;
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible { return DrawStep::done(); }
        if self.drawing.begin(cx, ()) {
            let opened = self.fold.opened() as f32;
            let header = self.fold.widget(cx, ids!(hdr));
            if let Some(mut view) = header.borrow_mut::<View>() {
                view.draw_bg.set_dyn_instance(cx, id!(hover), &[self.hovered as u8 as f32]);
                view.draw_bg.set_dyn_instance(cx, id!(opened), &[opened]);
            }
            if let Some(mut view) = self.fold.widget(cx, ids!(hdr.chevron)).borrow_mut::<View>() {
                view.draw_bg.set_dyn_instance(cx, id!(opened), &[opened]);
            }
            self.fold.widget(cx, ids!(hdr.grip)).set_visible(cx, self.grip);
            self.draw_bg.begin(cx, walk, Layout::flow_down());
        }
        self.fold.draw_walk(
            cx,
            scope,
            Walk { width: Size::fill(), height: Size::fit(), ..Walk::default() },
        )?;
        self.draw_bg.end(cx);
        // FoldHeader settles its endpoint during drawing.
        let opened = self.fold.opened() as f32;
        for path in [ids!(hdr).as_slice(), ids!(hdr.chevron).as_slice()] {
            if let Some(mut view) = self.fold.widget(cx, path).borrow_mut::<View>() {
                view.draw_bg.set_dyn_instance(cx, id!(opened), &[opened]);
                view.draw_bg.set_instance_on_area(cx, id!(opened), &[opened]);
            }
        }
        self.drawing.end();
        let header_area = self.fold.widget(cx, ids!(hdr)).area();
        cx.add_nav_stop(header_area, NavRole::TextInput, Inset::default());
        DrawStep::done()
    }
}

impl PropSectionRef {
    pub fn set_is_open(&self, cx: &mut Cx, open: bool, animate: Animate) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fold.set_is_open(cx, open, animate);
        }
    }

    /// How far the fold has opened, 0.0 (shut) to 1.0 (open), mid-animation in between.
    pub fn opened(&self, _cx: &Cx) -> f64 {
        self.borrow().map_or(1.0, |inner| inner.fold.opened())
    }

    pub fn is_open(&self, cx: &Cx) -> bool {
        self.borrow().map_or(true, |inner| inner.fold.is_open(cx))
    }
}
