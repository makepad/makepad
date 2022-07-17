use {
    crate::{
        makepad_derive_live::*,
        makepad_shader_compiler::ShaderEnum,
        makepad_math::*,
        area::Area,
        cx::Cx,
        live_traits::*,
        draw_2d::draw_quad::DrawQuad
    },
};

live_register!{
    use makepad_platform::shader::std::*;
    DrawShape: {{DrawShape}} {
        varying vertex_color: vec4

        fn vertex(self) -> vec4 {
            //return vec4(self.geom_pos.x,self.geom_pos.y,0.5,1.0);
            let ret =  self.scroll_and_clip_quad();
            match self.fill {
                Fill::Color=>{
                    self.vertex_color = self.color
                }
                Fill::GradientX=>{
                    self.vertex_color = mix(self.color, self.color2, self.pos.x)
                }
                Fill::GradientY=>{
                    self.vertex_color = mix(self.color, self.color2, self.pos.y)
                }
            }
            return ret;
        }
        
        fn pixel(self) -> vec4 {
            let color = self.vertex_color;
            match self.shape {
                Shape::None => {
                    return #000
                }
                Shape::Solid => {
                    return Pal::premul(color)
                }
                Shape::Rect => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.rect(
                        self.inset.x+self.border_width,
                        self.inset.y+self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z+self.border_width*2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w+self.border_width*2.0)
                    )
                    sdf.fill_keep(self.color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result
                }
                Shape::Box => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(
                        self.inset.x+self.border_width,
                        self.inset.y+self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z+self.border_width*2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w+self.border_width*2.0),
                        max(1.0,self.radius.x)
                    )
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result;
                }
                Shape::BoxX => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box_x(
                        self.inset.x+self.border_width,
                        self.inset.y+self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z+self.border_width*2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w+self.border_width*2.0),
                        self.radius.x,
                        self.radius.y
                    )
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result;
                }
                Shape::BoxY => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box_y(
                        self.inset.x+self.border_width,
                        self.inset.y+self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z+self.border_width*2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w+self.border_width*2.0),
                        self.radius.x,
                        self.radius.y
                    )
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result;
                }
                Shape::BoxAll => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.box_all(
                        self.inset.x+self.border_width,
                        self.inset.y+self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z+self.border_width*2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w+self.border_width*2.0),
                        self.radius.x,
                        self.radius.y,
                        self.radius.z,
                        self.radius.w
                    )
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result;
                }              
                Shape::Circle => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    if self.radius.x > 0.0 {
                        sdf.circle(
                            self.rect_size.x * 0.5,
                            self.rect_size.y * 0.5,
                            self.radius.x
                        )
                    }
                    else {
                        sdf.circle(
                            self.rect_size.x * 0.5,
                            self.rect_size.y * 0.5,
                            min(
                                (self.rect_size.x - (self.inset.x + self.inset.z+ 2.0*self.border_width)) * 0.5,
                                (self.rect_size.y -  (self.inset.y + self.inset.w+ 2.0*self.border_width)) * 0.5
                            )
                        )
                    }
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result
                }
                Shape::Hexagon => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    if self.radius.x > 0.0 {
                        sdf.hexagon(
                            self.rect_size.x * 0.5,
                            self.rect_size.y * 0.5,
                            self.radius.x
                        )
                    }
                    else {
                        sdf.hexagon(
                            self.rect_size.x * 0.5,
                            self.rect_size.y * 0.5,
                            min(
                                (self.rect_size.x - (self.inset.x + self.inset.z + 2.0*self.border_width)) * 0.5,
                                (self.rect_size.y -  (self.inset.y + self.inset.w + 2.0*self.border_width)) * 0.5
                            )
                        )
                    }
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result
                }
            }
            return #0f0;
        }
    }
}


#[derive(Live, LiveHook, PartialEq)]
#[repr(u32)]
pub enum Shape {
   #[pick] None,
    Solid,
    Rect,
    Box,
    BoxX,
    BoxY,
    BoxAll,
    Circle,
    Hexagon,
}

#[derive(Live, LiveHook, PartialEq)]
#[repr(u32)]
pub enum Fill {
   #[pick] Color,
    GradientX,
    GradientY
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawShape {
    #[live] pub deref_target: DrawQuad,
    #[live] pub shape: Shape,
    #[live] pub fill: Fill,
    #[live(vec4(0.0,1.0,0.0,1.0))] pub color: Vec4,
    #[live(vec4(0.0,1.0,0.0,1.0))] pub color2: Vec4,
    #[live] pub border_width: f32,
    #[live] pub border_color: Vec4,
    #[live] pub inset: Vec4,
    #[live(vec4(0.0,1.0,1.0,1.0))] pub radius: Vec4,
}
