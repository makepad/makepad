use {
    crate::{
        makepad_platform::*,
        cx_2d::Cx2d,
    }
};

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub struct Layout {
    pub scroll: DVec2,
    pub clip_x: bool,
    pub clip_y: bool,
    pub padding: Padding,
    pub align: Align,
    pub flow: Flow,
    pub spacing: f64
}

impl Default for Layout{
    fn default()->Self{
        Self{
            scroll: dvec2(0.0,0.0),
            clip_x: true,
            clip_y: true,
            padding: Padding::default(),
            align: Align{x:0.0,y:0.0},
            flow: Flow::Down,
            spacing: 0.0
        }
    }
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
#[live_ignore]
pub struct Walk {
    pub abs_pos: Option<DVec2>,
    pub margin: Margin,
    pub width: Size,
    pub height: Size,
}

#[derive(Clone, Copy, Default, Debug, Live, LiveHook)]
#[live_ignore]
pub struct Align {
    pub x: f64,
    pub y: f64
}

#[derive(Clone, Copy, Default, Debug, Live)]
#[live_ignore]
pub struct Padding {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum Axis {
    #[pick] Horizontal,
    Vertical
}

impl Default for Axis {
    fn default() -> Self {
        Axis::Horizontal
    }
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum Flow {
    #[pick] Right,
    Down,
    Overlay
}

#[derive(Copy, Clone, Debug, Live)]
#[live_ignore]
pub enum Size {
    #[pick] Fill,
    #[live(200.0)] Fixed(f64),
    Fit,
}

#[derive(Clone, Default, Debug)]
pub struct DeferWalk {
    defer_index: usize,
    margin: Margin,
    other_axis: Size,
    pos: DVec2
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
    shift: Option<DVec2>,
    pos: DVec2,
    origin: DVec2,
    width: f64,
    height: f64,
    width_used: f64,
    height_used: f64,
    draw_clip: (DVec2, DVec2),
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
        let size = dvec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        let margin_size = walk.margin.size();
        match turtle.layout.flow {
            Flow::Right if walk.width.is_fill() => {
                let spacing = turtle.child_spacing(self.turtle_walks.len());
                turtle.pos.x += margin_size.x + spacing.x;
                turtle.update_width_max(turtle.pos.x, 0.0);
                turtle.update_height_max(turtle.pos.y, size.y + margin_size.y);
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
                turtle.update_width_max(turtle.pos.x, size.x + margin_size.x);
                turtle.update_height_max(turtle.pos.y, 0.0);
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
    
    pub fn begin_overlay_turtle(&mut self, layout: Layout) {
        let pass_size = self.current_pass_size();
        let turtle = Turtle {
            walk: Walk::fill(),
            layout,
            draw_clip: (dvec2(0.0,0.0),pass_size),
            align_start: self.align_list.len(),
            turtle_walks_start: self.turtle_walks.len(),
            defer_count: 0,
            pos: DVec2 {
                x: layout.padding.left,
                y: layout.padding.top
            },
            origin: dvec2(0.0,0.0),
            width: pass_size.x,
            height: pass_size.y,
            shift: None,
            width_used: layout.padding.left,
            height_used: layout.padding.top,
            guard_area: Area::Empty,
        };
        self.turtles.push(turtle);
    }
    
    pub fn end_overlay_turtle(&mut self){
        let turtle = self.turtles.pop().unwrap();
        self.align_list.truncate(turtle.align_start);
    }
    
    pub fn begin_turtle_with_guard(&mut self, walk: Walk, layout: Layout, guard_area: Area) {
        let (origin, width, height, draw_clip) = if let Some(parent) = self.turtles.last() {
            
            let o = walk.margin.left_top() + if let Some(pos) = walk.abs_pos {pos} else {
                parent.pos + parent.child_spacing(self.turtle_walks.len())
            };
            
            let w = parent.eval_width(walk.width, walk.margin, parent.layout.flow);
            let h = parent.eval_height(walk.height, walk.margin, parent.layout.flow);
            
            // figure out new clipping rect
            let (x0, x1) = if layout.clip_x {
                (parent.draw_clip.0.x.max(o.x), if w.is_nan() {
                    parent.draw_clip.1.x
                } else {
                    parent.draw_clip.1.x.min(o.x + w)
                })
            } else {
                (parent.draw_clip.0.x, parent.draw_clip.1.x)
            };
            
            let (y0, y1) = if layout.clip_y {
                (parent.draw_clip.0.y.max(o.y), if h.is_nan() {
                    parent.draw_clip.1.y
                } else {
                    parent.draw_clip.1.y.min(o.y + h)
                })
            }else {(parent.draw_clip.0.y, parent.draw_clip.1.y)};
            
            (o - layout.scroll, w, h, (dvec2(x0, y0), dvec2(x1, y1)))
        }
        else {
            let o = DVec2 {x: walk.margin.left, y: walk.margin.top};
            let w = walk.width.fixed_or_nan();
            let h = walk.height.fixed_or_nan();
            
            (o, w, h, (dvec2(o.x, o.y), dvec2(o.x + w, o.y + h)))
        };
        
        let turtle = Turtle {
            walk,
            layout,
            draw_clip,
            align_start: self.align_list.len(),
            turtle_walks_start: self.turtle_walks.len(),
            defer_count: 0,
            pos: DVec2 {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top
            },
            origin,
            width,
            height,
            shift: None,
            width_used: layout.padding.left,
            height_used: layout.padding.top,
            guard_area,
        };
        
        self.turtles.push(turtle);
    }
    
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }
    
    pub fn end_turtle_with_area(&mut self, area: &mut Area) {
        let rect = self.end_turtle_with_guard(Area::Empty);
        self.add_aligned_rect_area(area, rect, self.turtle().draw_clip())
    }
    
    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let turtle = self.turtles.pop().unwrap();
        if guard_area != turtle.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard_area, guard_area)
        }
        
        // computed height
        let w = if turtle.width.is_nan() {
            Size::Fixed(turtle.width_used + turtle.layout.padding.right - turtle.layout.scroll.x)
        }
        else {
            Size::Fixed(turtle.width)
        };
        
        let h = if turtle.height.is_nan() {
            Size::Fixed(turtle.height_used + turtle.layout.padding.bottom - turtle.layout.scroll.y)
        }
        else {
            Size::Fixed(turtle.height)
        };
        
        match turtle.layout.flow {
            Flow::Right => {
                if turtle.defer_count > 0 {
                    let left = turtle.width_left();
                    let part = left / turtle.defer_count as f64;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = walk.defer_index as f64 * part;
                        let shift_y = turtle.layout.align.y * (turtle.padded_height_or_used() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * turtle.width_left();
                        let shift_y = turtle.layout.align.y * (turtle.padded_height_or_used() - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            Flow::Down => {
                if turtle.defer_count > 0 {
                    let left = turtle.height_left();
                    let part = left / turtle.defer_count as f64;
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * (turtle.padded_width_or_used() - walk.rect.size.x);
                        let shift_y = walk.defer_index as f64 * part;
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
                else {
                    for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = turtle.layout.align.x * (turtle.padded_width_or_used() - walk.rect.size.x);
                        let shift_y = turtle.layout.align.y * turtle.height_left();
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end);
                    }
                }
            },
            Flow::Overlay => {
                for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                    let walk = &self.turtle_walks[i];
                    let shift_x = turtle.layout.align.x * (turtle.padded_width_or_used() - walk.rect.size.x);
                    let shift_y = turtle.layout.align.y * (turtle.padded_height_or_used() - walk.rect.size.y);
                    let align_start = walk.align_start;
                    let align_end = self.get_turtle_walk_align_end(i);
                    self.move_align_list(shift_x, shift_y, align_start, align_end);
                }
            }
        }
        if let Some(shift) = turtle.shift {
            for i in turtle.turtle_walks_start..self.turtle_walks.len() {
                let walk = &self.turtle_walks[i];
                let align_start = walk.align_start;
                let align_end = self.get_turtle_walk_align_end(i);
                self.move_align_list(shift.x, shift.y, align_start, align_end);
            }
        }
        
        self.turtle_walks.truncate(turtle.turtle_walks_start);
        
        if self.turtles.len() == 0 {
            return Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(w.fixed_or_zero(), h.fixed_or_zero())
            }
        }
        let mut rect = self.walk_turtle_internal(Walk {width: w, height: h, ..turtle.walk}, turtle.align_start, true);
        if let Some(shift) = turtle.shift {
            rect.pos += shift;
        }
        rect
    }
    
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_internal(walk, self.align_list.len(), true)
    }
    
    pub fn walk_turtle_with_area(&mut self, area: &mut Area, walk: Walk) -> Rect {
        let rect = self.walk_turtle_internal(walk, self.align_list.len(), true);
        self.add_aligned_rect_area(area, rect, self.turtle().draw_clip());
        rect
    }
    
    pub fn walk_turtle_with_align(&mut self, walk: Walk, align_start: usize) -> Rect {
        self.walk_turtle_internal(walk, align_start, true)
    }
    
    pub fn peek_walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_internal(walk, self.align_list.len(), false)
    }
    
    pub fn walk_turtle_would_be_visible(&mut self, walk: Walk) -> bool {
        let rect = self.walk_turtle_internal(walk, self.align_list.len(), false);
        self.turtle().rect_is_visible(rect)
    }
    
    pub fn peek_walk_pos(&mut self, walk: Walk) -> DVec2 {
        if let Some(pos) = walk.abs_pos {
            pos + walk.margin.left_top()
        }
        else {
            let turtle = self.turtles.last_mut().unwrap();
            turtle.pos + walk.margin.left_top()
        }
    }
    
    fn walk_turtle_internal(&mut self, walk: Walk, align_start: usize, actually_move: bool) -> Rect {
        
        let turtle = self.turtles.last_mut().unwrap();
        let size = dvec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        
        if let Some(pos) = walk.abs_pos {
            if actually_move {
                self.turtle_walks.push(TurtleWalk {
                    align_start,
                    defer_index: 0,
                    rect: Rect {pos, size: size + walk.margin.size()}
                });
                
                match turtle.layout.flow {
                    Flow::Right=>turtle.update_height_max(pos.y, size.y + walk.margin.size().y),
                    Flow::Down=>turtle.update_width_max(pos.x, size.x + walk.margin.size().x),
                    _=>()
                }
            }
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.child_spacing(self.turtle_walks.len());
            let pos = turtle.pos;
            if actually_move {
                let margin_size = walk.margin.size();
                match turtle.layout.flow {
                    Flow::Right => {
                        turtle.pos.x = pos.x + size.x + margin_size.x + spacing.x;
                        if size.x < 0.0 {
                            turtle.update_width_min(turtle.pos.x, 0.0);
                            turtle.update_height_max(turtle.pos.y,size.y + margin_size.y);
                        }
                        else {
                            turtle.update_width_max(turtle.pos.x, 0.0);
                            turtle.update_height_max(turtle.pos.y,size.y + margin_size.y);
                        }
                    },
                    Flow::Down => {
                        turtle.pos.y = pos.y + size.y + margin_size.y + spacing.y;
                        if size.y < 0.0 {
                            turtle.update_width_max(turtle.pos.x, size.x + margin_size.x);
                            turtle.update_height_min(turtle.pos.y,0.0);
                        }
                        else {
                            turtle.update_width_max(turtle.pos.x, size.x + margin_size.x);
                            turtle.update_height_max(turtle.pos.y,0.0);
                        }
                    },
                    Flow::Overlay => { // do not walk
                        turtle.update_width_max(turtle.pos.x, size.x);
                        turtle.update_height_max(turtle.pos.y,size.y);
                    }
                };
                
                self.turtle_walks.push(TurtleWalk {
                    align_start,
                    defer_index: turtle.defer_count,
                    rect: Rect {pos, size: size + margin_size}
                });
            }
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    fn move_align_list(&mut self, dx: f64, dy: f64, align_start: usize, align_end: usize) {
        let dx = if dx.is_nan() {0.0}else {dx};
        let dy = if dy.is_nan() {0.0}else {dy};
        if dx == 0.0 && dy == 0.0 {
            return
        }
        let dx = (dx * self.current_dpi_factor).floor() / self.current_dpi_factor;
        let dy = (dy * self.current_dpi_factor).floor() / self.current_dpi_factor;
        let d = dvec2(dx, dy);
        for i in align_start..align_end {
            let align_item = &self.align_list[i];
            match align_item {
                Area::Instance(inst) => {
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    let inst_buf = draw_item.instances.as_mut().unwrap();
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            inst_buf[inst.instance_offset + rect_pos + 0 + i * sh.mapping.instances.total_slots] += dx as f32;
                            inst_buf[inst.instance_offset + rect_pos + 1 + i * sh.mapping.instances.total_slots] += dy as f32;
                            if let Some(draw_clip) = sh.mapping.draw_clip {
                                inst_buf[inst.instance_offset + draw_clip + 0 + i * sh.mapping.instances.total_slots] += dx as f32;
                                inst_buf[inst.instance_offset + draw_clip + 1 + i * sh.mapping.instances.total_slots] += dy as f32;
                                inst_buf[inst.instance_offset + draw_clip + 2 + i * sh.mapping.instances.total_slots] += dx as f32;
                                inst_buf[inst.instance_offset + draw_clip + 3 + i * sh.mapping.instances.total_slots] += dy as f32;
                            }
                        }
                    }
                },
                Area::Rect(ra) => {
                    let draw_list = &mut self.cx.draw_lists[ra.draw_list_id];
                    let rect_area = &mut draw_list.rect_areas[ra.rect_id];
                    rect_area.rect.pos += d;
                    rect_area.draw_clip.0 += d;
                    rect_area.draw_clip.1 += d;
                }
                
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
    
    pub fn add_rect_area(&mut self, area: &mut Area, rect: Rect) {
        let turtle = self.turtle();
        self.add_aligned_rect_area(area, rect, turtle.draw_clip())
    }
}

impl Turtle {
    pub fn draw_clip(&self) -> (DVec2, DVec2) {
        self.draw_clip
    }
    
    pub fn update_width_max(&mut self, pos:f64, dx: f64) {
        self.width_used = self.width_used.max((pos + dx) - self.origin.x);
    }
    
    pub fn update_height_max(&mut self, pos:f64, dy: f64) {
        self.height_used = self.height_used.max((pos + dy) - self.origin.y);
    }
    
    pub fn update_width_min(&mut self, pos:f64, dx: f64) {
        self.width_used = self.width_used.min((pos + dx) - self.origin.x);
    }
    
    pub fn update_height_min(&mut self, pos:f64, dy: f64) {
        self.height_used = self.height_used.min((pos + dy) - self.origin.y);
    }
    
    pub fn set_shift(&mut self, shift: DVec2) {
        self.shift = Some(shift);
    }
    
    pub fn used(&self) -> DVec2 {
        dvec2(self.width_used, self.height_used)
    }
    
    pub fn set_used(&mut self, width_used: f64, height_used: f64) {
        self.width_used = width_used;
        self.height_used = height_used;
    }
    /*
    pub fn move_pos(&mut self, dx: f64, dy: f64) {
        self.pos.x += dx;
        self.pos.y += dy;
        self.update_width_max(0.0);
        self.update_height_max(0.0);
    }
    */
    pub fn set_pos(&mut self, pos: DVec2) {
        self.pos = pos
    }
    
    fn child_spacing(&self, walks_len: usize) -> DVec2 {
        if self.turtle_walks_start < walks_len || self.defer_count > 0 {
            match self.layout.flow {
                Flow::Right => {
                    dvec2(self.layout.spacing, 0.0)
                }
                Flow::Down => {
                    dvec2(0.0, self.layout.spacing)
                }
                Flow::Overlay => {
                    dvec2(0.0, 0.0)
                }
            }
        }
        else {
            dvec2(0.0, 0.0)
        }
    }
    
    pub fn rect_is_visible(&self, geom: Rect) -> bool {
        let view = Rect {pos: self.draw_clip.0, size: self.draw_clip.1 - self.draw_clip.0};
        return view.intersects(geom)
    }
    
    pub fn origin(&self) -> DVec2 {
        self.origin
    }
    
    pub fn rel_pos(&self) -> DVec2 {
        DVec2 {
            x: self.pos.x - self.origin.x,
            y: self.pos.y - self.origin.y
        }
    }
    
    pub fn pos(&self) -> DVec2 {
        self.pos
    }
    
    pub fn scroll(&self) -> DVec2 {
        self.layout.scroll
    }
    
    pub fn eval_width(&self, width: Size, margin: Margin, flow: Flow) -> f64 {
        return match width {
            Size::Fit => std::f64::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::Right => {
                        max_zero_keep_nan(self.width_left() - margin.width())
                    },
                    Flow::Down | Flow::Overlay => {
                        let r = max_zero_keep_nan(self.width - self.layout.padding.width() - margin.width());
                        if r.is_nan() {
                            return self.width_used - margin.width() - self.layout.padding.right
                        }
                        return r
                    }
                }
            },
        }
    }
    
    pub fn eval_height(&self, height: Size, margin: Margin, flow: Flow) -> f64 {
        return match height {
            Size::Fit => std::f64::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::Right | Flow::Overlay => {
                        let r = max_zero_keep_nan(self.height - self.layout.padding.height() - margin.height());
                        if r.is_nan() {
                            return self.height_used - margin.height() - self.layout.padding.bottom
                        }
                        return r
                    }
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
            size: dvec2(self.width, self.height)
        }
    }
    pub fn padded_rect_used(&self) -> Rect {
        Rect {
            pos: self.origin + self.layout.padding.left_top(),
            size: self.used() - self.layout.padding.left_top()
        }
    }
    pub fn rect_left(&self) -> Rect {
        Rect {
            pos: self.pos,
            size: dvec2(self.width_left(), self.height_left())
        }
    }
    
    pub fn padded_rect(&self) -> Rect {
        Rect {
            pos: self.origin + self.layout.padding.left_top(),
            size: dvec2(self.width, self.height) - self.layout.padding.size()
        }
    }
    
    pub fn size(&self) -> DVec2 {
        dvec2(self.width, self.height)
    }
    
    pub fn width_left(&self) -> f64 {
        return max_zero_keep_nan(self.width - self.width_used - self.layout.padding.right);
    }
    
    pub fn height_left(&self) -> f64 {
        return max_zero_keep_nan(self.height - self.height_used - self.layout.padding.bottom);
    }
    
    pub fn padded_height_or_used(&self) -> f64 {
        let r = max_zero_keep_nan(self.height - self.layout.padding.height());
        if r.is_nan() {
            self.height_used - self.layout.padding.bottom
        }
        else {
            r
        }
    }
    
    pub fn padded_width_or_used(&self) -> f64 {
        let r = max_zero_keep_nan(self.width - self.layout.padding.width());
        if r.is_nan() {
            self.width_used - self.layout.padding.right
        }
        else {
            r
        }
    }
}

impl DeferWalk {
    pub fn resolve(&self, cx: &Cx2d) -> Walk {
        let turtle = cx.turtles.last().unwrap();
        match turtle.layout.flow {
            Flow::Right => {
                let left = turtle.width_left();
                let part = left / turtle.defer_count as f64;
                Walk {
                    abs_pos: Some(self.pos + dvec2(part * self.defer_index as f64, 0.)),
                    margin: self.margin,
                    width: Size::Fixed(part),
                    height: self.other_axis
                }
            },
            Flow::Down => {
                let left = turtle.height_left();
                let part = left / turtle.defer_count as f64;
                Walk {
                    abs_pos: Some(self.pos + dvec2(0., part * self.defer_index as f64)),
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
    
    pub fn flow_down() -> Self {
        Self {
            flow: Flow::Down,
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
    fn before_apply(&mut self, _cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
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
    fn before_apply(&mut self, cx: &mut Cx, _apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Expr {..} => {
                match live_eval(&cx.live_registry.clone().borrow(), index, &mut (index + 1), nodes) {
                    Ok(ret) => match ret {
                        LiveEval::Float64(v) => {
                            *self = Self::Fixed(v);
                        }
                        LiveEval::Int64(v) => {
                            *self = Self::Fixed(v as f64);
                        }
                        _ => {
                            cx.apply_error_wrong_expression_type_for_primitive(live_error_origin!(), index, nodes, "bool", ret);
                        }
                    }
                    Err(err) => cx.apply_error_eval(err)
                }
                Some(nodes.skip_node(index))
            }
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
            Self::Fill => true,
            _ => false
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


