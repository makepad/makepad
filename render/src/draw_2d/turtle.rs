pub use {
    std::{
        any::TypeId,
    },
    crate::{
        makepad_derive_live::*,
        area::Area,
        live_traits::*,
        draw_2d::cx_2d::Cx2d,
        cx::Cx,
    }
};

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum LineWrap {
    #[pick] None,
    NewLine,
    #[live(8.0)] MaxSize(f32)
}

impl Default for LineWrap {
    fn default() -> Self {
        LineWrap::None
    }
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Layout {
    pub padding: Padding,
    pub align: Align,
    pub direction: Direction,
    pub line_wrap: LineWrap,
    pub new_line_padding: f32,
    pub abs_origin: Option<Vec2>,
    pub abs_size: Option<Vec2>,
    pub walk: Walk,
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Walk {
    pub margin: Margin,
    pub width: Width,
    pub height: Height,
}

impl Walk{
    pub fn wh(w:Width, h:Height)->Self{
        Self{
            margin:Margin::default(),
            width:w,
            height:h,
        }
    } 
    
    pub fn fixed(w:f32, h:f32)->Self{
        Self{
            margin:Margin::default(),
            width:Width::Fixed(w),
            height:Height::Fixed(h),
        }
    }
}

#[derive(Clone, Copy, Default, Debug, Live, LiveHook)]
pub struct Align {
    pub fx: f32,
    pub fy: f32
}

#[derive(Clone, Copy, Default, Debug, Live, LiveHook)]
pub struct Margin {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

#[derive(Clone, Copy, Default, Debug, Live, LiveHook)]
pub struct Padding {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum Direction {
    Left,
    #[pick] Right,
    Up,
    Down
}

impl Default for Direction {
    fn default() -> Self {Self::Right}
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
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
pub enum Width {
    #[pick] Filled,
    #[live(200.0)] Fixed(f32),
    Computed,
    /*
    #[live] ComputeFill,
    #[live(0.0)] FillPad(f32),
    #[live(0.0)] FillScale(f32),
    #[live(0.0, 0.0)] FillScalePad(f32, f32),
    #[live(0.0)] Scale(f32),
    #[live(0.0, 0.0)] ScalePad(f32, f32),
    */
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
pub enum Height {
    #[pick] Filled,
    #[live(200.0)] Fixed(f32),
    Computed,
    /*
    #[live] ComputeFill,
    #[live(0.0)] FillPad(f32),
    #[live(0.0)] FillScale(f32),
    #[live(0.0, 0.0)] FillScalePad(f32, f32),
    #[live(0.0)] Scale(f32),
    #[live(0.0, 0.0)] ScalePad(f32, f32),
    */
}

impl Default for Width {
    fn default() -> Self {
        Width::Filled
    }
}


impl Default for Height {
    fn default() -> Self {
        Height::Filled
    }
}


impl Width {
    pub fn fixed(&self) -> f32 {
        match self {
            Width::Fixed(v) => *v,
            _ => 0.
        }
    }
    
}

impl Height {
    pub fn fixed(&self) -> f32 {
        match self {
            Height::Fixed(v) => *v,
            _ => 0.
        }
    }
}


impl<'a> Cx2d<'a> {
    //pub fn debug_pt(&self, x:f32, y:f32, color:i32){
    //self.debug_pts.borrow_mut().push((x,y,color));
    //}
    
    pub fn set_count_of_aligned_instance(&mut self, instance_count: usize) -> Area {
        let mut area = self.align_list.last_mut().unwrap();
        if let Area::Instance(inst) = &mut area {
            inst.instance_count = instance_count;
        }
        area.clone()
    }
    
    // begin a new turtle with a layout
    pub fn begin_turtle(&mut self, layout: Layout) {
        self.begin_turtle_with_guard(layout, Area::Empty)
    }

    pub fn begin_turtle_with_guard(&mut self, layout: Layout, guard_area: Area) {
        
        // fetch origin and size from parent
        let (mut origin, mut abs_size) = if let Some(parent) = self.turtles.last() {
            (Vec2 {x: layout.walk.margin.left + parent.pos.x, y: layout.walk.margin.top + parent.pos.y}, parent.abs_size)
        }
        else {
            (Vec2 {x: layout.walk.margin.left, y: layout.walk.margin.top}, Vec2::default())
        };
        
        // see if layout overrode size
        if let Some(layout_abs_size) = layout.abs_size {
            abs_size = layout_abs_size;
        }
        
        // same for origin
        let is_abs_origin;
        if let Some(abs_origin) = layout.abs_origin {
            origin = abs_origin;
            is_abs_origin = true;
        }
        else {
            is_abs_origin = false;
        }
        
        // abs origin overrides the computation of width/height to use the parent abs_origin
        let (width, min_width) = self.eval_width(&layout.walk.width, layout.walk.margin, is_abs_origin, abs_size.x);
        let (height, min_height) = self.eval_height(&layout.walk.height, layout.walk.margin, is_abs_origin, abs_size.y);
        
        let turtle = Turtle {
            align_list_x: self.align_list.len(),
            align_list_y: self.align_list.len(),
            origin: origin,
            pos: Vec2 {x: origin.x + layout.padding.left, y: origin.y + layout.padding.top},
            layout: layout,
            biggest: 0.0,
            bound_left_top: Vec2 {x: std::f32::INFINITY, y: std::f32::INFINITY},
            bound_right_bottom: Vec2 {x: std::f32::NEG_INFINITY, y: std::f32::NEG_INFINITY},
            width: width,
            height: height,
            min_width: min_width,
            min_height: min_height,
            width_used: 0.,
            height_used: 0.,
            abs_size: abs_size,
            guard_area: guard_area,
            //..Default::default()
        };
        
        self.turtles.push(turtle);
    }
    
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }

    pub fn end_turtle_with_guard(&mut self, guard_area: Area) -> Rect {
        let old = self.turtles.pop().unwrap();
        if guard_area != old.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", old.guard_area, guard_area)
        }
        
        let w = if old.width.is_nan() {
            if old.bound_right_bottom.x == std::f32::NEG_INFINITY { // nothing happened, use padding
                Width::Fixed(old.layout.padding.left + old.layout.padding.right)
            }
            else { // use the bounding box
                Width::Fixed(max_zero_keep_nan(old.bound_right_bottom.x - old.origin.x + old.layout.padding.right).max(old.min_width))
            }
        }
        else {
            Width::Fixed(old.width)
        };
        
        let h = if old.height.is_nan() {
            if old.bound_right_bottom.y == std::f32::NEG_INFINITY { // nothing happened use the padding
                Height::Fixed(old.layout.padding.top + old.layout.padding.bottom)
            }
            else { // use the bounding box
                Height::Fixed(max_zero_keep_nan(old.bound_right_bottom.y - old.origin.y + old.layout.padding.bottom).max(old.min_height))
            }
        }
        else {
            Height::Fixed(old.height)
        };
        
        let margin = old.layout.walk.margin.clone();
        //let align_after = old.layout.walk.align_after;
        // if we have alignment set, we should now align our childnodes
        let dx = Self::compute_align_turtle_x(&old);
        if dx > 0.0 {
            self.do_align_x(dx, old.align_list_x);
        }
        let dy = Self::compute_align_turtle_y(&old);
        if dy > 0.0 {
            self.do_align_y(dy, old.align_list_y);
        }
        
        // when a turtle is x-abs / y-abs you dont walk the parent
        if !old.layout.abs_origin.is_none() {
            let abs_origin = if let Some(abs_origin) = old.layout.abs_origin {abs_origin} else {Vec2::default()};
            let w = if let Width::Fixed(vw) = w {vw} else {0.};
            let h = if let Height::Fixed(vh) = h {vh} else {0.};
            return Rect {pos: abs_origin, size: vec2(w, h)};
        }
        
        return self.walk_turtle_with_old(Walk {width: w, height: h, margin}, Some(&old))
    }
    
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_with_old(walk, None)
    }
    
    // walk the turtle with a 'w/h' and a margin
    pub fn walk_turtle_with_old(&mut self, walk: Walk, old_turtle: Option<&Turtle>) -> Rect {
        let mut align_dx = 0.0;
        let mut align_dy = 0.0;
        let (w, _mw) = self.eval_width(&walk.width, walk.margin, false, 0.0);
        let (h, _mh) = self.eval_height(&walk.height, walk.margin, false, 0.0);
        
        let ret = if let Some(turtle) = self.turtles.last_mut() {
            let (x, y) = match turtle.layout.direction {
                Direction::Right => {
                    match turtle.layout.line_wrap {
                        LineWrap::NewLine => {
                            if (turtle.pos.x + walk.margin.left + w) >
                            (turtle.origin.x + turtle.width - turtle.layout.padding.right) + 0.01 {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
                                turtle.pos.y += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::MaxSize(max_size) => {
                            let new_size = turtle.pos.x + walk.margin.left + w;
                            if new_size > (turtle.origin.x + turtle.width - turtle.layout.padding.right)
                                || new_size > (turtle.origin.x + max_size - turtle.layout.padding.right) {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
                                turtle.pos.y += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::None => {
                        }
                    }
                    
                    let x = turtle.pos.x + walk.margin.left;
                    let y = turtle.pos.y + walk.margin.top;
                    // walk it normally
                    turtle.pos.x += w + walk.margin.left + walk.margin.right;
                    
                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = h + walk.margin.top + walk.margin.bottom;
                    if biggest > turtle.biggest {
                        turtle.biggest = biggest;
                    }
                    (x, y)
                },
                Direction::Down => {
                    match turtle.layout.line_wrap {
                        LineWrap::NewLine => {
                            if (turtle.pos.y + walk.margin.top + h) >
                            (turtle.origin.y + turtle.height - turtle.layout.padding.bottom) + 0.01 {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.y = turtle.origin.y + turtle.layout.padding.top;
                                turtle.pos.x += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::MaxSize(max_size) => {
                            let new_size = turtle.pos.y + walk.margin.top + h;
                            if new_size > (turtle.origin.y + turtle.height - turtle.layout.padding.bottom)
                                || new_size > (turtle.origin.y + max_size - turtle.layout.padding.bottom) {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.y = turtle.origin.y + turtle.layout.padding.top;
                                turtle.pos.x += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::None => {
                        }
                    }
                    
                    let x = turtle.pos.x + walk.margin.left;
                    let y = turtle.pos.y + walk.margin.top;
                    // walk it normally
                    turtle.pos.y += h + walk.margin.top + walk.margin.bottom;
                    
                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = w + walk.margin.right + walk.margin.left;
                    if biggest > turtle.biggest {
                        turtle.biggest = biggest;
                    }
                    (x, y)
                },
                _ => {
                    (turtle.pos.x + walk.margin.left, turtle.pos.y + walk.margin.top)
                }
            };
            
            let bound_x2 = x + w + if walk.margin.right < 0. {walk.margin.right} else {0.};
            if bound_x2 > turtle.bound_right_bottom.x {
                turtle.bound_right_bottom.x = bound_x2;
            }
            // update y2 bounds (margin bottom is only added if its negative)
            let bound_y2 = y + h + walk.margin.top + if walk.margin.bottom < 0. {walk.margin.bottom} else {0.};
            if bound_y2 > turtle.bound_right_bottom.y {
                turtle.bound_right_bottom.y = bound_y2;
            }
            
            if x < turtle.bound_left_top.x {
                turtle.bound_left_top.x = x;
            }
            if y < turtle.bound_left_top.y {
                turtle.bound_left_top.y = y;
            }
            // we could directly h or v align this thing
            
            Rect {pos: vec2(x, y), size: vec2(w, h)}
        }
        else {
            Rect {pos: vec2(0.0, 0.0), size: vec2(w, h)}
        };
        
        if align_dx != 0.0 {
            if let Some(old_turtle) = old_turtle {
                self.do_align_x(align_dx, old_turtle.align_list_x);
            }
        };
        if align_dy != 0.0 {
            if let Some(old_turtle) = old_turtle {
                self.do_align_y(align_dy, old_turtle.align_list_y);
            }
        };
        
        ret
    }
    
    // high perf turtle with no indirections and compute visibility
    pub fn walk_turtle_right_no_wrap(&mut self, w: f32, h: f32, scroll: Vec2) -> Option<Rect> {
        if let Some(turtle) = self.turtles.last_mut() {
            let x = turtle.pos.x;
            let y = turtle.pos.y;
            // walk it normally
            turtle.pos.x += w;
            
            // keep track of biggest item in the line (include item margin bottom)
            let biggest = h;
            if biggest > turtle.biggest {
                turtle.biggest = biggest;
            }
            // update x2 bounds (margin right is only added if its negative)
            let bound_x2 = x + w;
            if bound_x2 > turtle.bound_right_bottom.x {
                turtle.bound_right_bottom.x = bound_x2;
            }
            // update y2 bounds (margin bottom is only added if its negative)
            let bound_y2 = turtle.pos.y + h;
            if bound_y2 > turtle.bound_right_bottom.y {
                turtle.bound_right_bottom.y = bound_y2;
            }
            
            let vx = turtle.origin.x + scroll.x;
            let vy = turtle.origin.y + scroll.y;
            let vw = turtle.width;
            let vh = turtle.height;
            
            if x > vx + vw || x + w < vx || y > vy + vh || y + h < vy {
                None
            }
            else {
                Some(Rect {pos: vec2(x, y), size: vec2(w, h)})
            }
        }
        else {
            None
        }
    }
    
    pub fn turtle_new_line(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            match turtle.layout.direction {
                Direction::Right => {
                    turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
                    turtle.pos.y += turtle.biggest + turtle.layout.new_line_padding;
                    turtle.biggest = 0.0;
                },
                Direction::Down => {
                    turtle.pos.y = turtle.origin.y + turtle.layout.padding.top;
                    turtle.pos.x += turtle.biggest + turtle.layout.new_line_padding;
                    turtle.biggest = 0.0;
                },
                _ => ()
            }
        }
    }
    
    pub fn turtle_line_is_visible(&mut self, min_height: f32, scroll: Vec2) -> bool {
        if let Some(turtle) = self.turtles.last_mut() {
            let y = turtle.pos.y;
            let h = turtle.biggest.max(min_height);
            let vy = turtle.origin.y + scroll.y;
            let vh = turtle.height;
            
            if y > vy + vh || y + h < vy {
                return false
            }
            else {
                return true
            }
        }
        false
    }
    
    
    pub fn turtle_new_line_min_height(&mut self, min_height: f32) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.pos.x = turtle.origin.x + turtle.layout.padding.left;
            turtle.pos.y += turtle.biggest.max(min_height);
            turtle.biggest = 0.0;
        }
    }
    
    fn do_align_x(&mut self, dx: f32, align_start: usize) {
        let dx = (dx * self.current_dpi_factor).floor() / self.current_dpi_factor;
        for i in align_start..self.align_list.len() {
            let align_item = &self.align_list[i];
            match align_item {
                Area::Instance(inst) => {
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_call = draw_list.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            draw_call.instances.as_mut().unwrap()[inst.instance_offset + rect_pos + i * sh.mapping.instances.total_slots] += dx;
                        }
                    }
                },
                _ => (),
            }
        }
    }
    
    fn do_align_y(&mut self, dy: f32, align_start: usize) {
        let dy = (dy * self.current_dpi_factor).floor() / self.current_dpi_factor;
        for i in align_start..self.align_list.len() {
            let align_item = &self.align_list[i];
            match align_item {
                Area::Instance(inst) => {
                    let draw_list = &mut self.cx.draw_lists[inst.draw_list_id];
                    let draw_call = &mut draw_list.draw_items[inst.draw_item_id].draw_call.as_mut().unwrap();
                    let sh = &self.cx.draw_shaders[draw_call.draw_shader.draw_shader_id];
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            draw_call.instances.as_mut().unwrap()[inst.instance_offset + rect_pos + 1 + i * sh.mapping.instances.total_slots] += dy;
                        }
                    }
                },
                _ => (),
            }
        }
    }
    
    pub fn get_turtle_rect(&self) -> Rect {
        if let Some(turtle) = self.turtles.last() {
            return Rect {
                pos: turtle.origin,
                size: vec2(turtle.width, turtle.height)
            }
        };
        return Rect::default();
    }
    
    pub fn get_turtle_padded_rect(&self) -> Rect {
        if let Some(turtle) = self.turtles.last() {
            let pad_lt = vec2(turtle.layout.padding.left,turtle.layout.padding.top);
            let pad_br = vec2(turtle.layout.padding.right,turtle.layout.padding.bottom);
            return Rect {
                pos: turtle.origin + pad_lt,
                size: vec2(turtle.width, turtle.height) - (pad_lt+pad_br)
            }
        };
        return Rect::default();
    }

    pub fn get_turtle_size(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            vec2(turtle.width, turtle.height)
        }
        else {
            Vec2::default()
        }
    }
    
    pub fn get_turtle_biggest(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            turtle.biggest
        }
        else {
            0.
        }
    }
    
    pub fn get_turtle_bounds(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            
            return Vec2 {
                x: if turtle.bound_right_bottom.x<0. {0.}else {turtle.bound_right_bottom.x} + turtle.layout.padding.right - turtle.origin.x,
                y: if turtle.bound_right_bottom.y<0. {0.}else {turtle.bound_right_bottom.y} + turtle.layout.padding.bottom - turtle.origin.y
            };
        }
        return Vec2::default()
    }
    
    pub fn set_turtle_bounds(&mut self, bound: Vec2) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.bound_right_bottom = Vec2 {
                x: bound.x - turtle.layout.padding.right + turtle.origin.x,
                y: bound.y - turtle.layout.padding.bottom + turtle.origin.y
            }
        }
    }
    
    pub fn get_turtle_origin(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            return turtle.origin;
        }
        return Vec2::default()
    }
    
    pub fn move_turtle(&mut self, dx: f32, dy: f32) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.pos.x += dx;
            turtle.pos.y += dy;
        }
    }
    
    pub fn get_turtle_pos(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            turtle.pos
        }
        else {
            Vec2::default()
        }
    }
    
    pub fn set_turtle_pos(&mut self, pos: Vec2) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.pos = pos
        }
    }
    
    pub fn get_rel_turtle_pos(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            Vec2 {x: turtle.pos.x - turtle.origin.x, y: turtle.pos.y - turtle.origin.y}
        }
        else {
            Vec2::default()
        }
    }
    /*
    pub fn set_turtle_padding(&mut self, padding: Padding) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.padding = padding
        }
    }*/
    
    pub fn visible_in_turtle(&self, geom: Rect, scroll: Vec2) -> bool {
        if let Some(turtle) = self.turtles.last() {
            let view = Rect {pos: scroll, size: vec2(turtle.width, turtle.height)};
            return view.intersects(geom)
        }
        else {
            false
        }
    }
    
    fn compute_align_turtle_x(turtle: &Turtle) -> f32 {
        if turtle.layout.align.fx > 0.0 {
            let dx = turtle.layout.align.fx *
            ((turtle.width - turtle.width_used - (turtle.layout.padding.left + turtle.layout.padding.right)) - (turtle.bound_right_bottom.x - (turtle.origin.x + turtle.layout.padding.left)));
            if dx.is_nan() {return 0.0}
            dx
        }
        else {
            0.
        }
    }
    
    fn compute_align_turtle_y(turtle: &Turtle) -> f32 {
        if turtle.layout.align.fy > 0.0 {
            let dy = turtle.layout.align.fy *
            ((turtle.height - turtle.height_used - (turtle.layout.padding.top + turtle.layout.padding.bottom)) - (turtle.bound_right_bottom.y - (turtle.origin.y + turtle.layout.padding.top)));
            if dy.is_nan() {return 0.0}
            dy
        }
        else {
            0.
        }
    }
    
    pub fn compute_turtle_width(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            if turtle.width.is_nan() {
                if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY { // nothing happened, use padding
                    turtle.width = max_zero_keep_nan(turtle.bound_right_bottom.x - turtle.origin.x + turtle.layout.padding.right);
                    turtle.width_used = 0.;
                    turtle.bound_right_bottom.x = 0.;
                }
            }
        }
    }
    
    pub fn compute_turtle_height(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            if turtle.height.is_nan() {
                if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY { // nothing happened use the padding
                    turtle.height = max_zero_keep_nan(turtle.bound_right_bottom.y - turtle.origin.y + turtle.layout.padding.bottom);
                    turtle.height_used = 0.;
                    turtle.bound_right_bottom.y = 0.;
                }
            }
        }
    }
    
    // used for a<>b layouts horizontally
    pub fn change_turtle_align_x_ab(&mut self, fx: f32) {
        self.change_turtle_align_x(fx, false);
    }
    
    // used for a<b>c layouts horizontally
    pub fn change_turtle_align_x_cab(&mut self, fx: f32) {
        self.change_turtle_align_x(fx, true);
    }
    
    // used for a<b>c layouts horizontally
    pub fn change_turtle_align_x(&mut self, fx: f32, width_used: bool) {
        let (dx, align_origin_x) = if let Some(turtle) = self.turtles.last_mut() {
            (Self::compute_align_turtle_x(&turtle), turtle.align_list_x)
        }
        else {
            (0., 0)
        };
        if dx > 0.0 {
            self.do_align_x(dx, align_origin_x);
        }
        // reset turtle props
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.align_list_x = self.align_list.len();
            turtle.layout.align.fx = fx;
            turtle.width_used = if width_used {turtle.bound_right_bottom.x - turtle.origin.x}else {0.0};
            turtle.bound_right_bottom.x = std::f32::NEG_INFINITY;
        }
    }
    
    // used for a<>b layouts horizontally
    pub fn change_turtle_align_y_ab(&mut self, fx: f32) {
        self.change_turtle_align_y(fx, false);
    }
    
    // used for a<b>c layouts horizontally
    pub fn change_turtle_align_y_cab(&mut self, fx: f32) {
        self.change_turtle_align_y(fx, true);
    }
    
    // used for a<b>c layouts vertically
    pub fn change_turtle_align_y(&mut self, fy: f32, height_used: bool) {
        let (dy, align_origin_y) = if let Some(turtle) = self.turtles.last_mut() {
            (Self::compute_align_turtle_y(&turtle), turtle.align_list_y)
        }
        else {
            (0.0, 0)
        };
        if dy > 0.0 {
            self.do_align_y(dy, align_origin_y);
        }
        // reset turtle props
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.align_list_y = self.align_list.len();
            turtle.layout.align.fy = fy;
            turtle.height_used = if height_used {turtle.bound_right_bottom.y - turtle.origin.y}else {0.0};
            turtle.bound_right_bottom.y = std::f32::NEG_INFINITY;
        }
    }
    
    // call this every time to align the last group on the y axis
    pub fn turtle_align_y(&mut self) {
        let fy = if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.align.fy
        }
        else {
            return
        };
        self.change_turtle_align_y(fy, false);
    }
    
    pub fn turtle_align_x(&mut self) {
        let fx = if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.align.fx
        }
        else {
            return
        };
        self.change_turtle_align_x(fx, false);
    }
    
    pub fn reset_turtle_bounds(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.bound_left_top = Vec2 {x: std::f32::INFINITY, y: std::f32::INFINITY};
            turtle.bound_right_bottom = Vec2 {x: std::f32::NEG_INFINITY, y: std::f32::NEG_INFINITY};
        }
    }
    
    pub fn reset_turtle_pos(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            // subtract used size so 'fill' works
            turtle.pos = Vec2 {
                x: turtle.origin.x + turtle.layout.padding.left,
                y: turtle.origin.y + turtle.layout.padding.top
            };
        }
    }
    
    fn _get_width_left(&self, abs: bool, abs_size: f32) -> f32 {
        if !abs {
            self.get_width_left()
        }
        else {
            abs_size
        }
    }
    
    pub fn get_width_left(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            let nan_val = max_zero_keep_nan(turtle.width - (turtle.layout.padding.right) - turtle.width_used - (turtle.pos.x - turtle.origin.x));
            if nan_val.is_nan() { // if we are a computed height, if some value is known, use that
                if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.x - turtle.origin.x
                }
            }
            return nan_val
        }
        0.
    }
    
    fn _get_width_total(&self, abs: bool, abs_size: f32) -> f32 {
        if !abs {
            self.get_width_total()
        }
        else {
            abs_size
        }
    }
    
    pub fn get_width_total(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            let nan_val = max_zero_keep_nan(turtle.width - (turtle.layout.padding.left + turtle.layout.padding.right));
            if nan_val.is_nan() { // if we are a computed width, if some value is known, use that
                if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.x - turtle.origin.x + turtle.layout.padding.right
                }
            }
            return nan_val
        }
        0.
    }
    
    fn _get_height_left(&self, abs: bool, abs_size: f32) -> f32 {
        if !abs {
            self.get_height_left()
        }
        else {
            abs_size
        }
    }
    
    pub fn get_height_left(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            let nan_val = max_zero_keep_nan(turtle.height - ( turtle.layout.padding.bottom) - turtle.height_used - (turtle.pos.y - turtle.origin.y));
            if nan_val.is_nan() { // if we are a computed height, if some value is known, use that
                if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.y - turtle.origin.y
                }
            }
            return nan_val
        }
        0.
    }
    
    fn _get_height_total(&self, abs: bool, abs_size: f32) -> f32 {
        if !abs {
            self.get_height_total()
        }
        else {
            abs_size
        }
    }
    
    pub fn get_height_total(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            let nan_val = max_zero_keep_nan(turtle.height - (turtle.layout.padding.top + turtle.layout.padding.bottom));
            if nan_val.is_nan() { // if we are a computed height, if some value is known, use that
                if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.y - turtle.origin.y + turtle.layout.padding.bottom
                }
            }
            return nan_val
        }
        0.
    }
    
    pub fn is_height_computed(&self) -> bool {
        if let Some(turtle) = self.turtles.last() {
            if let Height::Computed = turtle.layout.walk.height {
                return true
            }
        }
        false
    }
    
    pub fn is_width_computed(&self) -> bool {
        if let Some(turtle) = self.turtles.last() {
            if let Width::Computed = turtle.layout.walk.width {
                return true
            }
        }
        false
    }
    
    
    pub fn eval_width(&self, width: &Width, margin: Margin, abs: bool, abs_pos: f32) -> (f32, f32) {
        match width {
            Width::Computed => (std::f32::NAN, 0.),
            //Width::ComputeFill => (std::f32::NAN, self._get_width_left(abs, abs_pos) - (margin.l + margin.r)),
            Width::Fixed(v) => (max_zero_keep_nan(*v), 0.),
            Width::Filled => (max_zero_keep_nan(self._get_width_left(abs, abs_pos) - (margin.left + margin.right)), 0.),
            //Width::FillPad(p) => (max_zero_keep_nan(self._get_width_left(abs, abs_pos) - p - (margin.l + margin.r)), 0.),
            //Width::FillScale(s) => (max_zero_keep_nan(self._get_width_left(abs, abs_pos) * s - (margin.l + margin.r)), 0.),
            //Width::FillScalePad(s, p) => (max_zero_keep_nan(self._get_width_left(abs, abs_pos) * s - p - (margin.l + margin.r)), 0.),
            //Width::Scale(s) => (max_zero_keep_nan(self._get_width_total(abs, abs_pos) * s - (margin.l + margin.r)), 0.),
            //Width::ScalePad(s, p) => (max_zero_keep_nan(self._get_width_total(abs, abs_pos) * s - p - (margin.l + margin.r)), 0.),
        }
    }
    
    pub fn eval_height(&self, height: &Height, margin: Margin, abs: bool, abs_pos: f32) -> (f32, f32) {
        match height {
            Height::Computed => (std::f32::NAN, 0.),
            //Height::ComputeFill => (std::f32::NAN, self._get_height_left(abs, abs_pos) - (margin.t + margin.b)),
            Height::Fixed(v) => (max_zero_keep_nan(*v), 0.),
            Height::Filled=> (max_zero_keep_nan(self._get_height_left(abs, abs_pos) - (margin.top + margin.bottom)), 0.),
            //Height::FillPad(p) => (max_zero_keep_nan(self._get_height_left(abs, abs_pos) - p - (margin.t + margin.b)), 0.),
            //Height::FillScale(s) => (max_zero_keep_nan(self._get_height_left(abs, abs_pos) * s - (margin.t + margin.b)), 0.),
            //Height::FillScalePad(s, p) => (max_zero_keep_nan(self._get_height_left(abs, abs_pos) * s - p - (margin.t + margin.b)), 0.),
            //Height::Scale(s) => (max_zero_keep_nan(self._get_height_total(abs, abs_pos) * s - (margin.t + margin.b)), 0.),
            //Height::ScalePad(s, p) => (max_zero_keep_nan(self._get_height_total(abs, abs_pos) * s - p - (margin.t + margin.b)), 0.),
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

/*
thread_local!(pub static debug_pts_store: RefCell<Vec<(f32,f32,i32,String)>> = RefCell::new(Vec::new()));
pub fn debug_pt(x:f32, y:f32, color:i32, s:&str){
    debug_pts_store.with(|c|{
        let mut store = c.borrow_mut();
        store.push((x,y,color,s.to_string()));
    })
}


        debug_pts_store.with(|c|{
            let mut store = c.borrow_mut();
            for (x,y,col,s) in store.iter(){
                self.debug_qd.color = match col{
                    0=>color("red"),
                    1=>color("green"),
                    2=>color("blue"),
                    _=>color("yellow")
                };
                self.debug_qd.draw_abs(cx, false, *x, *y,2.0,2.0);
                if s.len() != 0{
                    self.debug_tx.draw_text(cx, Fixed(*x), Fixed(*y), s);
                }
            }
            store.truncate(0);
        })*/



#[derive(Clone, Default, Debug)]
pub struct Turtle {
    pub align_list_x: usize,
    pub align_list_y: usize,
    pub pos: Vec2,
    pub origin: Vec2,
    pub bound_left_top: Vec2,
    pub bound_right_bottom: Vec2,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    pub abs_size: Vec2,
    pub width_used: f32,
    pub height_used: f32,
    pub biggest: f32,
    pub layout: Layout,
    pub guard_area: Area
}
