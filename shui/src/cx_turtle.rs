use crate::cx::*;

impl Cx{
    //pub fn debug_pt(&self, x:f32, y:f32, color:i32){
        //self.debug_pts.borrow_mut().push((x,y,color));
    //}

    pub fn instance_aligned_set_count(&mut self, instance_count:usize)->Area{
        let mut area =  self.align_list.last_mut().unwrap();
        if let Area::Instance(inst) = &mut area{
            inst.instance_count = instance_count;
        }
        area.clone()
    }

    // begin a new turtle with a layout
    pub fn begin_turtle(&mut self, layout:&Layout){

        let start_x = if layout.x_abs.is_none(){
            if let Some(parent) = self.turtles.last(){
                parent.walk_x
            }
            else{
                0.0
            }
        }
        else{
            layout.x_abs.unwrap()
        };
        let start_y = if layout.y_abs.is_none(){
            if let Some(parent) = self.turtles.last(){
                parent.walk_y
            }
            else{
                0.0
            }
        }
        else{
            layout.y_abs.unwrap()
        };

        let width = layout.w.eval_width(self);
        let height = layout.h.eval_height(self);

        self.turtles.push(Turtle{
            align_start:self.align_list.len(),
            layout:layout.clone(),
            start_x:start_x,
            start_y:start_y,
            walk_x:start_x + layout.padding.l,
            walk_y:start_y + layout.padding.t,
            biggest:0.0,
            bound_x1:std::f32::INFINITY,
            bound_x2:std::f32::NEG_INFINITY,
            bound_y1:std::f32::INFINITY,
            bound_y2:std::f32::NEG_INFINITY,
            width:width,
            height:height,
            ..Default::default()
        });
    }

    // walk the turtle with a 'w/h' and a margin
    pub fn walk_turtle(&mut self, vw:Bounds, vh:Bounds, margin:Margin, old_turtle:Option<&Turtle>)->Rect{
        let w = nan_clamp_neg(vw.eval_width(self));
        let h = nan_clamp_neg(vh.eval_height(self));
        let mut do_align = false;
        let mut align_dx = 0.0;
        let mut align_dy = 0.0;
        let ret = if let Some(turtle) = self.turtles.last_mut(){
            let (x,y) = match turtle.layout.direction{
                Direction::Right=>{
                    if !turtle.layout.nowrap && (turtle.walk_x + margin.l + w) >
                        (turtle.start_x + turtle.width - turtle.layout.padding.r){
                        // what is the move delta. 
                        let old_x = turtle.walk_x;
                        let old_y = turtle.walk_y;
                        turtle.walk_x = turtle.start_x + turtle.layout.padding.l;
                        turtle.walk_y += turtle.biggest;
                        turtle.biggest = 0.0;
                        do_align = true;
                        align_dx = turtle.walk_x - old_x;
                        align_dy = turtle.walk_y - old_y;
                    }
                    let x = turtle.walk_x + margin.l;
                    let y = turtle.walk_y + margin.t;
                    // walk it normally
                    turtle.walk_x += w + margin.l + margin.r;

                    // keep track of biggest item in the line (include item margin bottom)
                    let biggest = h + margin.t + margin.b;
                    if biggest > turtle.biggest{
                        turtle.biggest = biggest;
                    }
                    // update x2 bounds
                    let bound_x2 = x + w; 
                    if bound_x2 > turtle.bound_x2{
                        turtle.bound_x2 = bound_x2;
                    }
                    // update y2 bounds (does not include item margin bottom)
                    let bound_y2 = turtle.walk_y + h + margin.t;
                    if bound_y2 > turtle.bound_y2{
                        turtle.bound_y2 = bound_y2;
                    }
                    (x,y)
                },
                _=>{
                    (turtle.walk_x + margin.l, turtle.walk_y + margin.t)
                }
            };
            if x < turtle.bound_x1{
                turtle.bound_x1 = x;
            }
            if y < turtle.bound_y1{
                turtle.bound_y1 = y;
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

        if do_align{
            if let Some(old_turtle) = old_turtle{
                self.do_align(align_dx, align_dy, old_turtle);
            }
        };

        ret
    }

    pub fn new_line(&mut self){
        if let Some(turtle) = self.turtles.last_mut(){
            match turtle.layout.direction{
                Direction::Right=>{
                    turtle.walk_x = turtle.start_x + turtle.layout.padding.l;
                    turtle.walk_y += turtle.biggest;
                    turtle.biggest = 0.0;
                },
                _=>()
            }
        }
    }

    fn do_align(&mut self, dx:f32, dy:f32, turtle:&Turtle){

        for i in turtle.align_start..self.align_list.len(){
            let align_item = &self.align_list[i];
            // please note, this is supposed to be move_xy IN area but
            // the rust borrowchecker won't let me. This is because the 'cd' arg
            // locks the whole thing, but i just need a few subprops
            // this is a shortcoming in the rust borrowchecker imho, but its the only one in this
            // new arrangement of everything on cxdrawing. so whatever.
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
                x:turtle.start_x,
                y:turtle.start_y,
                w:turtle.width,
                h:turtle.height
            }
        };
        return Rect::zero();
    }

    pub fn turtle_bounds(&self)->Vec2{
        if let Some(turtle) = self.turtles.last(){
            return vec2(turtle.bound_x2 + turtle.layout.padding.r, turtle.bound_y2 + turtle.layout.padding.b);
        }
        return vec2(0.0,0.0)        
    }

    // end a turtle returning computed geometry
    pub fn end_turtle(&mut self)->Rect{
        let old = self.turtles.pop().unwrap();
    
        let w = if old.width.is_nan(){
            if old.bound_x2 == std::f32::NEG_INFINITY{ // nothing happened, use padding
                Bounds::Fix(old.layout.padding.l + old.layout.padding.r)
            }
            else{ // use the bounding box
                Bounds::Fix(nan_clamp_neg(old.bound_x2 - old.start_x + old.layout.padding.r))
            }
        }
        else{
            Bounds::Fix(old.width)
        };

        let h = if old.height.is_nan(){
            if old.bound_y2 == std::f32::NEG_INFINITY{ // nothing happened use the padding
                Bounds::Fix(old.layout.padding.t + old.layout.padding.b)
            }
            else{ // use the bounding box
                Bounds::Fix(nan_clamp_neg(old.bound_y2 - old.start_y + old.layout.padding.b))
            }
        }
        else{
            Bounds::Fix(old.height)
        };

        let margin = old.layout.margin.clone();
        // if we have alignment set, we should now align our childnodes
        if old.layout.align.fx > 0.0 || old.layout.align.fy > 0.0{

            let mut dx = old.layout.align.fx * 
                ((old.width - (old.layout.padding.l + old.layout.padding.r)) - (old.bound_x2 - (old.start_x + old.layout.padding.l)));
            let mut dy = old.layout.align.fy * 
                ((old.height - (old.layout.padding.t + old.layout.padding.b)) - (old.bound_y2 - (old.start_y + old.layout.padding.t)));
            if dx.is_nan(){dx = 0.0}
            if dy.is_nan(){dy = 0.0}

            self.do_align(dx, dy, &old);
        }

        // now well walk the top turtle with our old geometry and margins
        //if self.turtles.len() == 0{
        //    return Rect{x:0.0, y:0.0, w:old.width, h:old.height};
       // }

        return self.walk_turtle(w, h, margin, Some(&old))
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
    Compute,
    Fix(f32),
    Scale(f32),
    ScalePad(f32, f32),
    Fill
}

impl Default for Bounds{
    fn default()->Self{
        Bounds::Compute
    }
}

impl Bounds{
    pub fn eval_width(&self, cx: &Cx)->f32{
        match self{
            Bounds::Compute=>std::f32::NAN,
            Bounds::Fix(v)=>*v,
            Bounds::Scale(v)=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.width - (turtle.layout.padding.l+turtle.layout.padding.r)) * v
                }
                else{
                    cx.target_size.x * v
                }
            },
            Bounds::ScalePad(v, p)=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.width - (turtle.layout.padding.l+turtle.layout.padding.r)) * v - p
                }
                else{
                    cx.target_size.x * v - p
                }
            },
            Bounds::Fill=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.width - (turtle.layout.padding.l+turtle.layout.padding.r)) 
                }
                else{
                    cx.target_size.x 
                }
            }
        }
    }

    pub fn eval_height(&self, cx: &Cx)->f32{
        match self{
            Bounds::Compute=>std::f32::NAN,
            Bounds::Fix(v)=>*v,
            Bounds::Scale(v)=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.height - (turtle.layout.padding.t+turtle.layout.padding.b))* v  
                }
                else{
                    cx.target_size.y * v
                }
            },
            Bounds::ScalePad(v, p)=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.height - (turtle.layout.padding.t+turtle.layout.padding.b))* v  - p
                }
                else{
                    cx.target_size.y * v - p
                }
            },
            Bounds::Fill=>{
                if let Some(turtle) = cx.turtles.last(){
                    nan_clamp_neg(turtle.height - (turtle.layout.padding.t+turtle.layout.padding.b))
                }
                else{
                    cx.target_size.y 
                }
            }
        }
    }
}

#[derive(Clone, Default, Debug)]
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

#[derive(Clone, Default, Debug)]
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

#[derive(Clone, Default, Debug)]
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
pub enum Orientation{
    Horizontal,
    Vertical
}

impl Default for Orientation{
    fn default()->Self{
        Orientation::Horizontal
    }
}

#[derive(Clone, Default)]
pub struct Layout{
    pub margin:Margin,
    pub padding:Padding,
    pub align:Align,
    pub direction:Direction,
    pub nowrap:bool,
    pub x_abs:Option<f32>,
    pub y_abs:Option<f32>,
    pub w:Bounds,
    pub h:Bounds,
}

impl Layout{
    pub fn new()->Layout{
        Layout{
            ..Default::default()
        }
    }
}

#[derive(Clone, Default)]
pub struct Turtle{
    pub align_start:usize,
    pub walk_x:f32,
    pub walk_y:f32,
    pub start_x:f32,
    pub start_y:f32,
    pub bound_x1:f32,
    pub bound_x2:f32,
    pub bound_y1:f32,
    pub bound_y2:f32,
    pub width:f32,
    pub height:f32,
    pub biggest:f32,
    pub layout:Layout
} 
//#[derive(Clone, Default)]
//pub struct CxTurtle{
 //   pub debug_pts:RefCell<Vec<(f32,f32,i32)>>
//}


pub fn nan_clamp_neg(v:f32)->f32{
    if v.is_nan(){
        v
    }
    else{
        f32::max(v,0.0)
    }
}