//! A formatted tooltip with a callout triangle/"arrow" that points to the referenced widget.
//!
//! By default, the tooltip has a black background color and white text.

use crate::{
    label::*, makepad_derive_widget::*, makepad_draw::*, tooltip::*, view::*, widget::*,
    widget_match_event::WidgetMatchEvent,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CalloutTooltipBase = #(CalloutTooltip::register_widget(vm))

    // A tooltip that appears when hovering over target's area
    mod.widgets.CalloutTooltip = mod.widgets.CalloutTooltipBase {
        content := RoundedView {
            width: Fit
            height: Fit
            padding: 18

            draw_bg +: {
                border_radius: 2.,
                background_color: instance(#3b444b),
                // Height of the callout triangle, which also insets the box so the triangle fits beside it
                triangle_height: instance(7.5),
                // Which edge the triangle sits on, in degrees: 0 top, 90 right, 180 bottom, 270 left
                callout_position: instance(180.0),
                // Where along that edge the triangle's tip goes, relative to the tooltip's top-left corner
                callout_offset: instance(0.0),

                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size);
                    let rect_size = self.rect_size;
                    let triangle_height = self.triangle_height;
                    // Draw rounded box with border equals to triangle_height.
                    sdf.box(
                        triangle_height,
                        triangle_height,
                        rect_size.x - (triangle_height * 2.0),
                        rect_size.y - (triangle_height * 2.0),
                        max(1.0, self.border_radius)
                    )

                    // The box and its triangle are ONE shape, filled once. Two
                    // fills overlap along the triangle's base, and a colour
                    // with any transparency shows that overlap as a darker bar.
                    // The tip stays clear of the rounded corners, and the
                    // triangle's base sits 2.0 inside the box.
                    let mut vertex1 = vec2(0.0, 0.0);
                    let mut vertex2 = vec2(0.0, 0.0);
                    let mut vertex3 = vec2(0.0, 0.0);
                    if self.callout_position == 0.0 {
                        // Point upwards
                        let tip = min(max(self.callout_offset, triangle_height * 2.0 + 2.0), rect_size.x - triangle_height * 2.0 - 2.0);
                        vertex1 = vec2(tip - triangle_height, triangle_height + 2.0);
                        vertex2 = vec2(tip, 2.0);
                        vertex3 = vec2(tip + triangle_height, triangle_height + 2.0);
                    } else if self.callout_position == 90.0 {
                        // Point rightwards, at the target's middle (callout_offset),
                        // clamped to the tooltip's own edges like the other sides.
                        let tip = min(max(self.callout_offset, triangle_height * 2.0 + 2.0), rect_size.y - triangle_height * 2.0 - 2.0);
                        vertex1 = vec2(rect_size.x - 2.0, tip);
                        vertex2 = vec2(vertex1.x - triangle_height, tip + triangle_height);
                        vertex3 = vec2(vertex1.x - triangle_height, tip - triangle_height);
                    } else if self.callout_position == 180.0 {
                        // Point downwards
                        let tip = min(max(self.callout_offset, triangle_height * 2.0 + 2.0), rect_size.x - triangle_height * 2.0 - 2.0);
                        vertex1 = vec2(tip + triangle_height, rect_size.y - triangle_height - 2.0);
                        vertex2 = vec2(tip, rect_size.y - 2.0);
                        vertex3 = vec2(tip - triangle_height, rect_size.y - triangle_height - 2.0);
                    } else {
                        // Point leftwards
                        let tip = min(max(self.callout_offset, triangle_height * 2.0 + 2.0), rect_size.y - triangle_height * 2.0 - 2.0);
                        vertex1 = vec2(2.0, tip);
                        vertex2 = vec2(2.0 + triangle_height, tip - triangle_height);
                        vertex3 = vec2(2.0 + triangle_height, tip + triangle_height);
                    }
                    // Every triangle above is right-angled with its base twice
                    // its height: the up and down cases list a base corner,
                    // the point, then the other base corner, and the left and
                    // right cases list the point first. The pointer primitive
                    // takes the base's middle and the point.
                    let mut base = (vertex1 + vertex3) * 0.5;
                    let mut point = vertex2;
                    if self.callout_position != 0.0 && self.callout_position != 180.0 {
                        base = (vertex2 + vertex3) * 0.5;
                        point = vertex1;
                    }
                    sdf.pointer(base.x, base.y, point.x, point.y);
                    sdf.fill(self.background_color);
                    return sdf.result;
                }
            }

            // Fill lets the label wrap when the tooltip is drawn narrower than its text.
            // Its padding must stay 0, since a label pads its text area again on top of the width it's given.
            tooltip_label := Label {
                width: Fill
                height: Fit
                padding: 0
                draw_text +: {
                    text_style: theme.font_regular {font_size: 9},
                    color: #FFF
                }
            }
        }
    }
}

/// Options that affect how a CalloutTooltip is displayed.
///
/// You don't have to specify all values, they each have a sensible default.
#[derive(Clone, Copy, Debug)]
pub struct CalloutTooltipOptions {
    /// The color of the tooltip text. Defaults to pure white: #FFFFFF.
    pub text_color: Vec4,
    /// The background color of the tooltip. Defaults to dark gray: #424C54.
    pub bg_color: Vec4,
    /// The position of the tooltip relative to the widget that it's related to.
    pub position: TooltipPosition,
    /// The height/length of the callout triangle that points to the related widget.
    pub triangle_height: f64,
}
impl Default for CalloutTooltipOptions {
    fn default() -> Self {
        Self {
            text_color: vec4(1.0, 1.0, 1.0, 1.0),
            bg_color: vec4(0.26, 0.30, 0.333, 1.0),
            position: TooltipPosition::default(),
            triangle_height: 7.5,
        }
    }
}

/// A tooltip widget with a callout pointing towards the referenced widget.
///
/// `CalloutTooltip` automatically listens for `TooltipAction::HoverIn` and
/// `TooltipAction::HoverOut` events from any widget in the action batch and
/// shows or hides itself accordingly. Apps generally do **not** need to
/// handle `TooltipAction` themselves. It is enough to instantiate one
/// `CalloutTooltip` somewhere in the widget tree and have hover-aware
/// widgets emit `TooltipAction::HoverIn` from their `handle_event` (e.g.
/// inside a `Hit::FingerHoverIn` arm).
#[derive(Script, ScriptHook, Widget)]
pub struct CalloutTooltip {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    tooltip: Tooltip,

    /// The shown text, kept so its unwrapped width can be measured on each draw.
    #[rust]
    text: String,
    #[rust]
    options: CalloutTooltipOptions,
}

impl Widget for CalloutTooltip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::ClearHover = event {
            self.tooltip.hide(cx);
        }

        // Auto-process `TooltipAction`s emitted by other widgets in the
        // action batch via the `WidgetMatchEvent` impl below.
        self.widget_match_event(cx, event, scope);

        self.tooltip.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.tooltip.anchor().is_some() {
            let label = self.tooltip.label(cx, ids!(content.tooltip_label));
            let text_width = label.borrow_mut().map_or(0.0, |mut label| {
                label.draw_text.color = self.options.text_color;
                let scale = (label.draw_text.font_scale as f64).max(0.0001);
                label.draw_text
                    .layout(cx, 0.0, 0.0, None, false, label.align, &self.text)
                    .size_in_lpxs.width as f64 * scale
            });
            let padding = self.tooltip.widget(cx, ids!(content)).borrow::<View>()
                .map_or(0.0, |content| content.layout.padding.width());
            if let Some(anchor) = self.tooltip.anchor_mut() {
                anchor.natural_width = Some((text_width + padding).ceil());
            }
        }

        self.tooltip.draw_walk(cx, scope, walk)?;

        // Now that the content is in place, point the callout at the anchor.
        let (Some(anchor), Some(placement)) = (self.tooltip.anchor(), self.tooltip.placement()) else {
            return DrawStep::done();
        };
        let center = anchor.rect.center();
        let (callout_position, callout_offset) = match placement.side {
            TooltipPosition::Top => (180.0, center.x - placement.rect.pos.x),
            TooltipPosition::Bottom => (0.0, center.x - placement.rect.pos.x),
            TooltipPosition::Left => (90.0, center.y - placement.rect.pos.y),
            TooltipPosition::Right => (270.0, center.y - placement.rect.pos.y),
        };
        let content = self.tooltip.widget(cx, ids!(content));
        let Some(mut content) = content.borrow_mut::<View>() else {
            return DrawStep::done();
        };
        let bg = self.options.bg_color;
        let draw_bg = &mut content.draw_bg;
        draw_bg.set_instance_on_area(cx, id!(background_color), &[bg.x, bg.y, bg.z, bg.w]);
        draw_bg.set_instance_on_area(cx, id!(triangle_height), &[self.options.triangle_height as f32]);
        draw_bg.set_instance_on_area(cx, id!(callout_position), &[callout_position]);
        draw_bg.set_instance_on_area(cx, id!(callout_offset), &[callout_offset as f32]);

        DrawStep::done()
    }
}

impl WidgetMatchEvent for CalloutTooltip {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        // Reduce all `TooltipAction`s in this batch to a single final state.
        // Widget tree traversal order means a `HoverIn` for a new button can
        // arrive before a `HoverOut` for the old button in the same batch;
        // applying them in queue order would show then immediately hide.
        // Rule: latest `HoverIn` wins; a `HoverOut` only clears the buffer
        // if its `widget_uid` matches the buffered `HoverIn`.
        let mut buffered: Option<(WidgetUid, String, Rect, CalloutTooltipOptions)> = None;
        let mut had_tooltip_event = false;

        for action in actions {
            match action.as_widget_action().cast() {
                TooltipAction::HoverIn {
                    text,
                    widget_rect,
                    options,
                } => {
                    had_tooltip_event = true;
                    if let Some(uid) = action.as_widget_action().map(|wa| wa.widget_uid) {
                        buffered = Some((uid, text, widget_rect, options));
                    }
                }
                TooltipAction::HoverOut => {
                    had_tooltip_event = true;
                    if let Some(uid) = action.as_widget_action().map(|wa| wa.widget_uid) {
                        if let Some((cur_uid, _, _, _)) = &buffered {
                            if *cur_uid == uid {
                                buffered = None;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if !had_tooltip_event {
            return;
        }

        if let Some((_uid, text, widget_rect, options)) = buffered {
            self.show_with_options(cx, &text, widget_rect, options);
        } else {
            self.tooltip.hide(cx);
        }
    }
}

impl CalloutTooltip {
    /// Shows the tooltip with its callout pointing at `widget_rect`, on the side `options.position` asks for.
    /// It flips to the opposite side when it doesn't fit and that side has more room, and wraps text that's too wide.
    pub fn show_with_options(
        &mut self,
        cx: &mut Cx,
        text: &str,
        widget_rect: Rect,
        options: CalloutTooltipOptions,
    ) {
        self.tooltip.set_text(cx, text);
        self.text = text.to_owned();
        self.options = options;
        self.tooltip.show_anchored(cx, TooltipAnchor {
            rect: widget_rect,
            side: options.position,
            gap: 0.0,
            natural_width: None,
        });
    }
}

impl CalloutTooltipRef {
    /// See [`CalloutTooltip::show_with_options()`].
    pub fn show_with_options(
        &mut self,
        cx: &mut Cx,
        text: &str,
        widget_rect: Rect,
        options: CalloutTooltipOptions,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_with_options(cx, text, widget_rect, options);
        }
    }

    /// See [`Tooltip::hide()`].
    pub fn hide(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.hide(cx);
        }
    }
}

/// Actions that can be emitted from anywhere to show or hide the `tooltip`.
#[derive(Clone, Debug, Default)]
pub enum TooltipAction {
    /// Show the tooltip with the given text and options.
    HoverIn {
        text: String,
        /// The location of the widget that the tooltip is positioned relative to.
        widget_rect: Rect,
        options: CalloutTooltipOptions,
    },
    /// Hide the tooltip.
    HoverOut,
    #[default]
    None,
}
