use {
    crate::{
        makepad_platform::*,
        shader::draw_quad::DrawQuad,
    },
};

live_design!{
    import makepad_draw_2d::shader::std::*;
    DrawShape= {{DrawShape}} {
        texture image: texture2d
        
        fn get_color(self)->vec4{
            return self.color
        }

        fn get_fill(self)->vec4{
            match self.fill {
                Fill::Color => {
                    return self.get_color()
                }
                Fill::GradientX => {
                    return mix(self.color, self.color2, self.pos.x)
                }
                Fill::GradientY => {
                    return mix(self.color, self.color2, self.pos.y)
                }
                Fill::Image => {
                    return sample2d(self.image, self.pos * self.image_scale + self.image_pan).xyzw;
                }
            }
        }

        fn pixel(self) -> vec4 {
            let color = self.get_fill();
            
            match self.shape {
                Shape::None => {
                    return #0000
                }
                Shape::Solid => {
                    return Pal::premul(color)
                }
                Shape::Rect => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    sdf.rect(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0)
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
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                        max(1.0, self.radius.x)
                    )
                    sdf.fill_keep(color)
                    if self.border_width > 0.0 {
                        sdf.stroke(self.border_color, self.border_width)
                    }
                    return sdf.result;
                }
                Shape::ShadowBox => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.blur = 20.0;
                    sdf.box(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                        max(1.0, self.radius.x)
                    )
                    sdf.fill_keep(color)
                    return sdf.result;
                }
                Shape::BoxX => {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box_x(
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
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
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
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
                        self.inset.x + self.border_width,
                        self.inset.y + self.border_width,
                        self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                        self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
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
                                (self.rect_size.x - (self.inset.x + self.inset.z + 2.0 * self.border_width)) * 0.5,
                                (self.rect_size.y - (self.inset.y + self.inset.w + 2.0 * self.border_width)) * 0.5
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
                                (self.rect_size.x - (self.inset.x + self.inset.z + 2.0 * self.border_width)) * 0.5,
                                (self.rect_size.y - (self.inset.y + self.inset.w + 2.0 * self.border_width)) * 0.5
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
    #[pick] None = shader_enum(1),
    Solid = shader_enum(2),
    Rect = shader_enum(3),
    Box = shader_enum(4),
    BoxX = shader_enum(5),
    BoxY = shader_enum(6),
    BoxAll = shader_enum(7),
    Circle = shader_enum(8),
    Hexagon = shader_enum(9),
    ShadowBox = shader_enum(10),
}

#[derive(Live, LiveHook, PartialEq)]
#[repr(u32)]
pub enum Fill {
    #[pick] Color = shader_enum(1),
    GradientX = shader_enum(2),
    GradientY = shader_enum(3),
    Image = shader_enum(4),
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawShape {
    #[live] pub draw_super: DrawQuad,
    #[live] pub shape: Shape,
    #[live] pub fill: Fill,
    #[live(vec4(0.0, 1.0, 0.0, 1.0))] pub color: Vec4,
    #[live(vec4(0.0, 1.0, 0.0, 1.0))] pub color2: Vec4,
    #[live] pub border_width: f32,
    #[live] pub border_color: Vec4,
    #[live] pub inset: Vec4,
    #[live(vec4(0.0, 1.0, 1.0, 1.0))] pub radius: Vec4,
    #[live(vec2(1.0,1.0))] pub image_scale: Vec2,
    #[live(vec2(0.0,0.0))] pub image_pan: Vec2,
}
