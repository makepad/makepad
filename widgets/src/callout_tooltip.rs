//! A formatted tooltip with a callout triangle/"arrow" that points to the referenced widget.
//!
//! By default, the tooltip has a black background color and white text.

use crate::{TooltipRef, TooltipWidgetExt, label::*, makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

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
    #[default] Right,
}

/// A tooltip widget that a callout pointing towards the referenced widget.
#[derive(Script, ScriptHook, Widget)]
pub struct CalloutTooltip {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,

    // The below items are a hack to re-populate this tooltip automatically
    // after a certain time interval, because its repositioning code is
    // currently broken and needs to be rewritten entirely.
    #[rust] timer_redraw: Timer,
    #[rust] latest_options: Option<(String, Rect, CalloutTooltipOptions)>,
    #[rust] text_unwrapped_width: Option<f64>,
    #[rust] previous_height: Option<f64>,
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

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl CalloutTooltip {
    /// Calculate tooltip position and layout parameters for a given position
    fn calculate_position(
        options: &CalloutTooltipOptions,
        widget_rect: Rect,
        expected_dimension: DVec2,
        screen_size: DVec2,
        triangle_height: f64,
    ) -> PositionCalculation {
        let pos = widget_rect.pos;
        let size = widget_rect.size;
        let mut tooltip_pos = DVec2 {
            x: min(pos.x, screen_size.x - expected_dimension.x),
            y: min(
                pos.y + widget_rect.size.y,
                screen_size.y - expected_dimension.y,
            ),
        };
        let mut fixed_width = false;
        let mut callout_position = 0.0;
        let mut width_to_be_fixed = screen_size.x;

        match options.position {
            TooltipPosition::Top => {
                tooltip_pos.y = widget_rect.pos.y - max(expected_dimension.y, size.y);
                callout_position = 180.0;
            }
            TooltipPosition::Bottom => {
                if tooltip_pos.y == screen_size.y - expected_dimension.y {
                    tooltip_pos.y = widget_rect.pos.y - max(expected_dimension.y, size.y);
                    callout_position = 180.0;
                } else {
                    tooltip_pos.y = widget_rect.pos.y + widget_rect.size.y;
                }
            }
            TooltipPosition::Left => {
                tooltip_pos.x = widget_rect.pos.x
                    - max(expected_dimension.x, widget_rect.size.x)
                    - triangle_height;
                tooltip_pos.y = widget_rect.pos.y
                    + 0.5 * (widget_rect.size.y - max(expected_dimension.y, widget_rect.size.y));
                callout_position = 90.0;
            }
            TooltipPosition::Right => {
                tooltip_pos.x = widget_rect.pos.x + widget_rect.size.x;
                tooltip_pos.y = widget_rect.pos.y + 0.5 * widget_rect.size.y - expected_dimension.y * 0.5;
                width_to_be_fixed = max(
                    screen_size.x - (pos.x + widget_rect.size.x + triangle_height * 2.0),
                    expected_dimension.x,
                );
                callout_position = 270.0;
            }
        }
        if expected_dimension.x > 0.0 && screen_size.x > 0.0 {
            Self::apply_edge_case_fix(
                &options.position,
                &mut tooltip_pos,
                &mut fixed_width,
                &mut width_to_be_fixed,
                screen_size,
                expected_dimension,
                widget_rect,
                triangle_height,
            );
        }

        PositionCalculation {
            tooltip_pos,
            callout_position,
            fixed_width,
            width_to_be_fixed,
        }
    }

    /// Check if width fixing is needed for edge cases
    fn needs_width_fix(tooltip_x: f64, screen_width: f64, expected_width: f64) -> bool {
        tooltip_x == screen_width - expected_width && tooltip_x < 0.0
    }
    
    /// Apply edge case handling for position and width fixing
    fn apply_edge_case_fix(
        position: &TooltipPosition,
        tooltip_pos: &mut DVec2,
        fixed_width: &mut bool,
        width_to_be_fixed: &mut f64,
        screen_size: DVec2,
        expected_dimension: DVec2,
        widget_rect: Rect,
        triangle_height: f64,
    ) {
        match position {
            TooltipPosition::Top | TooltipPosition::Bottom => {
                if Self::needs_width_fix(tooltip_pos.x, screen_size.x, expected_dimension.x) {
                    *fixed_width = true;
                    tooltip_pos.x = 0.0;
                }
            }
            TooltipPosition::Left => {
                if tooltip_pos.x < 0.0 {
                    *fixed_width = true;
                    *width_to_be_fixed = widget_rect.pos.x - triangle_height;
                    tooltip_pos.x = 0.0;
                }
            }
            TooltipPosition::Right => {
                if *width_to_be_fixed == expected_dimension.x
                    && *width_to_be_fixed > screen_size.x - widget_rect.pos.x - widget_rect.size.x
                {
                    *fixed_width = true;
                    *width_to_be_fixed = screen_size.x - widget_rect.pos.x - widget_rect.size.x;
                }
            }
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
        let tooltip_pos = vec2(position_calc.tooltip_pos.x as f32, position_calc.tooltip_pos.y as f32);
        let triangle_height = triangle_height as f32;
        let expected_dimension_x = expected_dimension.x as f32;
        let callout_position = position_calc.callout_position as f32;
        let margin = Inset {
            left: tooltip_pos.x as f64,
            top: tooltip_pos.y as f64,
            right: 0.0,
            bottom: 0.0,
        };

        let mut content_view = tooltip.view(cx, ids!(content));
        script_apply_eval!(cx, content_view, {
            margin: #(margin)
        });
        if position_calc.fixed_width {
            let fixed_width = position_calc.width_to_be_fixed.max(24.0);
            script_apply_eval!(cx, content_view, {
                width: #(fixed_width)
            });
        } else {
            script_apply_eval!(cx, content_view, {
                width: #(Size::fit())
            });
        }

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

        let tooltip_label = tooltip.label(cx, ids!(content.tooltip_label));
        if let Some(mut label) = tooltip_label.borrow_mut() {
            label.draw_text.color = text_color;
            if position_calc.fixed_width {
                // Because parent width is constrained, we MUST Fill so the text wraps line-by-line
                label.walk.width = Size::fill();
            } else {
                // Because parent width is Fit, we MUST Fit so the string natural width evaluates properly 
                label.walk.width = Size::fit();
            }
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
            self.latest_options = Some((
                text.to_owned(),
                widget_rect.clone(),
                options.clone(),
            ));
            self.text_unwrapped_width = None;
            self.previous_height = None;
        }

        let mut tooltip = self.view.tooltip(cx, ids!(tooltip));
        tooltip.label(cx, ids!(content.tooltip_label)).set_text(cx, &pad_last_line(text));

        let mut expected_dimension = tooltip
            .view(cx, ids!(content))
            .area()
            .rect(cx)
            .size;
        let screen_size = tooltip.area().rect(cx).size;

        if let Some(w) = self.text_unwrapped_width {
            expected_dimension.x = w;
        } else if expected_dimension.x > 0.0 {
            self.text_unwrapped_width = Some(expected_dimension.x);
        }

        let position_calc = Self::calculate_position(
            &options,
            widget_rect,
            expected_dimension,
            screen_size,
            options.triangle_height,
        );

        let target = vec2(
            widget_rect.pos.x as f32,
            widget_rect.pos.y as f32,
        );
        let target_size = vec2(
            widget_rect.size.x as f32,
            widget_rect.size.y as f32,
        );

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
    let lines = text.split('\n');
    let (lines_len, _) = lines.size_hint();
    if lines_len <= 1 {
        return text.to_string();
    }
    let longest_line = lines.clone().map(|s| s.len()).max().unwrap_or(0);
    let mut full_text = String::with_capacity(text.len() + longest_line + 4);
    for (i, line) in lines.enumerate() {
        full_text.push_str(line);
        if i < lines_len - 1 {
            full_text.push('\n');
        } else {
            // Plus 4 is added to add more width to the last line otherwise the first line is still being cut off
            full_text.push_str(&" ".repeat(longest_line - line.len() + 4));
        }
    }
    full_text
}
