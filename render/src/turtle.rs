use crate::cx::*;
use serde::*;

impl Cx {
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
    pub fn begin_turtle(&mut self, layout: Layout, guard_area: Area) {
        
        if !self.is_in_redraw_cycle {
            panic!("calling begin_turtle outside of redraw cycle is not possible!");
        }
        
        // fetch origin and size from parent
        let (mut origin, mut abs_size) = if let Some(parent) = self.turtles.last() {
            (Vec2 {x: layout.walk.margin.l + parent.pos.x, y: layout.walk.margin.t + parent.pos.y}, parent.abs_size)
        }
        else {
            (Vec2 {x: layout.walk.margin.l, y: layout.walk.margin.t}, Vec2::zero())
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
        let width = layout.walk.width.eval_width(self, layout.walk.margin, is_abs_origin, abs_size.x);
        let height = layout.walk.height.eval_height(self, layout.walk.margin, is_abs_origin, abs_size.y);
        
        let turtle = Turtle {
            align_list_x: self.align_list.len(),
            align_list_y: self.align_list.len(),
            origin: origin,
            pos: Vec2 {x: origin.x + layout.padding.l, y: origin.y + layout.padding.t},
            layout: layout,
            biggest: 0.0,
            bound_left_top: Vec2 {x: std::f32::INFINITY, y: std::f32::INFINITY},
            bound_right_bottom: Vec2 {x: std::f32::NEG_INFINITY, y: std::f32::NEG_INFINITY},
            width: width,
            height: height,
            width_used: 0.,
            height_used: 0.,
            abs_size: abs_size,
            guard_area: guard_area,
            //..Default::default()
        };
        
        self.turtles.push(turtle);
    }
    
    pub fn end_turtle(&mut self, guard_area: Area) -> Rect {
        let old = self.turtles.pop().unwrap();
        if guard_area != old.guard_area {
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", old.guard_area, guard_area)
        }
        
        let w = if old.width.is_nan() {
            if old.bound_right_bottom.x == std::f32::NEG_INFINITY { // nothing happened, use padding
                Width::Fix(old.layout.padding.l + old.layout.padding.r)
            }
            else { // use the bounding box
                Width::Fix(max_zero_keep_nan(old.bound_right_bottom.x - old.origin.x + old.layout.padding.r))
            }
        }
        else {
            Width::Fix(old.width)
        };
        
        let h = if old.height.is_nan() {
            if old.bound_right_bottom.y == std::f32::NEG_INFINITY { // nothing happened use the padding
                Height::Fix(old.layout.padding.t + old.layout.padding.b)
            }
            else { // use the bounding box
                Height::Fix(max_zero_keep_nan(old.bound_right_bottom.y - old.origin.y + old.layout.padding.b))
            }
        }
        else {
            Height::Fix(old.height)
        };
        
        let margin = old.layout.walk.margin.clone();
        // if we have alignment set, we should now align our childnodes
        let dx = Self::compute_align_turtle_x(&old);
        if dx > 0.0{
            self.do_align_x(dx, old.align_list_x);
        } 
        let dy = Self::compute_align_turtle_y(&old);
        if dy > 0.0 {
            self.do_align_y(dy, old.align_list_y);
        }
        
        // when a turtle is x-abs / y-abs you dont walk the parent
        if !old.layout.abs_origin.is_none() {
            let abs_origin = if let Some(abs_origin) = old.layout.abs_origin {abs_origin} else {Vec2::zero()};
            let w = if let Width::Fix(vw) = w {vw} else {0.};
            let h = if let Height::Fix(vh) = h {vh} else {0.};
            return Rect {x: abs_origin.x, y: abs_origin.y, w: w, h: h};
        }
        
        return self.walk_turtle_with_old(Walk{width:w, height:h, margin}, Some(&old))
    }
    
    pub fn walk_turtle(&mut self, walk:Walk) -> Rect {
        self.walk_turtle_with_old(walk, None)
    }
    
    // walk the turtle with a 'w/h' and a margin
    pub fn walk_turtle_with_old(&mut self, walk:Walk, old_turtle: Option<&Turtle>) -> Rect {
        let mut align_dx = 0.0;
        let mut align_dy = 0.0;
        let w = max_zero_keep_nan(walk.width.eval_width(self, walk.margin, false, 0.0));
        let h = max_zero_keep_nan(walk.height.eval_height(self, walk.margin, false, 0.0));
        let ret = if let Some(turtle) = self.turtles.last_mut() {
            let (x, y) = match turtle.layout.direction {
                Direction::Right => {
                    match turtle.layout.line_wrap {
                        LineWrap::NewLine => {
                            if (turtle.pos.x + walk.margin.l + w) >
                            (turtle.origin.x + turtle.width - turtle.layout.padding.r) {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.x = turtle.origin.x + turtle.layout.padding.l;
                                turtle.pos.y += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::None => {
                        }
                    }
                    
                    let x = turtle.pos.x + walk.margin.l;
                    let y = turtle.pos.y + walk.margin.t;
                    // walk it normally
                    turtle.pos.x += w + walk.margin.l + walk.margin.r;
                    
                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = h + walk.margin.t + walk.margin.b;
                    if biggest > turtle.biggest {
                        turtle.biggest = biggest;
                    }
                    (x, y)
                },
                Direction::Down => {
                    match turtle.layout.line_wrap {
                        LineWrap::NewLine => {
                            if (turtle.pos.y + walk.margin.t + h) >
                            (turtle.origin.y + turtle.height - turtle.layout.padding.b) {
                                // what is the move delta.
                                let old_x = turtle.pos.x;
                                let old_y = turtle.pos.y;
                                turtle.pos.y = turtle.origin.y + turtle.layout.padding.t;
                                turtle.pos.x += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.pos.x - old_x;
                                align_dy = turtle.pos.y - old_y;
                            }
                        },
                        LineWrap::None => {
                        }
                    }
                    
                    let x = turtle.pos.x + walk.margin.l;
                    let y = turtle.pos.y + walk.margin.t;
                    // walk it normally
                    turtle.pos.y += h + walk.margin.t + walk.margin.b;
                    
                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = w + walk.margin.r + walk.margin.l;
                    if biggest > turtle.biggest {
                        turtle.biggest = biggest;
                    }
                    (x, y)
                },
                _ => {
                    (turtle.pos.x + walk.margin.l, turtle.pos.y + walk.margin.t)
                }
            };
            
            let bound_x2 = x + w + if walk.margin.r < 0. {walk.margin.r} else {0.};
            if bound_x2 > turtle.bound_right_bottom.x {
                turtle.bound_right_bottom.x = bound_x2;
            }
            // update y2 bounds (margin bottom is only added if its negative)
            let bound_y2 = y + h + walk.margin.t + if walk.margin.b < 0. {walk.margin.b} else {0.};
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
            
            Rect {
                x: x,
                y: y,
                w: w,
                h: h
            }
        }
        else {
            Rect {
                x: 0.0,
                y: 0.0,
                w: w,
                h: h
            }
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
                Some(Rect {
                    x: x,
                    y: y,
                    w: w,
                    h: h
                })
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
                    turtle.pos.x = turtle.origin.x + turtle.layout.padding.l;
                    turtle.pos.y += turtle.biggest + turtle.layout.new_line_padding;
                    turtle.biggest = 0.0;
                },
                Direction::Down => {
                    turtle.pos.y = turtle.origin.y + turtle.layout.padding.t;
                    turtle.pos.x += turtle.biggest+ turtle.layout.new_line_padding;
                    turtle.biggest = 0.0;
                },
                _ => ()
            }
        }
    }
    
    pub fn turtle_line_is_visible(&mut self, min_height: f32, scroll: Vec2)->bool{
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
            turtle.pos.x = turtle.origin.x + turtle.layout.padding.l;
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
                    let cxview = &mut self.views[inst.view_id];
                    let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                    let sh = &self.shaders[draw_call.shader_id];
                    for i in 0..inst.instance_count {
                        if let Some(x) = sh.mapping.rect_instance_props.x {
                            draw_call.instance[inst.instance_offset + x + i * sh.mapping.instance_slots] += dx;
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
                    let cxview = &mut self.views[inst.view_id];
                    let draw_call = &mut cxview.draw_calls[inst.draw_call_id];
                    let sh = &self.shaders[draw_call.shader_id];
                    for i in 0..inst.instance_count {
                        if let Some(y) = sh.mapping.rect_instance_props.y {
                            draw_call.instance[inst.instance_offset + y + i * sh.mapping.instance_slots] += dy;
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
                x: turtle.origin.x,
                y: turtle.origin.y,
                w: turtle.width,
                h: turtle.height
            }
        };
        return Rect::zero();
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
                x: if turtle.bound_right_bottom.x<0. {0.}else {turtle.bound_right_bottom.x} + turtle.layout.padding.r - turtle.origin.x,
                y: if turtle.bound_right_bottom.y<0. {0.}else {turtle.bound_right_bottom.y} + turtle.layout.padding.b - turtle.origin.y
            };
        }
        return Vec2::zero()
    }
    
    pub fn set_turtle_bounds(&mut self, bound: Vec2) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.bound_right_bottom = Vec2 {
                x: bound.x - turtle.layout.padding.r + turtle.origin.x,
                y: bound.y - turtle.layout.padding.b + turtle.origin.y
            }
        }
    }
    
    pub fn get_turtle_origin(&self) -> Vec2 {
        if let Some(turtle) = self.turtles.last() {
            return turtle.origin;
        }
        return Vec2::zero()
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
            Vec2::zero()
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
            Vec2::zero()
        }
    }
    
    pub fn set_turtle_padding(&mut self, padding: Padding) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.padding = padding
        }
    }
    
    pub fn visible_in_turtle(&self, geom: Rect, scroll: Vec2) -> bool {
        if let Some(turtle) = self.turtles.last() {
            let view = Rect {
                x: scroll.x, //- margin.l,
                y: scroll.y, // - margin.t,
                w: turtle.width, // + margin.l + margin.r,
                h: turtle.height, // + margin.t + margin.b
            };
            
            return view.intersects(geom)
        }
        else {
            false
        }
    }
    
    fn compute_align_turtle_x(turtle: &Turtle) -> f32 {
        if turtle.layout.align.fx > 0.0{
            let dx = turtle.layout.align.fx *
            ((turtle.width - turtle.width_used - (turtle.layout.padding.l + turtle.layout.padding.r)) - (turtle.bound_right_bottom.x - (turtle.origin.x + turtle.layout.padding.l)));
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
            ((turtle.height - turtle.height_used - (turtle.layout.padding.t + turtle.layout.padding.b)) - (turtle.bound_right_bottom.y - (turtle.origin.y + turtle.layout.padding.t)));
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
                    turtle.width = max_zero_keep_nan(turtle.bound_right_bottom.x - turtle.origin.x + turtle.layout.padding.r);
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
                    turtle.height = max_zero_keep_nan(turtle.bound_right_bottom.y - turtle.origin.y + turtle.layout.padding.b);
                    turtle.height_used = 0.;
                    turtle.bound_right_bottom.y = 0.;
                }
            }
        }
    }
    
    // used for a<b>c layouts horizontally
    pub fn change_turtle_align_x(&mut self, fx:f32) {
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
            turtle.width_used = turtle.bound_right_bottom.x - turtle.origin.x;
            turtle.bound_right_bottom.x  = std::f32::NEG_INFINITY;
        }
    }
    
    // used for a<b>c layouts vertically
    pub fn change_turtle_align_y(&mut self, fy:f32) {
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
            turtle.height_used = turtle.bound_right_bottom.y - turtle.origin.y;
            turtle.bound_right_bottom.y  = std::f32::NEG_INFINITY;
        }
    }
    
    // call this every time to align the last group on the y axis
    pub fn turtle_align_y(&mut self){
        let fy = if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.align.fy
        }
        else{
            return
        };
        self.change_turtle_align_y(fy);
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.height_used = 0.;
        }
    }

    pub fn turtle_align_x(&mut self){
        let fx = if let Some(turtle) = self.turtles.last_mut() {
            turtle.layout.align.fx
        }
        else{
            return
        };
        self.change_turtle_align_x(fx);
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.width_used = 0.;
        }
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
                x: turtle.origin.x + turtle.layout.padding.l,
                y: turtle.origin.y + turtle.layout.padding.t
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
            let nan_val = max_zero_keep_nan(turtle.width - turtle.width_used - (turtle.pos.x - turtle.origin.x));
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
            let nan_val = max_zero_keep_nan(turtle.width - (turtle.layout.padding.l + turtle.layout.padding.r));
            if nan_val.is_nan() { // if we are a computed width, if some value is known, use that
                if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.x - turtle.origin.x
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
            let nan_val = max_zero_keep_nan(turtle.height - turtle.height_used - (turtle.pos.y - turtle.origin.y));
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
            let nan_val = max_zero_keep_nan(turtle.height - (turtle.layout.padding.t + turtle.layout.padding.b));
            if nan_val.is_nan() { // if we are a computed height, if some value is known, use that
                if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.y - turtle.origin.y
                }
            }
            return nan_val
        }
        0.
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


#[derive(Copy, Clone, Debug)]
pub enum Width {
    Fill,
    Fix(f32),
    Compute,
    FillPad(f32),
    FillScale(f32),
    FillScalePad(f32, f32),
    Scale(f32),
    ScalePad(f32, f32),
}

#[derive(Copy, Clone, Debug)]
pub enum Height {
    Fill,
    Fix(f32),
    Compute,
    FillPad(f32),
    FillScale(f32),
    FillScalePad(f32, f32),
    Scale(f32),
    ScalePad(f32, f32),
}

impl Default for Width {
    fn default() -> Self {
        Width::Fill
    }
}


impl Default for Height {
    fn default() -> Self {
        Height::Fill
    }
}


impl Width {
    pub fn fixed(&self)->f32{
        match self {
            Width::Fix(v) => *v,
            _=>0.
        }
    }

    pub fn eval_width(&self, cx: &Cx, margin: Margin, abs: bool, abs_pos: f32) -> f32 {
        match self {
            Width::Compute => std::f32::NAN,
            Width::Fix(v) => *v,
            Width::Fill => cx._get_width_left(abs, abs_pos) - (margin.l + margin.r),
            Width::FillPad(p) => cx._get_width_left(abs, abs_pos) - p - (margin.l + margin.r),
            Width::FillScale(s) => cx._get_width_left(abs, abs_pos) * s - (margin.l + margin.r),
            Width::FillScalePad(s, p) => cx._get_width_left(abs, abs_pos) * s - p - (margin.l + margin.r),
            Width::Scale(s) => cx._get_width_total(abs, abs_pos) * s - (margin.l + margin.r),
            Width::ScalePad(s, p) => cx._get_width_total(abs, abs_pos) * s - p - (margin.l + margin.r),
        }
    }
}

impl Height{
    pub fn fixed(&self)->f32{
        match self {
            Height::Fix(v) => *v,
            _=>0.
        }
    }
    pub fn eval_height(&self, cx: &Cx, margin: Margin, abs: bool, abs_pos: f32) -> f32 {
        match self {
            Height::Compute => std::f32::NAN,
            Height::Fix(v) => *v,
            Height::Fill => cx._get_height_left(abs, abs_pos) - (margin.t + margin.b),
            Height::FillPad(p) => cx._get_height_left(abs, abs_pos) - p - (margin.t + margin.b),
            Height::FillScale(s) => cx._get_height_left(abs, abs_pos) * s - (margin.t + margin.b),
            Height::FillScalePad(s, p) => cx._get_height_left(abs, abs_pos) * s - p - (margin.t + margin.b),
            Height::Scale(s) => cx._get_height_total(abs, abs_pos) * s - (margin.t + margin.b),
            Height::ScalePad(s, p) => cx._get_height_total(abs, abs_pos) * s - p - (margin.t + margin.b),
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Align {
    pub fx: f32,
    pub fy: f32
}

impl Align {
    pub fn left_top() -> Align {Align {fx: 0., fy: 0.}}
    pub fn center_top() -> Align {Align {fx: 0.5, fy: 0.0}}
    pub fn right_top() -> Align {Align {fx: 1.0, fy: 0.0}}
    pub fn left_center() -> Align {Align {fx: 0.0, fy: 0.5}}
    pub fn center() -> Align {Align {fx: 0.5, fy: 0.5}}
    pub fn right_center() -> Align {Align {fx: 1.0, fy: 0.5}}
    pub fn left_bottom() -> Align {Align {fx: 0., fy: 1.0}}
    pub fn center_bottom() -> Align {Align {fx: 0.5, fy: 1.0}}
    pub fn right_bottom() -> Align {Align {fx: 1.0, fy: 1.0}}
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Margin {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32
}

impl Margin {
    pub fn zero() -> Margin {
        Margin {l: 0.0, t: 0.0, r: 0.0, b: 0.0}
    }
    
    pub fn all(v: f32) -> Margin {
        Margin {l: v, t: v, r: v, b: v}
    }
}

impl Rect {
    pub fn contains_with_margin(&self, x: f32, y: f32, margin: &Option<Margin>) -> bool {
        if let Some(margin) = margin {
            return x >= self.x - margin.l && x <= self.x + self.w + margin.r &&
            y >= self.y - margin.t && y <= self.y + self.h + margin.b;
        }
        else {
            return self.contains(x, y);
        }
        
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Padding {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32
}

impl Padding {
    pub fn zero() -> Padding {
        Padding {l: 0.0, t: 0.0, r: 0.0, b: 0.0}
    }
    pub fn all(v: f32) -> Padding {
        Padding {l: v, t: v, r: v, b: v}
    }
}


#[derive(Copy, Clone, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Right
    }
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical
}

impl Default for Axis {
    fn default() -> Self {
        Axis::Horizontal
    }
}

#[derive(Copy, Clone, Debug)]
pub enum LineWrap {
    None,
    NewLine
}
impl Default for LineWrap {
    fn default() -> Self {
        LineWrap::None
    }
}

#[derive(Copy, Clone, Default, Debug)]
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

#[derive(Copy, Clone, Default, Debug)]
pub struct Walk{
    pub margin: Margin,
    pub width: Width,
    pub height: Height,
}

impl Walk{
    pub fn wh(w:Width, h:Height)->Self{
        Self{
            width:w, height:h, margin:Margin::zero(),
        }
    }
}

impl Layout{
    pub fn abs_origin_zero()->Self{
        Layout {
            abs_origin: Some(Vec2::zero()),
            ..Default::default()
        }
    }
}

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
    pub abs_size: Vec2,
    pub width_used: f32,
    pub height_used: f32,
    pub biggest: f32,
    pub layout: Layout,
    pub guard_area: Area
}

pub fn max_zero_keep_nan(v: f32) -> f32 {
    if v.is_nan() {
        v
    }
    else {
        f32::max(v, 0.0)
    }
}