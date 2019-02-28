use crate::math::*;
use std::cell::RefCell;
use crate::cxdrawing::*;
use crate::cxshaders::*;
use crate::area::*;

thread_local!(pub static debug_pts_store: RefCell<Vec<(f32,f32,i32,String)>> = RefCell::new(Vec::new()));
pub fn debug_pt(x:f32, y:f32, color:i32, s:&str){
    debug_pts_store.with(|c|{
        let mut store = c.borrow_mut();
        store.push((x,y,color,s.to_string()));
    })
}

/*
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
pub enum Value{
    Computed,
    Fixed(f32),
    Expression(String),
    Percent(f32)
}

impl Default for Value{
    fn default()->Self{
        Value::Computed
    }
}

impl Value{
    pub fn eval_width(&self, cx_turtle: &CxTurtle)->f32{
        match self{
            Value::Computed=>std::f32::NAN,
            Value::Fixed(v)=>*v,
            Value::Expression(_v)=>0.0,
            Value::Percent(v)=>{
                if let Some(turtle) = cx_turtle.turtles.last(){
                    nan_clamp_neg(turtle.width - (turtle.layout.padding.l+turtle.layout.padding.r)) * (v * 0.01)
                }
                else{
                    cx_turtle.target_size.x * (v * 0.01)
                }
            }
        }
    }

    pub fn eval_height(&self, cx_turtle: &CxTurtle)->f32{
        match self{
            Value::Computed=>std::f32::NAN,
            Value::Fixed(v)=>*v,
            Value::Expression(_v)=>0.0,
            Value::Percent(v)=>{
                if let Some(turtle) = cx_turtle.turtles.last(){
                    nan_clamp_neg(turtle.height - (turtle.layout.padding.t+turtle.layout.padding.b))* (v * 0.01)
                }
                else{
                    cx_turtle.target_size.y * (v * 0.01)
                }
            }
        }
    }
}

pub fn percent(v:f32)->Value{
    Value::Percent(v)
}

pub fn fixed(v:f32)->Value{
    Value::Fixed(v)
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
    pub fn i32(v:i32)->Margin{
        Margin{l:v as f32,t:v as f32,r:v as f32,b:v as f32}
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
    pub fn i32(v:i32)->Padding{
        Padding{l:v as f32,t:v as f32,r:v as f32,b:v as f32}
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

#[derive(Clone, Default)]
pub struct Layout{
    pub margin:Margin,
    pub padding:Padding,
    pub align:Align,
    pub direction:Direction,
    pub nowrap:bool,
    pub x:Value,
    pub y:Value,
    pub w:Value,
    pub h:Value,
}

impl Layout{
    pub fn filled()->Layout{
        Layout{
            w:Value::Percent(100.0),
            h:Value::Percent(100.0),
            ..Default::default()
        }
    }

    pub fn filled_padded(around:f32)->Layout{
        Layout{
            w:Value::Percent(100.0),
            h:Value::Percent(100.0),
            padding:Padding{
                l:around,t:around,r:around,b:around
            },
            ..Default::default()
        }
    }

    pub fn filled_paddedf(l:f32,t:f32,r:f32,b:f32)->Layout{
        Layout{
            w:Value::Percent(100.0),
            h:Value::Percent(100.0),
            padding:Padding{
                l:l,t:t,r:r,b:b
            },
            ..Default::default()
        }
    }

    pub fn new()->Layout{
        Layout{..Default::default()}
    }

    pub fn padded(around:f32)->Layout{
        Layout{
            padding:Padding{
                l:around,t:around,r:around,b:around
            },
            ..Default::default()
        }
    }

    pub fn paddedf(l:f32,t:f32,r:f32,b:f32)->Layout{
        Layout{
            padding:Padding{
                l:l,t:t,r:r,b:b
            },
            ..Default::default()
        }
    }

    pub fn sized(w:f32, h:f32)->Layout{
        Layout{
            w:Value::Fixed(w),
            h:Value::Fixed(h),
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

impl Turtle{

}

#[derive(Clone, Default)]
pub struct CxTurtle{
    pub turtles:Vec<Turtle>,
    pub align_list:Vec<Area>,
    pub target_size:Vec2,
    pub target_dpi_factor:f32,
    pub debug_pts:RefCell<Vec<(f32,f32,i32)>>
}

pub struct WalkWrap<'a>{
    turtle:&'a Turtle,
    drawing:&'a mut CxDrawing, 
    shaders:&'a CxShaders
}

impl CxTurtle{
    pub fn debug_pt(&self, x:f32, y:f32, color:i32){
        self.debug_pts.borrow_mut().push((x,y,color));
    }

    pub fn instance_aligned_set_count(&mut self, instance_count:usize)->Area{
        let mut area =  self.align_list.last_mut().unwrap();
        if let Area::Instance(inst) = &mut area{
            inst.instance_count = instance_count;
        }
        area.clone()
    }

    // begin a new turtle with a layout
    pub fn begin(&mut self, layout:&Layout){

        let x = layout.x.eval_width(self);
        let y = layout.y.eval_height(self);
        let (start_x,start_y) = if let Some(parent) = self.turtles.last(){
            // when we have a parent we use the walking position
            (if x.is_nan(){parent.walk_x} else {x},
            if y.is_nan(){parent.walk_y} else {y})
        }
        else{ // we don't have a parent, set to 0
            (if x.is_nan(){0.0} else {x},
            if y.is_nan(){0.0} else {y})
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
    pub fn walk_wh(&mut self, vw:Value, vh:Value, margin:Margin, walk_wrap:Option<WalkWrap>)->Rect{
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
            if let Some(walk_wrap) = walk_wrap{
                self.do_align(align_dx, align_dy, walk_wrap);
            }
        };

        ret
    }

    fn do_align(&self, dx:f32, dy:f32, walkwrap:WalkWrap){
        
        // shift all the items in the drawlist with dx/dy
        for i in walkwrap.turtle.align_start..self.align_list.len(){
            let align_item = &self.align_list[i];
            align_item.move_xy(dx, dy, walkwrap.drawing, walkwrap.shaders);
        }
    }

    // end a turtle returning computed geometry
    pub fn end(&mut self, drawing:&mut CxDrawing, shaders:&CxShaders)->Rect{
        let old = self.turtles.pop().unwrap();
    
        let w = if old.width.is_nan(){
            if old.bound_x2 == std::f32::NEG_INFINITY{ // nothing happened, use padding
                Value::Fixed(old.layout.padding.l + old.layout.padding.r)
            }
            else{ // use the bounding box
                Value::Fixed(nan_clamp_neg(old.bound_x2 - old.start_x + old.layout.padding.r))
            }
        }
        else{
            Value::Fixed(old.width)
        };

        let h = if old.height.is_nan(){
            if old.bound_y2 == std::f32::NEG_INFINITY{ // nothing happened use the padding
                Value::Fixed(old.layout.padding.t + old.layout.padding.b)
            }
            else{ // use the bounding box
                Value::Fixed(nan_clamp_neg(old.bound_y2 - old.start_y + old.layout.padding.b))
            }
        }
        else{
            Value::Fixed(old.height)
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

            self.do_align(dx, dy, WalkWrap{
                turtle:&old,
                drawing:drawing,
                shaders:shaders
            });
        }

        // now well walk the top turtle with our old geometry and margins
        if self.turtles.len() == 0{
            return Rect::zero();
        }

        return self.walk_wh(w, h, margin, Some(WalkWrap{
            turtle:&old,
            drawing:drawing,
            shaders:shaders
        }));
    }
}

pub fn nan_clamp_neg(v:f32)->f32{
    if v.is_nan(){
        v
    }
    else{
        f32::max(v,0.0)
    }
}