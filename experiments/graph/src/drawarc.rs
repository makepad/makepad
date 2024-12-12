
use crate::{makepad_draw::*, makepad_widgets::*};

live_design! {
    use makepad_draw::shader::std::*;
   
    DrawCornerArc = {{DrawCornerArc}} {
       
        fn stroke(self, side:float, progress: float) -> vec4{
            return self.color;
        }

        fn roundedBoxSDF(self, CenterPosition:vec2,  Size: vec2,  Radius: float) -> float{
            return length(max(abs(CenterPosition)-Size+Radius,0.0))-Radius;
        }

        fn pixel(self) -> vec4 {
            let pixelpos = self.pos * self.rect_size;
               let dist= max(self.roundedBoxSDF(pixelpos-self.center, self.xysize+vec2(self.width, self.width ), self.radius+self.width/2.),-self.roundedBoxSDF(pixelpos-self.center, self.xysize-vec2(self.width, self.width), self.radius-self.width));
            
            // let dist= self.roundedBoxSDF(pixelpos-self.center, self.xysize-vec2(self.width, self.width), self.radius-self.width);
            
          
          
            //return self.color*sin(dist)/dist;
            let linemult = smoothstep(-1.,1., dist);
            let C = self.stroke(dist, 0);
            return C * (1. - linemult) + vec4((1.-self.color.x)*0.1,(1.-self.color.y)*0.1,(1.-self.color.z)*0.1, 0);
        }
    }

    DrawArc = {{DrawArc}} {
       
        fn stroke(self, side:float, progress: float) -> vec4{
            return self.color;
        }

        // adapted from iq.
        fn square_capped_arc( p: vec2, n:vec2, radius:float, thickness:float ) -> float{
            p.x = abs(p.x); 
            p = mat2(n.x,n.y,-n.y,n.x)*p;

            return max( abs(length(p)-radius)-thickness*0.5,
                        length(vec2(p.x,max(0.0,abs(radius-p.y)-thickness*0.5)))*sign(p.x) );
        }


        fn round_capped_arc (self, p:vec2, a0:float, a1:float, r:float ) -> float{

            let a = mod(atan(p.y, p.x), 6.283);

            let  ap = a - a0;
            if (ap < 0.) {
               ap+=6.283;
            }
            
            let  a1p = a1 - a0;

            if (a1p < 0.) {
                a1p += 6.283;
            }

            if (ap >= a1p) {
                let q0 = vec2(r * cos(a0), r * sin(a0));
                let q1 = vec2(r * cos(a1), r * sin(a1));
                return min(length(p - q0), length(p - q1));
            }

            return abs(length(p) - r);
        }

        fn pixel(self) -> vec4 {
            let pixelpos = self.pos * self.rect_size;
            let dist= self.round_capped_arc(pixelpos-self.center, self.arc_a0,self.arc_a1, self.radius);
            let linemult = smoothstep(self.width-1., self.width, dist);
            let C = self.stroke(dist, 0);
            return C * (1. - linemult) + vec4((1.-self.color.x)*0.1,(1.-self.color.y)*0.1,(1.-self.color.z)*0.1, 0);
        }
    }



    VectorArc = {{VectorArc}} {
        width: Fill,
        height: Fill
    }

    VectorCornerArc = {{VectorCornerArc}}{
        width: Fill, 
        height: Fill
    }
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
struct DrawArc {
    #[deref]
    draw_super: DrawQuad,
    #[calc] arc_start: Vec2,
    #[calc] arc_end: Vec2,
    #[calc] center: Vec2,
    #[calc] width: f32,
    #[calc] arc_a0: f32,
    #[calc] arc_a1: f32,
    #[calc] radius: f32,
    #[calc] color: Vec4,
}

#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
struct DrawCornerArc {
    #[deref]
    draw_super: DrawQuad,
    #[calc] xysize: Vec2,
    #[calc] center: Vec2,
    #[calc] width: f32,
    #[calc] radius: f32,
    #[calc] color: Vec4,
}

#[derive(Copy, Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum QuadCorner
{
    TopLeft,
    TopRight,
    #[pick] BottomRight,
    BottomLeft,
    UnspecifiedCorner
}

impl QuadCorner {
    fn _from_u32(value: u32) -> QuadCorner {
        match value {
            0 => QuadCorner::TopLeft,
            1 => QuadCorner::TopRight,
            2 => QuadCorner::BottomRight,
            3 => QuadCorner::BottomLeft,
           
            _ => QuadCorner::UnspecifiedCorner
        }
    }
}

fn _get_rect_corner( input:Rect, corner:QuadCorner) -> DVec2{
    match corner{
        QuadCorner::TopLeft => input.pos,
        QuadCorner::TopRight => dvec2(input.pos.x + input.size.x, input.pos.y),
        QuadCorner::BottomRight => input.pos+input.size,
        QuadCorner::BottomLeft => dvec2(input.pos.x , input.pos.y+ input.size.y),
        _ => {dvec2(0.,0.)}
    }
}

fn _get_rect_corner_i32( input:Rect, corner:i32) -> DVec2{
    match corner{
        0 /*topleft  */  => input.pos,
        1 /*topright */  => dvec2(input.pos.x + input.size.x, input.pos.y),
        2 /*bottomright */=> input.pos+input.size,
        3 /*bottomleft */=> dvec2(input.pos.x , input.pos.y+ input.size.y),
_ => {dvec2(0.,0.)}
    }
}


fn get_rect_corner_u32( input:Rect, corner:u32) -> DVec2{
    match corner{
        0 /*topleft  */  => input.pos,
        1 /*topright */  => dvec2(input.pos.x + input.size.x, input.pos.y),
        2 /*bottomright */=> input.pos+input.size,
        3 /*bottomleft */=> dvec2(input.pos.x , input.pos.y+ input.size.y),
_ => {dvec2(0.,0.)}
    }
}

#[derive(Copy, Clone, Debug, Live, LiveHook, PartialEq)]
#[live_ignore]
pub enum Winding{
    #[pick]ClockWise,
    CounterClockWise
}

#[derive(Live, LiveHook, Widget)]
pub struct VectorArc{
    #[walk] walk: Walk,
    #[live] draw_arc: DrawArc,
    #[redraw] #[rust] area: Area,
    #[live(true)] contained: bool,
    
    #[live(15.0)] line_width: f64,
    #[live] color: Vec4,
   
   #[live(QuadCorner::UnspecifiedCorner)] arc_start_corner: QuadCorner,
   #[live(QuadCorner::UnspecifiedCorner)] arc_end_corner: QuadCorner,
   #[live(Winding::ClockWise)] arc_winding: Winding,
   
    #[rust(dvec2(350., 10.))] arc_start: DVec2,
    #[rust(dvec2(1000., 1440.))] arc_end: DVec2,
    #[rust(dvec2(1000., 1440.))] arc_center: DVec2,
   
}

#[derive(Live, LiveHook, Widget)]
pub struct VectorCornerArc
{
    #[walk] walk: Walk,
    #[live] draw_arc: DrawCornerArc,
    #[redraw] #[rust] area: Area,
    #[live(true)] contained: bool,
    
    #[live(15.0)] line_width: f64,
    #[live(vec4(1.,1.,0.,1.))] color: Vec4,  
    #[live(QuadCorner::TopLeft)] corner: QuadCorner,
}

impl Widget for VectorCornerArc {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // lets draw a bunch of quads
        let mut fullrect = cx.walk_turtle_with_area(&mut self.area, walk);
                    
        let mut rect = fullrect;
        let hw = self.line_width / 2.;
                            
        if self.contained{
            rect.size.x -= self.line_width;
            rect.size.y -= self.line_width;
            rect.pos.x += hw;
            rect.pos.y += hw;
                                        
        } else {
            fullrect.size.x+= self.line_width;
            fullrect.size.y += self.line_width;
            fullrect.pos.x -= hw;
            fullrect.pos.y -= hw;
        }
                          
        let _maxpixels = 300. as f64;
                           
        let mut _usecomputedcenter = true;
                    
        let cmid = get_rect_corner_u32(rect, (self.corner as u32 + 2)%4);
        let copp = get_rect_corner_u32(rect, (self.corner as u32 )%4);
                    
        let center = cmid + (cmid-copp);
                            
                    
        let shortedge = min(rect.size.x, rect.size.y);
        let computedradius = shortedge;
                          
                    
                               
        self.draw_arc.radius = computedradius as f32;//(arc_start - arc_center).length() as f32;
        self.draw_arc.center = (center - fullrect.pos).into_vec2();        
        self.draw_arc.xysize = rect.size.into_vec2() * 2.;
        self.draw_arc.color = self.color;
        self.draw_arc.width = (self.line_width/2.) as f32;
        self.draw_arc.draw_abs(cx,fullrect);
                    
        DrawStep::done()
    }
}

impl Widget for VectorArc {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // lets draw a bunch of quads
        let mut fullrect = cx.walk_turtle_with_area(&mut self.area, walk);
        
        let mut rect = fullrect;
        let hw = self.line_width / 2.;
                
        if self.contained{
            rect.size.x -= self.line_width;
            rect.size.y -= self.line_width;
            rect.pos.x += hw;
            rect.pos.y += hw;
                        
        } else {
            fullrect.size.x+= self.line_width;
            fullrect.size.y += self.line_width;
            fullrect.pos.x -= hw;
            fullrect.pos.y -= hw;
        }
              
        let _maxpixels = 300. as f64;
        let mut arc_start = self.arc_start;
        let mut arc_end = self.arc_end;
        let mut usecomputedcenter = true;
        
        match self.arc_start_corner
        {
            QuadCorner::TopLeft => {arc_start = dvec2(rect.pos.x, rect.pos.y )}
            QuadCorner::TopRight => {arc_start = dvec2(rect.pos.x + rect.size.x, rect.pos.y )}
            QuadCorner::BottomRight => {arc_start = dvec2(rect.pos.x + rect.size.x, rect.pos.y +rect.size.y)}
            QuadCorner::BottomLeft => {arc_start = dvec2(rect.pos.x, rect.pos.y +rect.size.y)}
                     
            _ => {usecomputedcenter = false;}
        }
                
        match self.arc_end_corner
        {
            QuadCorner::TopLeft => {arc_end = dvec2(rect.pos.x, rect.pos.y )}
            QuadCorner::TopRight => {arc_end = dvec2(rect.pos.x + rect.size.x, rect.pos.y )}
            QuadCorner::BottomRight => {arc_end = dvec2(rect.pos.x + rect.size.x, rect.pos.y +rect.size.y)}
            QuadCorner::BottomLeft => {arc_end = dvec2(rect.pos.x, rect.pos.y +rect.size.y)}
                     
            _ => {usecomputedcenter = false;}
        }
        
        let shortedge = min(rect.size.x, rect.size.y);
        let mut computedradius = shortedge;
        let mut arc_center = self.arc_center;
        
        if usecomputedcenter
        {
            let _endcornerid = self.arc_end_corner as u32;
            let startcornerid = self.arc_start_corner as u32;
                                
            let cornerdelta = ((self.arc_end_corner as i32 - self.arc_start_corner as i32 + 4) as u32)%4;
            match cornerdelta
            {
                0 => {}
                1 => {computedradius*=0.5;arc_center = (get_rect_corner_u32(rect, startcornerid) +  get_rect_corner_u32(rect, (startcornerid+1)%4)) /2.}
                2 => {arc_center = 
                    match self.arc_winding
                    { 
                        Winding::ClockWise => get_rect_corner_u32(rect, (startcornerid+3)%4), 
                        Winding::CounterClockWise => get_rect_corner_u32(rect, (startcornerid+1)%4)                                                          
                    }
                }
                3 => {computedradius*=0.5;arc_center = (get_rect_corner_u32(rect, startcornerid) +  get_rect_corner_u32(rect, (startcornerid+3)%4)) /2.}
                _ => {}
            }
            
        }
                
        if  Winding::CounterClockWise == self.arc_winding
        {
            std::mem::swap(&mut arc_start, &mut arc_end);
                      
        }
        self.draw_arc.radius = computedradius as f32;//(arc_start - arc_center).length() as f32;
        self.draw_arc.arc_a0 = (arc_start - arc_center).angle_in_radians() as f32;
        self.draw_arc.arc_a1 = (arc_end - arc_center).angle_in_radians() as f32;
        self.draw_arc.center = (arc_center - fullrect.pos).into_vec2();        
                
        self.draw_arc.color = self.color;
        self.draw_arc.width = (self.line_width/2.) as f32;
        self.draw_arc.draw_abs(cx,fullrect);
                    
        DrawStep::done()
    }
}

