use {
    crate::{
        makepad_platform::*,
        cx_2d::{Cx2d},
    }
};

#[derive(Copy, Clone, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Layout {
    #[live] pub scroll: DVec2,
    #[live(true)] pub clip_x: bool,
    #[live(true)] pub clip_y: bool,
    #[live] pub padding: Padding,
    #[live] pub align: Align,
    #[live] pub flow: Flow,
    #[live] pub spacing: f64,
    //#[live] pub line_spacing: f64
}

impl Default for Layout{
    fn default()->Self{
        Self{
            scroll: dvec2(0.0,0.0),
            clip_x: true,
            clip_y: true,
            padding: Padding::default(),
            align: Align{x:0.0,y:0.0},
            flow: Flow::Right,
            spacing: 0.0,
            //line_spacing: 0.0
        }
    }
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Walk {
    #[live] pub abs_pos: Option<DVec2>,
    #[live] pub margin: Margin,
    #[live] pub width: Size,
    #[live] pub height: Size,
}

#[derive(Clone, Copy, Default, Debug, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct Align {
    #[live] pub x: f64,
    #[live] pub y: f64
}

#[derive(Clone, Copy, Default, Debug, Live, LiveRegister)]
#[live_ignore]
pub struct Padding {
    #[live] pub left: f64,
    #[live] pub top: f64,
    #[live] pub right: f64,
    #[live] pub bottom: f64
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum Axis2 {
    #[pick] Horizontal,
    Vertical
}

impl Default for Axis2 {
    fn default() -> Self {
        Axis2::Horizontal
    }
}

#[derive(Copy, Clone, Debug, Live, LiveHook, PartialEq)]
#[live_ignore]
pub enum Flow {
    #[pick] Right,
    Down,
    //Left,
    //Up,
    Overlay, 
    RightWrap
}

#[derive(Copy, Clone, Debug, Live)]
#[live_ignore]
pub enum Size {
    #[pick] Fill,
    #[live(100.0)] WeightedFill(f64),
    #[live(200.0)] Fixed(f64),
    Fit,
}

#[derive(Clone, Debug)]
pub enum DeferredWalk {
    Unresolved {
        index: usize,
        pos: DVec2,
        margin: Margin,
        other_axis: Size,
    },
    Resolved(Walk)
}

impl DeferredWalk {
    pub fn resolve(&mut self, cx: &Cx2d) -> Walk {
        match *self {
            Self::Unresolved{index, pos, margin, other_axis}=>{
                let turtle = cx.turtles.last().unwrap();

                let walk = match turtle.layout.flow {
                    Flow::Right => Walk {
                        abs_pos: Some(pos + dvec2(turtle.deferred_width_up_to(index), 0.0)),
                        margin,
                        width: Size::Fixed(turtle.deferred_width_at(index)),
                        height: other_axis
                    },
                    Flow::Down => Walk {
                        abs_pos: Some(pos + dvec2(0.0, turtle.deferred_height_up_to(index))),
                        margin: margin,
                        height: Size::Fixed(turtle.deferred_height_at(index)),
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

#[derive(Debug)]
pub enum AlignEntry{
    Unset,
    Area(Area),
    ShiftTurtle{area:Area, shift:DVec2, skip:usize},
    SkipTurtle{skip:usize},
    BeginTurtle(DVec2,DVec2),
    EndTurtle
}

#[derive(Clone, Default, Debug)]
pub struct FinishedWalk {
    align_start: usize,
    deferred_count_before: usize,
    outer_size: DVec2,
}

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
    deferred_walk_weights: Vec<f64>,
    deferred_walk_weight_sum: f64,
    shift: DVec2,
    pos: DVec2,
    origin: DVec2,
    guard_area: Area
}

impl Turtle {
    /// Returns a reference to this turtle's spacing.
    pub fn spacing(&self) -> f64 {
        self.layout.spacing
    }

    /// Returns a reference to this turtle's padding.
    pub fn padding(&self) -> Padding {
        self.layout.padding
    }

    /// Return a reference to this turtle's margin.
    pub fn margin(&self) -> &Margin {
        &self.walk.margin
    }

    /// Returns the inner rectangle of this turtle.
    pub fn inner_rect(&self) -> Rect {
        Rect {
            pos: self.inner_origin(),
            size: self.inner_size(),
        }
    }

    /// Returns the inner rectangle of this turtle, without scrolling applied.
    pub fn inner_rect_unscrolled(&self) -> Rect {
        Rect {
            pos: self.inner_origin_unscrolled(),
            size: self.inner_size(),
        }
    }

    /// Returns the origin of this turtle's inner rectangle.
    pub fn inner_origin(&self) -> DVec2 {
        self.origin + self.padding().left_top()
    }

    /// Returns the origin of this turtle's inner rectangle, without scrolling applied.
    pub fn inner_origin_unscrolled(&self) -> DVec2 {
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
    /// If the inner unused width is unknown, then NaN is returned.
    pub fn inner_unused_width(&self) -> f64 {
        self.inner_width() - self.inner_used_width().min(self.inner_width())
    }

    /// Returns the unused width of this turtle's inner rectangle on the current row.
    /// 
    /// If the inner unused width on the current row is unknown, then NaN is returned.
    pub fn inner_unused_width_current_row(&self) -> f64 {
        self.inner_width() - self.inner_used_width_current_row().min(self.inner_width())
    }

    /// Returns the unused height of this turtle's inner rectangle.
    /// 
    /// If the inner unused height is unknown, then NaN is returned.
    pub fn inner_unused_height(&self) -> f64 {
        self.inner_height() - self.inner_used_height().min(self.inner_height())
    }

    /// Returns the effective width of this turtle's inner rectangle.
    /// 
    /// This is either the inner width, or the inner used width if the inner width is unknown.
    pub fn inner_effective_width(&self) -> f64 {
        if !self.inner_width().is_nan() {
            self.inner_width()
        } else {
            self.inner_used_width()
        }
    }

    /// Returns the effective height of this turtle's inner rectangle.
    /// 
    /// This is either the inner height, or the inner used height if the inner height is unknown.
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

    /// Returns the outer rectangle of this turtle.
    pub fn outer_rect(&self) -> Rect {
        Rect {
            pos: self.outer_origin(),
            size: self.outer_size(),
        }
    }

    /// Returns the outer rectangle of this turtle, without scrolling applied.
    pub fn outer_rect_unscrolled(&self) -> Rect {
        Rect {
            pos: self.outer_origin_unscrolled(),
            size: self.outer_size(),
        }
    }

    /// Returns the origin of this turtle's outer rectangle.
    pub fn outer_origin(&self) -> DVec2 {
        self.origin() - self.margin().left_top()
    }

    /// Returns the origin of this turtle's outer rectangle, without scrolling applied.
    pub fn outer_origin_unscrolled(&self) -> DVec2 {
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
    pub fn outer_used_width(&self) -> f64 {
        self.used_width() + self.margin().left
    }

    /// Returns the used width of this turtle's outer rectangle on the current row.
    pub fn outer_used_width_current_row(&self) -> f64 {
        self.used_width_current_row() + self.margin().left
    }

    /// Returns the used height of this turtle's outer rectangle.
    pub fn outer_used_height(&self) -> f64 {
        self.used_height() + self.margin().top
    }

    /// Returns the unused width of this turtle's outer rectangle.
    /// 
    /// If the outer unused width is unknown, then NaN is returned.
    pub fn outer_unused_width(&self) -> f64 {
        self.outer_width() - self.outer_used_width().min(self.outer_width())
    }

    /// Returns the unused width of this turtle's outer rectangle on the current row.
    /// 
    /// If the outer unused width on the current row is unknown, then NaN is returned.
    pub fn outer_unused_width_current_row(&self) -> f64 {
        self.outer_width() - self.outer_used_width_current_row().min(self.outer_width())
    }

    /// Returns the unused height of this turtle's outer rectangle.
    /// 
    /// If the outer unused height is unknown, then NaN is returned.
    pub fn outer_unused_height(&self) -> f64 {
        self.outer_height() - self.outer_used_height().min(self.outer_height())
    }

    /// Returns the effective width of this turtle's outer rectangle.
    ///
    /// This is either the outer width, or the outer used width if the outer width is unknown.
    pub fn outer_effective_width(&self) -> f64 {
        if !self.outer_width().is_nan() {
            self.outer_width()
        } else {
            self.outer_used_width()
        }
    }

    /// Returns the effective height of this turtle's outer rectangle.
    ///
    /// This is either the outer height, or the outer used height if the outer height is unknown.
    pub fn outer_effective_height(&self) -> f64 {
        if !self.outer_height().is_nan() {
            self.outer_height()
        } else {
            self.outer_used_height()
        }
    }

    pub fn eval_size(&self, width: Size, height: Size, margin: Margin) -> DVec2 {
        dvec2(
            self.eval_width(width, margin),
            self.eval_height(height, margin),
        )
    }

    pub fn eval_width(&self, width: Size, margin: Margin) -> f64 {
        match width {
            Size::Fill | Size::WeightedFill(_) => {
                let outer_width = match self.layout.flow {
                    Flow::Right => self.inner_unused_width(),
                    Flow::RightWrap => self.inner_unused_width_current_row(),
                    Flow::Down | Flow::Overlay => self.inner_effective_width(),
                };
                outer_width - margin.width()
            },
            Size::Fixed(width) => width.max(0.0),
            Size::Fit => f64::NAN,
        }
    }
    
    pub fn eval_height(&self, height: Size, margin: Margin) -> f64 {
        match height {
            Size::Fill | Size::WeightedFill(_) => {
                let outer_height = match self.layout.flow {
                    Flow::RightWrap | Flow::Right | Flow::Overlay => self.inner_effective_height(),
                    Flow::Down => self.inner_unused_height()
                };
                outer_height - margin.height()
            }
            Size::Fixed(height) => height.max(0.0),
            Size::Fit => f64::NAN,
        }
    }

    /// Sets this turtle's position.
    pub fn set_pos(&mut self, pos: DVec2) {
        self.pos = pos
    }

    /// Moves this turtle right and down by the given amount.
    pub fn move_right_down(&mut self, amount: DVec2) {
        self.set_pos(self.pos() + amount);
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
    pub fn allocate_width(&mut self, additional: f64) {
        self.used_width = self.used_width.max(self.pos().x + additional - self.origin().x);
    }

    /// Allocates additional height below this turtle's position.
    pub fn allocate_height(&mut self, additional: f64) {
        self.used_height = self.used_height.max(self.pos().y + additional - self.origin().y);
    }

    /// Returns true if this turtle has a previous walk.
    /// 
    /// This is true if the turtle has any finished or deferred walks.
    fn has_previous_walk(&self, finished_walks_end: usize) -> bool {
        self.finished_walks_start != finished_walks_end || self.deferred_walk_count() > 0
    }

    fn deferred_walk_count(&self) -> usize {
        self.deferred_walk_weights.len()
    }

    /// Returns the spacing for the next walk.
    /// 
    /// This is either the spacing for this turtle in the direction of the flow, or zero if there
    /// is no previous walk.
    fn spacing_for_next_walk(&self, finished_walks_end: usize) -> DVec2 {
        if self.has_previous_walk(finished_walks_end) {
            match self.layout.flow {
                Flow::Right | Flow::RightWrap => dvec2(self.spacing(), 0.0),
                Flow::Down => dvec2(0.0, self.spacing()),
                Flow::Overlay => dvec2(0.0, 0.0),
            }
        } else {
            dvec2(0.0, 0.0)
        }
    }

    fn deferred_width_at(&self, index: usize) -> f64 {
        self.inner_unused_width() * self.deferred_walk_weights[index] / self.deferred_walk_weight_sum
    }

    fn deferred_width_up_to(&self, end: usize) -> f64 {
        let mut deferred_width = 0.0;
        for index in 0..end {
            deferred_width += self.deferred_width_at(index);
        }
        deferred_width
    }

    fn deferred_height_at(&self, index: usize) -> f64 {
        self.inner_unused_height() * self.deferred_walk_weights[index] / self.deferred_walk_weight_sum
    }

    fn deferred_height_up_to(&self, end: usize) -> f64 {
        let mut deferred_height = 0.0;
        for index in 0..end {
            deferred_height += self.deferred_height_at(index);
        }
        deferred_height
    }

    fn push_deferred_walk(&mut self, weight: f64) {
        self.deferred_walk_weights.push(weight);
        self.deferred_walk_weight_sum += weight;
    }
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

    /// Walks the turtle with the given `walk`.
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_internal(walk, self.align_list.len())
    }

    fn walk_turtle_internal(&mut self, walk: Walk, align_start: usize) -> Rect {
        let turtle = self.turtles.last_mut().unwrap();

        let old_pos = turtle.pos();
        
        let size = turtle.eval_size(walk.width, walk.height, walk.margin);
        let outer_size = size + walk.margin.size();
        
        if let Some(pos) = walk.abs_pos {
            turtle.set_pos(pos);

            match turtle.layout.flow {
                Flow::Right | Flow::RightWrap => turtle.allocate_height(outer_size.y),
                Flow::Down => turtle.allocate_width(outer_size.x),
                Flow::Overlay => {
                    turtle.allocate_width(size.x);
                    turtle.allocate_height(size.y);
                }
            }

            turtle.set_pos(old_pos);

            self.finished_walks.push(FinishedWalk {
                align_start,
                deferred_count_before: 0,
                outer_size: size + walk.margin.size(),
            });
            
            Rect {
                pos: pos + walk.margin.left_top(),
                size
            }
        }
        else {
            let spacing = turtle.spacing_for_next_walk(self.finished_walks.len());
            
            match turtle.layout.flow {
                Flow::RightWrap if size.x > turtle.inner_unused_width_current_row() => {
                    let new_pos = dvec2(
                        turtle.origin.x + turtle.layout.padding.left,
                        turtle.origin.y + turtle.used_height + turtle.wrap_spacing
                    );
                    let shift = new_pos.x - turtle.pos() - spacing;
                    
                    turtle.set_pos(new_pos);
                    turtle.allocate_size(outer_size);
            
                    self.move_align_list(shift.x, shift.y, align_start, self.align_list.len(), false, dvec2(0.0,0.0));
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
            
            let defer_index = self.turtle().deferred_walk_count();
            self.finished_walks.push(FinishedWalk {
                align_start,
                deferred_count_before: defer_index,
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
        
        match turtle.layout.flow {
            Flow::Right => {
                let Some(weight) = walk.width.fill() else {
                    return None
                };

                let old_pos = turtle.pos();

                let spacing = turtle.spacing_for_next_walk(self.finished_walks.len());
                let size = dvec2(0.0, turtle.eval_height(walk.height, walk.margin));
                let outer_size = size + walk.margin.size();
                
                turtle.move_right(spacing.x);
                turtle.allocate_size(outer_size);
                turtle.move_right(outer_size.x);

                let index = turtle.deferred_walk_count();
                turtle.push_deferred_walk(weight);

                Some(DeferredWalk::Unresolved{
                    index,
                    pos: old_pos + spacing,
                    margin: walk.margin,
                    other_axis: walk.height,
                })
            },
            Flow::Down => {
                let Some(weight) = walk.height.fill() else {
                    return None
                };

                let old_pos = turtle.pos();

                let spacing = turtle.spacing_for_next_walk(self.finished_walks.len());
                let size = dvec2(turtle.eval_width(walk.width, walk.margin), 0.0);
                let outer_size = size + walk.margin.size();

                turtle.move_down(spacing.y);
                turtle.allocate_size(outer_size);
                turtle.move_down(outer_size.y);

                let index = turtle.deferred_walk_count();
                turtle.push_deferred_walk(weight);

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
    
    pub fn begin_turtle(&mut self, walk: Walk, layout: Layout) {
        self.begin_turtle_with_guard(walk, layout, Area::Empty)
    }
    
    pub fn begin_pass_sized_turtle_no_clip(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_sized_turtle_no_clip(size, layout)
    }
    
    pub fn begin_pass_sized_turtle(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_sized_turtle(size, layout)
    }
    
    pub fn begin_sized_turtle_no_clip(&mut self, size:DVec2,layout: Layout) {
        self.begin_sized_turtle(size, layout);
        *self.align_list.last_mut().unwrap() = AlignEntry::Unset;
    }
                    
    pub fn begin_sized_turtle(&mut self, size:DVec2, layout: Layout) {
        self.align_list.push(AlignEntry::BeginTurtle(dvec2(0.0,0.0),size));
        let turtle = Turtle {
            walk: Walk::fill(),
            layout,
            align_start: self.align_list.len() - 1,
            finished_walks_start: self.finished_walks.len(),
            deferred_walk_weights: Vec::new(),
            deferred_walk_weight_sum: 0.0,
            pos: DVec2 {
                x: layout.padding.left,
                y: layout.padding.top
            },
            wrap_spacing: 0.0,
            origin: dvec2(0.0, 0.0),
            width: size.x,
            height: size.y,
            shift: dvec2(0.0, 0.0),
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            guard_area: Area::Empty,
        };
        self.turtles.push(turtle);
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
    
    pub fn begin_turtle_with_guard(&mut self, walk: Walk, layout: Layout, guard_area: Area) {
        let (origin, width, height, draw_clip) = if let Some(parent) = self.turtles.last() {
            
            let o = walk.margin.left_top() + if let Some(pos) = walk.abs_pos {pos} else {
                parent.pos + parent.spacing_for_next_walk(self.finished_walks.len())
            };
            
            let w = parent.eval_width(walk.width, walk.margin);
            let h = parent.eval_height(walk.height, walk.margin);
            
            // figure out new clipping rect
            let (x0, x1) = if layout.clip_x {
                (/*parent.draw_clip.0.x.max(*/o.x/*)*/, if w.is_nan() {
                    f64::NAN
                    //parent.draw_clip.1.x
                } else {
                    /*parent.draw_clip.1.x.min*/o.x + w/*)*/
                })
            } else {
                (f64::NAN, f64::NAN)//parent.draw_clip.0.x, parent.draw_clip.1.x)
            };
            
            let (y0, y1) = if layout.clip_y {
                (/*parent.draw_clip.0.y.max(*/o.y/*)*/, if h.is_nan() {
                    f64::NAN
                } else {
                    /*parent.draw_clip.1.y.min(*/o.y + h/*)*/
                })
            }else {(f64::NAN,f64::NAN)};//parent.draw_clip.0.y, parent.draw_clip.1.y)};
            
            (o - layout.scroll, w, h, (dvec2(x0, y0), (dvec2(x1, y1))))
        }
        else {
            let o = DVec2 {x: walk.margin.left, y: walk.margin.top};
            let w = walk.width.fixed_or_nan();
            let h = walk.height.fixed_or_nan();
            
            (o, w, h, (dvec2(o.x, o.y), dvec2(o.x + w, o.y + h)))
        };
        self.align_list.push(AlignEntry::BeginTurtle(draw_clip.0,draw_clip.1));
        let turtle = Turtle {
            walk,
            layout,
            align_start: self.align_list.len()-1,
            finished_walks_start: self.finished_walks.len(),
            deferred_walk_weights: Vec::new(),
            deferred_walk_weight_sum: 0.0,
            wrap_spacing: 0.0,
            pos: DVec2 {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top
            },
            origin,
            width,
            height,
            shift: dvec2(0.0,0.0),
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            guard_area,
        };
        
        self.turtles.push(turtle);
    }
    
    pub fn turtle_has_align_items(&mut self)->bool{
        self.align_list.len() != self.turtle().align_start + 1
    }
    
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }
    
    pub fn end_turtle_with_area(&mut self, area: &mut Area)->Rect {
        let rect = self.end_turtle_with_guard(Area::Empty);
        self.add_aligned_rect_area(area, rect);
        rect
    }
    
    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let mut turtle = self.turtles.last().unwrap();
        if guard_area != turtle.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard_area, guard_area)
        }
        
        let turtle_align_start = turtle.align_start;
        let turtle_abs_pos = turtle.walk.abs_pos;
        let turtle_margin = turtle.walk.margin;
        let turtle_walks_start = turtle.finished_walks_start;
        let turtle_shift = turtle.shift;
                
        // computed width / height
        let w = if turtle.width.is_nan() {
            let w = turtle.used_width + turtle.layout.padding.right - turtle.layout.scroll.x;
            // we should update the clip pos
            if let AlignEntry::BeginTurtle(p1,p2) = &mut self.align_list[turtle_align_start]{
                p2.x = p1.x + w;
            }
            Size::Fixed(w)
        }
        else {
            Size::Fixed(turtle.width)
        };
        
        let h = if turtle.height.is_nan() {
            let h =  turtle.used_height + turtle.layout.padding.bottom - turtle.layout.scroll.y;
            // we should update the clip pos
            if let AlignEntry::BeginTurtle(p1,p2) = &mut self.align_list[turtle_align_start]{
                p2.y = p1.y + h;
            }
            Size::Fixed(h)
        }
        else {
            Size::Fixed(turtle.height)
        };
                
        match turtle.layout.flow {
            Flow::Right => {
                if turtle.deferred_walk_count() > 0 {
                    let align_y = turtle.layout.align.y;
                    let padded_height_or_used = turtle.inner_effective_height();
                    for i in turtle_walks_start..self.finished_walks.len() {
                        let walk = &self.finished_walks[i];
                        let shift_x = turtle.deferred_width_up_to(walk.deferred_count_before);
                        let shift_y = align_y * (padded_height_or_used - walk.outer_size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                        turtle = self.turtles.last_mut().unwrap();
                    }
                }
                else {
                    let align_x = turtle.layout.align.x;
                    let align_y = turtle.layout.align.y;
                    let width_left = turtle.inner_unused_width();
                    let padded_height_or_used = turtle.inner_effective_height();
                    if align_x != 0.0 || align_y != 0.0{
                        for i in turtle_walks_start..self.finished_walks.len() {
                            let walk = &self.finished_walks[i];
                            let shift_x = align_x * width_left;
                            let shift_y = align_y * (padded_height_or_used - walk.outer_size.y);
                            let align_start = walk.align_start;
                            let align_end = self.get_turtle_walk_align_end(i);
                            self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                        }
                    }
                }
            },
            Flow::RightWrap=>{
                if turtle.deferred_walk_count() > 0{panic!()}
                // for now we only support align:0,0
            }
            Flow::Down => {
                if turtle.deferred_walk_count() > 0 {
                    let padded_width_or_used = turtle.inner_effective_width();
                    let align_x = turtle.layout.align.x;
                    for i in turtle_walks_start..self.finished_walks.len() {
                        let walk = &self.finished_walks[i];
                        let shift_x = align_x * (padded_width_or_used- walk.outer_size.x);
                        let shift_y = turtle.deferred_height_up_to(walk.deferred_count_before);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                        turtle = self.turtles.last_mut().unwrap();
                    }
                }
                else {
                    let align_x = turtle.layout.align.x;
                    let align_y = turtle.layout.align.y;
                    let padded_width_or_used = turtle.inner_effective_width();
                    let height_left = turtle.inner_unused_height();
                    if align_x != 0.0 || align_y != 0.0{
                        for i in turtle_walks_start..self.finished_walks.len() {
                            let walk = &self.finished_walks[i];
                            let shift_x = align_x * (padded_width_or_used - walk.outer_size.x);
                            let shift_y = align_y * height_left;
                            let align_start = walk.align_start;
                            let align_end = self.get_turtle_walk_align_end(i);
                            self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                        }
                    }
                }
            },
            Flow::Overlay => {
                let align_x = turtle.layout.align.x;
                let align_y = turtle.layout.align.y;
                if align_x != 0.0 || align_y != 0.0{
                    let padded_width_or_used = turtle.inner_effective_width();
                    let padded_height_or_used = turtle.inner_effective_height();
                    for i in turtle_walks_start..self.finished_walks.len() {
                        let walk = &self.finished_walks[i];
                        let shift_x = align_x * (padded_width_or_used - walk.outer_size.x);
                        let shift_y = align_y * (padded_height_or_used - walk.outer_size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                    }
                }
            }
        }
        self.turtles.pop();
        self.finished_walks.truncate(turtle_walks_start);
        self.align_list.push(AlignEntry::EndTurtle);
        if self.turtles.len() == 0 {
            return Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(w.fixed_or_zero(), h.fixed_or_zero())
            }
        }
        let rect = self.walk_turtle_internal(Walk {width: w, height: h, abs_pos:turtle_abs_pos, margin:turtle_margin}, turtle_align_start);
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
            align_start: self.align_list.len(),
            deferred_count_before: turtle.deferred_walk_count(),
            outer_size: rect.size,
        });
    }
    
    fn walk_turtle_peek(&self, walk: Walk) -> Rect {
        if self.turtles.len() == 0{
            return Rect::default()
        }
        let turtle = self.turtles.last().unwrap();
        let size = dvec2(
            turtle.eval_width(walk.width, walk.margin),
            turtle.eval_height(walk.height, walk.margin),
        );
        
        if let Some(pos) = walk.abs_pos {
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.spacing_for_next_walk(self.finished_walks.len());
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
    
    fn move_align_list(&mut self, dx: f64, dy: f64, align_start: usize, align_end: usize, shift_clip: bool, turtle_shift:DVec2) {
        //let current_dpi_factor = self.current_dpi_factor();
        let dx = if dx.is_nan() {0.0}else {dx} + turtle_shift.x;
        let dy = if dy.is_nan() {0.0}else {dy} + turtle_shift.y;
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
                    self.move_align_list(rect.pos.x+shift.x, rect.pos.y+shift.y, i + 1, skip, true, dvec2(0.0,0.0));
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
    
    fn get_turtle_walk_align_end(&self, i: usize) -> usize {
        if i < self.finished_walks.len() - 1 {
            self.finished_walks[i + 1].align_start
        }
        else {
            self.align_list.len()
        }
    }
    
    pub fn get_turtle_align_range(&self) -> TurtleAlignRange {
        TurtleAlignRange{
            start:  self.turtles.last().unwrap().align_start,
            end: self.align_list.len()
        }
    }
    
    pub fn shift_align_range(&mut self, range: &TurtleAlignRange, shift: DVec2) {
        self.move_align_list(shift.x, shift.y, range.start, range.end, true, dvec2(0.0,0.0));
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
    
    pub fn set_shift(&mut self, shift: DVec2) {
        self.shift = shift;
    }
    
    pub fn layout(&self)->&Layout{
        &self.layout
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
        Some(self.eval_width(walk.width, walk.margin) as f64)
    }

    pub fn max_height(&self, walk: Walk) -> Option<f64> {
        if walk.height.is_fit() {
            return None
        }
        Some(self.eval_width(walk.height, walk.margin) as f64)
    }
}

impl Layout {
    pub fn flow_right() -> Self {
        Self {
            flow: Flow::Right,
            ..Self::default()
        }
    }
    
    pub fn flow_down() -> Self {
        Self {
            flow: Flow::Down,
            ..Self::default()
        }
    }
    
    pub fn flow_overlay() -> Self {
        Self {
            flow: Flow::Overlay,
            ..Self::default()
        }
    }

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
    
    pub fn with_padding_top(mut self, v: f64) -> Self {
        self.padding.top = v;
        self
    }
    
    pub fn with_padding_right(mut self, v: f64) -> Self {
        self.padding.right = v;
        self
    }
    
    pub fn with_padding_bottom(mut self, v: f64) -> Self {
        self.padding.bottom = v;
        self
    }
    
    pub fn with_padding_left(mut self, v: f64) -> Self {
        self.padding.left = v;
        self
    }
    
    pub fn with_padding(mut self, v: Padding) -> Self {
        self.padding = v;
        self
    }
}

impl Walk {
    pub fn empty() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fixed(0.0),
            height: Size::Fixed(0.0),
        }
    }
    
    pub fn size(w: Size, h: Size) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: w,
            height: h,
        }
    }

    pub fn abs_rect(rect:Rect) -> Self {
        Self {
            abs_pos: Some(rect.pos),
            margin: Margin::default(),
            width: Size::Fixed(rect.size.x),
            height: Size::Fixed(rect.size.y),
        }
    }
    
    pub fn fixed(w:f64, h:f64) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fixed(w),
            height: Size::Fixed(h),
        }
    }
        
    pub fn fixed_size(size: DVec2) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fixed(size.x),
            height: Size::Fixed(size.y),
        }
    }
    
    pub fn fit() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fit,
            height: Size::Fit,
        }
    }
    
    pub fn fill() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fill,
            height: Size::Fill,
        }
    }
    
    pub fn fill_fit() -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fill,
            height: Size::Fit,
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
    
    pub fn with_margin_top(mut self, v: f64) -> Self {
        self.margin.top = v;
        self
    }
    
    pub fn with_margin_right(mut self, v: f64) -> Self {
        self.margin.right = v;
        self
    }
    
    pub fn with_margin_bottom(mut self, v: f64) -> Self {
        self.margin.bottom = v;
        self
    }
    
    pub fn with_margin_left(mut self, v: f64) -> Self {
        self.margin.left = v;
        self
    }
    
    pub fn with_margin(mut self, v: Margin) -> Self {
        self.margin = v;
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

impl Padding {
    pub fn left_top(&self) -> DVec2 {
        dvec2(self.left, self.top)
    }
    pub fn right_bottom(&self) -> DVec2 {
        dvec2(self.right, self.bottom)
    }
    pub fn size(&self) -> DVec2 {
        dvec2(self.left + self.right, self.top + self.bottom)
    }
    pub fn width(&self) -> f64 {
        self.left + self.right
    }
    pub fn height(&self) -> f64 {
        self.top + self.bottom
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

impl Default for Flow {
    fn default() -> Self {Self::Down}
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

impl Default for Size {
    fn default() -> Self {
        Size::Fill
    }
}

impl Size {
    pub fn fixed_or_zero(&self) -> f64 {
        match self {
            Self::Fixed(v) => *v,
            _ => 0.
        }
    }
    
    pub fn fixed_or_nan(&self) -> f64 {
        match self {
            Self::Fixed(v) => max_zero_keep_nan(*v),
            _ => std::f64::NAN,
        }
    }

    pub fn is_fixed(&self) -> bool {
        match self {
            Self::Fixed(_) => true,
            _ => false
        }
    }

    pub fn is_fit(&self) -> bool {
        match self {
            Self::Fit => true,
            _ => false
        }
    }
    
    pub fn is_fill(&self) -> bool {
        match self {
            Self::Fill | Self::WeightedFill(_) => true,
            _ => false
        }
    }

    pub fn fill(self) -> Option<f64> {
        match self {
            Self::Fill => Some(100.0),
            Self::WeightedFill(weight) => Some(weight),
            _ => None,
        }
    }
}

fn max_zero_keep_nan(v: f64) -> f64 {
    if v.is_nan() {
        v
    }
    else {
        f64::max(v, 0.0)
    }
}
