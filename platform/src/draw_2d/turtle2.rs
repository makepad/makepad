use {
    crate::{
        makepad_derive_live::*,
        makepad_math::*,
        area::Area,
        live_traits::*,
        draw_2d::cx_2da::Cx2da,
        cx::Cx,
    }
};

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum LineWrap2 {
    #[pick] None,
    NewLine,
    #[live(8.0)] MaxSize(f32)
}

impl Default for LineWrap2 {
    fn default() -> Self {
        LineWrap2::None
    }
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Layout2 {
    pub padding: Padding2,
    pub align: Align2,
    pub flow: Flow,
    pub spacing: f32
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Walk2 {
    pub abs_pos: Option<Vec2>,
    pub margin: Margin2,
    pub width: Size2,
    pub height: Size2,
}

impl Walk2 {
    pub fn wh(w: Size2, h: Size2) -> Self {
        Self {
            abs_pos: None,
            margin: Margin2::default(),
            width: w,
            height: h,
        }
    }
    
    pub fn fixed(w: f32, h: f32) -> Self {
        Self {
            abs_pos: None,
            margin: Margin2::default(),
            width: Size2::Fixed(w),
            height: Size2::Fixed(h),
        }
    }
}

#[derive(Clone, Copy, Debug, Live, LiveHook)]
pub struct Align2 {
    fx: f32,
    fy: f32
}

impl Default for Align2 {
    fn default() -> Self {
        Self {fx: 0.0, fy: 0.0}
    }
}

#[derive(Clone, Copy, Default, Debug, Live)]
pub struct Margin2 {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

impl LiveHook for Margin2 {
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

impl Margin2 {
    fn left_top(&self) -> Vec2 {
        vec2(self.left, self.top)
    }
    fn right_bottom(&self) -> Vec2 {
        vec2(self.right, self.bottom)
    }
    fn size(&self) -> Vec2 {
        vec2(self.left + self.right, self.top + self.bottom)
    }
}

#[derive(Clone, Copy, Default, Debug, Live)]
pub struct Padding2 {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

impl LiveHook for Padding2 {
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

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum Flow {
    #[pick] Right,
    Down,
    RightWrap
}

impl Default for Flow {
    fn default() -> Self {Self::Right}
}

#[derive(Copy, Clone, Debug, Live)]
pub enum Size2 {
    #[pick] Fill,
    #[live(200.0)] Fixed(f32),
    Fit,
}

impl LiveHook for Size2 {
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

impl Default for Size2 {
    fn default() -> Self {
        Size2::Fill
    }
}

impl Size2 {
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

impl<'a> Cx2da<'a> {
    
    pub fn begin_turtle(&mut self, walk: Walk2, layout: Layout2) {
        self.begin_turtle_with_guard(walk, layout, Area::Empty)
    }
    
    pub fn fill_walk(&mut self, walk: Walk2) -> Option<FillWalk> {
        let turtle = self.turtles.last_mut().unwrap();
        let fill_index = turtle.fill_count;
        let pos = turtle.pos;
        let margin_size = walk.margin.size();
        match turtle.layout.flow {
            Flow::Right if walk.width.is_fill() => {
                let spacing = turtle.child_spacing(self.turtle_walks.len());
                turtle.pos.x += margin_size.x + spacing.x;
                turtle.update_used(0.0, margin_size.y);
                turtle.fill_count += 1;
                Some(FillWalk {
                    fill_index,
                    margin: walk.margin,
                    other_axis: walk.height,
                    pos: pos + spacing
                })
            },
            Flow::Down if walk.height.is_fill() => {
                let spacing = turtle.child_spacing(self.turtle_walks.len());
                turtle.pos.y += margin_size.y + spacing.y;
                turtle.update_used(margin_size.x, 0.0);
                turtle.fill_count += 1;
                Some(FillWalk {
                    fill_index,
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
    
    pub fn resolve_fill(&mut self, fill: FillWalk) -> Walk2 {
        let turtle = self.turtles.last().unwrap();
        match turtle.layout.flow {
            Flow::Right => { 
                let left = turtle.width_left();
                let part = left / turtle.fill_count as f32;
                Walk2 {
                    abs_pos: Some(fill.pos + vec2(part * fill.fill_index as f32, 0.)),
                    margin: fill.margin,
                    width: Size2::Fixed(part),
                    height: fill.other_axis
                }
            },
            Flow::Down => { 
                let left = turtle.height_left();
                let part = left / turtle.fill_count as f32;
                Walk2 {
                    abs_pos: Some(fill.pos + vec2(0., part * fill.fill_index as f32)),
                    margin: fill.margin,
                    height: Size2::Fixed(part),
                    width: fill.other_axis
                }
            },
            _ => {
                panic!()
            }
        }
    }
    
    pub fn begin_turtle_with_guard(&mut self, walk: Walk2, layout: Layout2, guard_area: Area) {
        
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
        
        let turtle = Turtle2 {
            walk,
            layout,
            align_start: self.align_list.len(),
            turtle_walks_start: self.turtle_walks.len(),
            fill_count: 0,
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
    
    fn get_turtle_walk_align_end(&self, i: usize) -> usize {
        if i < self.turtle_walks.len() - 1 {
            self.turtle_walks[i + 1].align_start
        }
        else {
            self.align_list.len()
        }
    }
    
    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let turtle = self.turtles.pop().unwrap();
        if guard_area != turtle.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard_area, guard_area)
        }
        
        // computed height
        let w = if turtle.width.is_nan() {
            Size2::Fixed(turtle.width_used + turtle.layout.padding.right)
        }
        else {
            Size2::Fixed(turtle.width)
        };
        
        let h = if turtle.height.is_nan() {
            Size2::Fixed(turtle.height_used + turtle.layout.padding.bottom)
        }
        else {
            Size2::Fixed(turtle.height)
        };
        
        match turtle.layout.flow {
            Flow::Right => { 
                if turtle.fill_count > 0 {
                    let left = turtle.width_left();
                    let part = left / turtle.fill_count as f32;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = walk.fill_index as f32 * part;
                        let shift_y = turtle.layout.align.fy * (turtle.no_pad_height() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.fx * turtle.width_left();
                        let shift_y = turtle.layout.align.fy * (turtle.no_pad_height() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            Flow::Down => { 
                if turtle.fill_count > 0 {
                    let left = turtle.height_left();
                    let part = left / turtle.fill_count as f32;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_y = walk.fill_index as f32 * part;
                        let shift_x = turtle.layout.align.fx * (turtle.no_pad_width() - walk.rect.size.x);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_y = turtle.layout.align.fy * turtle.height_left();
                        let shift_x = turtle.layout.align.fx * (turtle.no_pad_width() - walk.rect.size.x);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            _ => {
                panic!()
            }
        }
        
        self.turtle_walks.truncate(turtle.turtle_walks_start);
        
        if self.turtles.len() == 0 {
            return Rect {
                pos: vec2(0.0, 0.0),
                size: vec2(w.fixed_or_zero(), h.fixed_or_zero())
            }
        }
        return self.walk_turtle_with_align(Walk2 {width: w, height: h, ..turtle.walk}, turtle.align_start)
    }
    
    pub fn walk_turtle(&mut self, walk: Walk2) -> Rect {
        self.walk_turtle_with_align(walk, self.align_list.len())
    }
    
    fn walk_turtle_with_align(&mut self, walk: Walk2, align_start: usize) -> Rect {
        
        let turtle = self.turtles.last_mut().unwrap();
        let size = vec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        
        if let Some(pos) = walk.abs_pos {
            self.turtle_walks.push(TurtleWalk {
                align_start,
                fill_index: 0,
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
                _ => todo!()
            };
            turtle.width_used = turtle.width_used.max(turtle.pos.x - turtle.origin.x);
            turtle.height_used = turtle.height_used.max(turtle.pos.y - turtle.origin.y);
            
            self.turtle_walks.push(TurtleWalk {
                align_start,
                fill_index: turtle.fill_count,
                rect: Rect {pos, size: size + margin_size}
            });
            
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    fn move_align(&mut self, dx: f32, dy: f32, align_start: usize, align_end: usize) {
        let dx = if dx.is_nan(){0.0}else{dx};
        let dy = if dy.is_nan(){0.0}else{dy};
        if dx == 0.0 && dy == 0.0{
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
    
    pub fn turtle(&self) -> &Turtle2 {
        self.turtles.last().unwrap()
    }
    
    pub fn turtle_mut(&mut self) -> &mut Turtle2 {
        self.turtles.last_mut().unwrap()
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

#[derive(Clone, Default, Debug)]
pub struct FillWalk {
    pub fill_index: usize,
    pub margin: Margin2,
    pub other_axis: Size2,
    pub pos: Vec2
}

#[derive(Clone, Default, Debug)]
pub struct TurtleWalk {
    pub align_start: usize,
    pub fill_index: usize,
    pub rect: Rect, 
}

#[derive(Clone, Default, Debug)]
pub struct Turtle2 {
    walk: Walk2,
    layout: Layout2,
    align_start: usize,
    turtle_walks_start: usize,
    fill_count: usize,
    pos: Vec2,
    origin: Vec2,
    width: f32,
    height: f32,
    width_used: f32,
    height_used: f32,
    guard_area: Area
}

impl Turtle2 {
    pub fn child_spacing(&self, walks_len: usize) -> Vec2 {
        if self.turtle_walks_start < walks_len || self.fill_count > 0 {
            match self.layout.flow {
                Flow::Right => {
                    vec2(self.layout.spacing, 0.0)
                }
                Flow::Down => {
                    vec2(0.0, self.layout.spacing)
                }
                _ => todo!()
            }
        }
        else {
            vec2(0.0, 0.0)
        }
    }
    
    pub fn update_used(&mut self, dx: f32, dy: f32) {
        self.width_used = self.width_used.max((self.pos.x + dx) - self.origin.x);
        self.height_used = self.height_used.max((self.pos.y + dy) - self.origin.y);
    }
    
    pub fn rect_is_visible(&self, geom: Rect, scroll: Vec2) -> bool {
        let view = Rect {pos: scroll, size: vec2(self.width, self.height)};
        return view.intersects(geom)
    }
    
    pub fn origin(&self) -> Vec2 {
        self.origin
    }
    
    pub fn move_pos(&mut self, dx: f32, dy: f32) {
        self.pos.x += dx;
        self.pos.y += dy;
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
    
    pub fn set_pos(&mut self, pos: Vec2) {
        self.pos = pos
    }
    
    pub fn eval_width(&self, width: Size2, margin: Margin2, flow: Flow) -> f32 {
        return match width {
            Size2::Fit => std::f32::NAN,
            Size2::Fixed(v) => max_zero_keep_nan(v),
            Size2::Fill => {
                match flow {
                    Flow::Right => {
                        max_zero_keep_nan(self.width_left() - (margin.left + margin.right))
                    },
                    Flow::Down => {
                        max_zero_keep_nan(self.no_pad_width() - (margin.left + margin.right))
                    }
                    _ => todo!()
                }
            },
        }
    }
    
    pub fn eval_height(&self, height: Size2, margin: Margin2, flow: Flow) -> f32 {
        return match height {
            Size2::Fit => std::f32::NAN,
            Size2::Fixed(v) => max_zero_keep_nan(v),
            Size2::Fill => {
                match flow {
                    Flow::Right => {
                        max_zero_keep_nan(self.no_pad_height() - (margin.top + margin.bottom))
                    },
                    Flow::Down => {
                        max_zero_keep_nan(self.height_left() - (margin.top + margin.bottom))
                    }
                    _ => todo!()
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
        let pad_lt = vec2(self.layout.padding.left, self.layout.padding.top);
        let pad_br = vec2(self.layout.padding.right, self.layout.padding.bottom);
        Rect {
            pos: self.origin + pad_lt,
            size: vec2(self.width, self.height) - (pad_lt + pad_br)
        }
    }
    
    pub fn size(&self) -> Vec2 {
        vec2(self.width, self.height)
    }
    
    pub fn width_left(&self) -> f32 {
        return max_zero_keep_nan(self.width - self.width_used - self.layout.padding.right);
    }
    
    pub fn no_pad_width(&self) -> f32 {
        return max_zero_keep_nan(self.width - (self.layout.padding.left + self.layout.padding.right));
    }
    
    pub fn height_left(&self) -> f32 {
        return max_zero_keep_nan(self.height - self.height_used - self.layout.padding.bottom);
    }
    
    pub fn no_pad_height(&self) -> f32 {
        return max_zero_keep_nan(self.height - (self.layout.padding.top + self.layout.padding.bottom));
    }
}
