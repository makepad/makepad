use {
    crate::makepad_platform::*
};

live_design! {
    link widgets;
    
    use link::theme::*;
    use crate::view::ViewBase;
    use crate::scroll_bars::ScrollBars;
    use makepad_draw::shader::std::*;
    
    pub View = <ViewBase> {}
    
    pub Hr = <View> {
        width: Fill, height: 5.,
        flow: Down,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}

        show_bg: true, 
        draw_bg: {
            uniform color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_size: (THEME_BEVELING)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let stroke_width = self.border_size
                sdf.move_to(1., 0.);
                sdf.line_to(self.rect_size.x, 0.0);
                sdf.stroke(self.color_1, stroke_width * 2.);

                let offset = stroke_width * 2.0;
                sdf.move_to(0., 1. + stroke_width);
                sdf.line_to(self.rect_size.x, 1. + stroke_width);
                sdf.stroke(self.color_2, stroke_width);

                return sdf.result
            }
        }
    }
    
    pub Vr = <View> {
        width: 5., height: Fill,
        flow: Right,
        spacing: 0.,
        margin: <THEME_MSPACE_V_2> {}

        show_bg: true, 
        draw_bg: {
            uniform color_1: (THEME_COLOR_BEVEL_SHADOW)
            uniform color_2: (THEME_COLOR_BEVEL_LIGHT)
            uniform border_size: (THEME_BEVELING)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)

                let stroke_width = self.border_size;
                sdf.move_to(1., 0.);
                sdf.line_to(0.0, self.rect_size.y);
                sdf.stroke(self.color_1, stroke_width * 2.);

                let offset = stroke_width * 2.0;
                sdf.move_to(1. + stroke_width, 0.);
                sdf.line_to(1. + stroke_width, self.rect_size.y);
                sdf.stroke(self.color_2, stroke_width);

                return sdf.result
            }
        }
    }
    
    // Spacer = <View> { width: Fill, height: Fill }
    pub Filler = <View> { width: Fill, height: Fill }
    
    pub SolidView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            fn get_color(self) -> vec4 {
                return self.color
            }
                    
            fn pixel(self) -> vec4 {
                return Pal::premul(self.get_color())
            }
        }
    }
    /*
    Debug = <View> {show_bg: true, draw_bg: {
        color: #f00
        fn pixel(self) -> vec4 {
            return self.color
        }
    }}*/
        
    pub RectView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                        
            fn get_color(self) -> vec4 {
                return self.color
            }
                        
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                        
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.rect(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0)
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
    }
        
    pub RectShadowView = <ViewBase> {
        clip_x:false,
        clip_y:false,
                
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform shadow_color: #0007
            uniform shadow_offset: vec2(0.0,0.0)
            uniform shadow_radius: 10.0
                    
            varying rect_size2: vec2,
            varying rect_size3: vec2,
            varying sdf_rect_pos: vec2,
            varying sdf_rect_size: vec2,
            varying rect_pos2: vec2,     
            varying rect_shift: vec2,  

            fn get_color(self) -> vec4 {
                return self.color
            }
                    
            fn vertex(self) -> vec4 {
                let min_offset = min(self.shadow_offset,vec2(0));
                self.rect_size2 = self.rect_size + 2.0*vec2(self.shadow_radius);
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset);
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset;
                self.rect_shift = -min_offset;
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius);
                return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
            }
                                                
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                                        
            fn pixel(self) -> vec4 {
                            
                let sdf = Sdf2d::viewport(self.pos * self.rect_size3)
                sdf.rect(
                    self.sdf_rect_pos.x,
                    self.sdf_rect_pos.y,
                    self.sdf_rect_size.x,
                    self.sdf_rect_size.y 
                )
                if sdf.shape > -1.0{ // try to skip the expensive gauss shadow
                    let m = self.shadow_radius;
                    let o = self.shadow_offset + self.rect_shift;
                    let v = GaussShadow::box_shadow(vec2(m) + o, self.rect_size2+o, self.pos * (self.rect_size3+vec2(m)) , m*0.5);
                    sdf.clear(self.shadow_color*v)
                }
                                                                
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    pub RoundedShadowView = <ViewBase>{
        clip_x:false,
        clip_y:false,
                            
        show_bg: true,
        draw_bg: {
            color: #8
            uniform border_radius: 2.5
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform shadow_color: #0007
            uniform shadow_radius: 20.0,
            uniform shadow_offset: vec2(0.0,0.0)
                                            
            varying rect_size2: vec2,
            varying rect_size3: vec2,
            varying rect_pos2: vec2,     
            varying rect_shift: vec2,    
            varying sdf_rect_pos: vec2,
            varying sdf_rect_size: vec2,
                                              
            fn get_color(self) -> vec4 {
                return self.color
            }
                                            
            fn vertex(self) -> vec4 {
                let min_offset = min(self.shadow_offset,vec2(0));
                self.rect_size2 = self.rect_size + 2.0*vec2(self.shadow_radius);
                self.rect_size3 = self.rect_size2 + abs(self.shadow_offset);
                self.rect_pos2 = self.rect_pos - vec2(self.shadow_radius) + min_offset;
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_size * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_size + self.shadow_radius);
                self.rect_shift = -min_offset;
                                                            
                return self.clip_and_transform_vertex(self.rect_pos2, self.rect_size3)
            }
                                                        
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                                                
            fn pixel(self) -> vec4 {
                                                                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size3)
                sdf.box(
                    self.sdf_rect_pos.x,
                    self.sdf_rect_pos.y,
                    self.sdf_rect_size.x,
                    self.sdf_rect_size.y, 
                    max(1.0, self.border_radius)
                )
                if sdf.shape > -1.0{ // try to skip the expensive gauss shadow
                    let m = self.shadow_radius;
                    let o = self.shadow_offset + self.rect_shift;
                    let v = GaussShadow::rounded_box_shadow(vec2(m) + o, self.rect_size2+o, self.pos * (self.rect_size3+vec2(m)), self.shadow_radius*0.5, self.border_radius*2.0);
                    sdf.clear(self.shadow_color*v)
                }
                                                                    
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    pub RoundedView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_radius: 2.5
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                
            fn get_color(self) -> vec4 {
                return self.color
            }
                                
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                                
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0),
                    max(1.0, self.border_radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result;
            }
        }
    }
                
    pub RoundedXView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
            uniform border_radius: vec2(2.5, 2.5)
                            
            fn get_color(self) -> vec4 {
                return self.color
            }
                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box_x(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0),
                    self.border_radius.x,
                    self.border_radius.y
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result;
            }
        }
    }
                
    pub RoundedYView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
            uniform border_radius: vec2(2.5, 2.5)
                            
            fn get_color(self) -> vec4 {
                return self.color
            }
                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box_y(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0),
                    self.border_radius.x,
                    self.border_radius.y
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result;
            }
        }
    }
                
    pub RoundedAllView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
            uniform border_radius: vec4(2.5, 2.5, 2.5, 2.5)
                            
            fn get_color(self) -> vec4 {
                return self.color
            }
                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box_all(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0),
                    self.border_radius.x,
                    self.border_radius.y,
                    self.border_radius.z,
                    self.border_radius.w
                )
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result;
            }
        }
    }
                
    pub CircleView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
            uniform border_radius: 5.0
                            
            fn get_color(self) -> vec4 {
                return self.color
            }
                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                if self.border_radius > 0.0 {
                    sdf.circle(
                        self.rect_size.x * 0.5,
                        self.rect_size.y * 0.5,
                        self.border_radius
                    )
                }
                else {
                    sdf.circle(
                        self.rect_size.x * 0.5,
                        self.rect_size.y * 0.5,
                        min(
                            (self.rect_size.x - (self.border_inset.x + self.border_inset.z + 2.0 * self.border_size)) * 0.5,
                            (self.rect_size.y - (self.border_inset.y + self.border_inset.w + 2.0 * self.border_size)) * 0.5
                        )
                    )
                }
                sdf.fill_keep(self.get_color())
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    pub HexagonView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #0000
            uniform border_inset: vec4(0.0, 0.0, 0.0, 0.0)
            uniform border_radius: vec2(0.0, 1.0)
                            
            fn get_color(self) -> vec4 {
                return self.color
            }
                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                if self.border_radius.x > 0.0 {
                    sdf.hexagon(
                        self.rect_size.x * 0.5,
                        self.rect_size.y * 0.5,
                        self.border_radius.x
                    )
                }
                else {
                    sdf.hexagon(
                        self.rect_size.x * 0.5,
                        self.rect_size.y * 0.5,
                        min(
                            (self.rect_size.x - (self.border_inset.x + self.border_inset.z + 2.0 * self.border_size)) * 0.5,
                            (self.rect_size.y - (self.border_inset.y + self.border_inset.w + 2.0 * self.border_size)) * 0.5
                        )
                    )
                }
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke(self.border_color, self.border_size)
                }
                return sdf.result
            }
        }
    }
                
    pub GradientXView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform color_1: #00f
            uniform color_2: #f00
            uniform color_dither: 1.0

            fn get_color(self) -> vec4 {
                
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                return mix(self.color_1, self.color_2, self.pos.x + dither)
            }
                            
            fn pixel(self) -> vec4 {
                return Pal::premul(self.get_color())
            }
        }
    }
                
    pub GradientYView = <ViewBase> {
        show_bg: true, 
        draw_bg: {
            uniform color_1: #00f
            uniform color_2: #f00
            uniform color_dither: 1.0

            fn get_color(self) -> vec4 {
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;
                return mix(self.color_1, self.color_2, self.pos.y + dither)
            }
                            
            fn pixel(self) -> vec4 {
                return Pal::premul(self.get_color())
            }
        }
    }
                
    pub CachedView = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            varying scale: vec2
            varying shift: vec2
            fn vertex(self) -> vec4 {
                let dpi = self.dpi_factor;
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size;
                self.shift = (self.rect_pos - floor_pos) / ceil_size;
                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }
            fn pixel(self) -> vec4 {
                return sample2d_rt(self.image, self.pos * self.scale + self.shift);// + mix(#f00,#0f0,self.pos.y);
            }
        }
    }
            
    pub CachedRoundedView = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            uniform border_size: 0.0
            uniform border_color: #000F
            uniform border_inset: vec4(0., 0., 0., 0.)
            uniform border_radius: 2.5
                                
            texture image: texture2d
            varying scale: vec2
            varying shift: vec2
                                            
            fn get_border_color(self) -> vec4 {
                return self.border_color
            }
                                        
            fn vertex(self) -> vec4 {
                let dpi = self.dpi_factor;
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size;
                self.shift = (self.rect_pos - floor_pos) / ceil_size;
                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }
                                
            fn pixel(self) -> vec4 {
                                        
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_inset.x + self.border_size,
                    self.border_inset.y + self.border_size,
                    self.rect_size.x - (self.border_inset.x + self.border_inset.z + self.border_size * 2.0),
                    self.rect_size.y - (self.border_inset.y + self.border_inset.w + self.border_size * 2.0),
                    max(1.0, self.border_radius)
                )
                let color = sample2d_rt(self.image, self.pos * self.scale + self.shift);
                sdf.fill_keep_premul(color);
                if self.border_size > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_size)
                }
                return sdf.result;
            }
        }
    }
    
    pub CachedScrollXY = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}
    }
        
    pub CachedScrollX = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}
    }
        
    pub CachedScrollY = <CachedView> {
        scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
    }
        
    pub ScrollXYView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: true}}
    pub ScrollXView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: true, show_scroll_y: false}}
    pub ScrollYView = <ViewBase> {scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}}
}