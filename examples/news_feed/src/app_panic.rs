use makepad_widgets::*;
   
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    IMG_A = dep("crate://self/resources/neom-THlO6Mkf5uI-unsplash.jpg")
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
    
    FONT_SIZE_SUB = §§.5
    FONT_SIZE_P = 12.5
    
    TEXT_SUB = {
        font_size: (FONT_SIZE_SUB),
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
    }
    
    TEXT_P = {
        font_size: (FONT_SIZE_P),
        height_factor: 1.65,
        font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
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
    
    FillerY = <View> {width: Fill}
    
    FillerX = <View> {height: Fill}
    
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
        padding: 9.0
        text: ""
    }
    
    IconButton = <Button> {
        draw_text: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: {
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
        padding: 9.0
        text: "1"
    }
    
    
    Header = <RoundedYView> {
        width: Fill,
        height: 70
        flow: Right,
        padding: 10.0,
        spacing: 10.0
        draw_bg: {color: (COLOR_OVERLAY_BG), inset: vec4(-0.5, -0.5, -1.0, 0.0), radius: vec2(0.5, 4.5)}
        
        <Logo> {
            height: Fit,
            width: Fill,
            margin: {top: 0.0}
            icon_walk: {width: Fit, height: 27.0}
        }
        
    }
    
    Menu = <RoundedYView> {
        width: Fill,
        height: 80
        flow: Right,
        padding: 10.0,
        spacing: 10.0
        draw_bg: {color: (COLOR_OVERLAY_BG), inset: vec4(-0.5, 0.0, -1.0, -1.0), radius: vec2(4.5, 0.5)}
        
        <View> {
            width: Fill,
            height: Fit,
            margin: 0.0
            flow: Right,
            padding: 0.0,
            spacing: 25.0,
            align: {x: 0.5, y: 0.5}
            
            <IconButton> {draw_icon: {svg_file: (ICO_HOME)} icon_walk: {width: 30.0, height: Fit}, text: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_FIND)} icon_walk: {width: 18.0, height: Fit}, text: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_ADD)} icon_walk: {width: 40.0, height: Fit}, text: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_LIKES)} icon_walk: {width: 20.0, height: Fit}, text: ""}
            <IconButton> {draw_icon: {svg_file: (ICO_USER)} icon_walk: {width: 15.0, height: Fit}, text: ""}
        }
    }
    
    LineH = <RoundedView> {
        width: Fill,
        height: 2,
        margin: 0.0
        padding: 0.0,
        spacing: 0.0
        draw_bg: {color: (COLOR_DIVIDER)}
    }
    
    PostMenu = <View> {
        width: Fill,
        height: Fit,
        margin: 0.0
        flow: Down,
        padding: 0.0,
        spacing: 0.0
        
            <View> {
            width: Fill,
            height: Fit,
            margin: 0.0
            flow: Right,
            padding: 0.0,
            spacing: 10.0
            
            likes = <IconButton> {draw_icon: {svg_file: (ICO_FAV)} icon_walk: {width: 15.0, height: Fit}}
            comments = <IconButton> {draw_icon: {svg_file: (ICO_COMMENT)} icon_walk: {width: 15.0, height: Fit}, text: "7"}
            
            <FillerX> {}
            reply = <IconButton> {draw_icon: {svg_file: (ICO_REPLY)} icon_walk: {width: 15.0, height: Fit}, text: ""}
        }
    }
    
    Post = <View> {
        width: Fill,
        height: Fit,
        margin: 0.0
        flow: Down,
        padding: 0.0,
        spacing: 0.0
        
        body = <View> {
            width: Fill,
            height: Fit
            flow: Right,
            padding: 10.0,
            spacing: 10.0
            
            profile = <View> {
                width: Fit,
                height: Fit,
                margin: {top: 7.5}
                flow: Down,
                padding: 0.0
                profile_img = <Image> {
                    source: (IMG_PROFILE_A)
                    margin: 0,
                    width: 50.,
                    height: 50.
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
                }
            }
            content = <View> {
                width: Fill,
                height: Fit
                flow: Down,
                padding: 0.0
                
                meta = <Label> {
                    margin: {bottom: 10.0, top: 10.0}
                    draw_text: {
                        text_style: <TEXT_SUB> {},
                        color: (COLOR_META_TEXT)
                    }
                    text: "@Hello world· 13h"
                }
                text = <Label> {
                    width: Fill,
                    height: Fit
                    draw_text: {
                        wrap: Word,
                        text_style: <TEXT_P> {},
                        color: (COLOR_P)
                    }
                    text: ""
                }
                
                <LineH> {
                    margin: {top: 10.0, bottom: 5.0}
                }
                
                <PostMenu> {}
            }
        }
        
        <LineH> {
            draw_bg: {color: (COLOR_DIVIDER_DARK)}
        }
    }
    
    PostImage = <View> {
        width: Fill,
        height: Fit
        flow: Down,
        padding: 0.0,
        spacing: 0.0
        
        hero = <Image> {
            source: (IMG_A),
            //image_scale: 1.0,
            margin: 0,
            width: Fill,
            height: 200
        }
        
        post = <Post> {
            margin: {top: -45.0}
            body = {
                content = {
                    meta = {
                        margin: {bottom: 30.0, top: 10.0}
                        draw_text: {
                            color: (COLOR_META_INV)
                        }
                    }
                }
            }
        }
    }
    
    
    App = {{App}} {
        ui: <Window> {
            window: {inner_size: vec2(428, 926), dpi_override: 2},
            show_bg: true
            
            
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return (COLOR_BG);
                }
            }
            body = {
                flow: Overlay,
                padding: 0.0
                spacing: 0,
                align: {
                    x: 0.0,
                    y: 0.0
                },
                
                
                news_feed = <PortalList> {
                    height: Fill,
                    width: Fill
                    flow: Down
                    TopSpace = <View> {height: 80}
                    Post = <Post> {}
                    PostImage = <PostImage> {}
                    BottomSpace = <View> {height: 100}
                }
                
                <View> {
                    height: Fill,
                    width: Fill
                    flow: Down
                    
                        <Header> {}
                    <FillerY> {}
                    <Menu> {}
                }
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
        let news_feeds = self.ui.portal_list_set(ids!(news_feed));
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                if let Some(mut list) = news_feeds.has_widget(&next).borrow_mut() {
                    // lets set our scroll range so the scrollbar has something
                    list.set_item_range(cx, 0, 1000);
                    // next nly returns items that are visible
                    // this means the performance here is O(visible)
                    while let Some(item_id) = list.next_visible_item(cx) {
                        let template = match item_id {
                            0 => live_id!(TopSpace),
                            x if x % 5 == 0 => live_id!(PostImage),
                            _ => live_id!(Post)
                        };
                        let item = list.item(cx, item_id, template).unwrap();
                        let text = match item_id % 4 {
                            1 => format!("The {} Requires recompile", item_id),
                            2 => format!("Item: {} 欢迎来到, adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqu", item_id),
                            3 => format!("Item: {} Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor", item_id),
                            _ => format!("Item: {} Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea", item_id),
                        };
                        item.label(id!(content.text)).set_text(&text);
                        item.button(id!(likes)).set_text(&format!("{}", item_id % 23));
                        item.button(id!(comments)).set_text(&format!("{}", item_id % 6));
                        item.draw_widget_all(cx);
                    }
                }
            }
            return
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        for (item_id, item) in news_feeds.items_with_actions(&actions) {
            if item.button(id!(likes)).clicked(&actions) {
                log!("Test {}", item_id);
            }
        }
    }
}