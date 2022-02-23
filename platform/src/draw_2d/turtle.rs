use {
    crate::{
        makepad_derive_live::*,
        makepad_math::*,
        area::Area,
        live_traits::*,
        draw_2d::cx_2d::Cx2d,
        cx::Cx,
    }
};

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Layout {
    pub padding: Padding,
    pub align: Align,
    pub flow: Flow,
    pub spacing: f32
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Walk {
    pub abs_pos: Option<Vec2>,
    pub margin: Margin,
    pub width: Size,
    pub height: Size,
}

#[derive(Clone, Copy, Debug, Live, LiveHook)]
pub struct Align {
    x: f32,
    y: f32
}

#[derive(Clone, Copy, Default, Debug, Live)]
pub struct Margin {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

#[derive(Clone, Copy, Default, Debug, Live)]
pub struct Padding {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum Flow {
    #[pick] Right,
    Down,
    Overlay
}

#[derive(Copy, Clone, Debug, Live)]
pub enum Size {
    #[pick] Fill,
    #[live(200.0)] Fixed(f32),
    Fit,
}

#[derive(Clone, Default, Debug)]
pub struct DeferWalk {
    defer_index: usize,
    margin: Margin,
    other_axis: Size,
    pos: Vec2
}

#[derive(Clone, Default, Debug)]
pub struct TurtleWalk {
    align_start: usize,
    defer_index: usize,
    rect: Rect,
}

#[derive(Clone, Default, Debug)]
pub struct Turtle {
    walk: Walk,
    layout: Layout,
    align_start: usize,
    turtle_walks_start: usize,
    defer_count: usize,
    pos: Vec2,
    origin: Vec2,
    width: f32,
    height: f32,
    width_used: f32,
    height_used: f32,
    guard_area: Area
}

impl<'a> Cx2d<'a> {
    pub fn turtle(&self) -> &Turtle {
        self.turtles.last().unwrap()
    }
    
    pub fn turtle_mut(&mut self) -> &mut Turtle {
        self.turtles.last_mut().unwrap()
    }
    
    pub fn begin_turtle(&mut self, walk: Walk, layout: Layout) {
        self.begin_turtle_with_guard(walk, layout, Area::Empty)
    }
    
    pub fn defer_walk(&mut self, walk: Walk) -> Option<DeferWalk> {
        let turtle = self.turtles.last_mut().unwrap();
        let defer_index = turtle.defer_count;
        let pos = turtle.pos;
        let margin_size = walk.margin.size();
        match turtle.layout.flow {
            Flow::Right if walk.width.is_fill() => {
                let spacing = turtle.child_spacing(self.turtle_walks.len());
                turtle.pos.x += margin_size.x + spacing.x;
                turtle.update_used(0.0, margin_size.y);
                turtle.defer_count += 1;
                Some(DeferWalk {
                    defer_index,
                    margin: walk.margin,
                    other_axis: walk.height,
                    pos: pos + spacing
                })
            },
            Flow::Down if walk.height.is_fill() => {
                let spacing = turtle.child_spacing(self.turtle_walks.len());
                turtle.pos.y += margin_size.y + spacing.y;
                turtle.update_used(margin_size.x, 0.0);
                turtle.defer_count += 1;
                Some(DeferWalk {
                    defer_index,
                    margin: walk.margin,
                    other_axis: walk.width,
                    pos: pos + spacing
                })
            },
            _ => {
                None
            }
        }
    }
    
    pub fn begin_turtle_with_guard(&mut self, walk: Walk, layout: Layout, guard_area: Area) {
        
        let (origin, width, height) = if let Some(parent) = self.turtles.last() {
            let o = walk.margin.left_top() + if let Some(pos) = walk.abs_pos {pos} else {
                parent.pos + parent.child_spacing(self.turtle_walks.len())
            };
            let w = parent.eval_width(walk.width, walk.margin, parent.layout.flow);
            let h = parent.eval_height(walk.height, walk.margin, parent.layout.flow);
            (o, w, h)
        }
        else {
            let o = Vec2 {x: walk.margin.left, y: walk.margin.top};
            let w = walk.width.fixed_or_nan();
            let h = walk.height.fixed_or_nan();
            (o, w, h)
        };
        
        let turtle = Turtle {
            walk,
            layout,
            align_start: self.align_list.len(),
            turtle_walks_start: self.turtle_walks.len(),
            defer_count: 0,
            pos: Vec2 {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top
            },
            origin,
            width,
            height,
            width_used: layout.padding.left,
            height_used: layout.padding.top,
            guard_area,
        };
        
        self.turtles.push(turtle);
    }
    
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }
    
    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let turtle = self.turtles.pop().unwrap();
        if guard_area != turtle.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard_area, guard_area)
        }
        
        // computed height
        let w = if turtle.width.is_nan() {
            Size::Fixed(turtle.width_used + turtle.layout.padding.right)
        }
        else {
            Size::Fixed(turtle.width)
        };
        
        let h = if turtle.height.is_nan() {
            Size::Fixed(turtle.height_used + turtle.layout.padding.bottom)
        }
        else {
            Size::Fixed(turtle.height)
        };
        
        match turtle.layout.flow {
            Flow::Right => {
                if turtle.defer_count > 0 {
                    let left = turtle.width_left();
                    let part = left / turtle.defer_count as f32;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = walk.defer_index as f32 * part;
                        let shift_y = turtle.layout.align.y * (turtle.no_pad_height() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * turtle.width_left();
                        let shift_y = turtle.layout.align.y * (turtle.no_pad_height() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            Flow::Down => {
                if turtle.defer_count > 0 {
                    let left = turtle.height_left();
                    let part = left / turtle.defer_count as f32;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * (turtle.no_pad_width() - walk.rect.size.x);
                        let shift_y = walk.defer_index as f32 * part;
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * (turtle.no_pad_width() - walk.rect.size.x);
                        let shift_y = turtle.layout.align.y * turtle.height_left();
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            Flow::Overlay=>{
                for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                    let walk = &self.turtle_walks[i];
                    let shift_x = turtle.layout.align.x * (turtle.no_pad_width() - walk.rect.size.x);
                    let shift_y = turtle.layout.align.y * (turtle.no_pad_height() - walk.rect.size.y);
                    let align_start = walk.align_start;
                    let align_end = self.get_turtle_walk_align_end(i);
                    self.move_align_list(shift_x, shift_y, align_start, align_end);
                }
            }
        }
        
        self.turtle_walks.truncate(turtle.turtle_walks_start);
        
        if self.turtles.len() == 0 {
            return Rect {
                pos: vec2(0.0, 0.0),
                size: vec2(w.fixed_or_zero(), h.fixed_or_zero())
            }
        }
        return self.walk_turtle_with_align(Walk {width: w, height: h, ..turtle.walk}, turtle.align_start)
    }
    
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_with_align(walk, self.align_list.len())
    }
    
    pub fn walk_turtle_with_align(&mut self, walk: Walk, align_start: usize) -> Rect {
        
        let turtle = self.turtles.last_mut().unwrap();
        let size = vec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        
        if let Some(pos) = walk.abs_pos {
            self.turtle_walks.push(TurtleWalk {
                align_start,
                defer_index: 0,
                rect: Rect {pos, size: size + walk.margin.size()}
            });
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.child_spacing(self.turtle_walks.len());
            let pos = turtle.pos;
            let margin_size = walk.margin.size();
            match turtle.layout.flow {
                Flow::Right => {
                    turtle.pos.x = pos.x + size.x + margin_size.x + spacing.x;
                    turtle.update_used(0.0, size.y + margin_size.y);
                },
                Flow::Down => {
                    turtle.pos.y = pos.y + size.y + margin_size.y + spacing.y;
                    turtle.update_used(size.x + margin_size.x, 0.0);
                },
                Flow::Overlay=>{ // do not walk
                }
            };
            turtle.width_used = turtle.width_used.max(turtle.pos.x - turtle.origin.x);
            turtle.height_used = turtle.height_used.max(turtle.pos.y - turtle.origin.y);
            
            self.turtle_walks.push(TurtleWalk {
                align_start,
                defer_index: turtle.defer_count,
                rect: Rect {pos, size: size + margin_size}
            });
            
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    fn move_align_list(&mut self, dx: f32, dy: f32, align_start: usize, align_end: usize) {
        let dx = if dx.is_nan() {0.0}else {dx};
        let dy = if dy.is_nan() {0.0}else {dy};
        if dx == 0.0 && dy == 0.0 {
            return
        }
        let dx = (dx * self.current_dpi_factor).floor() / self.current_dpi_factor;
        let dy = (dy * self.current_dpi_factor).floor() / self.current_dpi_factor;
        for i in align_start..align_end {
            let align_item = &self.align_list[i];
            match align_item {
                Area::Instance(inst) => {
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_call = draw_list.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    let inst_buf = draw_call.instances.as_mut().unwrap();
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            inst_buf[inst.instance_offset + rect_pos + i * sh.mapping.instances.total_slots] += dx;
                            inst_buf[inst.instance_offset + rect_pos + 1 + i * sh.mapping.instances.total_slots] += dy;
                        }
                    }
                },
                _ => (),
            }
        }
    }
    
    
    fn get_turtle_walk_align_end(&self, i: usize) -> usize {
        if i < self.turtle_walks.len() - 1 {
            self.turtle_walks[i + 1].align_start
        }
        else {
            self.align_list.len()
        }
    }
}

impl Turtle {
    pub fn update_used(&mut self, dx: f32, dy: f32) {
        self.width_used = self.width_used.max((self.pos.x + dx) - self.origin.x);
        self.height_used = self.height_used.max((self.pos.y + dy) - self.origin.y);
    }

    pub fn used(&self)->Vec2{
        vec2(self.width_used, self.height_used)
    }

    
    pub fn move_pos(&mut self, dx: f32, dy: f32) {
        self.pos.x += dx;
        self.pos.y += dy;
        self.update_used(0.0, 0.0);
    }
    
    pub fn set_pos(&mut self, pos: Vec2) {
        self.pos = pos
    }
    
    fn child_spacing(&self, walks_len: usize) -> Vec2 {
        if self.turtle_walks_start < walks_len || self.defer_count > 0 {
            match self.layout.flow {
                Flow::Right => {
                    vec2(self.layout.spacing, 0.0)
                }
                Flow::Down => {
                    vec2(0.0, self.layout.spacing)
                }
                Flow::Overlay=>{ 
                    vec2(0.0, 0.0)
                }
            }
        }
        else {
            vec2(0.0, 0.0)
        }
    }
    
    pub fn rect_is_visible(&self, geom: Rect, scroll: Vec2) -> bool {
        let view = Rect {pos: scroll, size: vec2(self.width, self.height)};
        return view.intersects(geom)
    }
    
    pub fn origin(&self) -> Vec2 {
        self.origin
    }
    
    pub fn rel_pos(&self) -> Vec2 {
        Vec2 {
            x: self.pos.x - self.origin.x,
            y: self.pos.y - self.origin.y
        }
    }
    
    pub fn pos(&self) -> Vec2 {
        self.pos
    }
    
    pub fn eval_width(&self, width: Size, margin: Margin, flow: Flow) -> f32 {
        return match width {
            Size::Fit => std::f32::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::Right => {
                        max_zero_keep_nan(self.width_left() - margin.width())
                    },
                    Flow::Down | Flow::Overlay=> {
                        max_zero_keep_nan(self.no_pad_width() - margin.width())
                    }
                }
            },
        }
    }
    
    pub fn eval_height(&self, height: Size, margin: Margin, flow: Flow) -> f32 {
        return match height {
            Size::Fit => std::f32::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::Right | Flow::Overlay => {
                        max_zero_keep_nan(self.no_pad_height() - margin.height())
                    },
                    Flow::Down => {
                        max_zero_keep_nan(self.height_left() - margin.height())
                    }
                }
            }
        }
    }
    
    pub fn rect(&self) -> Rect {
        Rect {
            pos: self.origin,
            size: vec2(self.width, self.height)
        }
    }
    
    pub fn padded_rect(&self) -> Rect {
        Rect {
            pos: self.origin + self.layout.padding.left_top(),
            size: vec2(self.width, self.height) - self.layout.padding.size()
        }
    }
    
    pub fn size(&self) -> Vec2 {
        vec2(self.width, self.height)
    }
    
    pub fn width_left(&self) -> f32 {
        return max_zero_keep_nan(self.width - self.width_used - self.layout.padding.right);
    }
    
    pub fn no_pad_width(&self) -> f32 {
        return max_zero_keep_nan(self.width - self.layout.padding.width());
    }
    
    pub fn height_left(&self) -> f32 {
        return max_zero_keep_nan(self.height - self.height_used - self.layout.padding.bottom);
    }
    
    pub fn no_pad_height(&self) -> f32 {
        return max_zero_keep_nan(self.height - self.layout.padding.height());
    }
}

impl DeferWalk {
    pub fn resolve(&self, cx: &Cx2d) -> Walk {
        let turtle = cx.turtles.last().unwrap();
        match turtle.layout.flow {
            Flow::Right => {
                let left = turtle.width_left();
                let part = left / turtle.defer_count as f32;
                Walk {
                    abs_pos: Some(self.pos + vec2(part * self.defer_index as f32, 0.)),
                    margin: self.margin,
                    width: Size::Fixed(part),
                    height: self.other_axis
                }
            },
            Flow::Down => {
                let left = turtle.height_left();
                let part = left / turtle.defer_count as f32;
                Walk {
                    abs_pos: Some(self.pos + vec2(0., part * self.defer_index as f32)),
                    margin: self.margin,
                    height: Size::Fixed(part),
                    width: self.other_axis
                }
            }
            Flow::Overlay => panic!()
        }
    }
}

impl Layout {
    pub fn flow_right() -> Self {
        Self {
            flow: Flow::Right,
            ..Self::default()
        }
    }
    
    pub fn flow_downn() -> Self {
        Self {
            flow: Flow::Down,
            ..Self::default()
        }
    }
    
    pub fn with_align_x(mut self, v: f32) -> Self {
        self.align.x = v;
        self
    }

    pub fn with_align_y(mut self, v: f32) -> Self {
        self.align.y = v;
        self
    }
    
    pub fn with_padding_all(mut self, v: f32) -> Self {
        self.padding = Padding {left: v, right: v, top: v, bottom: v};
        self
    }
    
    pub fn with_padding_top(mut self, v: f32) -> Self {
        self.padding.top = v;
        self
    }
    
    pub fn with_padding_right(mut self, v: f32) -> Self {
        self.padding.right = v;
        self
    }
    
    pub fn with_padding_bottom(mut self, v: f32) -> Self {
        self.padding.bottom = v;
        self
    }
    
    pub fn with_padding_left(mut self, v: f32) -> Self {
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
    
    pub fn fixed_size(w: f32, h: f32) -> Self {
        Self {
            abs_pos: None,
            margin: Margin::default(),
            width: Size::Fixed(w),
            height: Size::Fixed(h),
        }
    }
    
    pub fn with_margin_all(mut self, v: f32) -> Self {
        self.margin = Margin {left: v, right: v, top: v, bottom: v};
        self
    }
    
    pub fn with_margin_top(mut self, v: f32) -> Self {
        self.margin.top = v;
        self
    }
    
    pub fn with_margin_right(mut self, v: f32) -> Self {
        self.margin.right = v;
        self
    }
    
    pub fn with_margin_bottom(mut self, v: f32) -> Self {
        self.margin.bottom = v;
        self
    }
    
    pub fn with_margin_left(mut self, v: f32) -> Self {
        self.margin.left = v;
        self
    }
    
    pub fn with_margin(mut self, v: Margin) -> Self {
        self.margin = v;
        self
    }
}

impl Default for Align {
    fn default() -> Self {
        Self {x: 0.0, y: 0.0}
    }
}

impl LiveHook for Margin {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Float(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            LiveValue::Int(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            _ => None
        }
    }
}

impl Margin {
    fn left_top(&self) -> Vec2 {
        vec2(self.left, self.top)
    }
    fn right_bottom(&self) -> Vec2 {
        vec2(self.right, self.bottom)
    }
    fn size(&self) -> Vec2 {
        vec2(self.left + self.right, self.top + self.bottom)
    }
    fn width(&self) -> f32 {
        self.left + self.right
    }
    fn height(&self) -> f32 {
        self.top + self.bottom
    }
}

impl Padding {
    fn left_top(&self) -> Vec2 {
        vec2(self.left, self.top)
    }
    fn right_bottom(&self) -> Vec2 {
        vec2(self.right, self.bottom)
    }
    fn size(&self) -> Vec2 {
        vec2(self.left + self.right, self.top + self.bottom)
    }
    fn width(&self) -> f32 {
        self.left + self.right
    }
    fn height(&self) -> f32 {
        self.top + self.bottom
    }
}

impl LiveHook for Padding {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Float(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            LiveValue::Int(v) => {
                *self = Self {left: *v as f32, top: *v as f32, right: *v as f32, bottom: *v as f32};
                Some(index + 1)
            }
            _ => None
        }
    }
}

impl Default for Flow {
    fn default() -> Self {Self::Right}
}


impl LiveHook for Size {
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Float(v) => {
                *self = Self::Fixed(*v as f32);
                Some(index + 1)
            }
            LiveValue::Int(v) => {
                *self = Self::Fixed(*v as f32);
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
    pub fn fixed_or_zero(&self) -> f32 {
        match self {
            Self::Fixed(v) => *v,
            _ => 0.
        }
    }
    
    pub fn fixed_or_nan(&self) -> f32 {
        match self {
            Self::Fixed(v) => max_zero_keep_nan(*v),
            _ => std::f32::NAN,
        }
    }
    
    pub fn is_fill(&self) -> bool {
        match self {
            Self::Fill => true,
            _ => false
        }
    }
}

fn max_zero_keep_nan(v: f32) -> f32 {
    if v.is_nan() {
        v
    }
    else {
        f32::max(v, 0.0)
    }
}


