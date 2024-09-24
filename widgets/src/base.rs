use crate::makepad_platform::*;

live_design!{
    import crate::button::ButtonBase;
    import crate::check_box::CheckBoxBase;
    import crate::dock::DockBase;
    import crate::splitter::SplitterBase;
    import crate::desktop_button::DesktopButtonBase;
    import crate::window::WindowBase;
    import crate::multi_window::MultiWindowBase;
    import crate::drop_down::DropDownBase;
    import crate::file_tree::FileTreeBase;
    import crate::file_tree::FileTreeNodeBase;
    import crate::fold_button::FoldButtonBase;
    import crate::fold_header::FoldHeaderBase;
    import crate::image::ImageBase;
    import crate::multi_image::MultiImageBase;
    import crate::image_blend::ImageBlendBase;
    import crate::icon::IconBase;
    import crate::rotated_image::RotatedImageBase;
    import crate::modal::ModalBase;
    import crate::tooltip::TooltipBase;
    import crate::popup_notification::PopupNotificationBase;
    import crate::video::VideoBase;
    import crate::popup_menu::PopupMenuBase;
    import crate::label::LabelBase;
    import crate::link_label::LinkLabelBase;
    import crate::portal_list::PortalListBase;
    import crate::flat_list::FlatListBase;
    import crate::scroll_bars::ScrollBarsBase;
    import crate::view::ViewBase;
    import crate::nav_control::NavControlBase;
    import crate::popup_menu::PopupMenuItemBase;
    import crate::popup_menu::PopupMenuBase;
    import crate::radio_button::RadioButtonBase;
    import crate::scroll_bar::ScrollBarBase;
    import crate::scroll_bars::ScrollBarsBase;
    import crate::slide_panel::SlidePanelBase;
    import crate::slider::SliderBase;
    import crate::slides_view::SlidesViewBase;
    import crate::splitter::SplitterBase;
    import crate::tab::TabBase;
    import crate::tab_bar::TabBarBase;
    import crate::tab_close_button::TabCloseButtonBase;
    import crate::text_input::TextInputBase;
    import crate::scroll_shadow::DrawScrollShadowBase;
    import crate::page_flip::PageFlipBase;
    import crate::stack_navigation::StackNavigationViewBase;
    import crate::stack_navigation::StackNavigationBase;
    import crate::expandable_panel::ExpandablePanelBase;
    import crate::keyboard_view::KeyboardViewBase;
    import crate::window_menu::WindowMenuBase;
    import crate::html::HtmlBase;
    import crate::html::HtmlLinkBase;
    import crate::markdown::MarkdownBase,
    import crate::markdown::MarkdownLinkBase;
    import crate::root::RootBase;
    
    import crate::designer::DesignerBase;
    import crate::designer_outline::DesignerOutlineBase;
    import crate::designer_view::DesignerViewBase;
    import crate::designer_view::DesignerContainerBase;
    import crate::designer_outline_tree::DesignerOutlineTreeBase;
    import crate::designer_outline_tree::DesignerOutlineTreeNodeBase;
    import crate::designer_toolbox::DesignerToolboxBase
    import crate::color_picker::ColorPicker;
    
    import crate::bare_step::BareStep;
    import crate::turtle_step::TurtleStep;
    import crate::toggle_panel::*;
    import makepad_draw::shader::std::*;
    import makepad_draw::shader::draw_color::DrawColor;

    SlidePanel = <SlidePanelBase>{
        animator: {
            closed = {
                default: off,
                on = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.5}
                    }
                    ease: InQuad
                    apply: {
                        closed: 1.0
                    }
                }
                
                off = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.5}
                    }
                    ease: OutQuad
                    apply: {
                        closed: 0.0
                    }
                }
            }
        }
    }

    Icon = <IconBase> {
        width: Fit,
        height: Fit,

        icon_walk: {
            margin: {left: 5.0},
            width: Fit,
            height: Fit,
        }

        draw_bg: {
            instance color: #0000,
            fn pixel(self) -> vec4 {
                return self.color;
            }
        }
    }

    Image = <ImageBase> {
        width: 100
        height: 100
        
        draw_bg: {
            texture image: texture2d
            instance opacity: 1.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
            
            fn get_color_scale_pan(self, scale: vec2, pan: vec2) -> vec4 {
                return sample2d(self.image, self.pos * scale + pan).xyzw;
            }
            
            fn get_color(self) -> vec4 {
                return self.get_color_scale_pan(self.image_scale, self.image_pan)
            }
            
            fn pixel(self) -> vec4 {
                let color = self.get_color();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity))
            }
        }
    }
    
    MultiImage = <ImageBase> {
        width: 100
        height: 100
                
        draw_bg: {
            texture image1: texture2d
            texture image2: texture2d
            texture image3: texture2d
            texture image4: texture2d
            
            instance opacity: 1.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
                        
            fn get_color_scale_pan(self, scale: vec2, pan: vec2) -> vec4 {
                return sample2d(self.image1, self.pos * scale + pan).xyzw;
            }
                        
            fn get_color(self) -> vec4 {
                return self.get_color_scale_pan(self.image_scale, self.image_pan)
            }
                        
            fn pixel(self) -> vec4 {
                let color = self.get_color();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity))
            }
        }
    }
    
    ImageBlend = <ImageBlendBase> {
        width: 100
        height: 100
                
        draw_bg: {
            texture image0: texture2d
            texture image1: texture2d
            instance opacity: 1.0
            instance blend: 0.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.0, 0.0)
            instance breathe: 0.0            
            fn get_color_scale_pan(self, scale: vec2, pan: vec2) -> vec4 {
                let b = 1.0 - 0.1*self.breathe;
                let s = vec2(0.05*self.breathe);
                return mix(
                    sample2d(self.image0, self.pos * scale*b + pan+s).xyzw,
                    sample2d(self.image1, self.pos * scale*b + pan+s).xyzw,
                    self.blend
                )
            }
            
            fn get_color(self) -> vec4 {
                return self.get_color_scale_pan(self.image_scale, self.image_pan)
            }
                        
            fn pixel(self) -> vec4 {
                let color = self.get_color();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity))
            }
        }
                
        animator: {
            blend = {
                default: zero,
                zero = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {blend: 0.0}
                    }
                }
                one = {
                    from: {
                        all: Forward {duration: 0.1}
                    }
                    apply: {
                        draw_bg: {blend: 1.0}
                    }
                }
            } 
            breathe = {
                default: off,
                on = {
                    from: {all: BounceLoop {duration: 10., end:1.0}}
                    apply:{
                        draw_bg:{breathe:[{time: 0.0, value: 0.0}, {time:1.0,value:1.0}]}
                    }
                }
                off = {
                    from: {all: Forward {duration: 1}}
                    apply:{
                        draw_bg:{breathe:0.0}
                    }
                }
            }
        }
    }
      
    RotatedImage = <RotatedImageBase> {
        width: Fit
        height: Fit
        
        draw_bg: {
            texture image: texture2d
            
            instance rotation: 0.0
            instance opacity: 1.0
            instance scale: 1.0
            
            fn rotation_vertex_expansion(rotation: float, w: float, h: float) -> vec2 {
                let horizontal_expansion = (abs(cos(rotation)) * w + abs(sin(rotation)) * h) / w - 1.0;
                let vertical_expansion = (abs(sin(rotation)) * w + abs(cos(rotation)) * h) / h - 1.0;
                
                return vec2(horizontal_expansion, vertical_expansion);
            }
            
            fn rotate_2d_from_center(coord: vec2, a: float, size: vec2) -> vec2 {
                let cos_a = cos(-a);
                let sin_a = sin(-a);
                
                let centered_coord = coord - vec2(0.5, 0.5);
                
                // Denormalize the coordinates to use original proportions (between height and width)
                let denorm_coord = vec2(centered_coord.x, centered_coord.y * size.y / size.x);
                let demorm_rotated = vec2(denorm_coord.x * cos_a - denorm_coord.y * sin_a, denorm_coord.x * sin_a + denorm_coord.y * cos_a);
                
                // Restore the coordinates to use the texture coordinates proportions (between 0 and 1 in both axis)
                let rotated = vec2(demorm_rotated.x, demorm_rotated.y * size.x / size.y);
                
                return rotated + vec2(0.5, 0.5);
            }
            
            fn get_color(self) -> vec4 {
                let rot_padding = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y) / 2.0;
                
                // Current position is a traslated one, so let's get the original position
                let current_pos = self.pos.xy - rot_padding;
                let original_pos = rotate_2d_from_center(current_pos, self.rotation, self.rect_size);
                
                // Scale the current position by the scale factor
                let scaled_pos = original_pos / self.scale;
                
                // Take pixel color from the original image
                let color = sample2d(self.image, scaled_pos).xyzw;
                
                let faded_color = color * vec4(1.0, 1.0, 1.0, self.opacity);
                return faded_color;
            }
            
            fn pixel(self) -> vec4 {
                let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);
                
                // Debug
                // let line_width = 0.01;
                // if self.pos.x < line_width || self.pos.x > (self.scale + rot_expansion.x - line_width) || self.pos.y < line_width || self.pos.y > (self.scale + rot_expansion.y - line_width) {
                //     return #c86;
                // }
                
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                
                let translation_offset = vec2(self.rect_size.x * rot_expansion.x / 2.0, self.rect_size.y * self.scale * rot_expansion.y / 2.0);
                sdf.translate(translation_offset.x, translation_offset.y);
                
                let center = self.rect_size * 0.5;
                sdf.rotate(self.rotation, center.x, center.y);
                
                let scaled_size = self.rect_size * self.scale;
                sdf.box(0.0, 0.0, scaled_size.x, scaled_size.y, 1);
                
                sdf.fill_premul(Pal::premul(self.get_color()));
                return sdf.result
            }
            
            fn vertex(self) -> vec4 {
                let rot_expansion = rotation_vertex_expansion(self.rotation, self.rect_size.x, self.rect_size.y);
                let adjusted_pos = vec2(
                    self.rect_pos.x - self.rect_size.x * rot_expansion.x / 2.0,
                    self.rect_pos.y - self.rect_size.y * rot_expansion.y / 2.0
                );
                
                let expanded_size = vec2(self.rect_size.x * (self.scale + rot_expansion.x), self.rect_size.y * (self.scale + rot_expansion.y));
                let clipped: vec2 = clamp(
                    self.geom_pos * expanded_size + adjusted_pos,
                    self.draw_clip.xy,
                    self.draw_clip.zw
                );
                
                self.pos = (clipped - adjusted_pos) / self.rect_size;
                return self.camera_projection * (self.camera_view * (
                    self.view_transform * vec4(clipped.x, clipped.y, self.draw_depth + self.draw_zbias, 1.)
                ));
            }
            
            shape: Solid,
            fill: Image
        }
    }

    Video = <VideoBase> {
        width: 100, height: 100

        draw_bg: {
            shape: Solid,
            fill: Image
            texture video_texture: textureOES
            texture thumbnail_texture: texture2d
            uniform show_thumbnail: 0.0

            instance opacity: 1.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.5, 0.5)

            uniform source_size: vec2(1.0, 1.0)
            uniform target_size: vec2(-1.0, -1.0)

            fn get_color_scale_pan(self) -> vec4 {
                // Early return for default scaling and panning,
                // used when walk size is not specified or non-fixed.
                if self.target_size.x <= 0.0 && self.target_size.y <= 0.0 {
                    if self.show_thumbnail > 0.0 {
                        return sample2d(self.thumbnail_texture, self.pos).xyzw;
                    } else {
                        return sample2dOES(self.video_texture, self.pos);
                    }  
                }

                let scale = self.image_scale;
                let pan = self.image_pan;
                let source_aspect_ratio = self.source_size.x / self.source_size.y;
                let target_aspect_ratio = self.target_size.x / self.target_size.y;

                // Adjust scale based on aspect ratio difference
                if (source_aspect_ratio != target_aspect_ratio) {
                    if (source_aspect_ratio > target_aspect_ratio) {
                        scale.x = target_aspect_ratio / source_aspect_ratio;
                        scale.y = 1.0;
                    } else {
                        scale.x = 1.0;
                        scale.y = source_aspect_ratio / target_aspect_ratio;
                    }
                }

                // Calculate the range for panning
                let pan_range_x = max(0.0, (1.0 - scale.x));
                let pan_range_y = max(0.0, (1.0 - scale.y));

                // Adjust the user pan values to be within the pan range
                let adjusted_pan_x = pan_range_x * pan.x;
                let adjusted_pan_y = pan_range_y * pan.y;
                let adjusted_pan = vec2(adjusted_pan_x, adjusted_pan_y);
                let adjusted_pos = (self.pos * scale) + adjusted_pan;

                if self.show_thumbnail > 0.5 {
                    return sample2d(self.thumbnail_texture, adjusted_pos).xyzw;
                } else {
                    return sample2dOES(self.video_texture, adjusted_pos);
                }      
            }

            fn pixel(self) -> vec4 {
                let color = self.get_color_scale_pan();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity));
            }
        }
    }
    
    MultiWindow = <MultiWindowBase> {}
    View = <ViewBase> {}

    SolidView = <ViewBase> {show_bg: true, draw_bg: {
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    /*
    Debug = <View> {show_bg: true, draw_bg: {
        color: #f00
        fn pixel(self) -> vec4 {
            return self.color
        }
    }}*/
    
    RectView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.rect(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0)
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result
        }
    }}
    
    RectShadowView = <ViewBase> {
        clip_x:false,
        clip_y:false,
        
        show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance shadow_color: #0007
        instance shadow_offset: vec2(0.0,0.0)
        instance shadow_radius: 10.0
        
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
            self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_width * 2.0)
            self.sdf_rect_pos = -min_offset + vec2(self.border_width + self.shadow_radius);
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
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result
        }
    }}
        
    RoundedShadowView = <ViewBase>{
        clip_x:false,
        clip_y:false,
                
        show_bg: true, draw_bg: {
            color:#8
            instance border_width: 0.0
            instance border_color: #0000
            instance shadow_color: #0007
            instance shadow_radius: 20.0,
            instance shadow_offset: vec2(0.0,0.0)
            instance radius: 2.5
                            
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
                self.sdf_rect_size = self.rect_size2 - vec2(self.shadow_radius * 2.0 + self.border_width * 2.0)
                self.sdf_rect_pos = -min_offset + vec2(self.border_width + self.shadow_radius);
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
                    max(1.0, self.radius)
                )
                if sdf.shape > -1.0{ // try to skip the expensive gauss shadow
                    let m = self.shadow_radius;
                    let o = self.shadow_offset + self.rect_shift;
                    let v = GaussShadow::rounded_box_shadow(vec2(m) + o, self.rect_size2+o, self.pos * (self.rect_size3+vec2(m)), self.shadow_radius*0.5, self.radius*2.0);
                    sdf.clear(self.shadow_color*v)
                }
                                                
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result
            }
        }}
    
    RoundedView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 2.5
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                max(1.0, self.radius)
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    
    RoundedXView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec2(2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box_x(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                self.radius.x,
                self.radius.y
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    RoundedYView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec2(2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            sdf.box_y(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                self.radius.x,
                self.radius.y
            )
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    RoundedAllView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: vec4(2.5, 2.5, 2.5, 2.5)
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
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
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result;
        }
    }}
    
    CircleView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 5.0
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            if self.radius > 0.0 {
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.radius
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
            sdf.fill_keep(self.get_color())
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.border_width)
            }
            return sdf.result
        }
    }}
    
    HexagonView = <ViewBase> {show_bg: true, draw_bg: {
        instance border_width: 0.0
        instance border_color: #0000
        instance inset: vec4(0.0, 0.0, 0.0, 0.0)
        instance radius: 5
        
        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn get_border_color(self) -> vec4 {
            return self.border_color
        }
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
            if self.radius.x > 0.0 {
                sdf.hexagon(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.radius
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
    }}
    
    GradientXView = <ViewBase> {show_bg: true, draw_bg: {
        instance color2: #f00
        instance dither: 1.0
        fn get_color(self) -> vec4 {
            let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
            return mix(self.color, self.color2, self.pos.x + dither)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    GradientYView = <ViewBase> {show_bg: true, draw_bg: {
        instance color2: #f00
        instance dither: 1.0
        fn get_color(self) -> vec4 {
            let dither = Math::random_2d(self.pos.xy) * 0.04 * self.dither;
            return mix(self.color, self.color2, self.pos.y + dither)
        }
        
        fn pixel(self) -> vec4 {
            return Pal::premul(self.get_color())
        }
    }}
    
    CachedView = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform marked: float,
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
                return sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
            }
        }
    }

    CachedRoundedView = <ViewBase> {
                
        optimize: Texture,
        draw_bg: {
            instance border_width: 0.0
            instance border_color: #0000
            instance inset: vec4(0.0, 0.0, 0.0, 0.0)
            instance radius: 2.5
            
            texture image: texture2d
            uniform marked: float,
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
                    self.inset.x + self.border_width,
                    self.inset.y + self.border_width,
                    self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                    self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                let color = sample2d_rt(self.image, self.pos * self.scale + self.shift);
                sdf.fill_keep_premul(color);
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result;
            }
        }
    }

    ExpandablePanel = <ExpandablePanelBase> {
        flow: Overlay,
        width: Fill,
        height: Fill,

        initial_offset: 400.0;

        body = <View> {}

        panel = <View> {
            flow: Down,
            width: Fill,
            height: Fit,

            show_bg: true,
            draw_bg: {
                color: #FFF
            }

            align: { x: 0.5, y: 0 }
            padding: 20,
            spacing: 10,

            scroll_handler = <RoundedView> {
                width: 40,
                height: 6,

                show_bg: true,
                draw_bg: {
                    color: #333
                    radius: 2.
                }
            }
        }
    }
    
    MultiWindow = <MultiWindowBase>{}
    PageFlip = <PageFlipBase>{}
    KeyboardView = <KeyboardViewBase>{}
    // todo fix this by allowing reexporting imports
    // for now this works too\
    RootBase = <RootBase>{}
    HtmlBase = <HtmlBase>{}
    HtmlLinkBase = <HtmlLinkBase>{}
    MarkdownBase = <MarkdownBase>{}
    MarkdownLinkBase = <MarkdownLinkBase>{}
    KeyboardViewBase = <KeyboardViewBase>{}
    PageFlipBase = <PageFlipBase>{}
    ViewBase = <ViewBase>{}
    ButtonBase = <ButtonBase>{}
    CheckBoxBase = <CheckBoxBase>{}
    DockBase = <DockBase>{}
    MultiWindowBase = <MultiWindowBase>{}
    WindowBase = <WindowBase> {}
    DesktopButtonBase = <DesktopButtonBase> {}
    DropDownBase = <DropDownBase> {}
    FileTreeBase = <FileTreeBase> {}
    FileTreeNodeBase = <FileTreeNodeBase> {}
    FoldButtonBase = <FoldButtonBase> {}
    FoldHeaderBase = <FoldHeaderBase> {}
    ImageBase = <ImageBase> {}
    IconBase = <IconBase> {}
    RotatedImageBase = <RotatedImageBase> {}
    ModalBase = <ModalBase> {}
    TooltipBase = <TooltipBase> {}
    PopupNotificationBase = <PopupNotificationBase> {}
    VideoBase = <VideoBase> {}
    LabelBase = <LabelBase> {}
    LinkLabelBase = <LinkLabelBase> {}
    PortalListBase = <PortalListBase> {}
    FlatListBase = <FlatListBase>{}
    NavControlBase = <NavControlBase> {}
    PopupMenuBase = <PopupMenuBase> {}
    PopupMenuItemBase = <PopupMenuItemBase> {}
    RadioButtonBase = <RadioButtonBase> {}
    ScrollBarBase = <ScrollBarBase> {}
    ScrollBarsBase = <ScrollBarsBase> {}
    SlidePanelBase = <SlidePanelBase> {}   
    SliderBase = <SliderBase>{}
    SlidesViewBase = <SlidesViewBase>{}
    SplitterBase = <SplitterBase>{}
    TabBase = <TabBase>{}
    TabBarBase = <TabBarBase>{}
    TabCloseButtonBase = <TabCloseButtonBase>{}
    TextInputBase = <TextInputBase>{}
    DrawScrollShadowBase = <DrawScrollShadowBase>{}
    WindowMenuBase = <WindowMenuBase>{}
    StackNavigationViewBase = <StackNavigationViewBase>{}
    StackNavigationBase = <StackNavigationBase>{}
    ExpandablePanelBase = <ExpandablePanelBase>{}
    BareStep = <BareStep>{}
    TurtleStep = <TurtleStep>{}
    ColorPicker = <ColorPicker>{}
    TogglePanelBase = <TogglePanelBase>{}
    
    DesignerBase = <DesignerBase>{}
    DesignerOutlineBase = <DesignerOutlineBase>{}
    DesignerViewBase = <DesignerViewBase>{}
    DesignerContainerBase = <DesignerContainerBase>{}
    DesignerOutlineTreeBase = <DesignerOutlineTreeBase> {}
    DesignerOutlineTreeNodeBase = <DesignerOutlineTreeNodeBase> {}
    DesignerToolboxBase = <DesignerToolboxBase> {}
}
