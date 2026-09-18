//! ContentPlaceholder — the grey shape that stands in for content that has
//! not arrived yet.
//!
//! A blank pane while data loads reads as broken, and a spinner says "wait"
//! without saying "for what". A placeholder in the SHAPE of what is coming —
//! a title, three lines of body, an avatar beside two lines — says both:
//! the layout is already there, the eye already knows where to land, and
//! the content drops into place instead of arriving as a jolt. That is why
//! this is a family of presets (text, circle, image, button, input,
//! paragraph, row, card, table) over one shader rather than one grey box.
//!
//! The shimmer is what says the app is alive rather than hung. It is a
//! highlight band swept across the shape by `draw_pass.time`, or a pulse
//! of the whole shape, and it lives in its OWN shader: any shader that
//! reads the pass clock pins the window at display rate for as long as it
//! is on screen (see the glass panel's ripple), so the base shape is drawn
//! by a shader that never reads it, and the band is only drawn while the
//! placeholder is animating. `animation: Static`, or `reduced_motion` for
//! readers who asked the system for less movement, draws the shape alone
//! and costs the window nothing.
//!
//! Translucent placeholders sit over content that is still partly there
//! (a refresh, a stale row) without hiding it; opaque ones stand in for
//! content that is not.

use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

/// The outline the placeholder takes.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PlaceholderShape {
    #[pick]
    Rect = 0,
    Circle = 1,
    /// `lines` rows of `line_height`, the last one `last_line_width` wide.
    Lines = 2,
}

impl PlaceholderShape {
    pub fn name(self) -> &'static str {
        match self {
            PlaceholderShape::Rect => "rect",
            PlaceholderShape::Circle => "circle",
            PlaceholderShape::Lines => "lines",
        }
    }
}

/// How the placeholder moves: a band swept across it, the whole shape
/// breathing, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PlaceholderAnimation {
    #[pick]
    Wave = 0,
    Pulse = 1,
    Static = 2,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Rect and Circle are already the vector widgets' names, so the shape
    // stays qualified: `shape: PlaceholderShape.Circle`. The animation is
    // written bare: `animation: Pulse`.
    let PlaceholderShape = set_type_default() do #(PlaceholderShape::script_api(vm))
    mod.widgets.PlaceholderShape = PlaceholderShape
    mod.widgets.PlaceholderAnimation = set_type_default() do #(PlaceholderAnimation::script_api(vm))
    mod.widgets.splat(mod.widgets.PlaceholderAnimation)

    use mod.widgets.*

    mod.widgets.DrawPlaceholderBase = #(DrawPlaceholder::script_component(vm))
    set_type_default() do #(DrawPlaceholder::script_shader(vm)){
        ..mod.draw.DrawQuad
    }
    mod.widgets.DrawPlaceholderShimmerBase = #(DrawPlaceholderShimmer::script_component(vm))
    set_type_default() do #(DrawPlaceholderShimmer::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ContentPlaceholderBase = #(ContentPlaceholder::register_widget(vm))
    /** A grey shape standing in for content that has not loaded: a rect, a
     * circle or a stack of text lines, shimmering until the content comes. */
    mod.widgets.ContentPlaceholder = set_type_default() do mod.widgets.ContentPlaceholderBase{
        width: Fill
        height: 12
        /** PlaceholderShape.Rect Circle Lines */
        shape: PlaceholderShape.Rect
        /** Wave Pulse Static */
        animation: Wave
        /** no movement whatever `animation` says, for readers who asked for less */
        reduced_motion: false
        /** see through to content that is still partly there */
        translucent: false
        /** alpha while translucent 0..1 step 0.05 */
        translucent_opacity: 0.55
        /** rows of the Lines shape 1..12 step 1 */
        lines: 3
        /** height of one row of the Lines shape 4..24 step 0.5 */
        line_height: 10.0
        /** gap between rows of the Lines shape 0..16 step 0.5 */
        line_gap: 6.0
        /** width of the last row as a fraction of the whole 0.1..1 step 0.05 */
        last_line_width: 0.6
        /** corner radius of rects and rows 0..12 step 0.5 */
        radius: theme.radius_s
        /** alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        // The shape's geometry rides in the struct as instances; the colour
        // is the one uniform. This shader never reads the pass clock.
        draw_bg +: {
            /** the resting fill */
            color: uniform(theme.color_placeholder)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let p = self.pos * self.rect_size
                match self.shape {
                    PlaceholderShape.Circle => {
                        let r = min(self.rect_size.x, self.rect_size.y) * 0.5
                        sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, r)
                    }
                    PlaceholderShape.Lines => {
                        let pitch = self.line_height + self.line_gap
                        let i = floor(p.y / pitch)
                        let mut w = self.rect_size.x
                        if i >= self.lines - 1.0 {
                            w = self.rect_size.x * self.last_line_width
                        }
                        // `sdf.box` draws twice the radius it is given.
                        sdf.box(0.0, i * pitch, w, self.line_height, self.radius * 0.5)
                    }
                    _ => {
                        sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius * 0.5)
                    }
                }
                return sdf.fill(vec4(self.color.rgb, self.color.a * self.opacity))
            }
        }
        // The highlight, drawn over the shape only while animating: the one
        // shader here that reads `draw_pass.time`.
        draw_shimmer +: {
            /** the highlight the band or the pulse brings up */
            color_hl: uniform(theme.color_placeholder_hl)
            /** seconds for one sweep or one breath 0.4..5 step 0.1 */
            period: uniform(1.8)
            /** half-width of the swept band as a fraction of the shape 0.05..1 step 0.05 */
            band: uniform(0.35)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let p = self.pos * self.rect_size
                match self.shape {
                    PlaceholderShape.Circle => {
                        let r = min(self.rect_size.x, self.rect_size.y) * 0.5
                        sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, r)
                    }
                    PlaceholderShape.Lines => {
                        let pitch = self.line_height + self.line_gap
                        let i = floor(p.y / pitch)
                        let mut w = self.rect_size.x
                        if i >= self.lines - 1.0 {
                            w = self.rect_size.x * self.last_line_width
                        }
                        sdf.box(0.0, i * pitch, w, self.line_height, self.radius * 0.5)
                    }
                    _ => {
                        sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius * 0.5)
                    }
                }
                let t = fract(self.draw_pass.time / self.period)
                let hl = match self.animation {
                    PlaceholderAnimation.Pulse => 0.5 - 0.5 * cos(t * 6.2831853)
                    _ => {
                        // A diagonal band that enters at the left edge and
                        // leaves at the right, with a rest between sweeps.
                        let u = self.pos.x + self.pos.y * 0.25 - t * 2.2 + 0.5
                        1.0 - smoothstep(0.0, self.band, abs(u))
                    }
                }
                return sdf.fill(vec4(self.color_hl.rgb, self.color_hl.a * hl * self.opacity))
            }
        }
    }

    /** One line of text. */
    mod.widgets.PlaceholderText = mod.widgets.ContentPlaceholder{
        width: Fill
        height: 10
    }

    /** An avatar or an icon. */
    mod.widgets.PlaceholderCircle = mod.widgets.ContentPlaceholder{
        shape: PlaceholderShape.Circle
        width: 40
        height: 40
    }

    /** A picture. */
    mod.widgets.PlaceholderImage = mod.widgets.ContentPlaceholder{
        width: Fill
        height: 120
        radius: theme.radius_m
    }

    /** A button. */
    mod.widgets.PlaceholderButton = mod.widgets.ContentPlaceholder{
        width: 96
        height: theme.size_control_m
        radius: theme.radius_s
    }

    /** A text field, translucent because a field is a hole rather than a thing. */
    mod.widgets.PlaceholderInput = mod.widgets.ContentPlaceholder{
        width: Fill
        height: theme.size_control_m
        radius: theme.radius_s
        translucent: true
    }

    /** A title and three lines of body. */
    mod.widgets.PlaceholderParagraph = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        title := mod.widgets.ContentPlaceholder{
            width: 160
            height: 14
        }
        body := mod.widgets.ContentPlaceholder{
            shape: PlaceholderShape.Lines
            lines: 3
            width: Fill
            height: Fit
        }
    }

    /** An avatar beside two lines: a list row. */
    mod.widgets.PlaceholderRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0 y: 0.5}
        avatar := mod.widgets.PlaceholderCircle{}
        lines := View{
            width: Fill
            height: Fit
            flow: Down
            spacing: theme.space_1
            mod.widgets.ContentPlaceholder{width: 140 height: 10}
            mod.widgets.ContentPlaceholder{width: Fill height: 10}
        }
    }

    /** A picture, a title, two lines and a button on a card. */
    mod.widgets.PlaceholderCard = RoundedView{
        width: 240
        height: Fit
        flow: Down
        padding: theme.mspace_2
        spacing: theme.space_2
        show_bg: true
        draw_bg +: {
            color: theme.color_surface_container
            border_radius: theme.radius_m
        }
        image := mod.widgets.PlaceholderImage{height: 100}
        title := mod.widgets.ContentPlaceholder{width: 140 height: 14}
        body := mod.widgets.ContentPlaceholder{
            shape: PlaceholderShape.Lines
            lines: 2
            width: Fill
            height: Fit
            last_line_width: 0.8
        }
        button := mod.widgets.PlaceholderButton{}
    }

    // A `let` rather than a named child: a child declared inside the table
    // is an instance, and cannot be used as a template by its siblings.
    let PlaceholderTableRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        mod.widgets.ContentPlaceholder{width: 24 height: 10}
        mod.widgets.ContentPlaceholder{width: Fill height: 10}
        mod.widgets.ContentPlaceholder{width: 80 height: 10}
        mod.widgets.ContentPlaceholder{width: 48 height: 10}
    }

    /** Five rows of four cells. */
    mod.widgets.PlaceholderTable = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        PlaceholderTableRow{}
        PlaceholderTableRow{}
        PlaceholderTableRow{}
        PlaceholderTableRow{}
        PlaceholderTableRow{}
    }
}

/// The resting shape. Geometry as instances, set by the widget every draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlaceholder {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    shape: PlaceholderShape,
    #[live(3.0)]
    lines: f32,
    #[live(10.0)]
    line_height: f32,
    #[live(6.0)]
    line_gap: f32,
    #[live(0.6)]
    last_line_width: f32,
    #[live(4.0)]
    radius: f32,
    #[live(1.0)]
    opacity: f32,
}

/// The highlight over the shape: the same geometry plus which animation.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlaceholderShimmer {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    shape: PlaceholderShape,
    #[live(3.0)]
    lines: f32,
    #[live(10.0)]
    line_height: f32,
    #[live(6.0)]
    line_gap: f32,
    #[live(0.6)]
    last_line_width: f32,
    #[live(4.0)]
    radius: f32,
    #[live(1.0)]
    opacity: f32,
    #[live]
    animation: PlaceholderAnimation,
}

/// The height a stack of `lines` rows takes.
pub fn lines_height(lines: usize, line_height: f64, line_gap: f64) -> f64 {
    let lines = lines.max(1) as f64;
    lines * line_height + (lines - 1.0) * line_gap
}

#[derive(Script, ScriptHook, Widget)]
pub struct ContentPlaceholder {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The resting shape; always drawn, so it carries the area.
    #[redraw]
    #[live]
    draw_bg: DrawPlaceholder,
    /// The highlight; drawn inside the shape only while animating.
    #[live]
    draw_shimmer: DrawPlaceholderShimmer,
    #[live]
    pub shape: PlaceholderShape,
    #[live]
    pub animation: PlaceholderAnimation,
    /// No movement whatever `animation` says.
    #[live]
    pub reduced_motion: bool,
    #[live]
    pub translucent: bool,
    #[live(0.55)]
    pub translucent_opacity: f32,
    /// Rows of the Lines shape.
    #[live(3usize)]
    pub lines: usize,
    #[live(10.0)]
    pub line_height: f64,
    #[live(6.0)]
    pub line_gap: f64,
    /// Width of the last row as a fraction of the whole.
    #[live(0.6)]
    pub last_line_width: f64,
    /// Corner radius of rects and rows.
    #[live(4.0)]
    pub radius: f64,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,
    #[rust]
    disabled: bool,
}

impl ContentPlaceholder {
    /// Whether the highlight is drawn at all.
    pub fn animating(&self) -> bool {
        !self.reduced_motion && self.animation != PlaceholderAnimation::Static
    }

    /// A `Fit` walk becomes the size the shape asks for: a stack of rows,
    /// a circle as wide as it is tall, a rect one row high.
    fn sized(&self, mut walk: Walk) -> Walk {
        let fit_w = matches!(walk.width, Size::Fit { .. });
        let fit_h = matches!(walk.height, Size::Fit { .. });
        match self.shape {
            PlaceholderShape::Lines => {
                if fit_h {
                    walk.height = Size::Fixed(lines_height(self.lines, self.line_height, self.line_gap));
                }
                if fit_w {
                    walk.width = Size::fill();
                }
            }
            PlaceholderShape::Circle => {
                let d = match (walk.width, walk.height) {
                    (Size::Fixed(w), _) if fit_h => w,
                    (_, Size::Fixed(h)) if fit_w => h,
                    _ => 40.0,
                };
                if fit_w {
                    walk.width = Size::Fixed(d);
                }
                if fit_h {
                    walk.height = Size::Fixed(d);
                }
            }
            PlaceholderShape::Rect => {
                if fit_h {
                    walk.height = Size::Fixed(self.line_height);
                }
                if fit_w {
                    walk.width = Size::fill();
                }
            }
        }
        walk
    }

    pub fn set_animation(&mut self, cx: &mut Cx, animation: PlaceholderAnimation) {
        if self.animation != animation {
            self.animation = animation;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_lines(&mut self, cx: &mut Cx, lines: usize) {
        if self.lines != lines {
            self.lines = lines;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for ContentPlaceholder {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let mut opacity = if self.translucent { self.translucent_opacity } else { 1.0 };
        if self.disabled {
            opacity *= self.disabled_opacity;
        }
        let lines = self.lines.max(1) as f32;
        self.draw_bg.shape = self.shape;
        self.draw_bg.lines = lines;
        self.draw_bg.line_height = self.line_height as f32;
        self.draw_bg.line_gap = self.line_gap as f32;
        self.draw_bg.last_line_width = self.last_line_width as f32;
        self.draw_bg.radius = self.radius as f32;
        self.draw_bg.opacity = opacity;
        let walk = self.sized(walk);
        self.draw_bg.begin(cx, walk, Layout::default());
        if self.animating() {
            self.draw_shimmer.shape = self.shape;
            self.draw_shimmer.lines = lines;
            self.draw_shimmer.line_height = self.line_height as f32;
            self.draw_shimmer.line_gap = self.line_gap as f32;
            self.draw_shimmer.last_line_width = self.last_line_width as f32;
            self.draw_shimmer.radius = self.radius as f32;
            self.draw_shimmer.opacity = opacity;
            self.draw_shimmer.animation = self.animation;
            self.draw_shimmer.draw_walk(cx, Walk::fill());
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    /// A placeholder has no text: it stands in for text.
    fn text(&self) -> String {
        String::new()
    }

    fn set_text(&mut self, _cx: &mut Cx, _v: &str) {}

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    /// The shape, so a test can tell a circle from a stack of lines.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(match self.shape {
            PlaceholderShape::Lines => format!("lines {}", self.lines.max(1)),
            shape => shape.name().to_string(),
        })
    }
}

impl ContentPlaceholderRef {
    pub fn set_animation(&self, cx: &mut Cx, animation: PlaceholderAnimation) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_animation(cx, animation);
        }
    }

    pub fn set_lines(&self, cx: &mut Cx, lines: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lines(cx, lines);
        }
    }

    pub fn animating(&self) -> bool {
        self.borrow().map(|inner| inner.animating()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_wired() {
        let lib = include_str!("lib.rs");
        let placeholder = include_str!("placeholder.rs");
        assert!(lib.contains("pub mod placeholder;"));
        assert!(lib.contains("placeholder::*"));
        assert!(lib.contains("crate::placeholder::script_mod(vm);"));
        assert!(placeholder.contains(
            "mod.widgets.ContentPlaceholderBase = #(ContentPlaceholder::register_widget(vm))"
        ));
        assert!(placeholder.contains("mod.widgets.ContentPlaceholder = set_type_default()"));
        for preset in [
            "PlaceholderText",
            "PlaceholderCircle",
            "PlaceholderImage",
            "PlaceholderButton",
            "PlaceholderInput",
            "PlaceholderParagraph",
            "PlaceholderRow",
            "PlaceholderCard",
            "PlaceholderTable",
        ] {
            assert!(placeholder.contains(&format!("mod.widgets.{preset} = ")), "{preset}");
        }
        // The presets compose View and RoundedView, which register first.
        let view_ui = lib.find("crate::view_ui::script_mod(vm);").unwrap();
        assert!(view_ui < lib.find("crate::placeholder::script_mod(vm);").unwrap());
    }

    #[test]
    fn a_stack_of_lines_is_rows_plus_gaps() {
        assert_eq!(lines_height(1, 10.0, 6.0), 10.0);
        assert_eq!(lines_height(3, 10.0, 6.0), 42.0);
        // Zero rows still stands one row high rather than vanishing.
        assert_eq!(lines_height(0, 10.0, 6.0), 10.0);
    }
}
