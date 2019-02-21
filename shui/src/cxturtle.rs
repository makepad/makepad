pub use crate::math::*;

#[derive(Clone)]
pub enum Value{
    None,
    Const(f32),
    Expr(String)
}

impl Default for Value{
    fn default()->Self{
        Value::None
    }
}

impl Value{
    fn eval(&self, cx_turtle: &CxTurtle)->f32{
        match self{
            Value::None=>std::f32::NAN,
            Value::Const(v)=>*v,
            Value::Expr(_v)=>0.0
        }
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
pub struct Lay{
    pub margin:Pad,
    pub padding:Pad,
    pub align:Vec2,
    pub direction:Direction,
    pub wrap:bool,
    pub x:Value,
    pub y:Value,
    pub w:Value,
    pub h:Value,
}

impl Lay{
    pub fn dynamic()->Lay{
        Lay{..Default::default()}
    }

    pub fn padded(around:f32)->Lay{
        Lay{
            padding:Pad{
                l:around,t:around,r:around,b:around
            },
            ..Default::default()
        }
    }
    pub fn sized(w:f32, h:f32)->Lay{
        Lay{
            w:Value::Const(w),
            h:Value::Const(h),
            ..Default::default()
        }
    }
}

#[derive(Clone, Default)]
pub struct Turtle{
    pub walk_x:f32,
    pub walk_y:f32,
    pub init_x:f32,
    pub init_y:f32,
    pub start_x:f32,
    pub start_y:f32,
    pub bound_x1:f32,
    pub bound_x2:f32,
    pub bound_y1:f32,
    pub bound_y2:f32,
    pub width:f32,
    pub height:f32,
    pub biggest:f32,
    pub lay:Lay,
    pub parent:Lay
} 

impl Turtle{

}

#[derive(Clone, Default)]
pub struct CxTurtle{
    pub turtles:Vec<Turtle>
}

impl CxTurtle{
    // begin a new turtle
    pub fn begin(&mut self, lay:&Lay){
        let parent_lay;
        let mut init_x;
        let mut init_y;
        let mut width;
        let mut height;

        // we have a parent, take walk/width/height
        if let Some(parent) = self.turtles.last(){
            parent_lay = parent.lay.clone();
            // if lay.x is None p_x is parent
            init_x = lay.x.eval(self);
            if init_x.is_nan(){ init_x = parent.walk_x; }

            init_y = lay.y.eval(self);
            if init_y.is_nan(){ init_y = parent.walk_x; }

            width = lay.w.eval(self);
            if width.is_nan(){ width = parent.width; }

            height = lay.w.eval(self);
            if height.is_nan(){ height = parent.width; }
        }
        else{ // we don't have a parent, make a new one
            parent_lay = Lay::dynamic();
  
            init_x = lay.x.eval(self);
            if init_x.is_nan(){ init_x = 0.0; }
 
            init_y = lay.y.eval(self);
            if init_y.is_nan(){ init_y = 0.0; }

            width = lay.w.eval(self);
            height = lay.w.eval(self);
        };

        let start_x = init_x + lay.padding.l + lay.margin.l;
        let start_y = init_y + lay.padding.t + lay.margin.t;
        self.turtles.push(Turtle{
            lay:lay.clone(),
            parent:parent_lay,
            init_x:init_x,
            init_y:init_y,
            walk_x:start_x,
            walk_y:start_y,
            start_x:start_x,
            start_y:start_y,
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
    pub fn walk_wh(&mut self, vw:Value, vh:Value, margin:Pad)->Rect{
        let w = vw.eval(self);
        let h = vh.eval(self);
        let mut turtle = &mut self.turtles.last_mut().unwrap();
        match turtle.lay.direction{
            Direction::Right=>{
                // wrap the turtle
                if turtle.parent.wrap && turtle.walk_x + w + margin.l + margin.r >
                    turtle.start_x + turtle.width{
                    turtle.walk_x = turtle.start_x;
                    turtle.walk_y += turtle.biggest;
                    turtle.biggest = 0.0;
                }
                // walk it normally
                else{
                    turtle.walk_x += w + margin.l + margin.r;
                }
                // keep track of biggest item in the line
                let big = h + margin.t + margin.b;
                if big > turtle.biggest{
                    turtle.biggest = big;
                }
                // update bounds
                if turtle.walk_x > turtle.bound_x2{
                    turtle.bound_x2 = turtle.walk_x;
                }
                let biggest_y = turtle.walk_y + turtle.biggest;
                if biggest_y > turtle.bound_y2{
                    turtle.bound_y2 = biggest_y;
                }
            },
            _=>{}
        }
        let x = turtle.walk_x + margin.l;
        let y = turtle.walk_y + margin.t;
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

    pub fn end(&mut self)->Rect{
        // this is where the turtle does its thing.
        // and where we walk it with our lay
        self.turtles.pop();
        Rect{
            x:0.0,
            y:0.0,
            w:0.0,
            h:0.0
        }
        
    }
}
