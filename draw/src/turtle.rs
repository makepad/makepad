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
    #[live(200.0)] Fixed(f64),
    Fit,
    All
}

#[derive(Clone, Debug)]
pub enum DeferWalk{
    Unresolved{
        defer_index: usize,
        margin: Margin,
        other_axis: Size,
        pos: DVec2
    },
    Resolved(Walk)
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
pub struct TurtleWalk {
    align_start: usize,
    defer_index: usize,
    rect: Rect,
}

#[derive(Clone, Default, Debug)]
pub struct Turtle {
    walk: Walk,
    layout: Layout,
    wrap_spacing: f64,
    align_start: usize,
    turtle_walks_start: usize,
    defer_count: usize,
    shift: DVec2,
    pos: DVec2,
    origin: DVec2,
    width: f64,
    height: f64,
    width_used: f64,
    height_used: f64,
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
        if walk.abs_pos.is_some(){
            return None
        }
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
                Some(DeferWalk::Unresolved{
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
                Some(DeferWalk::Unresolved {
                    defer_index,
                    margin: walk.margin,
                    other_axis: walk.width,
                    pos: pos + spacing
                })
            },
            Flow::RightWrap if walk.width.is_fill() => {
                error!("flow RightWrap does not support fill childnodes");
                None
            },
            _ => {
                None
            }
        }
    }
    
    pub fn begin_pass_sized_turtle_no_clip(&mut self, layout: Layout) {
        self.begin_pass_sized_turtle(layout);
        *self.align_list.last_mut().unwrap() = AlignEntry::Unset;
    }
            
    pub fn begin_pass_sized_turtle(&mut self, layout: Layout) {
        let pass_size = self.current_pass_size();
        self.align_list.push(AlignEntry::BeginTurtle(dvec2(0.0,0.0),pass_size));
        let turtle = Turtle {
            walk: Walk::fill(),
            layout,
            align_start: self.align_list.len() - 1,
            turtle_walks_start: self.turtle_walks.len(),
            defer_count: 0,
            pos: DVec2 {
                x: layout.padding.left,
                y: layout.padding.top
            },
            wrap_spacing: 0.0,
            origin: dvec2(0.0, 0.0),
            width: pass_size.x,
            height: pass_size.y,
            shift: dvec2(0.0, 0.0),
            width_used: layout.padding.left,
            height_used: layout.padding.top,
            guard_area: Area::Empty,
        };
        self.turtles.push(turtle);
    }
    
    pub fn end_pass_sized_turtle_no_clip(&mut self) {
        let turtle = self.turtles.pop().unwrap();
                
        self.perform_nested_clipping_on_align_list_and_shift(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle{skip:self.align_list.len()};
        self.turtle_walks.truncate(turtle.turtle_walks_start);
    }
    
    pub fn end_pass_sized_turtle(&mut self){
        let turtle = self.turtles.pop().unwrap();
        // lets perform clipping on our alignlist.
        self.align_list.push(AlignEntry::EndTurtle);
        
        self.perform_nested_clipping_on_align_list_and_shift(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle{skip:self.align_list.len()};
        self.turtle_walks.truncate(turtle.turtle_walks_start);
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
        self.turtle_walks.truncate(turtle.turtle_walks_start);
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
            turtle_walks_start: self.turtle_walks.len(),
            defer_count: 0,
            wrap_spacing: 0.0,
            pos: DVec2 {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top
            },
            origin,
            width,
            height,
            shift: dvec2(0.0,0.0),
            width_used: layout.padding.left,
            height_used: layout.padding.top,
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
    
    pub fn end_turtle_with_area(&mut self, area: &mut Area) {
        let rect = self.end_turtle_with_guard(Area::Empty);
        self.add_aligned_rect_area(area, rect)
    }
    
    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let turtle = self.turtles.last().unwrap();
        if guard_area != turtle.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", turtle.guard_area, guard_area)
        }
        
        let turtle_align_start = turtle.align_start;
        let turtle_abs_pos = turtle.walk.abs_pos;
        let turtle_margin = turtle.walk.margin;
        let turtle_walks_start = turtle.turtle_walks_start;
        let turtle_shift = turtle.shift;
                
        // computed width / height
        let w = if turtle.width.is_nan() {
            let w = turtle.width_used + turtle.layout.padding.right - turtle.layout.scroll.x;
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
            let h =  turtle.height_used + turtle.layout.padding.bottom - turtle.layout.scroll.y;
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
                if turtle.defer_count > 0 {
                    let left = turtle.width_left();
                    let part = left / turtle.defer_count as f64;
                    let align_y = turtle.layout.align.y;
                    let padded_height_or_used = turtle.padded_height_or_used();
                    for i in turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = walk.defer_index as f64 * part;
                        let shift_y = align_y * (padded_height_or_used - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                    }
                }
                else {
                    let align_x = turtle.layout.align.x;
                    let align_y = turtle.layout.align.y;
                    let width_left = turtle.width_left();
                    let padded_height_or_used = turtle.padded_height_or_used();
                    if align_x != 0.0 || align_y != 0.0{
                        for i in turtle_walks_start..self.turtle_walks.len() {
                            let walk = &self.turtle_walks[i];
                            let shift_x = align_x * width_left;
                            let shift_y = align_y * (padded_height_or_used - walk.rect.size.y);
                            let align_start = walk.align_start;
                            let align_end = self.get_turtle_walk_align_end(i);
                            self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                        }
                    }
                }
            },
            Flow::RightWrap=>{
                if turtle.defer_count > 0{panic!()}
                // for now we only support align:0,0
            }
            Flow::Down => {
                if turtle.defer_count > 0 {
                    let left = turtle.height_left();
                    let part = left / turtle.defer_count as f64;
                    let padded_width_or_used = turtle.padded_width_or_used();
                    let align_x = turtle.layout.align.x;
                    for i in turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = align_x * (padded_width_or_used- walk.rect.size.x);
                        let shift_y = walk.defer_index as f64 * part;
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                    }
                }
                else {
                    let align_x = turtle.layout.align.x;
                    let align_y = turtle.layout.align.y;
                    let padded_width_or_used = turtle.padded_width_or_used();
                    let height_left = turtle.height_left();
                    if align_x != 0.0 || align_y != 0.0{
                        for i in turtle_walks_start..self.turtle_walks.len() {
                            let walk = &self.turtle_walks[i];
                            let shift_x = align_x * (padded_width_or_used - walk.rect.size.x);
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
                    let padded_width_or_used = turtle.padded_width_or_used();
                    let padded_height_or_used = turtle.padded_height_or_used();
                    for i in turtle_walks_start..self.turtle_walks.len() {
                        let walk = &self.turtle_walks[i];
                        let shift_x = align_x * (padded_width_or_used - walk.rect.size.x);
                        let shift_y = align_y * (padded_height_or_used - walk.rect.size.y);
                        let align_start = walk.align_start;
                        let align_end = self.get_turtle_walk_align_end(i);
                        self.move_align_list(shift_x, shift_y, align_start, align_end, false, turtle_shift);
                    }
                }
            }
        }
        self.turtles.pop();
        self.turtle_walks.truncate(turtle_walks_start);
        self.align_list.push(AlignEntry::EndTurtle);
        if self.turtles.len() == 0 {
            return Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(w.fixed_or_zero(), h.fixed_or_zero())
            }
        }
        let rect = self.walk_turtle_move(Walk {width: w, height: h, abs_pos:turtle_abs_pos, margin:turtle_margin}, turtle_align_start);
        rect
    }
    
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_move(walk, self.align_list.len())
    }
    
    pub fn set_turtle_wrap_spacing(&mut self, spacing: f64){
        self.turtle_mut().wrap_spacing = spacing;
    }

    pub fn walk_turtle_with_area(&mut self, area: &mut Area, walk: Walk) -> Rect {
        let rect = self.walk_turtle_move(walk, self.align_list.len());
        self.add_aligned_rect_area(area, rect);
        rect
    }
    
    pub fn walk_turtle_with_align(&mut self, walk: Walk, align_start: usize) -> Rect {
        self.walk_turtle_move(walk, align_start)
    }
    
    pub fn peek_walk_turtle(&self, walk: Walk) -> Rect {
        self.walk_turtle_peek(walk)
    }
    
    pub fn walk_turtle_would_be_visible(&mut self, walk: Walk) -> bool {
        let rect = self.walk_turtle_peek(walk);
        self.turtle().rect_is_visible(rect)
    }
       
    
    pub fn walk_turtle_would_be_visible2(&mut self, walk: Walk) -> bool {
        let rect = self.walk_turtle_peek(walk);
        let t = self.turtle();
        let view = Rect {pos: t.origin + t.layout.scroll, size: dvec2(t.width, t.height)};
        self.debug.rect(view, vec4(1.0,1.0,0.0,1.0));
        self.debug.rect(rect, vec4(0.0,1.0,0.0,1.0));
        return view.intersects(rect)
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
    
     fn walk_turtle_move(&mut self, walk: Walk, align_start: usize) -> Rect {
        
        let turtle = self.turtles.last_mut().unwrap();
        let size = dvec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        
        if let Some(pos) = walk.abs_pos {
            self.turtle_walks.push(TurtleWalk {
                align_start,
                defer_index: 0,
                rect: Rect {pos, size: size + walk.margin.size()}
            });
            
            match turtle.layout.flow {
                Flow::Right=>turtle.update_height_max(pos.y, size.y + walk.margin.size().y),
                Flow::Down=>turtle.update_width_max(pos.x, size.x + walk.margin.size().x),
                Flow::Overlay => { // do not walk
                    turtle.update_width_max(pos.x, size.x);
                    turtle.update_height_max(pos.y,size.y);
                }
                Flow::RightWrap=>{
                    panic!("Cannot use abs_pos in a flow::Rightwrap");
                }
            }
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.child_spacing(self.turtle_walks.len());
            let mut pos = turtle.pos;
            let margin_size = walk.margin.size();
            let defer_index = turtle.defer_count;
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
                Flow::RightWrap => {
                    if turtle.pos.x - turtle.origin.x + size.x > turtle.width - turtle.layout.padding.right{
                        
                        pos.x =  turtle.origin.x + turtle.layout.padding.left;
                        let dx = pos.x - turtle.pos.x;                        
                        turtle.pos.x = pos.x + size.x + margin_size.x + spacing.x;
                        
                        pos.y = turtle.height_used + turtle.origin.y + turtle.wrap_spacing;//turtle.layout.line_spacing;
                        let dy = pos.y - turtle.pos.y;
                        turtle.pos.y = pos.y;
                        
                        turtle.update_height_max(turtle.pos.y,size.y + margin_size.y);
                                                
                        if align_start != self.align_list.len(){
                            self.move_align_list(dx, dy, align_start, self.align_list.len(), false, dvec2(0.0,0.0));
                        }
                    }
                    else{
                        turtle.pos.x = pos.x + size.x + margin_size.x + spacing.x;
                        if size.x < 0.0 {
                            turtle.update_width_min(turtle.pos.x, 0.0);
                            turtle.update_height_max(turtle.pos.y,size.y + margin_size.y); 
                        }
                        else {
                            turtle.update_width_max(turtle.pos.x, 0.0);
                            turtle.update_height_max(turtle.pos.y,size.y + margin_size.y);
                        }
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
                defer_index,
                rect: Rect {pos, size: size + margin_size}
            });
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    fn walk_turtle_peek(&self, walk: Walk) -> Rect {
        if self.turtles.len() == 0{
            return Rect::default()
        }
        let turtle = self.turtles.last().unwrap();
        let size = dvec2(
            turtle.eval_width(walk.width, walk.margin, turtle.layout.flow),
            turtle.eval_height(walk.height, walk.margin, turtle.layout.flow)
        );
        
        if let Some(pos) = walk.abs_pos {
            Rect {pos: pos + walk.margin.left_top(), size}
        }
        else {
            let spacing = turtle.child_spacing(self.turtle_walks.len());
            let pos = turtle.pos;
            Rect {pos: pos + walk.margin.left_top() + spacing, size}
        }
    }
    
    pub fn turtle_new_line(&mut self){
        let turtle = self.turtles.last_mut().unwrap();
        turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
        let next_y = turtle.height_used + turtle.origin.y + turtle.wrap_spacing;
        if turtle.pos.y == next_y{
            turtle.pos.y += turtle.wrap_spacing;
        }
        else{
            turtle.pos.y = next_y;
        }
    }

    pub fn turtle_new_line_with_spacing(&mut self, spacing: f64){
        let turtle = self.turtles.last_mut().unwrap();
        turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
        let next_y = turtle.height_used + turtle.origin.y + spacing;
        if turtle.pos.y == next_y{
            turtle.pos.y += spacing;
        }
        else{
            turtle.pos.y = next_y;
        }
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
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
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
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
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
        if i < self.turtle_walks.len() - 1 {
            self.turtle_walks[i + 1].align_start
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
        self.shift = shift;
    }
    
    pub fn layout(&self)->&Layout{
        &self.layout
    }

    pub fn layout_mut(&mut self)-> &mut Layout {
        &mut self.layout
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
                Flow::RightWrap=>{
                    dvec2(self.layout.spacing, 0.0)
                }
            }
        }
        else {
            dvec2(0.0, 0.0)
        }
    }
        
    pub fn rect_is_visible(&self,  geom: Rect) -> bool {
        let view = Rect {pos: self.origin + self.layout.scroll, size: dvec2(self.width, self.height)};
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
    
    pub fn eval_width(&self, width: Size, margin: Margin, flow: Flow) -> f64 {
        return match width {
            Size::Fit => std::f64::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::RightWrap=> {
                        max_zero_keep_nan(self.width - (self.pos.x - self.origin.x) - margin.width() -self.layout.padding.right)
                    }
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
            Size::All=>self.width
        }
    }
    
    pub fn eval_height(&self, height: Size, margin: Margin, flow: Flow) -> f64 {
        return match height {
            Size::Fit => std::f64::NAN,
            Size::Fixed(v) => max_zero_keep_nan(v),
            Size::Fill => {
                match flow {
                    Flow::RightWrap | Flow::Right | Flow::Overlay => {
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
            Size::All=>self.height
        }
    }
    
    pub fn rect(&self) -> Rect {
        Rect {
            pos: self.origin,
            size: dvec2(self.width, self.height)
        }
    }
    
   pub fn unscrolled_rect(&self) -> Rect {
        Rect {
            pos: self.origin + self.layout.scroll,
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
    
    pub fn resolve(&mut self, cx: &Cx2d) -> Walk {
        match self{
            Self::Resolved(walk)=>{*walk},
            Self::Unresolved{pos, defer_index, margin, other_axis}=>{
                let turtle = cx.turtles.last().unwrap();
                let walk = match turtle.layout.flow {
                    Flow::Right => {
                        let left = turtle.width_left();
                        let part = left / turtle.defer_count as f64;
                        Walk {
                            abs_pos: Some(*pos + dvec2(part * *defer_index as f64, 0.)),
                            margin: *margin,
                            width: Size::Fixed(part),
                            height: *other_axis
                        }
                    },
                    Flow::RightWrap => {
                        panic!()
                    }
                    Flow::Down => { 
                        let left = turtle.height_left();
                        let part = left / turtle.defer_count as f64;
                        Walk {
                            abs_pos: Some(*pos + dvec2(0., part * *defer_index as f64)),
                            margin: *margin,
                            height: Size::Fixed(part),
                            width: *other_axis
                        }
                    }
                    Flow::Overlay => panic!()
                };
                *self = DeferWalk::Resolved(walk);
                walk
            }
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
