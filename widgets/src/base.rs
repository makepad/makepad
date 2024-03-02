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
    import crate::rotated_image::RotatedImageBase;
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
    // for now this works too
    HtmlBase = <HtmlBase>{}
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
    RotatedImageBase = <RotatedImageBase> {}
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
}
