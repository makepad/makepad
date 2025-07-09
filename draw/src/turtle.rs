use {
    crate::{
        makepad_platform::*,
        cx_2d::{Cx2d},
    }
};

#[derive(Clone, Debug)]
struct DeferredFill {
    weight: f64,
    max: Option<f64>,
    min: Option<f64>,
}

#[derive(Debug)]
pub enum AlignEntry{
    Unset,
    Area(Area),
    ShiftTurtle{area:Area, shift:DVec2, skip:usize},
    SkipTurtle{skip:usize},
    BeginTurtle(DVec2,DVec2),
    EndTurtle
}

/// Specifies how a turtle should walk.
#[derive(Copy, Clone, Default, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Walk {
    #[doc(hidden)]
    #[live]
    pub abs_pos: Option<DVec2>,

    /// The margin around this walk's rectangle.
    #[live]
    pub margin: Margin,

    /// The desired width of this walk's rectangle.
    #[live]
    pub width: Size,

    /// The desired height of this walk's rectangle.
    #[live]
    pub height: Size,
}

impl Walk {
    /// Returns a `Walk` with `width` and `height` set to the given value, and no margin.
    pub fn new(width: Size, height: Size) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width,
            height,
        }
    }

    /// Returns a `Walk` with both `width` and `height` set to 0.0, and no margin.
    pub fn empty() -> Self {
        Self::fixed(0.0, 0.0)
    }

    /// Returns a `Walk` with both `width` and `height` set to `Size::Fill`, and no margin.
    pub fn fill() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::fill(),
            height: Size::fill(),
        }
    }

    /// Returns a `Walk` with `width` and `height` set to the given fixed values, and no margin.
    pub fn fixed(width: f64, height: f64) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fixed(width),
            height: Size::Fixed(height),
        }
    }

    /// Returns a `Walk` with both `width` and `height` set to `Size::Fit`, and no margin.
    pub fn fit() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fit,
            height: Size::Fit,
        }
    }

    /// Returns a `Walk` with `width` set to `Size::Fill`, `height` set to `Size::Fit`, and no
    /// margin.
    pub fn fill_fit() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::fill(),
            height: Size::Fit,
        }
    }

    /// Returns a copy of this `Walk` with `margin` set to the given value.
    pub fn with_margin(self, margin: Margin) -> Self {
        Self {
            margin,
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the left margin set to the given value.
    pub fn with_margin_left(self, left: f64) -> Self {
        Self {
            margin: self.margin.with_left(left),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the right margin set to the given value.
    pub fn with_margin_top(self, top: f64) -> Self {
        Self {
            margin: self.margin.with_top(top),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the bottom margin set to the given value.
    pub fn with_margin_right(self, right: f64) -> Self {
        Self {
            margin: self.margin.with_right(right),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the bottom margin set to the given value.
    pub fn with_margin_bottom(self, v: f64) -> Self {
        Self {
            margin: self.margin.with_bottom(v),
            ..self
        }
    }
}

/// Specifies the desired width/height of a walk's rectangle.
/// 
/// See `Turtle::next_walk_width` and `Turtle::next_walk_height` for details on how the actual
/// width/height is computed based on the desired width/height.
#[derive(Copy, Clone, Debug, Live)]
#[live_ignore]
pub enum Size {
    #[pick {
        weight: 100.0,
        min: None,
        max: None,
    }]
    Fill {
        weight: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    #[live(200.0)]
    Fixed(f64),
    Fit,
}

impl Size {
    /// Returns a `Size::Fill` with a default `weight` of `100.0``, and no `min` or `max` constraints.
    pub fn fill() -> Self {
        Self::Fill {
            weight: 100.0,
            min: None,
            max: None,
        }
    }

    /// Returns `true` if this is a `Size::Fill`, or `false` otherwise.
    pub fn is_fill(self) -> bool {
        match self {
            Self::Fill { .. } => true,
            _ => false
        }
    }

    /// Returns `true` if this is a `Size::Fixed`, or `false` otherwise.
    pub fn is_fixed(self) -> bool {
        match self {
            Self::Fixed(_) => true,
            _ => false
        }
    }

    /// Returns `true` if this is a `Size::Fit`, or `false` otherwise.
    pub fn is_fit(self) -> bool {
        match self {
            Self::Fit => true,
            _ => false
        }
    }

    /// Returns the fixed size if this is a `Size::Fixed`, or `None` otherwise.
    pub fn to_fixed(self) -> Option<f64> {
        match self {
            Self::Fixed(size) => Some(size),
            _ => None,
        }
    }
}

impl Default for Size {
    fn default() -> Self {
        Size::fill()
    }
}

/// Specifies how walks should be laid out with respect to each other.
#[derive(Copy, Clone, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Layout {
    #[live] pub scroll: DVec2,
    #[live(true)] pub clip_x: bool,
    #[live(true)] pub clip_y: bool,

    /// The direction in which each walk is laid out.
    #[live] pub flow: Flow,

    /// The spacing between each walk.
    #[live]
    pub spacing: f64,

    /// The padding around the inner rectangle of each walk.
    #[live]
    pub padding: Padding,

    /// The alignment of each walk with respect to their turtle's rectangle.
    #[live]
    pub align: Align,
}

impl Layout {
    /// Creates a `Layout` in which walks are laid out from left to right, and all other fields
    /// are set to their default values.
    pub fn flow_right() -> Self {
        Self {
            flow: Flow::Right,
            ..Self::default()
        }
    }
    
    /// Creates a `Layout` in which walks are laid out from left to right, wrapping to the next row
    /// if we run out of space, and all other fields are set to their default values.
    pub fn flow_right_wrap() -> Self {
        Self {
            flow: Flow::RightWrap,
            ..Self::default()
        }
    }

    /// Creates a `Layout` in which walks are laid out from top to bottom, and all other fields
    /// are set to their default values.
    pub fn flow_down() -> Self {
        Self {
            flow: Flow::Down,
            ..Self::default()
        }
    }
    
    /// Creates a `Layout` in which walks are laid out on top of each other, and all other fields
    /// are set to their default values.
    pub fn flow_overlay() -> Self {
        Self {
            flow: Flow::Overlay,
            ..Self::default()
        }
    }

    /// Creates a copy of this `Layout` with `padding` set to the given value.
    pub fn with_padding(self, padding: Padding) -> Self {
        Self {
            padding,
            ..self
        }
    }
    
    /// Creates a copy of this `Layout` with the top padding set to the given value.
    pub fn with_padding_top(self, top: f64) -> Self {
        Self {
            padding: self.padding.with_top(top),
            ..self
        }
    }
    
    /// Creates a copy of this `Layout` with the right padding set to the given value.
    pub fn with_padding_right(self, right: f64) -> Self {
        Self {
            padding: self.padding.with_right(right),
            ..self
        }
    }
    
    /// Creates a copy of this `Layout` with the bottom padding set to the given value.
    pub fn with_padding_bottom(self, bottom: f64) -> Self {
        Self {
            padding: self.padding.with_bottom(bottom),
            ..self
        }
    }
    
    /// Creates a copy of this `Layout` with the left padding set to the given value.
    pub fn with_padding_left(self, left: f64) -> Self {
        Self {
            padding: self.padding.with_left(left),
            ..self
        }
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            scroll: dvec2(0.0,0.0),
            clip_x: true,
            clip_y: true,
            padding: Padding::default(),
            align: Align::default(),
            flow: Flow::default(),
            spacing: 0.0,
        }
    }
}

/// Specifies the alignment of each walk with respect to their turtle's rectangle.
#[derive(Clone, Copy, Default, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Align {
    /// The fraction of the turtle's unused inner width that will be added to the left of each walks:
    /// - Setting this to 0.0 will align each walk to the left.
    /// - Setting this to 0.5 will center each walk horizontally.
    /// - Setting this to 1.0 will align each walk to the right.
    #[live]
    pub x: f64,

    /// The fraction of the turtle's unused inner height that will be added above each walks:
    /// - Setting this to 0.0 will align each walk to the top.
    /// - Setting this to 0.5 will center each walk vertically.
    /// - Setting this to 1.0 will align each walk to the bottom.
    #[live]
    pub y: f64
}

/// Specifies the direction in which walks are laid out.
#[derive(Copy, Clone, Debug, Live, LiveHook, PartialEq)]
#[live_ignore]
pub enum Flow {
    // Walks are laid out from left to right.
    #[pick]
    Right,

    // Walks are laid out from left to right, wrapping to the next row if we run out of space.
    RightWrap,
    
    // Walks are laid out from top to bottom.
    Down,
    
    // Walks are laid out on top of each other.
    Overlay, 
}

impl Default for Flow {
    fn default() -> Self {
        Flow::Right
    }
}

/// Specifies the padding around a walk's inner rectangle.
#[derive(Clone, Copy, Default, Debug, Live, LiveRegister)]
#[live_ignore]
pub struct Padding {
    /// The left padding.
    #[live]
    pub left: f64,

    /// The top padding.
    #[live]
    pub top: f64,

    /// The right padding.
    #[live]
    pub right: f64,

    /// The bottom padding.
    #[live]
    pub bottom: f64
}

impl Padding {
    /// Returns a copy of this `Padding` with the left padding set to the given value.
    pub fn with_left(self, left: f64) -> Self {
        Self {
            left,
            ..self
        }
    }

    /// Returns a copy of this `Padding` with the top padding set to the given value.
    pub fn with_top(self, top: f64) -> Self {
        Self {
            top,
            ..self
        }
    }

    /// Returns a copy of this `Padding` with the right padding set to the given value.
    pub fn with_right(self, right: f64) -> Self {
        Self {
            right,
            ..self
        }
    }

    /// Returns a copy of this `Padding` with the bottom padding set to the given value.
    pub fn with_bottom(self, bottom: f64) -> Self {
        Self {
            bottom,
            ..self
        }
    }

    /// Returns a vector containing the left and top padding.
    pub fn left_top(self) -> DVec2 {
        dvec2(self.left, self.top)
    }

    /// Returns a vector containing the right and bottom padding.
    pub fn right_bottom(self) -> DVec2 {
        dvec2(self.right, self.bottom)
    }

    /// Returns a vector containing both the padding width and height.
    pub fn size(self) -> DVec2 {
        dvec2(self.width(), self.height())
    }

    /// Returns the horizontal padding.
    /// 
    /// This is the sum of the left and right padding.
    pub fn width(self) -> f64 {
        self.left + self.right
    }

    /// Returns the vertical padding.
    /// 
    /// This is the sum of the top and bottom padding.
    pub fn height(self) -> f64 {
        self.top + self.bottom
    }
}

/// The turtle is the main layout primitive in Makepad.
/// 
/// A turtle can be walked to allocate space on the screen. Each walk produces a rectangle that
/// represents the area allocated by the walk.
/// 
/// Turtles can be nested. When a nested turtle is created, the parent turtle starts a new walk. The
/// nested turtle then walks inside the rectangle of the parent turtle's walk. When the nested turtle
/// is finished, the parent turtle finishes its walk.
/// 
/// +-----------------+
/// |     Margin      |
/// | +-------------+ |
/// | |   Padding   | |
/// | | +---------+ | |
/// | | | Content | | |
/// | | +---------+ | |
/// | +-------------+ |
/// +-----------------+
/// 
/// Inner rectangle = content
/// Rectangle       = content + padding
/// Outer rectangle = content + padding + margin
#[derive(Clone, Default, Debug)]
pub struct Turtle {
    walk: Walk,
    layout: Layout,
    width: f64,
    height: f64,
    used_width: f64,
    used_height: f64,
    wrap_spacing: f64,
    align_start: usize,
    finished_walks_start: usize,
    deferred_fills: Vec<DeferredFill>,
    deferred_weight_prefix_sum: Vec<f64>,
    resolved_fills: Vec<f64>,
    resolved_length_suffix_sum: Vec<f64>,
    pos: DVec2,
    origin: DVec2,
    guard: Area
}

impl Turtle {
    /// Returns a reference to the `Walk` with which this turtle was created.
    pub fn walk(&self) -> &Walk {
        &self.walk
    }

    /// Returns a reference to the `Layout`` with which this turtle was created.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Return a reference to the margin around this turtle's rectangle.
    pub fn margin(&self) -> &Margin {
        &self.walk.margin
    }

    /// Returns the direction in which each walk of this turtle is laid out.
    pub fn flow(&self) -> Flow {
        self.layout.flow
    }

    /// Returns the spacing between each walk of this turtle.
    pub fn spacing(&self) -> f64 {
        self.layout.spacing
    }

    /// Returns a reference to the padding around the inner rectangle of each walk of this turtle.
    pub fn padding(&self) -> &Padding {
        &self.layout.padding
    }

    /// Returns the alignment of each walk of this turtle with respect to it's rectangle.
    pub fn align(&self) -> Align {
        self.layout.align
    }

    /// Returns this turtle's inner rectangle.
    pub fn inner_rect(&self) -> Rect {
        Rect {
            pos: self.inner_origin(),
            size: self.inner_size(),
        }
    }

    /// Returns this turtle's inner rectangle, without scrolling applied.
    pub fn unscrolled_inner_rect(&self) -> Rect {
        Rect {
            pos: self.unscrolled_inner_origin(),
            size: self.inner_size(),
        }
    }

    /// Returns the origin of this turtle's inner rectangle.
    pub fn inner_origin(&self) -> DVec2 {
        self.origin + self.padding().left_top()
    }

    /// Returns the origin of this turtle's inner rectangle, without scrolling applied.
    pub fn unscrolled_inner_origin(&self) -> DVec2 {
        self.origin + self.scroll()
    }

    /// Returns the size of this turtle's inner rectangle.
    pub fn inner_size(&self) -> DVec2 {
        dvec2(self.inner_width(), self.inner_height())
    }

    /// Returns the width of this turtle's inner rectangle.
    /// 
    /// If the inner width is unknown, then NaN is returned.
    pub fn inner_width(&self) -> f64 {
        self.width() - self.padding().width().min(self.width())
    }

    /// Returns the height of this turtle's inner rectangle.
    /// 
    /// If the inner height is unknown, then NaN is returned.
    pub fn inner_height(&self) -> f64 {
        self.height() - self.padding().height().min(self.height())
    }

    /// Returns the used width of this turtle's inner rectangle.
    pub fn inner_used_width(&self) -> f64 {
        self.used_width() - self.padding().left.min(self.used_width())
    }

    /// Returns the used width of this turtle's inner rectangle on the current row.
    pub fn inner_used_width_current_row(&self) -> f64 {
        self.used_width_current_row() - self.padding().left.min(self.used_width_current_row())
    }

    /// Returns the used height of this turtle's inner rectangle.
    pub fn inner_used_height(&self) -> f64 {
        self.used_height() - self.padding().top.min(self.used_height())
    }

    /// Returns the unused width of this turtle's inner rectangle.
    /// 
    /// If the unused inner width is unknown, then NaN is returned.
    pub fn unused_inner_width(&self) -> f64 {
        self.inner_width() - self.inner_used_width().min(self.inner_width())
    }

    /// Returns the unused width of this turtle's inner rectangle for the current row.
    /// 
    /// If the unused inner width on the current row is unknown, then NaN is returned.
    pub fn unused_inner_width_for_current_row(&self) -> f64 {
        self.inner_width() - self.inner_used_width_current_row().min(self.inner_width())
    }

    /// Returns the unused height of this turtle's inner rectangle.
    /// 
    /// If the unused inner height is unknown, then NaN is returned.
    pub fn unused_inner_height(&self) -> f64 {
        self.inner_height() - self.inner_used_height().min(self.inner_height())
    }

    /// Returns the effective width of this turtle's inner rectangle.
    /// 
    /// This is either the inner width, or the used inner width if the inner width is unknown.
    pub fn effective_inner_width(&self) -> f64 {
        if !self.inner_width().is_nan() {
            self.inner_width()
        } else {
            self.inner_used_width()
        }
    }

    /// Returns the effective height of this turtle's inner rectangle.
    /// 
    /// This is either the inner height, or the used inner height if the inner height is unknown.
    pub fn inner_effective_height(&self) -> f64 {
        if !self.inner_height().is_nan() {
            self.inner_height()
        } else {
            self.inner_used_height()
        }
    }

    /// Returns this turtle's rectangle.
    pub fn rect(&self) -> Rect {
        Rect {
            pos: self.origin(),
            size: self.size(),
        }
    }

    /// Returns this turtle's rectangle, without scrolling applied.
    pub fn rect_unscrolled(&self) -> Rect {
        Rect {
            pos: self.origin_unscrolled(),
            size: self.size(),
        }
    }

    /// Returns the origin of this turtle's rectangle.
    pub fn origin(&self) -> DVec2 {
        self.origin
    }

    /// Returns the origin of this turtle's rectangle, without scrolling applied.
    pub fn origin_unscrolled(&self) -> DVec2 {
        self.origin + self.layout.scroll
    }

    /// Returns the size of this turtle's rectangle.
    pub fn size(&self) -> DVec2 {
        dvec2(self.width(), self.height())
    }

    /// Returns the width of this turtle's rectangle.
    /// 
    /// If the width is unknown, then NaN is returned.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Returns the height of this turtle's rectangle.
    /// 
    /// If the height is unknown, then NaN is returned.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Returns the used width of this turtle's rectangle.
    pub fn used_width(&self) -> f64 {
        self.used_width
    }

    /// Returns the used width of this turtle's rectangle on the current row.
    pub fn used_width_current_row(&self) -> f64 {
        self.pos.x - self.origin.x
    }

    /// Returns the used height of this turtle's rectangle.
    pub fn used_height(&self) -> f64 {
        self.used_height
    }

    /// Returns the unused width of this turtle's rectangle.
    /// 
    /// If the unused width is unknown, then NaN is returned.
    pub fn unused_width(&self) -> f64 {
        self.width() - self.used_width().min(self.width())
    }

    /// Returns the unused width of this turtle's rectangle on the current row.
    /// 
    /// If the unused width on the current row is unknown, then NaN is returned.
    pub fn unused_width_current_row(&self) -> f64 {
        self.width() - self.used_width_current_row().min(self.width())
    }

    /// Returns the unused height of this turtle's rectangle.
    /// 
    /// If the unused height is unknown, then NaN is returned.
    pub fn unused_height(&self) -> f64 {
        self.height() - self.used_height().min(self.height())
    }

    /// Returns the effective width of this turtle's rectangle.
    /// 
    /// This is either the width, or the used width if the width is unknown.
    pub fn effective_width(&self) -> f64 {
        if !self.width().is_nan() {
            self.width()
        } else {
            self.used_width()
        }
    }

    /// Returns the effective height of this turtle's rectangle.
    /// 
    /// This is either the height, or the used height if the height is unknown.
    pub fn effective_height(&self) -> f64 {
        if !self.height().is_nan() {
            self.height()
        } else {
            self.used_height()
        }
    }

    /// Returns this turtle's outer rectangle.
    pub fn outer_rect(&self) -> Rect {
        Rect {
            pos: self.outer_origin(),
            size: self.outer_size(),
        }
    }

    /// Returns this turtle's outer rectangle, without scrolling applied.
    pub fn unscrolled_outer_rectangle(&self) -> Rect {
        Rect {
            pos: self.unscrolled_outer_origin(),
            size: self.outer_size(),
        }
    }

    /// Returns the origin of this turtle's outer rectangle.
    pub fn outer_origin(&self) -> DVec2 {
        self.origin() - self.margin().left_top()
    }

    /// Returns the origin of this turtle's outer rectangle, without scrolling applied.
    pub fn unscrolled_outer_origin(&self) -> DVec2 {
        self.origin_unscrolled() - self.margin().left_top()
    }

    /// Returns the size of this turtle's outer rectangle.
    pub fn outer_size(&self) -> DVec2 {
        dvec2(self.outer_width(), self.outer_height())
    }

    /// Returns the width of this turtle's outer rectangle.
    /// 
    /// If the outer width is unknown, then NaN is returned.
    pub fn outer_width(&self) -> f64 {
        self.width() + self.margin().width()
    }

    /// Returns the width of this turtle's outer rectangle.
    /// 
    /// If the outer height is unknown, then NaN is returned.
    pub fn outer_height(&self) -> f64 {
        self.height() + self.margin().height()
    }

    /// Returns the used width of this turtle's outer rectangle.
    /// 
    pub fn used_outer_width(&self) -> f64 {
        self.used_width() + self.margin().left
    }

    /// Returns the used width of this turtle's outer rectangle on the current row.
    pub fn used_outer_width_current_row(&self) -> f64 {
        self.used_width_current_row() + self.margin().left
    }

    /// Returns the used height of this turtle's outer rectangle.
    pub fn used_outer_height(&self) -> f64 {
        self.used_height() + self.margin().top
    }

    /// Returns the unused width of this turtle's outer rectangle.
    /// 
    /// If the unused outer width is unknown, then NaN is returned.
    pub fn unused_outer_width(&self) -> f64 {
        self.outer_width() - self.used_outer_width().min(self.outer_width())
    }

    /// Returns the unused width of this turtle's outer rectangle on the current row.
    /// 
    /// If the unused outer width on the current row is unknown, then NaN is returned.
    pub fn unused_outer_width_current_row(&self) -> f64 {
        self.outer_width() - self.used_outer_width_current_row().min(self.outer_width())
    }

    /// Returns the unused height of this turtle's outer rectangle.
    /// 
    /// If the unused outer height is unknown, then NaN is returned.
    pub fn unused_outer_height(&self) -> f64 {
        self.outer_height() - self.used_outer_height().min(self.outer_height())
    }

    /// Returns the effective width of this turtle's outer rectangle.
    ///
    /// This is either the outer width, or the used outer width if the outer width is unknown.
    pub fn effective_outer_width(&self) -> f64 {
        if !self.outer_width().is_nan() {
            self.outer_width()
        } else {
            self.used_outer_width()
        }
    }

    /// Returns the effective height of this turtle's outer rectangle.
    ///
    /// This is either the outer height, or the used outer height if the outer height is unknown.
    pub fn effective_outer_height(&self) -> f64 {
        if !self.outer_height().is_nan() {
            self.outer_height()
        } else {
            self.used_outer_height()
        }
    }

    /// Returns true if this turtle's next walk would be it's first.
    pub fn next_walk_is_first(&self, finished_walks_end: usize) -> bool {
        self.finished_walks_start == finished_walks_end && self.deferred_fills.len() == 0
    }

    /// Returns the offset to this turtle's next walk.
    /// 
    /// This is either zero if this turtle's next walk would be its first, or this turtle's
    /// spacing in the direction of it's flow.
    pub fn offset_to_next_walk(&self, finished_walks_end: usize) -> DVec2 {
        if self.next_walk_is_first(finished_walks_end) {
            dvec2(0.0, 0.0)
        } else {
            match self.layout.flow {
                Flow::Right | Flow::RightWrap => dvec2(self.spacing(), 0.0),
                Flow::Down => dvec2(0.0, self.spacing()),
                Flow::Overlay => dvec2(0.0, 0.0),
            }
        }
    }

    /// Returns the size of the rectangle of this turtle's next walk, based on the given desired
    /// `width`, `height`, and `margin`.
    pub fn size_of_next_walk(&self, width: Size, height: Size, margin: Margin) -> DVec2 {
        dvec2(
            self.width_of_next_walk(width, margin),
            self.height_of_next_walk(height, margin),
        )
    }

    /// Returns the width of the rectangle of this turtle's next walk, based on the given desired
    /// `width` and `margin`.
    ///
    /// - If the desired width is `Size::Fill`, then the actual width is computed as follows:
    /// 
    ///   First, we compute the actual outer width. This depends on the direction in which this
    ///   turtle's walks are laid out:
    ///   - If this is `Flow::Right`, then the actual outer width of this turtle's next walk is this
    ///     turtle's remaining unused inner width.
    ///   - If this is `Flow::RightWrap`, then the actual outer width of this turtle's next walk is
    ///     this turtle's remaining unused inner width on the current row.
    ///   - If this is either `Flow::Down` or `Flow::Overlay`, then the actual outer width of this
    ///     turtle's next walk is this turtle's effective inner width.
    ///   
    ///   Next, the actual outer width is clamped to the given `min` and `max`` constraints, if any.
    /// 
    ///   Finally, the actual width is computed from the actual outer width by subtracting the
    ///   margin width.
    /// 
    /// - If the desired width is `Size::Fixed`, then the actual width is simply the given width,
    ///   clamped to be at least 0.0.
    /// 
    /// - If the desired width is `Size::Fit`, then the actual width cannot be computed until this
    ///   turtle's final unused inner width is known, so we return NaN to indicate that the actual
    ///   width is not yet known.
    pub fn width_of_next_walk(&self, width: Size, margin: Margin) -> f64 {
        match width {
            Size::Fill { min, max, .. } => {
                let mut outer_width = match self.layout.flow {
                    Flow::Right => self.unused_inner_width(),
                    Flow::RightWrap => self.unused_inner_width_for_current_row(),
                    Flow::Down | Flow::Overlay => self.effective_inner_width(),
                };
                if let Some(min) = min {
                    outer_width = outer_width.max(min);
                }
                if let Some(max) = max {
                    outer_width = outer_width.min(max);
                }
                outer_width - margin.width()
            },
            Size::Fixed(width) => width.max(0.0),
            Size::Fit => f64::NAN,
        }
    }
    
    /// Returns the height of the rectangle of this turtle's next walk, based on the given desired
    /// `height` and `margin`.
    /// 
    /// - If the desired height is `Size::Fill`, then the actual height is computed as follows:
    ///   
    ///   First, we compute the actual outer height. This depends on the direction in which this
    ///   turtle's walks are laid out:
    ///   - If this is either `Flow::Right`, `Flow::RightWrap`, or `Flow::Overlay, then the actual
    ///     outer height of this turtle's next walk is this turtle's effective inner height.
    ///   - If this is `Flow::Down`, then the actual outer height of this turtle's next walk is
    ///     this turtle's remaining unused inner height.
    /// 
    ///   Next, the actual outer height is clamped to the given `min` and `max` constraints, if any.
    /// 
    ///   Finally, the actual height is computed from the actual outer height by subtracting the
    ///   margin height.
    /// 
    /// - If the desired height is `Size::Fixed`, then the actual height is simply the given height,
    ///   clamped to be at least 0.0.
    /// 
    /// - If the desired height is `Size::Fit`, then the actual height cannot be computed until this
    ///   turtle's final unused inner height is known, so we return NaN to indicate that the actual
    ///   height is not yet known.
    pub fn height_of_next_walk(&self, height: Size, margin: Margin) -> f64 {
        match height {
            Size::Fill { min, max, .. } => {
                let mut outer_height = match self.layout.flow {
                    Flow::Right | Flow::RightWrap | Flow::Overlay => self.inner_effective_height(),
                    Flow::Down => self.unused_inner_height()
                };
                if let Some(min) = min {
                    outer_height = outer_height.max(min);
                }
                if let Some(max) = max {
                    outer_height = outer_height.min(max);
                }
                outer_height - margin.height()
            }
            Size::Fixed(height) => height.max(0.0),
            Size::Fit => f64::NAN,
        }
    }

    /// Moves this turtle to the given position.
    pub fn move_to(&mut self, pos: DVec2) {
        self.pos = pos
    }

    /// Moves this turtle right and down by the given amount.
    pub fn move_right_down(&mut self, amount: DVec2) {
        self.move_to(self.pos() + amount);
    }

    /// Moves this turtle right by the given amount.
    pub fn move_right(&mut self, amount: f64) {
        self.move_right_down(dvec2(amount, 0.0))
    }

    /// Moves this turtle down by the given amount.
    pub fn move_down(&mut self, amount: f64) {
        self.move_right_down(dvec2(0.0, amount))
    }

    /// Allocates additional size to the right of and below this turtle's position.
    pub fn allocate_size(&mut self, additional: DVec2) {
        self.allocate_width(additional.x);
        self.allocate_height(additional.y);
    }

    /// Allocates additional width to the right of this turtle's position.
    /// 
    /// This will increase this turtle's used width if necessary.
    pub fn allocate_width(&mut self, additional: f64) {
        self.used_width = self.used_width.max(self.pos().x + additional - self.origin().x);
    }

    /// Allocates additional height below this turtle's position.
    /// 
    /// This will increase this turtle's used height if necessary.
    pub fn allocate_height(&mut self, additional: f64) {
        self.used_height = self.used_height.max(self.pos().y + additional - self.origin().y);
    }

    fn deferred_fill_count(&self) -> usize {
        self.deferred_fills.len()
    }

    fn resolved_fill_count(&self) -> usize {
        self.resolved_fills.len()
    }

    fn total_deferred_weight_from(&self, index: usize) -> f64 {
        if index == self.deferred_fill_count() {
            0.0
        } else {
            self.deferred_weight_prefix_sum[index]
        }
    }

    fn total_resolved_length_to(&self, index: usize) -> f64 {
        if index == 0 {
            0.0
        } else {
            self.resolved_length_suffix_sum[index - 1]
        }
    }

    fn inner_unused_length(&self) -> f64 {
        match self.layout.flow {
            Flow::Right => self.unused_inner_width(),
            Flow::Down => self.unused_inner_height(),
            _ => panic!(),
        }
    }

    fn unresolved_length_from(&self, index: usize) -> f64 {
        self.inner_unused_length() - self.total_resolved_length_to(index)
    }

    fn resolve_fill(&mut self, index: usize) -> f64 {
        let mut count = self.resolved_fill_count();
        while count <= index { 
            let unresolved_length = self.unresolved_length_from(count);
            let deferred_fill = &self.deferred_fills[count];
            let total_deferred_weight = self.total_deferred_weight_from(count);
            let mut length = unresolved_length * deferred_fill.weight / total_deferred_weight;
            if let Some(min) = deferred_fill.min {
                length = length.max(min);
            }
            if let Some(max) = deferred_fill.max {
                length = length.min(max);
            }
            self.push_resolved_fill(length);
            count += 1;
        }
        self.resolved_fills[index]
    }

    fn push_deferred_fill(&mut self, weight: f64, min: Option<f64>, max: Option<f64>) {
        self.deferred_fills.push(DeferredFill {
            weight,
            min,
            max,
        });
        let total_deferred_weight = self.deferred_weight_prefix_sum.first().copied().unwrap_or(0.0);
        self.deferred_weight_prefix_sum.insert(0, weight + total_deferred_weight);
    }

    fn push_resolved_fill(&mut self, length: f64) {
        self.resolved_fills.push(length);
        let total_resolved_length = self.resolved_length_suffix_sum.last().copied().unwrap_or(0.0);
        self.resolved_length_suffix_sum.push(total_resolved_length + length);
    }
}

/// Represents a deferred walk.
/// 
/// A deferred walk is a walk for which the width/height is not yet known. It must be resolved when
/// its turtle has finished walking.
#[derive(Clone, Debug)]
pub enum DeferredWalk {
    /// An unresolved deferred walk.
    Unresolved {
        index: usize,
        pos: DVec2,
        margin: Margin,
        other_axis: Size,
    },
    /// A resolved deferred walk.
    Resolved(Walk)
}

impl DeferredWalk {
    pub fn resolve(&mut self, cx: &mut Cx2d) -> Walk {
        match *self {
            Self::Unresolved{index, pos, margin, other_axis}=>{
                let turtle = cx.turtles.last_mut().unwrap();

                let walk = match turtle.flow() {
                    Flow::Right => Walk {
                        abs_pos: Some(pos + dvec2(turtle.total_resolved_length_to(index), 0.0)),
                        margin,
                        width: Size::Fixed(turtle.resolve_fill(index)),
                        height: other_axis
                    },
                    Flow::Down => Walk {
                        abs_pos: Some(pos + dvec2(0.0, turtle.total_resolved_length_to(index))),
                        margin: margin,
                        height: Size::Fixed(turtle.resolve_fill(index)),
                        width: other_axis
                    },
                    _ => panic!()
                };
                *self = DeferredWalk::Resolved(walk);
                walk
            }
            Self::Resolved(walk) => walk,
        }
    }
}

/// Represents a finished walk.
#[derive(Clone, Default, Debug)]
pub struct FinishedWalk {
    /// The start of the align list of this finished walk.
    /// 
    /// The end of the align list of this finished walk is implicit: it is either the start of the
    /// align tree of the next finished walk, or the end of the global align list if this is the
    /// last finished walk.
    align_list_start: usize,

    /// The number of deferred walks before this finished walk.
    deferred_before_count: usize,

    /// The size of the outer rectangle of this finished walk.
    outer_size: DVec2,
}

impl<'a,'b> Cx2d<'a,'b> {
    /// Returns a reference to the current turtle.
    pub fn turtle(&self) -> &Turtle {
        self.turtles.last().unwrap()
    }
    
    /// Returns a mutable reference to the current turtle.
    pub fn turtle_mut(&mut self) -> &mut Turtle {
        self.turtles.last_mut().unwrap()
    }

    /// Starts a root turtle.
    pub fn begin_root_turtle(&mut self, size: DVec2, layout: Layout) {
        self.align_list.push(AlignEntry::BeginTurtle(dvec2(0.0,0.0), size));

        let turtle = Turtle {
            walk: Walk::fixed(size.x, size.y),
            layout,
            align_start: self.align_list.len() - 1,
            finished_walks_start: self.finished_walks.len(),
            deferred_fills: Vec::new(),
            resolved_fills: Vec::new(),
            deferred_weight_prefix_sum: Vec::new(),
            resolved_length_suffix_sum: Vec::new(),
            pos: DVec2 {
                x: layout.padding.left,
                y: layout.padding.top
            },
            wrap_spacing: 0.0,
            origin: dvec2(0.0, 0.0),
            width: size.x,
            height: size.y,
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            guard: Area::Empty,
        };

        self.turtles.push(turtle);
    }

    /// Starts a root turtle with clipping disabled.
    pub fn begin_unclipped_root_turtle(&mut self, size:DVec2,layout: Layout) {
        self.begin_root_turtle(size, layout);
        *self.align_list.last_mut().unwrap() = AlignEntry::Unset;
    }

    /// Starts a root turtle for the current pass.
    pub fn begin_root_turtle_for_pass(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_root_turtle(size, layout)
    }

    /// Starts a root turtle with clipping disabled for the current pass.
    pub fn begin_unclipped_root_turtle_for_pass(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_unclipped_root_turtle(size, layout)
    }

    /// Starts a nested turtle.
    /// 
    /// When a nested turtle is started, the parent turtle starts a new walk with the given `walk`.
    /// The nested turtle then walks inside the rectangle of the parent turtle's walk. When the
    /// nested turtle is finished, the parent turtle finishes its walk.
    /// 
    /// The given `layout` determines how the nested turtle's walks are laid out with respect to
    /// each other.
    /// 
    /// The nested turtle's rectangle is that of the parent turtle's walk. Since the width/height
    /// of this walk may be `Size::Fit`, the width/height of this rectangle may not be known until
    /// the nested turtle is finished.
    pub fn begin_turtle(&mut self, walk: Walk, layout: Layout) {
        self.begin_turtle_with_guard(walk, layout, Area::Empty)
    }

    /// Starts a nested turtle, with a guard area.
    /// 
    /// When the nested turtle is later finished, it should be finished with the same guard area
    /// that was used to start it.
    /// 
    /// See [`begin_turtle`] for more information.
    pub fn begin_turtle_with_guard(&mut self, walk: Walk, layout: Layout, guard: Area) {
        let parent = self.turtle();

        let outer_origin = if let Some(outer_origin) = walk.abs_pos {
            outer_origin
        } else {
            parent.pos() + parent.offset_to_next_walk(self.finished_walks.len())
        };
        let origin = outer_origin + walk.margin.left_top();

        let size = parent.size_of_next_walk(walk.width, walk.height, walk.margin);

        let clip_min = dvec2(
            if layout.clip_x {
                origin.x
            } else {
                f64::NAN
            },
            if layout.clip_y {
                origin.y
            } else {
                f64::NAN
            }
        );

        let clip_max = dvec2(
            if layout.clip_x {
                origin.x + size.x
            } else {
                f64::NAN
            },
            if layout.clip_y {
                origin.y + size.y
            } else {
                f64::NAN
            }
        );

        let origin = origin - layout.scroll;
        
        self.align_list.push(AlignEntry::BeginTurtle(clip_min, clip_max));
        
        let turtle = Turtle {
            walk,
            layout,
            align_start: self.align_list.len()-1,
            finished_walks_start: self.finished_walks.len(),
            deferred_fills: Vec::new(),
            deferred_weight_prefix_sum: Vec::new(),
            resolved_fills: Vec::new(),
            resolved_length_suffix_sum: Vec::new(),
            wrap_spacing: 0.0,
            pos: DVec2 {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top
            },
            origin,
            width: size.x,
            height: size.y,
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            guard,
        };
        
        self.turtles.push(turtle);
    }

    /// Finishes the current turtle.
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }
    
    /// Finishes the current turtle, with a guard area.
    /// 
    /// The current turtle should be finished with the same guard area that was used to start it.
    pub fn end_turtle_with_guard(&mut self, guard: Area) -> Rect {
        let mut turtle = self.turtles.last().unwrap();
        if guard != turtle.guard {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard, guard)
        }
        
        let turtle_align_start = turtle.align_start;
        let turtle_abs_pos = turtle.walk.abs_pos;
        let turtle_margin = turtle.walk.margin;
        let turtle_walks_start = turtle.finished_walks_start;
                
        // If the current turtle's width is not yet known, we can now compute it based on the used width.
        let width = if !turtle.width.is_nan() {
            turtle.width
        } else {
            let width = turtle.used_width() + turtle.padding().right;

            if let AlignEntry::BeginTurtle(clip_min,clip_max) = &mut self.align_list[turtle.align_start] {
                clip_max.x = clip_min.x + width;
            }

            width
        };
        
        // If the current turtle's height is not yet known, we can now compute it based on the used height.
        let height = if !turtle.height.is_nan() {
            turtle.height
        } else {
            let height = turtle.used_height() + turtle.padding().bottom;

            if let AlignEntry::BeginTurtle(clip_min, clip_max) = &mut self.align_list[turtle.align_start] {
                clip_max.y = clip_min.y + height;
            }

            height
        };

        // Now that the current turtle's rectangle is known, we can align its finished walks.
        match turtle.flow() {
            Flow::Right => {
                if turtle.deferred_fills.len() == 0 {
                    // If walks are laid out from left to right, and there are no deferred walks,
                    // then the horizontal alignment is applied to all walks as a whole, while
                    // the vertical alignment is applied to each walk individually.
                    if turtle.align().x != 0.0 || turtle.align().y != 0.0 {
                        let inner_unused_width = turtle.unused_inner_width();
                        let inner_effective_height = turtle.inner_effective_height();

                        for finished_walk_index in turtle.finished_walks_start..self.finished_walks.len() {
                            let finished_walk = &self.finished_walks[finished_walk_index];
                            
                            let inner_unused_height = inner_effective_height - finished_walk.outer_size.y;

                            let dx = turtle.align().x * inner_unused_width;
                            let dy = turtle.align().y * inner_unused_height;

                            let align_start = finished_walk.align_list_start;
                            let align_end = self.finished_walk_align_list_end(finished_walk_index);
                            self.move_align_list(dx, dy, align_start, align_end, false);
                            
                            turtle = self.turtles.last_mut().unwrap();
                        }
                    }
                } else {
                    // If walks are laid out from left to right, and there are deferred walks, then
                    // the unused inner width is distributed over the deferred walks, while the
                    // vertical alignment is applied to each walk individually.
                    let inner_effective_height = turtle.inner_effective_height();

                    for finished_walk_index in turtle_walks_start..self.finished_walks.len() {
                        let finished_walk = &self.finished_walks[finished_walk_index];

                        let inner_unused_height = inner_effective_height - finished_walk.outer_size.y;

                        let dx = turtle.total_resolved_length_to(finished_walk.deferred_before_count);
                        let dy = turtle.align().y * inner_unused_height;

                        let align_start = finished_walk.align_list_start;
                        let align_end = self.finished_walk_align_list_end(finished_walk_index);
                        self.move_align_list(dx, dy, align_start, align_end, false);

                        turtle = self.turtles.last_mut().unwrap();
                    }
                }
            },
            Flow::RightWrap => {
                if turtle.deferred_fills.is_empty() {
                    // TODO   
                } else {
                    panic!()
                }
            }
            Flow::Down => {
                // If walks are laid out from top to bottom, and there are no deferred walks, then
                // the horizontal alignment is applied each walk individually, while the vertical
                // alignment is applied to all walks as a whole.
                if turtle.deferred_fills.is_empty() {
                    if turtle.align().x != 0.0 || turtle.align().y != 0.0 {
                        let inner_effective_width = turtle.effective_inner_width();
                        let inner_unused_height = turtle.unused_inner_height();
                        
                        for finished_walk_index in turtle_walks_start..self.finished_walks.len() {
                            let finished_walk = &self.finished_walks[finished_walk_index];

                            let inner_unused_width = inner_effective_width - finished_walk.outer_size.x;

                            let dx = turtle.align().x * inner_unused_width;
                            let dy = turtle.align().y * inner_unused_height;

                            let align_start = finished_walk.align_list_start;
                            let align_end = self.finished_walk_align_list_end(finished_walk_index);
                            self.move_align_list(dx, dy, align_start, align_end, false);

                            turtle = self.turtles.last_mut().unwrap();
                        }
                    }
                } else {
                    // If walks are laid out from top to bottom, and there are deferred walks, then
                    // the horizontal alignment is applied each walk individually, while the inner
                    // unused height is distributed over the deferred walks.
                    let inner_effective_width = turtle.effective_inner_width();

                    for finished_walk_index in turtle_walks_start..self.finished_walks.len() {
                        let finished_walk = &self.finished_walks[finished_walk_index];

                        let inner_unused_width = inner_effective_width - finished_walk.outer_size.x;

                        let shift_x = turtle.align().y * inner_unused_width;
                        let shift_y = turtle.total_resolved_length_to(finished_walk.deferred_before_count);

                        let align_start = finished_walk.align_list_start;
                        let align_end = self.finished_walk_align_list_end(finished_walk_index);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false);

                        turtle = self.turtles.last_mut().unwrap();
                    }
                }
            },
            Flow::Overlay => {
                // If walks are laid out on top of each other, then both the horizontal and vertical
                // alignment are applied to each walk individually.
                if turtle.align().x != 0.0 || turtle.align().y != 0.0 {
                    let inner_effective_width = turtle.effective_inner_width();
                    let inner_effective_height = turtle.inner_effective_height();

                    for finished_walk_index in turtle_walks_start..self.finished_walks.len() {
                        let finished_walk = &self.finished_walks[finished_walk_index];

                        let inner_unused_width = inner_effective_width - finished_walk.outer_size.x;
                        let inner_unused_height = inner_effective_height - finished_walk.outer_size.y;

                        let shift_x = turtle.align().x * inner_unused_width;
                        let shift_y = turtle.align().y * inner_unused_height;

                        let align_start = finished_walk.align_list_start;
                        let align_end = self.finished_walk_align_list_end(finished_walk_index);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false);

                        turtle = self.turtles.last_mut().unwrap();
                    }
                }
            }
        }

        self.align_list.push(AlignEntry::EndTurtle);

        // Pop the current turtle from the turtle stack.
        self.finished_walks.truncate(turtle.finished_walks_start);
        self.turtles.pop();

        if self.turtles.is_empty() {
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(width, height)
            }
        } else {
            self.walk_turtle_internal(
                Walk {
                    width: Size::Fixed(width),
                    height: Size::Fixed(height),
                    abs_pos: turtle_abs_pos,
                    margin:turtle_margin
                },
                turtle_align_start
            )
        }
    }

    // Returns the end of the align list of the finished walk with the given index.
    fn finished_walk_align_list_end(&self, index: usize) -> usize {
        if index + 1 < self.finished_walks.len() {
            self.finished_walks[index + 1].align_list_start
        } else {
            self.align_list.len()
        }
    }

    /// Walks the turtle with the given `walk` to allocate space on the screen.
    /// 
    /// Each walk produces a rectangle that represents the area allocated by the walk.
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_internal(walk, self.align_list.len())
    }

    fn walk_turtle_internal(&mut self, walk: Walk, align_start: usize) -> Rect {
        let turtle = self.turtles.last_mut().unwrap();

        let old_pos = turtle.pos();
        
        let size = turtle.size_of_next_walk(walk.width, walk.height, walk.margin);
        let outer_size = size + walk.margin.size();
        
        if let Some(pos) = walk.abs_pos {
            turtle.move_to(pos);

            match turtle.flow() {
                Flow::Right | Flow::RightWrap => turtle.allocate_height(outer_size.y),
                Flow::Down => turtle.allocate_width(outer_size.x),
                Flow::Overlay => {
                    turtle.allocate_width(size.x);
                    turtle.allocate_height(size.y);
                }
            }

            turtle.move_to(old_pos);

            self.finished_walks.push(FinishedWalk {
                align_list_start: align_start,
                deferred_before_count: 0,
                outer_size: size + walk.margin.size(),
            });
            
            Rect {
                pos: pos + walk.margin.left_top(),
                size
            }
        }
        else {
            let spacing = turtle.offset_to_next_walk(self.finished_walks.len());
            
            match turtle.flow() {
                Flow::RightWrap if size.x > turtle.unused_inner_width_for_current_row() => {
                    let new_pos = dvec2(
                        turtle.origin.x + turtle.layout.padding.left,
                        turtle.origin.y + turtle.used_height + turtle.wrap_spacing
                    );
                    let shift = new_pos.x - turtle.pos() - spacing;
                    
                    turtle.move_to(new_pos);
                    turtle.allocate_size(outer_size);
            
                    self.move_align_list(shift.x, shift.y, align_start, self.align_list.len(), false);
                },
                Flow::Right | Flow::RightWrap => {
                    turtle.move_right(spacing.x);
                    turtle.allocate_size(outer_size);
                    turtle.move_right(outer_size.x);
                },
                
                Flow::Down => {
                    turtle.move_down(spacing.y);
                    turtle.allocate_size(outer_size);
                    turtle.move_down(outer_size.y);
                },
                Flow::Overlay => turtle.allocate_size(outer_size),
            };
            
            let defer_index = self.turtle().deferred_fills.len();
            self.finished_walks.push(FinishedWalk {
                align_list_start: align_start,
                deferred_before_count: defer_index,
                outer_size,
            });

            Rect {
                pos: old_pos + spacing + walk.margin.left_top(),
                size
            }
        }
    }
    
    /// Defers walking the turtle with the given `Walk`.
    pub fn defer_walk_turtle(&mut self, walk: Walk) -> Option<DeferredWalk> {
        if walk.abs_pos.is_some() {
            return None
        }
        
        let turtle = self.turtles.last_mut().unwrap();
        
        match turtle.flow() {
            Flow::Right => {
                let Size::Fill { weight, min, max } = walk.width else {
                    return None
                };

                let old_pos = turtle.pos();

                let spacing = turtle.offset_to_next_walk(self.finished_walks.len());
                let size = dvec2(0.0, turtle.height_of_next_walk(walk.height, walk.margin));
                let outer_size = size + walk.margin.size();
                
                turtle.move_right(spacing.x);
                turtle.allocate_size(outer_size);
                turtle.move_right(outer_size.x);

                let index = turtle.deferred_fills.len();
                turtle.push_deferred_fill(weight, min, max);

                Some(DeferredWalk::Unresolved{
                    index,
                    pos: old_pos + spacing,
                    margin: walk.margin,
                    other_axis: walk.height,
                })
            },
            Flow::Down => {
                let Size::Fill { weight, min, max } = walk.height else {
                    return None
                };

                let old_pos = turtle.pos();

                let spacing = turtle.offset_to_next_walk(self.finished_walks.len());
                let size = dvec2(turtle.width_of_next_walk(walk.width, walk.margin), 0.0);
                let outer_size = size + walk.margin.size();

                turtle.move_down(spacing.y);
                turtle.allocate_size(outer_size);
                turtle.move_down(outer_size.y);

                let index = turtle.deferred_fills.len();
                turtle.push_deferred_fill(weight, min, max);
                
                Some(DeferredWalk::Unresolved {
                    index,
                    margin: walk.margin,
                    other_axis: walk.width,
                    pos: old_pos + spacing
                })
            },
            Flow::RightWrap if walk.width.is_fill() => {
                error!("flow: RightWrap does not support width: Fill");
                None
            },
            _ => None,
        }
    }
    
    pub fn end_pass_sized_turtle_no_clip(&mut self) {
        let turtle = self.turtles.pop().unwrap();
                
        self.perform_nested_clipping_on_align_list_and_shift(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle{skip:self.align_list.len()};
        self.finished_walks.truncate(turtle.finished_walks_start);
    }
    
    pub fn end_pass_sized_turtle(&mut self){
        let turtle = self.turtles.pop().unwrap();
        // lets perform clipping on our alignlist.
        self.align_list.push(AlignEntry::EndTurtle);
        
        self.perform_nested_clipping_on_align_list_and_shift(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle{skip:self.align_list.len()};
        self.finished_walks.truncate(turtle.finished_walks_start);
    }
    
    pub fn end_pass_sized_turtle_with_shift(&mut self, area:Area, shift:DVec2){
        let turtle = self.turtles.pop().unwrap();
        // lets perform clipping on our alignlist.
        self.align_list.push(AlignEntry::EndTurtle);
        
        self.perform_nested_clipping_on_align_list_and_shift(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::ShiftTurtle{
            area,
            shift, 
            skip: self.align_list.len()
        };
        self.finished_walks.truncate(turtle.finished_walks_start);
    }
    
    pub fn turtle_has_align_items(&mut self)->bool{
        self.align_list.len() != self.turtle().align_start + 1
    }
    
    pub fn end_turtle_with_area(&mut self, area: &mut Area)->Rect {
        let rect = self.end_turtle_with_guard(Area::Empty);
        self.add_aligned_rect_area(area, rect);
        rect
    }
    
    pub fn set_turtle_wrap_spacing(&mut self, spacing: f64){
        self.turtle_mut().wrap_spacing = spacing;
    }

    pub fn walk_turtle_with_area(&mut self, area: &mut Area, walk: Walk) -> Rect {
        let rect = self.walk_turtle_internal(walk, self.align_list.len());
        self.add_aligned_rect_area(area, rect);
        rect
    }
    
    pub fn walk_turtle_with_align(&mut self, walk: Walk, align_start: usize) -> Rect {
        self.walk_turtle_internal(walk, align_start)
    }
    
    pub fn peek_walk_turtle(&self, walk: Walk) -> Rect {
        self.walk_turtle_peek(walk)
    }
    
    pub fn walk_turtle_would_be_visible(&mut self, walk: Walk) -> bool {
        let rect = self.walk_turtle_peek(walk);
        self.turtle().rect_is_visible(rect)
    }
       
    pub fn peek_walk_pos(&self, walk: Walk) -> DVec2 {
        if let Some(pos) = walk.abs_pos {
            pos + walk.margin.left_top()
        }
        else {
            let turtle = self.turtles.last().unwrap();
            turtle.pos + walk.margin.left_top()
        }
    }
    
    pub fn emit_turtle_walk(&mut self, rect:Rect){
        let turtle = self.turtles.last().unwrap();
        self.finished_walks.push(FinishedWalk {
            align_list_start: self.align_list.len(),
            deferred_before_count: turtle.deferred_fills.len(),
            outer_size: rect.size,
        });
    }
    
    fn walk_turtle_peek(&self, walk: Walk) -> Rect {
        if self.turtles.len() == 0{
            return Rect::default()
        }
        let turtle = self.turtles.last().unwrap();
        let size = dvec2(
            turtle.width_of_next_walk(walk.width, walk.margin),
            turtle.height_of_next_walk(walk.height, walk.margin),
        );
        
        if let Some(pos) = walk.abs_pos {
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.offset_to_next_walk(self.finished_walks.len());
            let pos = turtle.pos;
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    
    pub fn turtle_new_line(&mut self){
        let turtle = self.turtles.last_mut().unwrap();
        turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
        let next_y = turtle.used_height + turtle.origin.y + turtle.wrap_spacing;
        turtle.pos.y = turtle.pos.y.max(next_y);
        turtle.used_height = turtle.pos.y - turtle.origin.y;
        turtle.wrap_spacing = 0.0;
    }

    pub fn turtle_new_line_with_spacing(&mut self, spacing: f64){
        let turtle = self.turtles.last_mut().unwrap();
        turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
        let next_y = turtle.used_height + turtle.origin.y + turtle.wrap_spacing + spacing;
        turtle.pos.y = turtle.pos.y.max(next_y);
        turtle.used_height = turtle.pos.y - turtle.origin.y;
        turtle.wrap_spacing = 0.0;
    }
    
    fn move_align_list(&mut self, dx: f64, dy: f64, align_start: usize, align_end: usize, shift_clip: bool) {
        //let current_dpi_factor = self.current_dpi_factor();
        let dx = if dx.is_nan() {0.0}else {dx};
        let dy = if dy.is_nan() {0.0}else {dy};
        if dx.abs() <  0.000000001 && dy.abs() <  0.000000001{
            return 
        }
        //let dx = (dx * current_dpi_factor).floor() / current_dpi_factor;
        //let dy = (dy * current_dpi_factor).floor() / current_dpi_factor;
        let d = dvec2(dx, dy);
        let mut c = align_start;
        while c < align_end {
            let align_item = &mut self.align_list[c];
            match align_item {
                AlignEntry::Area(Area::Instance(inst)) => {
                    let draw_list = &mut self.cx.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    let inst_buf = draw_item.instances.as_mut().unwrap();
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            inst_buf[inst.instance_offset + rect_pos + 0 + i * sh.mapping.instances.total_slots] += dx as f32;
                            inst_buf[inst.instance_offset + rect_pos + 1 + i * sh.mapping.instances.total_slots] += dy as f32;
                            if shift_clip{
                                if let Some(draw_clip) = sh.mapping.draw_clip {
                                    inst_buf[inst.instance_offset + draw_clip + 0 + i * sh.mapping.instances.total_slots] += dx as f32;
                                    inst_buf[inst.instance_offset + draw_clip + 1 + i * sh.mapping.instances.total_slots] += dy as f32;
                                    inst_buf[inst.instance_offset + draw_clip + 2 + i * sh.mapping.instances.total_slots] += dx as f32;
                                    inst_buf[inst.instance_offset + draw_clip + 3 + i * sh.mapping.instances.total_slots] += dy as f32;
                                }
                            }
                        }
                    }
                },
                AlignEntry::Area(Area::Rect(ra)) => {
                    let draw_list = &mut self.cx.draw_lists[ra.draw_list_id];
                    let rect_area = &mut draw_list.rect_areas[ra.rect_id];
                    rect_area.rect.pos += d;
                    if shift_clip{
                        rect_area.draw_clip.0 += d;
                        rect_area.draw_clip.1 += d;
                    }
                }
                AlignEntry::BeginTurtle(clip0, clip1)=>{
                    *clip0 += d;
                    *clip1 += d;
                }
                AlignEntry::SkipTurtle{skip} | AlignEntry::ShiftTurtle{skip,..} =>{
                    c = *skip;
                    continue;
                }
                _ => (),
            }
            c += 1;
        }
    }
    
    fn perform_nested_clipping_on_align_list_and_shift(&mut self, align_start:usize, align_end:usize){
        self.turtle_clips.clear();
        let mut i = align_start;
        while i < align_end{
            let align_item = &self.align_list[i];
            match align_item {
                AlignEntry::SkipTurtle{skip} =>{
                    i = *skip;
                    continue;
                }
                AlignEntry::ShiftTurtle{area, shift, skip} =>{
                    let rect = area.rect(self);
                    let skip = *skip;
                    self.move_align_list(rect.pos.x+shift.x, rect.pos.y+shift.y, i + 1, skip, true);
                    i = skip;
                    continue;
                }
                AlignEntry::BeginTurtle(clip0, clip1)=>{
                    if let Some((tclip0, tclip1)) = self.turtle_clips.last(){
                        self.turtle_clips.push((
                            dvec2(clip0.x.max(tclip0.x),clip0.y.max(tclip0.y)),
                            dvec2(clip1.x.min(tclip1.x),clip1.y.min(tclip1.y)),
                        ));
                    }
                    else{
                        self.turtle_clips.push((*clip0, *clip1));
                    }
                }
                AlignEntry::EndTurtle=>{
                    self.turtle_clips.pop().unwrap();
                }
                AlignEntry::Area(Area::Instance(inst)) => if let Some((clip0, clip1)) = self.turtle_clips.last(){
                    let draw_list = &mut self.cx.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    let inst_buf = draw_item.instances.as_mut().unwrap();
                    for i in 0..inst.instance_count {
                        if let Some(draw_clip) = sh.mapping.draw_clip {
                            inst_buf[inst.instance_offset + draw_clip + 0 + i * sh.mapping.instances.total_slots] = clip0.x as f32;
                            inst_buf[inst.instance_offset + draw_clip + 1 + i * sh.mapping.instances.total_slots] = clip0.y as f32;
                            inst_buf[inst.instance_offset + draw_clip + 2 + i * sh.mapping.instances.total_slots] = clip1.x as f32;
                            inst_buf[inst.instance_offset + draw_clip + 3 + i * sh.mapping.instances.total_slots] = clip1.y as f32;
                        }
                    }
                },
                AlignEntry::Area(Area::Rect(ra)) => if let Some((clip0, clip1)) = self.turtle_clips.last(){
                    let draw_list = &mut self.cx.draw_lists[ra.draw_list_id];
                    let rect_area = &mut draw_list.rect_areas[ra.rect_id];
                    rect_area.draw_clip.0 = *clip0;
                    rect_area.draw_clip.1 = *clip1;
                }
                AlignEntry::Unset=>{}
                AlignEntry::Area(_)=>{}
            }
            i += 1;
        }
    }
    
    pub fn get_turtle_align_range(&self) -> TurtleAlignRange {
        TurtleAlignRange{
            start:  self.turtles.last().unwrap().align_start,
            end: self.align_list.len()
        }
    }
    
    pub fn shift_align_range(&mut self, range: &TurtleAlignRange, shift: DVec2) {
        self.move_align_list(shift.x, shift.y, range.start, range.end, true);
    }
    
    pub fn add_rect_area(&mut self, area: &mut Area, rect: Rect) {
        //let turtle = self.turtle();
        self.add_aligned_rect_area(area, rect)
    }
}

pub struct TurtleAlignRange{
    start: usize,
    end: usize
}

impl Turtle {
    pub fn row_height(&self)->f64{
        self.used_height - (self.pos.y - self.origin.y) + self.wrap_spacing
    }
    
    pub fn used(&self) -> DVec2 {
        dvec2(self.used_width, self.used_height)
    }
    
    pub fn set_used(&mut self, width_used: f64, height_used: f64) {
        self.used_width = width_used;
        self.used_height = height_used;
    }
        
    pub fn set_wrap_spacing(&mut self, value: f64){
        self.wrap_spacing = self.wrap_spacing.max(value);
    }

    pub fn rect_is_visible(&self,  geom: Rect) -> bool {
        let view = Rect {pos: self.origin + self.layout.scroll, size: dvec2(self.width, self.height)};
        return view.intersects(geom)
    }
    
    pub fn rel_pos(&self) -> DVec2 {
        DVec2 {
            x: self.pos.x - self.origin.x,
            y: self.pos.y - self.origin.y
        }
    }
    
    pub fn rel_pos_padded(&self) -> DVec2 {
        DVec2 {
            x: self.pos.x - self.origin.x - self.layout.padding.left,
            y: self.pos.y - self.origin.y - self.layout.padding.right
        }
    }
    
    pub fn pos(&self) -> DVec2 {
        self.pos
    }

    pub fn scroll(&self) -> DVec2 {
        self.layout.scroll
    }

    pub fn max_width(&self, walk: Walk) -> Option<f64> {
        if walk.width.is_fit() {
            return None;
        }
        Some(self.width_of_next_walk(walk.width, walk.margin) as f64)
    }

    pub fn max_height(&self, walk: Walk) -> Option<f64> {
        if walk.height.is_fit() {
            return None
        }
        Some(self.width_of_next_walk(walk.height, walk.margin) as f64)
    }
}

impl Walk {
    pub fn abs_rect(rect:Rect) -> Self {
        Self {
            abs_pos: Some(rect.pos),
            margin: Margin::default(),
            width: Size::Fixed(rect.size.x),
            height: Size::Fixed(rect.size.y),
        }
    }
    
    pub fn with_abs_pos(mut self, v: DVec2) -> Self {
        self.abs_pos = Some(v);
        self
    }
    pub fn with_margin_all(mut self, v: f64) -> Self {
        self.margin = Margin {left: v, right: v, top: v, bottom: v};
        self
    }
    
    pub fn with_add_padding(mut self, v: Padding) -> Self {
        self.margin.top += v.top;
        self.margin.left += v.left;
        self.margin.right += v.right;
        self.margin.bottom += v.bottom;
        self
    }
}

impl Layout {
    pub fn with_scroll(mut self, v: DVec2) -> Self {
        self.scroll = v;
        self
    }
    
    pub fn with_align_x(mut self, v: f64) -> Self {
        self.align.x = v;
        self
    }
    
    pub fn with_align_y(mut self, v: f64) -> Self {
        self.align.y = v;
        self
    }
    
    pub fn with_clip(mut self, clip_x:bool, clip_y:bool) -> Self {
        self.clip_x = clip_x;
        self.clip_y = clip_y;
        self
    }
    
    pub fn with_padding_all(mut self, v: f64) -> Self {
        self.padding = Padding {left: v, right: v, top: v, bottom: v};
        self
    }
}

impl LiveHook for Padding {
    fn skip_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        if let Some(v) = nodes[index].value.as_float(){
            *self = Self {left: v, top: v, right: v, bottom: v};
            Some(index + 1)
        }
        else{
            None
        }
    }
}

impl LiveHook for Size {
    fn skip_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Array => {
                fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
                    if let Some(index) = nodes.last_child(index) {
                        if nodes[index].value.is_object() {
                            return nodes.child_by_name(index, live_id!(value).as_field());
                        }
                        else {
                            return Some(index)
                        }
                    }
                    None
                }

                if let Some(inner_index) = last_keyframe_value_from_array(index, nodes) {
                    match &nodes[inner_index].value {
                        LiveValue::Float64(val) => {
                            *self = Self::Fixed(*val);
                        }
                        LiveValue::Int64(val) => {
                            *self = Self::Fixed(*val as f64);
                        }
                        _ => {
                            cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Animation array");
                        }
                    }
                }
                else {
                    cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Animation array");
                }
                Some(nodes.skip_node(index))
            }
            LiveValue::BareEnum(live_id!(Fill))=>{
                *self = Self::fill();
                Some(index + 1)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Float32(v) => {
                *self = Self::Fixed(*v as f64);
                Some(index + 1)
            }
            LiveValue::Float64(v) => {
                *self = Self::Fixed(*v);
                Some(index + 1)
            }
            LiveValue::Int64(v) => {
                *self = Self::Fixed(*v as f64);
                Some(index + 1)
            }
            _ => None
        }
    }
}