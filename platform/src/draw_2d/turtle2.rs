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
    pub new_line_padding: f32,
    pub margin: Margin2,
    pub width: Size2,
    pub height: Size2,
}

#[derive(Copy, Clone, Default, Debug, Live, LiveHook)]
pub struct Walk2 {
    pub margin: Margin2,
    pub width: Size2,
    pub height: Size2,
}

impl Walk2 {
    pub fn wh(w: Size2, h: Size2) -> Self {
        Self {
            margin: Margin2::default(),
            width: w,
            height: h,
        }
    }
    
    pub fn fixed(w: f32, h: f32) -> Self {
        Self {
            margin: Margin2::default(),
            width: Size2::Fixed(w),
            height: Size2::Fixed(h),
        }
    }
}

#[derive(Clone, Copy, Debug, Live, LiveHook)]
pub enum Align2 {
    #[pick] TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight
}

impl Default for Align2 {
    fn default() -> Self {
        Self::TopLeft
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
    pub fn fixed(&self) -> f32 {
        match self {
            Size2::Fixed(v) => *v,
            _ => 0.
        }
    }
    
}

impl<'a> Cx2da<'a> {
    //pub fn debug_pt(&self, x:f32, y:f32, color:i32){
    //self.debug_pts.borrow_mut().push((x,y,color));
    //}
    /*
    pub fn set_count_of_aligned_instance(&mut self, instance_count: usize) -> Area {
        let mut area = self.align_list.last_mut().unwrap();
        if let Area::Instance(inst) = &mut area {
            inst.instance_count = instance_count;
        }
        area.clone()
    }*/
    
    // begin a new turtle with a layout
    pub fn begin_turtle(&mut self, layout: Layout2) {
        self.begin_turtle_with_guard(layout, Area::Empty)
    }
    
    pub fn begin_turtle_with_guard(&mut self, layout: Layout2, guard_area: Area) {

        let origin = if let Some(parent) = self.turtles.last() {
            Vec2 {x: layout.margin.left + parent.pos.x, y: layout.margin.top + parent.pos.y}
        }
        else {
            Vec2 {x: layout.margin.left, y: layout.margin.top}
        };

        let width = self.eval_width(&layout.width, layout.margin);
        let height = self.eval_height(&layout.height, layout.margin);
        
        let turtle = Turtle2 {
            align_list_x: self.align_list.len(),
            align_list_y: self.align_list.len(),
            origin: origin,
            pos: Vec2 {x: origin.x + layout.padding.left, y: origin.y + layout.padding.top},
            layout: layout,
            bound_left_top: Vec2 {x: std::f32::INFINITY, y: std::f32::INFINITY},
            bound_right_bottom: Vec2 {x: std::f32::NEG_INFINITY, y: std::f32::NEG_INFINITY},
            width: width,
            height: height,
            width_used: 0.,
            height_used: 0.,
            guard_area: guard_area,
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
        
        // computed height
        let w = if old.width.is_nan() {
            if old.bound_right_bottom.x == std::f32::NEG_INFINITY { // nothing happened, use padding
                Size2::Fixed(old.layout.padding.left + old.layout.padding.right)
            }
            else { // use the bounding box
                Size2::Fixed(max_zero_keep_nan(old.bound_right_bottom.x - old.origin.x + old.layout.padding.right))
            }
        }
        else {
            Size2::Fixed(old.width)
        };
        
        let h = if old.height.is_nan() {
            if old.bound_right_bottom.y == std::f32::NEG_INFINITY { // nothing happened use the padding
                Size2::Fixed(old.layout.padding.top + old.layout.padding.bottom)
            }
            else { // use the bounding box
                Size2::Fixed(max_zero_keep_nan(old.bound_right_bottom.y - old.origin.y + old.layout.padding.bottom))
            }
        }
        else {
            Size2::Fixed(old.height)
        };
        
        let margin = old.layout.margin.clone();
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
        
        return self.walk_turtle_with_old(Walk2 {width: w, height: h, margin}, Some(&old))
    }
    
    pub fn walk_turtle(&mut self, walk: Walk2) -> Rect {
        self.walk_turtle_with_old(walk, None)
    }
    
    // walk the turtle with a 'w/h' and a margin
    pub fn walk_turtle_with_old(&mut self, walk: Walk2, old_turtle: Option<&Turtle2>) -> Rect {
        
        // we can only do a fill in the direction we are stacking
        
        let w = self.eval_width(&walk.width, walk.margin);
        let h = self.eval_height(&walk.height, walk.margin);
        
        let ret = if let Some(turtle) = self.turtles.last_mut() {
            let (x, y) = match turtle.layout.flow {
                Flow::Right => {
                    let x = turtle.pos.x + walk.margin.left;
                    let y = turtle.pos.y + walk.margin.top;
                    // walk it normally
                    turtle.pos.x += w + walk.margin.left + walk.margin.right;
                    (x, y)
                },
                Flow::Down => {
                    let x = turtle.pos.x + walk.margin.left;
                    let y = turtle.pos.y + walk.margin.top;
                    // walk it normally
                    turtle.pos.y += h + walk.margin.top + walk.margin.bottom;
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

        ret
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
            let pad_lt = vec2(turtle.layout.padding.left, turtle.layout.padding.top);
            let pad_br = vec2(turtle.layout.padding.right, turtle.layout.padding.bottom);
            return Rect {
                pos: turtle.origin + pad_lt,
                size: vec2(turtle.width, turtle.height) - (pad_lt + pad_br)
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
    /*
    pub fn get_turtle_biggest(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            turtle.biggest
        }
        else {
            0.
        }
    }*/
    
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
    
    fn compute_align_turtle_x(turtle: &Turtle2) -> f32 {
        /*if turtle.layout.align.fx > 0.0 {
            let dx = turtle.layout.align.fx *
            ((turtle.width - turtle.width_used - (turtle.layout.padding.left + turtle.layout.padding.right)) - (turtle.bound_right_bottom.x - (turtle.origin.x + turtle.layout.padding.left)));
            if dx.is_nan() {return 0.0}
            dx
        }
        else {*/
            0.
        /*}*/
    }
    
    fn compute_align_turtle_y(turtle: &Turtle2) -> f32 {
        /*if turtle.layout.align.fy > 0.0 {
            let dy = turtle.layout.align.fy *
            ((turtle.height - turtle.height_used - (turtle.layout.padding.top + turtle.layout.padding.bottom)) - (turtle.bound_right_bottom.y - (turtle.origin.y + turtle.layout.padding.top)));
            if dy.is_nan() {return 0.0}
            dy
        }
        else {*/
            0.
        //}
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
    /*
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
    */
    // used for a<>b layouts horizontally
    
    /*
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
    }*/
    /*
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
    }*/
    /*
    pub fn reset_turtle_bounds(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            turtle.bound_left_top = Vec2 {x: std::f32::INFINITY, y: std::f32::INFINITY};
            turtle.bound_right_bottom = Vec2 {x: std::f32::NEG_INFINITY, y: std::f32::NEG_INFINITY};
        }
    }*/
    /*
    pub fn reset_turtle_pos(&mut self) {
        if let Some(turtle) = self.turtles.last_mut() {
            // subtract used size so 'fill' works
            turtle.pos = Vec2 {
                x: turtle.origin.x + turtle.layout.padding.left,
                y: turtle.origin.y + turtle.layout.padding.top
            };
        }
    }*/
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
    
    pub fn get_height_left(&self) -> f32 {
        if let Some(turtle) = self.turtles.last() {
            let nan_val = max_zero_keep_nan(turtle.height - (turtle.layout.padding.bottom) - turtle.height_used - (turtle.pos.y - turtle.origin.y));
            if nan_val.is_nan() { // if we are a computed height, if some value is known, use that
                if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY {
                    return turtle.bound_right_bottom.y - turtle.origin.y
                }
            }
            return nan_val
        }
        0.
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
            if let Size2::Fit = turtle.layout.height {
                return true
            }
        }
        false
    }
    
    pub fn is_width_computed(&self) -> bool {
        if let Some(turtle) = self.turtles.last() {
            if let Size2::Fit = turtle.layout.width {
                return true
            }
        }
        false
    }
    
    
    pub fn eval_width(&self, width: &Size2, margin: Margin2) -> f32 {
        match width {
            Size2::Fit => std::f32::NAN,
            Size2::Fixed(v) => max_zero_keep_nan(*v),
            Size2::Fill => max_zero_keep_nan(self.get_width_left() - (margin.left + margin.right)),
        }
    }
    
    pub fn eval_height(&self, height: &Size2, margin: Margin2) -> f32 {
        match height {
            Size2::Fit => std::f32::NAN,
            Size2::Fixed(v) => max_zero_keep_nan(*v),
            Size2::Fill => max_zero_keep_nan(self.get_height_left() - (margin.top + margin.bottom)),
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

#[derive(Clone, Default, Debug)]
pub struct Turtle2 {
    pub align_list_x: usize,
    pub align_list_y: usize,
    pub pos: Vec2,
    pub origin: Vec2,
    pub bound_left_top: Vec2,
    pub bound_right_bottom: Vec2,
    pub width: f32,
    pub height: f32,
    pub width_used: f32,
    pub height_used: f32,
    pub layout: Layout2,
    pub guard_area: Area
}
