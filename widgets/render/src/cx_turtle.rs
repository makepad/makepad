use crate::cx::*;

impl Cx{
    //pub fn debug_pt(&self, x:f32, y:f32, color:i32){
        //self.debug_pts.borrow_mut().push((x,y,color));
    //}

    pub fn set_count_of_aligned_instance(&mut self, instance_count:usize)->Area{
        let mut area =  self.align_list.last_mut().unwrap();
        if let Area::Instance(inst) = &mut area{
            inst.instance_count = instance_count;
        }
        area.clone()
    }

    // begin a new turtle with a layout
    pub fn begin_turtle(&mut self, layout:&Layout, guard_area:Area){
        if !self.is_in_redraw_cycle{
            panic!("calling begin_turtle outside of redraw cycle is not possible!");
        }

        let is_abs = !layout.abs_start.is_none();

        let start = if is_abs{
            layout.abs_start.unwrap()
        }
        else{
            if let Some(parent) = self.turtles.last(){
                Vec2{x:layout.margin.l+parent.walk.x, y:layout.margin.t+parent.walk.y}
            }
            else{
                Vec2{x:layout.margin.l, y:layout.margin.t}
            }
        };
        let width = layout.width.eval_width(self, layout.margin, is_abs);
        let height = layout.height.eval_height(self, layout.margin, is_abs);
        
        self.turtles.push(Turtle{
            align_start:self.align_list.len(),
            layout:layout.clone(),
            start:start,
            walk:Vec2{x:start.x + layout.padding.l, y:start.y + layout.padding.t},
            biggest:0.0,
            bound_left_top:Vec2{x:std::f32::INFINITY, y:std::f32::INFINITY},
            bound_right_bottom:Vec2{x:std::f32::NEG_INFINITY, y:std::f32::NEG_INFINITY},
            width:width,
            height:height,
            width_used:0.,
            height_used:0.,
            guard_area:guard_area,
            ..Default::default()
        });
    }

    // walk the turtle with a 'w/h' and a margin
    pub fn walk_turtle(&mut self, vw:Bounds, vh:Bounds, margin:Margin, old_turtle:Option<&Turtle>)->Rect{
        let w = max_zero_keep_nan(vw.eval_width(self, margin, false));
        let h = max_zero_keep_nan(vh.eval_height(self, margin, false));
        let mut align_dx = 0.0;
        let mut align_dy = 0.0;
        let ret = if let Some(turtle) = self.turtles.last_mut(){
            let (x,y) = match turtle.layout.direction{
                Direction::Right=>{
                    match turtle.layout.line_wrap{
                        LineWrap::NewLine=>{
                             if (turtle.walk.x + margin.l + w) >
                                (turtle.start.x + turtle.width - turtle.layout.padding.r){
                                // what is the move delta. 
                                let old_x = turtle.walk.x;
                                let old_y = turtle.walk.y;
                                turtle.walk.x = turtle.start.x + turtle.layout.padding.l;
                                turtle.walk.y += turtle.biggest;
                                turtle.biggest = 0.0;
                                align_dx = turtle.walk.x - old_x;
                                align_dy = turtle.walk.y - old_y;
                            }
                        },
                        LineWrap::None=>{
                        }
                    }

                    let x = turtle.walk.x + margin.l;
                    let y = turtle.walk.y + margin.t;
                    // walk it normally
                    turtle.walk.x += w + margin.l + margin.r;

                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = h + margin.t + margin.b;
                    if biggest > turtle.biggest{
                        turtle.biggest = biggest;
                    }
                    // update x2 bounds (margin right is only added if its negative)
                    let bound_x2 = x + w + if margin.r < 0.{margin.r} else{0.};
                    if bound_x2 > turtle.bound_right_bottom.x{
                        turtle.bound_right_bottom.x = bound_x2;
                    }
                    // update y2 bounds (margin bottom is only added if its negative)
                    let bound_y2 = turtle.walk.y + h + margin.t + if margin.b < 0.{margin.b} else{0.};
                    if bound_y2 > turtle.bound_right_bottom.y{
                        turtle.bound_right_bottom.y = bound_y2;
                    }
                    (x,y)
                },
                _=>{
                    (turtle.walk.x + margin.l, turtle.walk.y + margin.t)
                }
            };
            if x < turtle.bound_left_top.x{
                turtle.bound_left_top.x = x;
            }
            if y < turtle.bound_left_top.y{
                turtle.bound_left_top.y = y;
            }
            Rect{
                x:x,
                y:y,
                w:w,
                h:h
            }
        }
        else{
            Rect{
                x:0.0,
                y:0.0,
                w:w,
                h:h
            }
        };

        if align_dx != 0.0 || align_dy != 0.0{
            if let Some(old_turtle) = old_turtle{
                self.do_align(align_dx, align_dy, old_turtle.align_start);
            }
        };

        ret
    }
    
    pub fn walk_turtle_text(&mut self, w:f32, h:f32, scroll:Vec2)->Option<Rect>{
        if let Some(turtle) = self.turtles.last_mut(){
            let x = turtle.walk.x;
            let y = turtle.walk.y;
            // walk it normally
            turtle.walk.x += w;

            // keep track of biggest item in the line (include item margin bottom)
            let biggest = h;
            if biggest > turtle.biggest{
                turtle.biggest = biggest;
            }
            // update x2 bounds (margin right is only added if its negative)
            let bound_x2 = x + w;
            if bound_x2 > turtle.bound_right_bottom.x{
                turtle.bound_right_bottom.x = bound_x2;
            }
            // update y2 bounds (margin bottom is only added if its negative)
            let bound_y2 = turtle.walk.y + h;
            if bound_y2 > turtle.bound_right_bottom.y{
                turtle.bound_right_bottom.y = bound_y2;
            }

            let vx = turtle.start.x + scroll.x;
            let vy = turtle.start.y + scroll.y;
            let vw = turtle.width;
            let vh = turtle.height; 
            
            if x > vx + vw || x + w < vx || y > vy + vh || y + h < vy{
                None
            }
            else{
                Some(Rect{
                    x:x,
                    y:y,
                    w:w,
                    h:h
                })
            }
        }
        else{
            None
        }
    }

    pub fn turtle_new_line(&mut self){
        if let Some(turtle) = self.turtles.last_mut(){
            match turtle.layout.direction{
                Direction::Right=>{
                    turtle.walk.x = turtle.start.x + turtle.layout.padding.l;
                    turtle.walk.y += turtle.biggest;
                    turtle.biggest = 0.0;
                },
                _=>()
            }
        }
    }

    pub fn turtle_new_line_min_height(&mut self, h:f32){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.walk.x = turtle.start.x + turtle.layout.padding.l;
            turtle.walk.y += turtle.biggest.max(h);
            turtle.biggest = 0.0;
        }
    }

    fn do_align(&mut self, dx:f32, dy:f32, align_start:usize){

        for i in align_start..self.align_list.len(){
            let align_item = &self.align_list[i];
            return match align_item{
                Area::Instance(inst)=>{
                    if inst.instance_count == 0{
                        return;
                    }
                    let draw_list = &mut self.draw_lists[inst.draw_list_id];
                    let draw_call = &mut draw_list.draw_calls[inst.draw_call_id];
                    let csh = &self.compiled_shaders[draw_call.shader_id];

                    for i in 0..inst.instance_count{
                        if let Some(x) = csh.rect_instance_props.x{
                            draw_call.instance[inst.instance_offset + x + i * csh.instance_slots] += dx;
                        }
                        if let Some(y) = csh.rect_instance_props.y{
                            draw_call.instance[inst.instance_offset + y+ i * csh.instance_slots] += dy;
                        }
                    }
                },
                Area::DrawList(area_draw_list)=>{
                     let draw_list = &mut self.draw_lists[area_draw_list.draw_list_id];
                     draw_list.rect.x += dx;
                     draw_list.rect.y += dy;
                }
                _=>(),
            }
        }
    }

    pub fn turtle_rect(&self)->Rect{
        if let Some(turtle) = self.turtles.last(){
            return Rect{
                x:turtle.start.x,
                y:turtle.start.y,
                w:turtle.width,
                h:turtle.height
            }
        };
        return Rect::zero();
    }

    pub fn turtle_biggest(&self)->f32{
        if let Some(turtle) = self.turtles.last(){
            turtle.biggest
        }
        else{
            0.
        }
    }

    pub fn turtle_bounds(&self)->Vec2{
        if let Some(turtle) = self.turtles.last(){
            return Vec2{
                x: turtle.bound_right_bottom.x + turtle.layout.padding.r - turtle.start.x, 
                y: turtle.bound_right_bottom.y + turtle.layout.padding.b - turtle.start.y
            };
        }
        return Vec2::zero()
    }

    pub fn set_turtle_bounds(&mut self, bound:Vec2){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.bound_right_bottom = Vec2{
                x: bound.x - turtle.layout.padding.r + turtle.start.x,
                y: bound.y - turtle.layout.padding.b + turtle.start.y
            }
        }
    }

    pub fn turtle_origin(&self)->Vec2{
        if let Some(turtle) = self.turtles.last(){
            return turtle.start;
        }
        return Vec2::zero()
    }

    pub fn move_turtle(&mut self, rel_x:f32, rel_y:f32){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.walk.x = rel_x + turtle.start.x;
            turtle.walk.y = rel_y + turtle.start.y;
        }
    }
/*
    pub fn reset_turtle_bounds(&mut self){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.bound_left_top = vec2(std::f32::INFINITY,std::f32::INFINITY);
            turtle.bound_right_bottom = vec2(std::f32::NEG_INFINITY, std::f32::NEG_INFINITY);
        }
    }
*/
    fn compute_align_turtle(turtle:&Turtle)->Vec2{
        if turtle.layout.align.fx > 0.0 || turtle.layout.align.fy > 0.0{

            let mut dx = turtle.layout.align.fx * 
                ((turtle.width - turtle.width_used - (turtle.layout.padding.l + turtle.layout.padding.r)) - (turtle.bound_right_bottom.x - (turtle.start.x + turtle.layout.padding.l)));
            let mut dy = turtle.layout.align.fy * 
                ((turtle.height - turtle.height_used - (turtle.layout.padding.t + turtle.layout.padding.b)) - (turtle.bound_right_bottom.y - (turtle.start.y + turtle.layout.padding.t)));
            if dx.is_nan(){dx = 0.0}
            if dy.is_nan(){dy = 0.0}
            Vec2{x:dx, y:dy}
        }
        else{
            Vec2::zero()
        }
    }

    // restarts the turtle with a new alignment, used for a<b>c layouts
    pub fn realign_turtle(&mut self, align:Align, set_used:bool){
        let (align_delta, align_start) = if let Some(turtle) = self.turtles.last_mut(){
            (Self::compute_align_turtle(&turtle), turtle.align_start)
        }
        else{
            (Vec2::zero(), 0)
        };
        if align_delta.x > 0.0 || align_delta.y > 0.0{
            self.do_align(align_delta.x, align_delta.y, align_start);
        }
        // reset turtle props
        if let Some(turtle) = self.turtles.last_mut(){
            // restart align_list for the next pass
            turtle.align_start = self.align_list.len();
            // subtract used size so 'fill' works
            turtle.layout.align = align; 
            if set_used{
                turtle.width_used = turtle.bound_right_bottom.x - turtle.start.x;
                turtle.height_used = turtle.bound_right_bottom.y - turtle.start.y;
            }
            turtle.bound_left_top = Vec2{x:std::f32::INFINITY, y:std::f32::INFINITY};
            turtle.bound_right_bottom = Vec2{x:std::f32::NEG_INFINITY, y:std::f32::NEG_INFINITY};
        }
    }

    pub fn delta_turtle(&mut self, dx:f32, dy:f32){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.walk.x += dx;
            turtle.walk.y += dy;
        }
    }

     pub fn get_turtle_walk(&self)->Vec2{
        if let Some(turtle) = self.turtles.last(){
            turtle.walk
        }
        else{
            Vec2::zero()
        }
    }

    pub fn set_turtle_walk(&mut self, walk:Vec2){
        if let Some(turtle) = self.turtles.last_mut(){
            turtle.walk = walk
        }
    }

     pub fn get_rel_turtle_walk(&self)->Vec2{
        if let Some(turtle) = self.turtles.last(){
            Vec2{x:turtle.walk.x - turtle.start.x, y:turtle.walk.y - turtle.start.y}
        }
        else{
            Vec2::zero()
        }
    }    

    pub fn visible_in_turtle(&self, geom:Rect, scroll:Vec2)->bool{
        if let Some(turtle) = self.turtles.last(){
            let view = Rect{
                x:scroll.x,//- margin.l,
                y:scroll.y,// - margin.t,
                w:turtle.width,// + margin.l + margin.r,
                h:turtle.height,// + margin.t + margin.b 
            };

            return view.intersects(geom)
        }
        else{
            false
        }
    }

    // end a turtle returning computed geometry
    pub fn end_turtle(&mut self, guard_area:Area)->Rect{
        let old = self.turtles.pop().unwrap();
        if guard_area != old.guard_area{
            panic!("End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}", old.guard_area,  guard_area)
        }

        let w = if old.width.is_nan(){
            if old.bound_right_bottom.x == std::f32::NEG_INFINITY{ // nothing happened, use padding
                Bounds::Fix(old.layout.padding.l + old.layout.padding.r)
            }
            else{ // use the bounding box
                Bounds::Fix(max_zero_keep_nan(old.bound_right_bottom.x - old.start.x + old.layout.padding.r))
            }
        }
        else{
            Bounds::Fix(old.width)
        };

        let h = if old.height.is_nan(){
            if old.bound_right_bottom.y == std::f32::NEG_INFINITY{ // nothing happened use the padding
                Bounds::Fix(old.layout.padding.t + old.layout.padding.b)
            }
            else{ // use the bounding box
                Bounds::Fix(max_zero_keep_nan(old.bound_right_bottom.y - old.start.y + old.layout.padding.b))
            }
        }
        else{
            Bounds::Fix(old.height)
        };

        let margin = old.layout.margin.clone();
        // if we have alignment set, we should now align our childnodes
        let align_delta = Self::compute_align_turtle(&old);
        if align_delta.x > 0.0 || align_delta.y > 0.0{
            self.do_align(align_delta.x, align_delta.y, old.align_start);
        }
        
        // when a turtle is x-abs / y-abs you dont walk the parent
        if !old.layout.abs_start.is_none(){
            let abs_start = if let Some(abs_start) = old.layout.abs_start{abs_start} else {Vec2::zero()};
            let w = if let Bounds::Fix(vw) = w{vw} else {0.};
            let h = if let Bounds::Fix(vh) = h{vh} else {0.};
            return Rect{x:abs_start.x, y:abs_start.y, w:w, h:h};
        }

        //if self.turtles.len() == 0{
        //    return Rect{x:0.0, y:0.0, w:old.width, h:old.height};
       // }

        return self.walk_turtle(w, h, margin, Some(&old))
    }

    pub fn width_left(&self, abs:bool)->f32{
        if !abs{
            if let Some(turtle) = self.turtles.last(){
                let nan_val = max_zero_keep_nan(turtle.width - turtle.width_used - (turtle.walk.x-turtle.start.x) );
                if nan_val.is_nan(){ // if we are a computed height, if some value is known, use that
                    if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY{
                        return turtle.bound_right_bottom.x - turtle.start.x
                    }
                }
                return nan_val
            }
        }
        return self.target_size.x
    }

    pub fn width_total(&self, abs:bool)->f32{
        if !abs{
            if let Some(turtle) = self.turtles.last(){
                let nan_val = max_zero_keep_nan(turtle.width - (turtle.layout.padding.l+turtle.layout.padding.r));
                if nan_val.is_nan(){ // if we are a computed width, if some value is known, use that
                    if turtle.bound_right_bottom.x != std::f32::NEG_INFINITY{
                        return turtle.bound_right_bottom.x - turtle.start.x
                    }
                }
                return nan_val
            }
        }
        return self.target_size.x
    }

    pub fn height_left(&self, abs:bool)->f32{
        if !abs{
            if let Some(turtle) = self.turtles.last(){
                let nan_val = max_zero_keep_nan(turtle.height - turtle.height_used - (turtle.walk.y -turtle.start.y) );
                if nan_val.is_nan(){ // if we are a computed height, if some value is known, use that
                    if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY{
                        return turtle.bound_right_bottom.y - turtle.start.y
                    }
                }
                return nan_val
            }
        }
        return self.target_size.y
    }

    pub fn height_total(&self, abs:bool)->f32{
        if !abs{
            if let Some(turtle) = self.turtles.last(){
                let nan_val = max_zero_keep_nan(turtle.height - (turtle.layout.padding.t+turtle.layout.padding.b));
                if nan_val.is_nan(){ // if we are a computed height, if some value is known, use that
                    if turtle.bound_right_bottom.y != std::f32::NEG_INFINITY{
                        return turtle.bound_right_bottom.y - turtle.start.y
                    }
                }
                return nan_val
            }
        }
        return self.target_size.y
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


#[derive(Clone)]
pub enum Bounds{
    Fill,
    Fix(f32),
    Compute,
    FillPad(f32),
    FillScale(f32),
    FillScalePad(f32,f32),
    Scale(f32),
    ScalePad(f32, f32),
}

impl Default for Bounds{
    fn default()->Self{
        Bounds::Fill
    }
}

impl Bounds{
    pub fn eval_width(&self, cx: &Cx, margin:Margin, abs:bool)->f32{
        match self{
            Bounds::Compute=>std::f32::NAN,
            Bounds::Fix(v)=>*v,
            Bounds::Fill=>cx.width_left(abs) - (margin.l + margin.r),
            Bounds::FillPad(p)=>cx.width_left(abs) - p - (margin.l + margin.r),
            Bounds::FillScale(s)=>cx.width_left(abs)*s - (margin.l + margin.r),
            Bounds::FillScalePad(s,p)=>cx.width_left(abs)*s - p - (margin.l + margin.r),
            Bounds::Scale(s)=> cx.width_total(abs) * s - (margin.l + margin.r),
            Bounds::ScalePad(s, p)=>cx.width_total(abs) * s - p - (margin.l + margin.r),
        }
    }

    pub fn eval_height(&self, cx: &Cx, margin:Margin, abs:bool)->f32{
        match self{
            Bounds::Compute=>std::f32::NAN,
            Bounds::Fix(v)=>*v,
            Bounds::Fill=>cx.height_left(abs) - (margin.t + margin.b),
            Bounds::FillPad(p)=>cx.height_left(abs) - p - (margin.t + margin.b),
            Bounds::FillScale(s)=>cx.height_left(abs)*s - (margin.t + margin.b),
            Bounds::FillScalePad(s,p)=>cx.height_left(abs)*s - p - (margin.t + margin.b),
            Bounds::Scale(s)=> cx.height_total(abs) * s - (margin.t + margin.b),
            Bounds::ScalePad(s, p)=>cx.height_total(abs) * s - p - (margin.t + margin.b),
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Align{
    pub fx:f32,
    pub fy:f32
}

impl Align{
    pub fn left_top()->Align{Align{fx:0.,fy:0.}}
    pub fn center_top()->Align{Align{fx:0.5,fy:0.0}}
    pub fn right_top()->Align{Align{fx:1.0,fy:0.0}}
    pub fn left_center()->Align{Align{fx:0.0,fy:0.5}}
    pub fn center()->Align{Align{fx:0.5,fy:0.5}}
    pub fn right_center()->Align{Align{fx:1.0,fy:0.5}}
    pub fn left_bottom()->Align{Align{fx:0.,fy:1.0}}
    pub fn center_bottom()->Align{Align{fx:0.5,fy:1.0}}
    pub fn right_bottom()->Align{Align{fx:1.0,fy:1.0}}
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Margin{
    pub l:f32,
    pub t:f32,
    pub r:f32,
    pub b:f32
}

impl Margin{
    pub fn zero()->Margin{
        Margin{l:0.0,t:0.0,r:0.0,b:0.0}
    }

    pub fn all(v:f32)->Margin{
        Margin{l:v,t:v,r:v,b:v}
    }
}

impl Rect{
    pub fn contains_with_margin(&self, x:f32, y:f32, margin:&Option<Margin>)->bool{
        if let Some(margin) = margin{
            return x >= self.x - margin.l && x <= self.x + self.w + margin.r &&
                   y >= self.y - margin.t && y <= self.y + self.h + margin.b;
        }
        else{
            return self.contains(x, y);
        }

    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Padding{
    pub l:f32,
    pub t:f32,
    pub r:f32,
    pub b:f32
}

impl Padding{
    pub fn zero()->Padding{
        Padding{l:0.0,t:0.0,r:0.0,b:0.0}
    }
    pub fn all(v:f32)->Padding{
        Padding{l:v,t:v,r:v,b:v}
    }
}


#[derive(Clone)]
pub enum Direction{
    Left,
    Right,
    Up,
    Down
}

impl Default for Direction{
    fn default()->Self{
        Direction::Right
    }
}

#[derive(Clone)]
pub enum Axis{
    Horizontal,
    Vertical
}

impl Default for Axis{
    fn default()->Self{
        Axis::Horizontal
    }
}

#[derive(Clone)]
pub enum LineWrap{
    None,
    NewLine
}
impl Default for LineWrap{
    fn default()->Self{
        LineWrap::None
    }
}

#[derive(Clone, Default)]
pub struct Layout{
    pub margin:Margin,
    pub padding:Padding,
    pub align:Align,
    pub direction:Direction,
    pub line_wrap:LineWrap,
    pub abs_start:Option<Vec2>,
    pub width:Bounds,
    pub height:Bounds,
}

#[derive(Clone, Default)]
pub struct Turtle{
    pub align_start:usize,
    pub walk:Vec2,
    pub start:Vec2,
    pub bound_left_top:Vec2,
    pub bound_right_bottom:Vec2,
    pub width:f32,
    pub height:f32,
    pub width_used:f32,
    pub height_used:f32,
    pub biggest:f32,
    pub layout:Layout,
    pub guard_area:Area
} 
//#[derive(Clone, Default)]
//pub struct CxTurtle{
 //   pub debug_pts:RefCell<Vec<(f32,f32,i32)>>
//}


pub fn max_zero_keep_nan(v:f32)->f32{
    if v.is_nan(){
        v
    }
    else{
        f32::max(v,0.0)
    }
}