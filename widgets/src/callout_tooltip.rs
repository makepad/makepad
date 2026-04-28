//! A formatted tooltip with a callout triangle/"arrow" that points to the referenced widget.
//!
//! By default, the tooltip has a black background color and white text.

use crate::{
    label::*, makepad_derive_widget::*, makepad_draw::*, view::*, widget::*,
    widget_match_event::WidgetMatchEvent, TooltipRef, TooltipWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A tooltip that appears when hovering over target's area
    mod.widgets.CalloutTooltipInner = Tooltip {
        content := RoundedView {
            width: Fit
            height: Fit
            padding: 15

            draw_bg +: {
                color: #fff,
                border_color: #D0D5DD,
                border_radius: 2.,
                background_color: instance(#3b444b),
                // Absolute position of top left corner of the tooltip
                tooltip_pos: instance(vec2(0.0, 0.0)),
                // Absolute position of the moused over widget
                target_pos: instance(vec2(0.0, 0.0)),
                // Size of the moused over widget
                target_size: instance(vec2(0.0, 0.0)),
                // Expected Width of the the tooltip
                expected_dimension_x: instance(0.0),
                // Determine height of the triangle in the callout pointer
                triangle_height: instance(7.5),
                // Determine angle of the triangle in the callout pointer in degrees
                callout_position: instance(180.0),

                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size);
                    let rect_size = self.rect_size;
                    let triangle_height = self.triangle_height;
                    // If there is no expected_dimension_x, it means the tooltip size is not calculated yet, do not draw anything
                    if self.expected_dimension_x == 0.0 {
                        return sdf.result;
                    }
                    // Draw rounded box with border equals to triangle_height.
                    sdf.box(
                        triangle_height,
                        triangle_height,
                        rect_size.x - (triangle_height * 2.0),
                        rect_size.y - (triangle_height * 2.0),
                        max(1.0, self.border_radius)
                    )
                    sdf.fill(self.background_color);

                    let mut vertex1 = vec2(0.0, 0.0);
                    let mut vertex2 = vec2(0.0, 0.0);
                    let mut vertex3 = vec2(0.0, 0.0);
                    if self.callout_position == 0.0 {
                        // Point upwards
                        // + 2.0 to overlap the triangle
                        let diff_x = self.target_pos.x + self.target_size.x / 2.0 - self.tooltip_pos.x - triangle_height;
                        vertex1 = vec2(
                            min(max(triangle_height + 2.0, diff_x), rect_size.x - triangle_height * 3.0 - 2.0),
                            triangle_height + 2.0
                        );
                        vertex2 = vec2(vertex1.x + triangle_height, vertex1.y - triangle_height);
                        vertex3 = vec2(vertex1.x + triangle_height * 2.0, vertex1.y);
                    } else if self.callout_position == 90.0 {
                        // Point rightwards
                        // Triangle points to the right from the left edge of the tooltip
                        vertex1 = vec2(rect_size.x - 2.0, rect_size.y * 0.5);
                        vertex2 = vec2(vertex1.x - triangle_height, vertex1.y + triangle_height);
                        vertex3 = vec2(vertex1.x - triangle_height, vertex1.y - triangle_height);
                    } else if self.callout_position == 180.0 {
                        // Point downwards
                        // +/- 2.0 to overlap the triangle
                        let diff_x = self.target_pos.x + self.target_size.x / 2.0 - self.tooltip_pos.x + triangle_height;
                        vertex1 = vec2(
                            min(max(triangle_height * 3.0 + 2.0, diff_x), rect_size.x - triangle_height - 2.0),
                            rect_size.y - triangle_height - 2.0
                        );
                        vertex2 = vec2(vertex1.x - triangle_height, vertex1.y + triangle_height);
                        vertex3 = vec2(vertex1.x - triangle_height * 2.0, vertex1.y);
                    } else {
                        // Point leftwards
                        // Triangle points to the left from the right edge of the tooltip
                        vertex1 = vec2(2.0, rect_size.y * 0.5);
                        vertex2 = vec2(vertex1.x + triangle_height, vertex1.y - triangle_height);
                        vertex3 = vec2(vertex1.x + triangle_height, vertex1.y + triangle_height);
                    }
                    sdf.move_to(vertex1.x, vertex1.y);
                    sdf.line_to(vertex2.x, vertex2.y);
                    sdf.line_to(vertex3.x, vertex3.y);
                    sdf.close_path();
                    sdf.fill(self.background_color);
                    return sdf.result;
                }
            }

            tooltip_label := Label {
                width: Fit
                height: Fit
                draw_text +: {
                    text_style: theme.font_regular {font_size: 9},
                    color: #FFF
                }
            }
        }
    }

    mod.widgets.CalloutTooltip = #(CalloutTooltip::register_widget(vm)) {
        tooltip := mod.widgets.CalloutTooltipInner { }
    }
}

/// Options that affect how a CalloutTooltip is displayed.
///
/// You don't have to specify all values, they each have a sensible default.
#[derive(Clone, Debug)]
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

/// The location of the tooltip with respect to its target widget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TooltipPosition {
    /// The tooltip will be drawn above the target widget.
    Top,
    /// The tooltip will be drawn below the target widget.
    Bottom,
    /// The tooltip will be drawn to the left of the target widget.
    Left,
    /// (Default) The tooltip will be drawn to the right of the target widget.
    #[default]
    Right,
}

/// A tooltip widget that a callout pointing towards the referenced widget.
///
/// `CalloutTooltip` automatically listens for `TooltipAction::HoverIn` and
/// `TooltipAction::HoverOut` events from any widget in the action batch and
/// shows or hides itself accordingly. Apps generally do **not** need to
/// handle `TooltipAction` themselves — it is enough to instantiate one
/// `CalloutTooltip` somewhere in the widget tree and have hover-aware
/// widgets emit `TooltipAction::HoverIn` from their `handle_event` (e.g.
/// inside a `Hit::FingerHoverIn` arm).
#[derive(Script, ScriptHook, Widget)]
pub struct CalloutTooltip {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    // The below items are a hack to re-populate this tooltip automatically
    // after a certain time interval, because its repositioning code is
    // currently broken and needs to be rewritten entirely.
    #[rust]
    timer_redraw: Timer,
    #[rust]
    latest_options: Option<(String, Rect, CalloutTooltipOptions)>,
    #[rust]
    text_unwrapped_width: Option<f64>,
    #[rust]
    previous_height: Option<f64>,
}

#[derive(Debug)]
struct PositionCalculation {
    tooltip_pos: DVec2,
    callout_position: f64,
    fixed_width: bool,
    width_to_be_fixed: f64,
}

impl Widget for CalloutTooltip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.timer_redraw.is_event(event).is_some() {
            if let Some((text, widget_rect, options)) = self.latest_options.clone() {
                self.show_with_options(cx, &text, widget_rect, options, true);
            }
        }

        // Auto-process `TooltipAction`s emitted by other widgets in the
        // action batch via the `WidgetMatchEvent` impl below.
        self.widget_match_event(cx, event, scope);

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
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
            self.show_with_options(cx, &text, widget_rect, options, false);
        } else {
            self.hide(cx);
        }
    }
}

impl CalloutTooltip {
    /// Calculate tooltip position and layout parameters for a given position.
    ///
    /// For `Top`/`Bottom`, the tooltip is centered horizontally on the target
    /// widget (clamped to the available rect). For `Left`/`Right`, the tooltip
    /// is centered vertically on the target widget (clamped to the available
    /// rect). If the tooltip can't fit the available axis, `fixed_width` is
    /// set so the caller can constrain the layout width and force text
    /// wrapping.
    ///
    /// `available_rect` is the on-screen region the tooltip is allowed to
    /// occupy — typically the full pass minus any platform safe-area insets
    /// (notch, rounded corners, home indicator). Tooltips will not be drawn
    /// outside this rect.
    fn calculate_position(
        options: &CalloutTooltipOptions,
        widget_rect: Rect,
        expected_dimension: DVec2,
        available_rect: Rect,
        triangle_height: f64,
    ) -> PositionCalculation {
        let pos = widget_rect.pos;
        let size = widget_rect.size;
        let widget_center_x = pos.x + size.x * 0.5;
        let widget_center_y = pos.y + size.y * 0.5;

        let avail_left = available_rect.pos.x;
        let avail_top = available_rect.pos.y;
        let avail_right = avail_left + available_rect.size.x;
        let avail_bottom = avail_top + available_rect.size.y;
        let avail_width = available_rect.size.x;
        let avail_height = available_rect.size.y;

        let mut tooltip_pos = DVec2::default();
        let mut fixed_width = false;
        let mut callout_position = 0.0;
        let mut width_to_be_fixed = avail_width;

        // Skip full layout calculations until we know the tooltip's natural
        // size (first pre-draw pass reports zero). Position at the target's
        // top-left so the initial invisible draw is cheap.
        if expected_dimension.x <= 0.0 || avail_width <= 0.0 {
            tooltip_pos = DVec2 { x: pos.x, y: pos.y };
            return PositionCalculation {
                tooltip_pos,
                callout_position,
                fixed_width,
                width_to_be_fixed,
            };
        }

        match options.position {
            TooltipPosition::Top | TooltipPosition::Bottom => {
                // Center the tooltip horizontally on the target widget.
                let mut desired_x = widget_center_x - expected_dimension.x * 0.5;
                if expected_dimension.x >= avail_width {
                    // Tooltip is (or would be) wider than the available area:
                    // pin to the available left edge and clamp the width so
                    // the text wraps.
                    fixed_width = true;
                    width_to_be_fixed = avail_width;
                    desired_x = avail_left;
                } else {
                    // Clamp so the tooltip stays inside the available rect.
                    if desired_x < avail_left {
                        desired_x = avail_left;
                    } else if desired_x + expected_dimension.x > avail_right {
                        desired_x = avail_right - expected_dimension.x;
                    }
                }
                tooltip_pos.x = desired_x;

                // Choose vertical placement. For `Top` we go above the widget
                // (flipping below if there isn't room). For `Bottom` we go
                // below (flipping above if there isn't room).
                let y_above = pos.y - max(expected_dimension.y, size.y);
                let y_below = pos.y + size.y;
                let fits_above = y_above >= avail_top;
                let fits_below = y_below + expected_dimension.y <= avail_bottom;

                match options.position {
                    TooltipPosition::Top => {
                        if fits_above || !fits_below {
                            tooltip_pos.y = y_above;
                            callout_position = 180.0;
                        } else {
                            tooltip_pos.y = y_below;
                            callout_position = 0.0;
                        }
                    }
                    TooltipPosition::Bottom => {
                        if fits_below || !fits_above {
                            tooltip_pos.y = y_below;
                            callout_position = 0.0;
                        } else {
                            tooltip_pos.y = y_above;
                            callout_position = 180.0;
                        }
                    }
                    _ => {}
                }
            }
            TooltipPosition::Left => {
                tooltip_pos.x = pos.x - expected_dimension.x - triangle_height;
                if tooltip_pos.x < avail_left {
                    fixed_width = true;
                    // Leave room for the callout arrow.
                    width_to_be_fixed = (pos.x - triangle_height - avail_left).max(24.0);
                    tooltip_pos.x = avail_left;
                }

                let mut desired_y = widget_center_y - expected_dimension.y * 0.5;
                if desired_y < avail_top {
                    desired_y = avail_top;
                } else if desired_y + expected_dimension.y > avail_bottom {
                    desired_y = (avail_bottom - expected_dimension.y).max(avail_top);
                }
                tooltip_pos.y = desired_y;
                callout_position = 90.0;
            }
            TooltipPosition::Right => {
                tooltip_pos.x = pos.x + size.x;
                let available_x = (avail_right - tooltip_pos.x - triangle_height * 2.0).max(0.0);
                if expected_dimension.x > available_x {
                    fixed_width = true;
                    width_to_be_fixed = available_x.max(24.0);
                }

                let mut desired_y = widget_center_y - expected_dimension.y * 0.5;
                if desired_y < avail_top {
                    desired_y = avail_top;
                } else if desired_y + expected_dimension.y > avail_bottom {
                    desired_y = (avail_bottom - expected_dimension.y).max(avail_top);
                }
                tooltip_pos.y = desired_y;
                callout_position = 270.0;
            }
        }

        // Final top/bottom clamp to keep the tooltip inside the available rect.
        if tooltip_pos.y < avail_top {
            tooltip_pos.y = avail_top;
        } else if tooltip_pos.y + expected_dimension.y > avail_bottom {
            tooltip_pos.y = (avail_bottom - expected_dimension.y).max(avail_top);
        }
        // Vertical height clamp: tooltip wider than available height shouldn't
        // overflow — pin to top and let the content clip if necessary.
        if expected_dimension.y >= avail_height {
            tooltip_pos.y = avail_top;
        }

        PositionCalculation {
            tooltip_pos,
            callout_position,
            fixed_width,
            width_to_be_fixed,
        }
    }

    /// Apply tooltip configuration with given parameters
    fn apply_tooltip_config(
        tooltip: &mut TooltipRef,
        cx: &mut Cx,
        position_calc: &PositionCalculation,
        target: Vec2,
        target_size: Vec2,
        expected_dimension: DVec2,
        triangle_height: f64,
        text_color: Vec4,
        bg_color: Vec4,
    ) {
        let tooltip_pos = vec2(
            position_calc.tooltip_pos.x as f32,
            position_calc.tooltip_pos.y as f32,
        );
        let triangle_height = triangle_height as f32;
        let expected_dimension_x = expected_dimension.x as f32;
        let callout_position = position_calc.callout_position as f32;
        let margin = Inset {
            left: tooltip_pos.x as f64,
            top: tooltip_pos.y as f64,
            right: 0.0,
            bottom: 0.0,
        };

        // Apply the draw_bg shader instances FIRST. `script_apply_eval!` on a
        // View runs `apply` over the whole struct, which can reset fields not
        // mentioned in the script (like `walk.width`) back to their DSL
        // defaults (Fit). Doing this BEFORE the direct walk writes below
        // ensures our width override wins.
        let mut content = tooltip.view(cx, ids!(content));
        script_apply_eval!(cx, content, {
            draw_bg +: {
                triangle_height: #(triangle_height)
                background_color: #(bg_color)
                tooltip_pos: #(tooltip_pos)
                target_pos: #(target)
                target_size: #(target_size)
                expected_dimension_x: #(expected_dimension_x)
                callout_position: #(callout_position)
            }
        });

        // Now set the content view's walk fields directly via `borrow_mut()`
        // so the next draw uses the correct width. We use `Size::Fixed` for
        // both the content view AND the label when `fixed_width` is set,
        // because the wrapping in `DrawText::draw_walk` keys off the turtle's
        // resolved width — `Size::Fill` inside an Overlay-flow parent can
        // resolve to the full pass width on some platforms instead of the
        // parent's constrained inner width, which is the iOS bug we hit.
        // `Size::Fixed` removes that ambiguity entirely.
        const CONTENT_PADDING: f64 = 15.0;
        let content_view = tooltip.view(cx, ids!(content));
        if let Some(mut view) = content_view.borrow_mut() {
            view.walk.margin = margin;
            if position_calc.fixed_width {
                let fixed_width = position_calc.width_to_be_fixed.max(24.0);
                view.walk.width = Size::Fixed(fixed_width);
            } else {
                view.walk.width = Size::fit();
            }
            view.redraw(cx);
        }

        let tooltip_label = tooltip.label(cx, ids!(content.tooltip_label));
        if let Some(mut label) = tooltip_label.borrow_mut() {
            label.draw_text.color = text_color;
            if position_calc.fixed_width {
                let fixed_width = position_calc.width_to_be_fixed.max(24.0);
                let label_width = (fixed_width - CONTENT_PADDING * 2.0).max(24.0);
                label.walk.width = Size::Fixed(label_width);
            } else {
                label.walk.width = Size::fit();
            }
            label.redraw(cx);
        };
    }

    /// Shows a tooltip with the given text and options.
    ///
    /// The tooltip comes with a callout pointing to its target.
    ///
    /// By default, the tooltip will be displayed to the widget's right.
    ///
    /// If the widget is too close to the edge of the window, the tooltip is positioned
    /// to avoid being cut off, with automatic fallback to opposite directions.
    pub fn show_with_options(
        &mut self,
        cx: &mut Cx,
        text: &str,
        widget_rect: Rect,
        options: CalloutTooltipOptions,
        is_internal_redraw: bool,
    ) {
        if !is_internal_redraw {
            self.latest_options = Some((text.to_owned(), widget_rect.clone(), options.clone()));
            self.text_unwrapped_width = None;
            self.previous_height = None;
        }

        let mut tooltip = self.view.tooltip(cx, ids!(tooltip));
        tooltip
            .label(cx, ids!(content.tooltip_label))
            .set_text(cx, &pad_last_line(text));

        let screen_size = tooltip.area().rect(cx).size;

        // Subtract platform safe-area insets (notch, rounded corners, home
        // indicator) so the tooltip doesn't spill into hardware-occluded
        // regions. On macOS/desktop these are all zero.
        let insets = cx.display_context.safe_area_insets;
        let available_rect = Rect {
            pos: DVec2 {
                x: insets.left,
                y: insets.top,
            },
            size: DVec2 {
                x: (screen_size.x - insets.left - insets.right).max(0.0),
                y: (screen_size.y - insets.top - insets.bottom).max(0.0),
            },
        };

        // On the external (non-internal-redraw) call we MUST NOT trust the
        // content view's `area().rect()`: it still reflects the previous
        // draw of the *previous* tooltip text. Saving that as
        // `text_unwrapped_width` would lock the wrap decision to a stale
        // value (e.g. a previous "Fully synced" tooltip) and the new long
        // text would never trigger `fixed_width`. Force `expected_dimension`
        // to zero on the external call; the next draw measures the new
        // text's natural width with `Size::Fit`, and the internal redraw
        // (5ms later) reads that fresh value.
        let mut expected_dimension = if is_internal_redraw {
            tooltip.view(cx, ids!(content)).area().rect(cx).size
        } else {
            DVec2::default()
        };

        if let Some(w) = self.text_unwrapped_width {
            expected_dimension.x = w;
        } else if is_internal_redraw && expected_dimension.x > 0.0 {
            self.text_unwrapped_width = Some(expected_dimension.x);
        }

        let position_calc = Self::calculate_position(
            &options,
            widget_rect,
            expected_dimension,
            available_rect,
            options.triangle_height,
        );

        let target = vec2(widget_rect.pos.x as f32, widget_rect.pos.y as f32);
        let target_size = vec2(widget_rect.size.x as f32, widget_rect.size.y as f32);

        // A temp hack to hide the tooltip until the size is calculated.
        // Once we can immediately move the tooltip after its first draw,
        // we can remove this and other similar hacks.
        let mut text_color = options.text_color;
        if expected_dimension.x == 0.0 {
            text_color.w = 0.0;
        }

        Self::apply_tooltip_config(
            &mut tooltip,
            cx,
            &position_calc,
            target,
            target_size,
            expected_dimension,
            options.triangle_height,
            text_color,
            options.bg_color,
        );

        cx.stop_timer(self.timer_redraw);

        const REDRAW_DELAY: f64 = 0.005; // 5 ms

        // Stabilize layout check: Trigger another rendering sweep if the height fundamentally changed
        if is_internal_redraw {
            if let Some(prev) = self.previous_height {
                if prev != expected_dimension.y {
                    self.previous_height = Some(expected_dimension.y);
                    self.timer_redraw = cx.start_timeout(REDRAW_DELAY);
                } else {
                    self.latest_options = None;
                }
            } else {
                self.previous_height = Some(expected_dimension.y);
                self.timer_redraw = cx.start_timeout(REDRAW_DELAY);
            }
        } else {
            self.previous_height = Some(expected_dimension.y);
            self.timer_redraw = cx.start_timeout(REDRAW_DELAY);
        }

        self.view.tooltip(cx, ids!(tooltip)).show(cx);
    }

    /// Hide the tooltip.
    pub fn hide(&mut self, cx: &mut Cx) {
        self.latest_options = None;
        cx.stop_timer(self.timer_redraw);
        self.timer_redraw = Timer::empty();
        self.view.tooltip(cx, ids!(tooltip)).hide(cx);
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
            inner.show_with_options(cx, text, widget_rect, options, false);
        }
    }

    /// See [`CalloutTooltip::hide()`].
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

/// Takes a string and lengthens the last line of the string to be the same
/// length as the longest line in the string.
///
/// This is useful for creating tooltips that line up with the text above
/// them.
fn pad_last_line(text: &str) -> String {
    // `Split::size_hint()` returns a bound, not the actual count, so the
    // previous version often early-returned without padding. Collect once and
    // use the real length.
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= 1 {
        return text.to_string();
    }
    let longest_line = lines.iter().map(|s| s.len()).max().unwrap_or(0);
    let last_idx = lines.len() - 1;
    let mut full_text = String::with_capacity(text.len() + longest_line + 4);
    for (i, line) in lines.iter().enumerate() {
        full_text.push_str(line);
        if i < last_idx {
            full_text.push('\n');
        } else {
            // Plus 4 is added to add more width to the last line otherwise the first line is still being cut off
            full_text.push_str(&" ".repeat(longest_line - line.len() + 4));
        }
    }
    full_text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_options(position: TooltipPosition) -> CalloutTooltipOptions {
        CalloutTooltipOptions {
            position,
            ..Default::default()
        }
    }

    fn full_screen(width: f64, height: f64) -> Rect {
        Rect {
            pos: DVec2::default(),
            size: DVec2 {
                x: width,
                y: height,
            },
        }
    }

    #[test]
    fn top_centers_horizontally_on_widget_when_fits() {
        // Widget at x=180-220 (center=200), screen 400 wide, tooltip natural 100 wide.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 180.0, y: 100.0 },
            size: DVec2 { x: 40.0, y: 30.0 },
        };
        let expected = DVec2 { x: 100.0, y: 50.0 };
        let avail = full_screen(400.0, 600.0);
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        // Tooltip should be centered horizontally on widget (center=200 - 50 = 150).
        assert_eq!(calc.tooltip_pos.x, 150.0);
        assert!(
            !calc.fixed_width,
            "tooltip fits, should not need fixed_width"
        );
    }

    #[test]
    fn top_clamps_to_left_edge_when_widget_near_left() {
        // Widget at x=10-30 (center=20), tooltip 100 wide, would extend past left edge.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 10.0, y: 100.0 },
            size: DVec2 { x: 20.0, y: 30.0 },
        };
        let expected = DVec2 { x: 100.0, y: 50.0 };
        let avail = full_screen(400.0, 600.0);
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        assert_eq!(calc.tooltip_pos.x, 0.0);
        assert!(!calc.fixed_width);
    }

    #[test]
    fn top_clamps_to_right_edge_when_widget_near_right() {
        // Widget at x=370-390, tooltip 100 wide centered would extend past right edge (400).
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 370.0, y: 100.0 },
            size: DVec2 { x: 20.0, y: 30.0 },
        };
        let expected = DVec2 { x: 100.0, y: 50.0 };
        let avail = full_screen(400.0, 600.0);
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        // Right edge of tooltip should be at screen edge => tooltip_pos.x = 300.
        assert_eq!(calc.tooltip_pos.x, 300.0);
        assert!(!calc.fixed_width);
    }

    #[test]
    fn top_triggers_fixed_width_when_tooltip_wider_than_screen() {
        // Tooltip natural width 600 > screen 400. Should set fixed_width = true.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 350.0, y: 100.0 },
            size: DVec2 { x: 30.0, y: 30.0 },
        };
        let expected = DVec2 { x: 600.0, y: 50.0 };
        let avail = full_screen(400.0, 600.0);
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        assert!(
            calc.fixed_width,
            "tooltip wider than screen must trigger fixed_width"
        );
        assert_eq!(calc.tooltip_pos.x, 0.0);
        assert_eq!(calc.width_to_be_fixed, 400.0);
    }

    #[test]
    fn external_call_with_zero_expected_dimension_early_returns() {
        // expected.x = 0 (uninitialized) => early return, fixed_width = false.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 100.0, y: 100.0 },
            size: DVec2 { x: 40.0, y: 30.0 },
        };
        let expected = DVec2 { x: 0.0, y: 0.0 };
        let avail = full_screen(400.0, 600.0);
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        assert!(!calc.fixed_width);
        assert_eq!(calc.tooltip_pos.x, 100.0);
        assert_eq!(calc.tooltip_pos.y, 100.0);
    }

    #[test]
    fn safe_area_insets_clamp_to_inset_left_edge() {
        // Landscape iOS: 60px left inset (notch), tooltip wider than safe area.
        // Tooltip should pin to the inset left edge, not absolute x=0, and the
        // fixed width should be the safe-area width, not the full screen.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 700.0, y: 350.0 },
            size: DVec2 { x: 50.0, y: 50.0 },
        };
        let expected = DVec2 { x: 800.0, y: 80.0 };
        // Full screen is 800x400; safe area excludes 60px on left/right.
        let avail = Rect {
            pos: DVec2 { x: 60.0, y: 0.0 },
            size: DVec2 { x: 680.0, y: 400.0 },
        };
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        assert!(calc.fixed_width);
        assert_eq!(
            calc.tooltip_pos.x, 60.0,
            "should pin to safe-area left, not 0"
        );
        assert_eq!(calc.width_to_be_fixed, 680.0, "should use safe-area width");
    }

    #[test]
    fn safe_area_insets_clamp_centered_tooltip_inside_inset_right_edge() {
        // Widget near the safe-area right edge: centered tooltip would extend
        // past the inset right edge — clamp it back inside.
        let options = make_options(TooltipPosition::Top);
        let widget_rect = Rect {
            pos: DVec2 { x: 720.0, y: 200.0 },
            size: DVec2 { x: 20.0, y: 20.0 },
        };
        let expected = DVec2 { x: 200.0, y: 60.0 };
        let avail = Rect {
            pos: DVec2 { x: 60.0, y: 0.0 },
            size: DVec2 { x: 680.0, y: 400.0 },
        };
        let calc = CalloutTooltip::calculate_position(&options, widget_rect, expected, avail, 7.5);

        // Available right edge = 60 + 680 = 740. Tooltip right edge clamped
        // to 740 => tooltip_pos.x = 740 - 200 = 540.
        assert_eq!(calc.tooltip_pos.x, 540.0);
        assert!(!calc.fixed_width);
    }

    #[test]
    fn pad_last_line_pads_multiline_to_longest() {
        // Longest line "This device is unverif" is 22 chars; last line
        // "verify it." is 10 chars. Last line should be padded with
        // (22 - 10) + 4 = 16 trailing spaces.
        let input = "Logged in.\nThis device is unverif\nverify it.";
        let out = pad_last_line(input);
        assert!(out.starts_with("Logged in.\nThis device is unverif\nverify it."));
        let trailing_spaces = out.chars().rev().take_while(|c| *c == ' ').count();
        assert_eq!(trailing_spaces, 16);
    }

    #[test]
    fn pad_last_line_leaves_single_line_unchanged() {
        let input = "Just one line";
        assert_eq!(pad_last_line(input), input);
    }

    #[test]
    fn pad_last_line_handles_empty_lines() {
        // Real-world tooltip text uses "\n\n" for paragraph breaks.
        let input = "First.\n\nSecond.";
        let out = pad_last_line(input);
        assert!(out.starts_with("First.\n\nSecond."));
        // Longest is "Second." (7) — last line is "Second." (7) — pad = 0 + 4.
        let trailing_spaces = out.chars().rev().take_while(|c| *c == ' ').count();
        assert_eq!(trailing_spaces, 4);
    }
}
