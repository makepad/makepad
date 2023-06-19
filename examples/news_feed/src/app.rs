use makepad_widgets::*;

live_design!{
    import makepad_widgets::theme::*;
    import makepad_draw::shader::std::*;
    
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::frame::*;
    import makepad_widgets::slider::Slider;
    import makepad_widgets::text_input::TextInput;
    import makepad_widgets::drop_down::DropDown;
    
    IMG_A = dep("crate://self/resources/neom-THlO6Mkf5uI-unsplash.jpg")
    IMG_B = dep("crate://self/resources/mario-von-rotz-2FxSOXvfXVM-unsplash.jpg")
    IMG_PROFILE_A = dep("crate://self/resources/profile_1.jpg")
    IMG_PROFILE_B = dep("crate://self/resources/profile_2.jpg")
    
    LOGO = dep("crate://self/resources/logo.svg")
    ICO_FAV = dep("crate://self/resources/icon_favorite.svg")
    ICO_COMMENT = dep("crate://self/resources/icon_comment.svg")
    ICO_REPLY = dep("crate://self/resources/icon_reply.svg")
    ICO_HOME = dep("crate://self/resources/icon_home.svg")
    ICO_FIND = dep("crate://self/resources/icon_find.svg")
    ICO_LIKES = dep("crate://self/resources/icon_likes.svg")
    ICO_USER = dep("crate://self/resources/icon_user.svg")
    ICO_ADD = dep("crate://self/resources/icon_add.svg")
    
    FONT_SIZE_SUB = 9.5
    FONT_SIZE_P = 12.5
    
    TEXT_SUB = {
        font_size: (FONT_SIZE_SUB),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    
    TEXT_P = {
        font_size: (FONT_SIZE_P),
        height_factor: 1.65,
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
    }
    
    COLOR_BG = #xfff8ee
    COLOR_BRAND = #xf88
    COLOR_BRAND_HOVER = #xf66
    COLOR_META_TEXT = #xaaa
    COLOR_META = #xccc
    COLOR_META_INV = #xfffa
    COLOR_OVERLAY_BG = #x000000d8
    COLOR_DIVIDER = #x00000018
    COLOR_DIVIDER_DARK = #x00000044
    COLOR_PROFILE_CIRCLE = #xfff8ee
    COLOR_P = #x999
    
    FillerY = <Frame> {walk: {width: Fill}}
    
    FillerX = <Frame> {walk: {height: Fill}}
    
    Logo = <Button> {
        draw_icon: {
            svg_file: (LOGO),
            fn get_color(self) -> vec4 {
                return (COLOR_BRAND)
            }
        }
        icon_walk: {width: 7.5, height: Fit}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        layout: {padding: 9.0}
        label: ""
    }
    
    IconButton = <Button> {
        draw_label: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: {
                font: {
                    //path: d"resources/IBMPlexSans-SemiBold.ttf"
                }
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_META_TEXT),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_HOVER),
                    self.pressed
                )
            }
        }
        draw_icon: {
            svg_file: (ICO_FAV),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_META),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_HOVER),
                    self.pressed
                )
            }
        }
        icon_walk: {width: 7.5, height: Fit, margin: {left: 5.0}}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        layout: {padding: 9.0}
        label: "34"
    }
    
    
    Header = <Box> {
        walk: {width: Fill, height: 100}
        layout: {flow: Right, padding: 10.0, spacing: 10.0}
        draw_bg: {color: (COLOR_OVERLAY_BG), inset: vec4(-0.5, -0.5, -1.0, 0.0), radius: 4.5}
        
        <Logo> {
            walk: {height: Fit, width: Fill, margin: {top: 30.0}}
            icon_walk: {width: Fit, height: 27.0}
        }
        
    }
    
    Menu = <Box> {
        walk: {width: Fill, height: 100}
        layout: {flow: Right, padding: 10.0, spacing: 10.0}
        draw_bg: {color: (COLOR_OVERLAY_BG), inset: vec4(-0.5, 0.0, -1.0, -1.0), radius: 4.5}
        
        <Frame> {
            walk: {width: Fill, height: Fit, margin: 0.0}
            layout: {flow: Right, padding: 0.0, spacing: 25.0, align: {x: 0.5, y: 0.5}}
            
            <IconButton> {draw_icon: {svg_file: (ICO_HOME)} icon_walk: {width: 20.0, height: Fit}, label: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_FIND)} icon_walk: {width: 18.0, height: Fit}, label: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_ADD)} icon_walk: {width: 40.0, height: Fit}, label: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_LIKES)} icon_walk: {width: 20.0, height: Fit}, label: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_USER)} icon_walk: {width: 15.0, height: Fit}, label: ""}
        }
    }
    
    LineH = <Box> {
        walk: {width: Fill, height: 2, margin: 0.0}
        layout: {padding: 0.0, spacing: 0.0}
        draw_bg: {color: (COLOR_DIVIDER)}
    }
    
    PostMenu = <Frame> {
        walk: {width: Fill, height: Fit, margin: 0.0}
        layout: {flow: Down, padding: 0.0, spacing: 0.0}
        
        <Frame> {
            walk: {width: Fill, height: Fit, margin: 0.0}
            layout: {flow: Right, padding: 0.0, spacing: 10.0}
            
            likes = <IconButton> {draw_icon: {svg_file: (ICO_FAV)} icon_walk: {width: 15.0, height: Fit}}
            comments = <IconButton> {draw_icon: {svg_file: (ICO_COMMENT)} icon_walk: {width: 15.0, height: Fit}, label: "7"}
            
            <FillerX> {}
            reply = <IconButton> {draw_icon: {svg_file: (ICO_REPLY)} icon_walk: {width: 15.0, height: Fit}, label: ""}
        }
    }
    
    Post = <Frame> {
        walk: {width: Fill, height: Fit, margin: 0.0}
        layout: {flow: Down, padding: 0.0, spacing: 0.0}
        
        body = <Frame> {
            walk: {width: Fill, height: Fit}
            layout: {flow: Right, padding: 10.0, spacing: 10.0}
            
            profile = <Frame> {
                walk: {width: Fit, height: Fit, margin: {top: 7.5}}
                layout: {flow: Down, padding: 0.0}
                profile_img = <Image> {
                    image: (IMG_PROFILE_A)
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            let c = self.rect_size * 0.5;
                            sdf.circle(c.x, c.y, c.x - 2.)
                            sdf.fill_keep(self.get_color());
                            sdf.stroke((COLOR_PROFILE_CIRCLE), 1);
                            return sdf.result
                        }
                    }
                    image_scale: 0.65
                    walk: {margin: 0}
                    layout: {padding: 0}
                }
            }
            content = <Frame> {
                walk: {width: Fill, height: Fit}
                layout: {flow: Down, padding: 0.0}
                
                meta = <Label> {
                    walk: {margin: {bottom: 10.0, top: 10.0}}
                    draw_label: {
                        text_style: <TEXT_SUB> {},
                        color: (COLOR_META_TEXT)
                    }
                    label: "@username · 13h"
                }
                
                text = <Label> {
                    walk:{width:Fill, height:Fit},
                    draw_label: {
                        wrap: Word,
                        text_style: <TEXT_P> {},
                        color: (COLOR_P)
                    }
                    label: "Never underestimate the resilience it takes to live in a desert. It's a testament to life's adaptability, endurance, and tenacity. The cacti, creatures, and people who call it home are nature's ultimate survivalists. #DesertStrong"
                }
                
                <LineH> {
                    walk: {margin: {top: 10.0, bottom: 5.0}}
                }
                
                <PostMenu> {}
            }
        }
        
        <LineH> {
            draw_bg: {color: (COLOR_DIVIDER_DARK)}
        }
    }
    
    PostImage = <Frame> {
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, padding: 0.0, spacing: 0.0}
        
        hero = <Image> {
            image: (IMG_A),
            image_scale: 1.0,
            walk: {margin: 0}
            layout: {padding: 0}
        }
        
        post = <Post> {
            walk: {margin: {top: -45.0}}
            body = {
                content = {
                    meta = {
                        walk: {margin: {bottom: 30.0, top: 10.0}}
                        draw_label: {
                            color: (COLOR_META_INV)
                        }
                    }
                }
            }
        }
    }
    
    
    App = {{App}} {
        ui: <DesktopWindow> {
            window: {inner_size: vec2(428, 926), dpi_override: 2},
            show_bg: true
            layout: {
                flow: Overlay,
                padding: 0.0
                spacing: 0,
                align: {
                    x: 0.0,
                    y: 0.0
                }
            },
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return (COLOR_BG);
                }
            }
            
            <ScrollY> {
                walk: {height: Fill, width: Fill}
                layout: {flow: Down}
                <Frame> {walk: {height: 100}}
                <PostImage> {}
                <Post> {
                    body = {profile = {profile_img = {
                        image: (IMG_PROFILE_B)
                    }}}
                }
                <PostImage> {
                    hero = {image: (IMG_B),}
                    post = {body = {profile = {profile_img = {
                        image: (IMG_PROFILE_B)
                    }}}}
                }
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Post> {}
                <Frame> {walk: {height: 100}}
            }
            
            <Frame> {
                walk: {height: Fill, width: Fill}
                layout: {flow: Down}
                
                <Header> {}
                <FillerY> {}
                <Menu> {}
            }
        }
    }
}

app_main!(App);

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.get_button(id!(button1)).clicked(&actions) {
        }
    }
}
