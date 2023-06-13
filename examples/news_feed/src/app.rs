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
    IMG_PROFILE_A = dep("crate://self/resources/joseph-gonzalez-iFgRcqHznqg-unsplash.jpg")
    
    ICO_FAV = dep("crate://self/resources/Icon_Favorite.svg")
    ICO_COMMENT = dep("crate://self/resources/Icon_Comment.svg")
    ICO_REPLY = dep("crate://self/resources/Icon_Reply.svg")
    LOGO = dep("crate://self/resources/leap_logo.svg")
    
    FONT_SIZE_SUB = 9.5
    FONT_SIZE_P = 16.0
    
    TEXT_SUB = {
        font_size: (FONT_SIZE_SUB),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    
    TEXT_P = {
        font_size: (FONT_SIZE_P),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
    }

    Logo = <Button> {
        draw_icon: {
            svg_file: (LOGO),
            fn get_color(self) -> vec4 {
                return #FF8888
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
        draw_icon: {
            svg_file: (ICO_FAV),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #ccc,
                        #FF8888,
                        self.hover
                    ),
                    #000,
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
        draw_bg: {color: #00000088}
        
        <Logo> {
            walk: {height: Fit, width: Fill, margin: {top: 30.0}}
            icon_walk: {width: Fit, height: 27.0}
        }
        
    }

    PostMenu = <Frame> {
        walk: { width: Fill, height: Fit, margin: 0.0 }
        layout: { flow: Down, padding: 0.0, spacing: 0.0 }

        <LineH> {}
        <Frame> {
            walk: { width: Fill, height: Fit, margin: 0.0 }
            layout: { flow: Right, padding: 0.0, spacing: 10.0 }

            likes = <IconButton> { draw_icon: { svg_file: (ICO_FAV) } icon_walk: { width: 20.0, height: Fit } }
            comments = <IconButton> { draw_icon: { svg_file: (ICO_COMMENT) } icon_walk: { width: 20.0, height: Fit }, label: "7"}

            <FillerX> {}
            reply = <IconButton> { draw_icon: { svg_file: (ICO_REPLY) } icon_walk: { width: 20.0, height: Fit }, label: "" }
        }
    } 

    Post = <Frame> {
        walk: {width: Fill, height: Fit, margin: 0.0}
        layout: {flow: Down, padding: 0.0, spacing: 0.0}
        
        <LineH> {}

        body = <Frame> {
            walk: {width: Fill, height: Fit }
            layout: { flow: Right, padding: 10.0, spacing: 10.0 }

            profile = <Frame> {
                walk: {width: Fit, height: Fit }
                layout: { flow: Down, padding: 0.0 }
                profile = <Image> {
                    image: (IMG_PROFILE_A)
                    draw_bg:{
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            let c = self.rect_size * 0.5;
                            sdf.circle(c.x,c.y,c.x - 2.)
                            sdf.fill_keep(self.get_color());
                            sdf.stroke(#FFF8EE, 1);
                            return sdf.result
                        }
                    }
                    image_scale:0.5
                    walk: {margin: 0}
                    layout: {padding: 0}
                }
            }
            content = <Frame> {
                walk: {width: Fill, height: Fit }
                layout: { flow: Down, padding: 0.0 }
                meta = <Label> {
                    walk: {margin: {bottom: 15.0}}
                    draw_label: {
                        text_style: <TEXT_SUB> {},
                        color: #000
                    }
                    label: "@username · 13h"
                }
                
                text = <Label> {
                    draw_label: {
                        text_style: <TEXT_P> {},
                        color: #999
                    }
                    label: "Never underestimate the resilience it."
                }

                <LineH> {}
                
                likes = <IconButton> {draw_icon: {svg_file: (ICO_FAV)} icon_walk: {width: 20.0, height: Fit}}

                <PostMenu> {}
            }
            
        }
    }

	   PostImage = <Frame> {
        walk: {width: Fill, height: Fit }
        layout: {flow: Down, padding: 0.0, spacing: 0.0}
        
        hero = <Image> {
            image: (IMG_A),
            image_scale: 1.0,
            walk: {margin: 0}
            layout: {padding: 0}
        }
        
        <Post> {
            walk: {margin: {top: -32.0}}
            body= {
                content= {
                   meta= {
                        walk: { margin: {bottom: 15.0 }}
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
                spacing: 0,
                align: {
                    x: 0.0,
                    y: 0.0
                }
            },
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return #FFF8EE;
                }
            }
            
            <ScrollY> {
                walk: {height: Fill, width: Fill}
                layout: {flow: Down}
                <Frame>{walk:{height:100}}
                <PostImage> {}
                <Post> {}
                <PostImage> {}
                <Post> {}
                <Post> {}
            }
            
            <Header> {}
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
