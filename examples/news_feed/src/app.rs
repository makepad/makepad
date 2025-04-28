use makepad_widgets::*;

live_design!{
    use link::widgets::*;
    use link::theme::*;
    use link::shaders::*;
    
    IMG_A = dep("crate://self/resources/neom-THlO6Mkf5uI-unsplash.jpg")
    IMG_PROFILE_A = dep("crate://self/resources/profile_1.jpg")
    LOGO = dep("crate://self/resources/logo.svg")
    ICO_FAV = dep("crate://self/resources/icon_favorite.svg")
    ICO_COMMENT = dep("crate://self/resources/icon_comment.svg")
    ICO_HOME = dep("crate://self/resources/icon_home.svg")
    ICO_FIND = dep("crate://self/resources/icon_find.svg")
    ICO_LIKES = dep("crate://self/resources/icon_likes.svg")
    ICO_NOTIFICATION = dep("crate://self/resources/icon_notification.svg")
    ICO_USER = dep("crate://self/resources/icon_user.svg")

    COLOR_BG = #xECECEC
    COLOR_BRAND = #x0CF
    COLOR_BRAND_DARK = #x08C
    COLOR_TEXT = #x8
    COLOR_TEXT_LIGHT = #xCCC
    COLOR_USER = #x444
    COLOR_DIVIDER = #x00000010
    Logo = <Button> {
        height: Fit, width: Fit,
        draw_icon: {
            svg_file: (LOGO),
            fn get_color(self) -> vec4 {
                return (COLOR_BRAND)
            }
        }
        icon_walk: {width: Fit, height: 20.0}
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
            instance down: 0.0
            text_style: {
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_TEXT_LIGHT),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_DARK),
                    self.down
                )
            }
        }
        
        draw_icon: {
            svg_file: (ICO_FAV),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_TEXT_LIGHT),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_DARK),
                    self.down
                )
            }
        }

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        text: ""

        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.5}}
                    redraw: true,
                    ease: OutQuint
                    apply: {
                        icon_walk: {
                            width: 10.0
                        }
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_icon: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on = {
                    redraw: true,
                    ease: OutElastic
                    from: {
                        all: Forward {duration: 0.5}
                    }
                    apply: {
                        icon_walk: {
                            width: 15.
                        }
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

            }
        }
     }

    IconButtonMenu = <Button> {
        draw_text: {
            instance hover: 0.0
            instance down: 0.0
            text_style: {
                font_size: 11.0
            }

            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_TEXT_LIGHT),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_DARK),
                    self.down
                )
            }
        }

        draw_icon: {
            svg_file: (ICO_FAV),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        (COLOR_TEXT_LIGHT),
                        (COLOR_BRAND),
                        self.hover
                    ),
                    (COLOR_BRAND_DARK),
                    self.down
                )
            }
        }

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        padding: 9.0
        text: ""

        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.2}}
                    redraw: true,
                    ease: OutQuad
                    apply: {
                        icon_walk: {
                            width: 20.0
                        }
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_icon: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on = {
                    redraw: true,
                    ease: OutQuad
                    from: {
                        all: Forward {duration: 0.2 }
                    }
                    apply: {
                        icon_walk: {
                            width: 30.
                        }
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }
    }

    Header = <View> {
        width: Fill, height: Fit,
        flow: Down,
        show_bg: true,
        draw_bg: {color: (COLOR_BG)}

        <View> {
            width: Fill, height: 60,
            flow: Right,
            padding: 10.0,
            spacing: 5.0
            align: { x: 0., y: 0.5}

            <Logo> {}

            <Image> {
                width: 30., height: 30.,
                source: (IMG_PROFILE_A)
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        let c = self.rect_size * 0.5;
                        sdf.circle(c.x, c.y, c.x - 2.)
                        sdf.fill_keep(self.get_color());
                        sdf.stroke((#f), 1);
                        return sdf.result
                    }
                }
            }

            <TextInput> {
                width: Fill
                text: "",
                empty_text: "What's up?",

                draw_bg: {
                    instance radius: (THEME_CORNER_RADIUS)
                    instance hover: 0.0
                    instance focus: 0.0
                    instance bodytop: #0000000C
                    instance bodybottom: #00000010

                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        let grad_top = 5.0;
                        let grad_bot = 1.5;

                        let body = mix(
                            // self.bodytop,
                            mix(#0000000C, #0000000C, self.pos.y * 2.5),
                            mix(#00000020, #00000010, self.pos.y * 2.5),
                            // self.bodybottom,
                            self.focus
                        );

                        let body_transp = (THEME_COLOR_D_HIDDEN)

                        let top_gradient = mix(
                            body_transp,
                            mix(#00000000, #xD0D0D0, self.focus),
                            max(0.0, grad_top - sdf.pos.y) / grad_top
                        );

                        let bot_gradient = mix(
                            #fff,
                            top_gradient,
                            clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                        );

                        sdf.box(
                            1.,
                            1.,
                            self.rect_size.x - 2.0,
                            self.rect_size.y - 2.0,
                            self.radius
                        )

                        sdf.fill_keep(body)

                        sdf.stroke(
                            bot_gradient,
                            THEME_BEVELING * 0.9
                        )

                        return sdf.result
                    }
                }

                draw_text: {
                    instance hover: 0.0
                    instance focus: 0.0
                    fn get_color(self) -> vec4 {
                        return
                        mix(
                            #00000044,
                            #00000099,
                            self.focus
                        )
                    }
                }

                draw_cursor: {
                    instance focus: 0.0
                    uniform border_radius: 0.5
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                        sdf.box(
                            0.,
                            0.,
                            self.rect_size.x,
                            self.rect_size.y,
                            self.border_radius
                        )
                        sdf.fill(mix(THEME_COLOR_U_HIDDEN, COLOR_BRAND, self.focus));
                        return sdf.result
                    }
                }

                animator: {
                    hover = {
                        default: off
                        off = {
                            from: {all: Forward {duration: 0.1}}
                            apply: {
                                draw_text: {hover: 0.0},
                                draw_highlight: {hover: 0.0}
                            }
                        }
                        on = {
                            from: {all: Snap}
                            apply: {
                                draw_text: {hover: 1.0},
                                draw_highlight: {hover: 1.0}
                            }
                        }
                    }
                    focus = {
                        default: off
                        off = {
                            redraw: true,
                            from: {all: Forward { duration: 0.7 }}
                            ease: OutElastic
                            apply: {
                                draw_bg: {focus: 0.0},
                                draw_text: {
                                    focus: 0.0,
                                    text_style: {
                                        font_size: 10.0
                                    }
                                }
                                draw_cursor: {focus: 0.0},
                                draw_highlight: {focus: 0.0}
                            }
                        }
                        on = {
                            redraw: true,
                            from: {all: Forward {duration: .25 }}
                            ease: OutQuint
                            apply: {
                                draw_bg: {focus: 1.0},
                                draw_text: {
                                    focus: 1.0,
                                    text_style: {
                                        font_size: 12.
                                    }
                                }
                                draw_cursor: {focus: 1.0},
                                draw_highlight: {focus: 1.0}
                            }
                        }
                    }
                }

            }

            find = <IconButton> {
                draw_icon: { svg_file: (ICO_FIND) }
                icon_walk: {width: 20.0, height: Fit},
                width: 30.

                animator: {
                    hover = {
                        off = {
                            apply: { icon_walk: { width: 13.0 } }
                        }
                        on = {
                            apply: {
                                icon_walk: {
                                    width: 17.
                                }
                            }
                        }
                    }
                }
            }
        }

    }

    Menu = <View> {
        width: Fill, height: 80
        flow: Right,
        padding: 10.0,
        show_bg: true,
        draw_bg: {color: (COLOR_BG)}

        <View> {
            width: Fill, height: Fit,
            flow: Right,
            align: {x: 0.5, y: 1.0}

            margin: 0.0,
            padding: 0.0,

            <Filler> {}
            <View> {
                align: { x: .0, y: 0.5 }
                height: 40, width: Fit,
                <IconButtonMenu> {
                    draw_icon: {svg_file: (ICO_HOME)},
                    icon_walk: {height: Fit},
                    text: "",
                    animator: {
                        hover = {
                            off = {
                                apply: {
                                    icon_walk: {
                                        width: 27.5
                                    }
                                }
                            }

                            on = {
                                apply: {
                                    icon_walk: {
                                        width: 40.
                                    }
                                }
                            }
                        }
                    }
                }
            }
            <View> {
                align: { x: 0.5, y: 0.5 }
                height: 40, width: Fit,
                <IconButtonMenu> {
                    draw_icon: { svg_file: (ICO_LIKES) },
                    icon_walk: {width: 20.0, height: Fit},
                    text: "",
                    animator: {
                        hover = {
                            off = {
                                apply: {
                                    icon_walk: {
                                        width: 24.5
                                    }
                                }
                            }

                            on = {
                                apply: {
                                    icon_walk: {
                                        width: 42.
                                    }
                                }
                            }
                        }
                    }
                }
            }
            <View> {
                align: { x: 0.5, y: 0.5 }
                height: 40, width: Fit,
                <IconButtonMenu> {
                    draw_icon: { svg_file: (ICO_USER)},
                    icon_walk: {width: 18.0, height: Fit},
                    text: ""
                    animator: {
                        hover = {
                            off = {
                                apply: {
                                    icon_walk: {
                                        width: 22.0
                                    }
                                }
                            }

                            on = {
                                apply: {
                                    icon_walk: {
                                        width: 35.
                                    }
                                }
                            }
                        }
                    }
                }
            }
            <View> {
                align: { x: 0.5, y: 0.5 }
                height: 40, width: Fit,
                <IconButtonMenu> {
                    draw_icon: { svg_file: (ICO_NOTIFICATION)},
                    icon_walk: {width: 22.0, height: Fit},
                    text: ""
                    animator: {
                        hover = {
                            off = {
                                apply: {
                                    icon_walk: {
                                        width: 22.0
                                    }
                                }
                            }

                            on = {
                                apply: {
                                    icon_walk: {
                                        width: 34.5
                                    }
                                }
                            }
                        }
                    }
                }
            }
            <Filler> {}
        }
    }

    LineH = <RoundedView> {
        width: Fill, height: 2,
        margin: 0.0, padding: 0.0,
        spacing: 0.0
        draw_bg: {color: (COLOR_DIVIDER)}
    }

    PostMenu = <View> {
        width: Fit, height: Fit,
        flow: Right,
        align: { x: 0., y: 0.5 }

        likes = <IconButton> {
            draw_icon: {svg_file: (ICO_FAV)} icon_walk: {width: 11.0, height: Fit}
            width: 50.
            margin: 0, padding: 0.
        }

        comments = <IconButton> {
            draw_icon: {svg_file: (ICO_COMMENT)} icon_walk: {width: 11.0, height: Fit}, text: "7"
            width: 40.
            margin: 0, padding: 0.
        }

    }

    Post = <View> {
        width: Fill, height: Fit,
        margin: { top: 0, right: 10., bottom: 10., left: 10. }
        flow: Down,

        body = <RoundedView> {
            width: Fill, height: Fit
            flow: Right,
            padding: 10.0,
            spacing: 10.0
            show_bg: true,
            draw_bg: {color: #f}

            profile = <View> {
                width: Fit, height: Fit,
                margin: {top: 2.5}
                flow: Down,

                profile_img = <Image> {
                    width: 40., height: 40.
                    source: (IMG_PROFILE_A)
                    draw_bg: {
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            let c = self.rect_size * 0.5;
                            sdf.circle(c.x, c.y, c.x - 2.)
                            sdf.fill_keep(self.get_color());
                            sdf.stroke((#f), 1);
                            return sdf.result 
                        }
                    }
                }
            }

            content = <View> {
                width: Fill, height: Fit
                flow: Down,
                spacing: 15.,

                <View> {
                    flow: Right,
                    height: Fit, width: Fill,
                    spacing: 5.,
                    margin: { bottom: 10.0, top: 5.}

                    meta = <Pbold> {
                        width: Fit,
                        margin: 0.,
                        draw_text: { color: (COLOR_USER) }
                        text: "John Doe"
                    }
                    <P> {
                        text: "13h",
                        draw_text: { color: (COLOR_TEXT_LIGHT) }
                    }

                    <PostMenu> {}
                }

                text = <P> {
                    width: Fill,
                    height: Fit
                    draw_text: {
                        text_style: { line_spacing: 1.3 }
                        wrap: Word,
                        color: (COLOR_TEXT)
                    }
                    text: ""
                }

                <LineH> {}

                <TextInput> {
                    width: Fill,

                    empty_message: "Reply"
                    margin: { top: -17. }

                    draw_bg: {
                        instance radius: (THEME_CORNER_RADIUS)
                        instance hover: 0.0
                        instance focus: 0.0
                        instance bodytop: #00000000
                        instance bodybottom: #00000010

                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            let grad_top = 5.0;
                            let grad_bot = 1.5;

                            let body = mix(
                                self.bodytop,
                                self.bodybottom,
                                self.focus
                            );

                            let body_transp = (THEME_COLOR_D_HIDDEN)

                            let top_gradient = mix(
                                body_transp,
                                mix(#00000000, #00000030, self.focus),
                                max(0.0, grad_top - sdf.pos.y) / grad_top
                            );

                            let bot_gradient = mix(
                                (THEME_COLOR_BEVEL_LIGHT),
                                top_gradient,
                                clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                            );

                            sdf.box(
                                1.,
                                1.,
                                self.rect_size.x - 2.0,
                                self.rect_size.y - 2.0,
                                self.radius
                            )

                            sdf.fill_keep(body)

                            sdf.stroke(
                                bot_gradient,
                                THEME_BEVELING * 0.9
                            )

                            return sdf.result
                        }
                    }

                    draw_text: {
                        instance hover: 0.0
                        instance focus: 0.0
                        wrap: Word,
                        text_style: <THEME_FONT_REGULAR> {
                            line_spacing: (THEME_FONT_LINE_SPACING),
                            font_size: (THEME_FONT_SIZE_P)
                        }
                        fn get_color(self) -> vec4 {
                            return mix(
                                mix(#A, (COLOR_BRAND), self.hover),
                                #9,
                                self.focus
                            )
                        }
                    }

                    draw_cursor: {
                        instance focus: 0.0
                        uniform border_radius: 0.5
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            sdf.box(
                                0.,
                                0.,
                                self.rect_size.x,
                                self.rect_size.y,
                                self.border_radius
                            )
                            sdf.fill(mix(THEME_COLOR_U_HIDDEN, COLOR_BRAND, self.focus));
                            return sdf.result
                        }
                    }

                    animator: {
                        hover = {
                            default: off
                            off = {
                                from: {all: Forward {duration: 0.1}}
                                apply: {
                                    draw_text: {hover: 0.0},
                                    draw_highlight: {hover: 0.0}
                                }
                            }
                            on = {
                                from: {all: Snap}
                                apply: {
                                    draw_text: {hover: 1.0},
                                    draw_highlight: {hover: 1.0}
                                }
                            }
                        }
                        focus = {
                            default: off
                            off = {
                                redraw: true,
                                from: {all: Forward { duration: 0.4 }}
                                ease: OutQuint
                                apply: {
                                    draw_bg: {focus: 0.0},
                                    padding: { left: 0., top: 7.5 }
                                    draw_text: {
                                        focus: 0.0,
                                        text_style: {
                                            font_size: 9.0
                                        }
                                    }
                                    draw_cursor: {focus: 0.0},
                                    draw_highlight: {focus: 0.0}
                                }
                            }
                            on = {
                                redraw: true,
                                from: {all: Forward {duration: .25 }}
                                ease: OutQuint
                                apply: {
                                    draw_bg: {focus: 1.0},
                                    padding: { left: 5., top: 5.}
                                    draw_text: {
                                        focus: 1.0,
                                        text_style: {
                                            font_size: 11.
                                        }
                                    }
                                    draw_cursor: {focus: 1.0},
                                    draw_highlight: {focus: 1.0}
                                }
                            }
                        }
                    }

                }
            }
        }
    }

    PostImage = <View> {
        width: Fill, height: Fit
        flow: Down,
        spacing: 0.0

        hero = <Image> {
            source: (IMG_A),
            margin: 0,
            fit: Biggest,
            width: Fill,
            height: 200
        }

        post = <Post> {
            margin: { top: -30, right: 10., bottom: 10., left: 10. }
            body = {
                padding: { top: 10., right: 10., left: 10., bottom: 10. }
                content = {
                    /*meta = {
                        margin: {bottom: 30.0, top: 10.0}
                        draw_text: {
                            color: (COLOR_BRAND_DARK)
                        }
                    }*/
                }
            }
        }
    }

    myScrollBar = <ScrollBar> {
        bar_size: 10.0,
        bar_side_margin: 3.0
        min_handle_size: 30.0
        axis: Vertical
        smoothing: 10.0
        use_vertical_finger_scroll: false

        draw_bg: {
            instance down: 0.0
            instance hover: 0.0
            
            instance color: #888,
            instance color_hover: #999
            instance color_down: #666
            
            uniform size: 6.0
            uniform border_radius: 1.5

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                if self.is_vertical > 0.5 {
                    sdf.box(
                        1.,
                        self.rect_size.y * self.norm_scroll,
                        self.size,
                        self.rect_size.y * self.norm_handle,
                        self.border_radius
                    );
                }
                else {
                    sdf.box(
                        self.rect_size.x * self.norm_scroll,
                        1.,
                        self.rect_size.x * self.norm_handle,
                        self.size,
                        self.border_radius
                    );
                }
                sdf.fill(mix(
                    self.color, 
                    mix(
                        self.color_hover,
                        self.color_down,
                        self.down
                    ),
                    self.hover
                ));
                return sdf.result;
            }
        }
    }

    NewsFeed ={{NewsFeed}}{
        list = <PortalList>{
            scroll_bar: <myScrollBar> {}
            TopSpace = <View> {height: 0}
            Post = <CachedView>{<Post> {}}
            PostImage = <PostImage> {}
            BottomSpace = <View> {height: 100}
        }
    }

    App = {{App}} {
        ui: <Root>{
                <Window> {
    
                window: {inner_size: vec2(428, 926)},
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
    
    
                    <View> {
                        flow: Down
                        <Header> {}
                        <Filler> {}
                        <Menu> {}
                    }
    
                    news_feed = <NewsFeed> {
                        padding: {top: 60., bottom: 90.}
                    }
    
                }
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveHook, Widget)]
struct NewsFeed{
    #[deref] view:View,
}

impl Widget for NewsFeed{
    fn draw_walk(&mut self, cx:&mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(item) =  self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 1000);
                while let Some(item_id) = list.next_visible_item(cx) {
                    let template = match item_id{
                        0 => live_id!(TopSpace),
                        x if x % 5 == 0 => live_id!(PostImage),
                        _ => live_id!(Post)
                    };
                    let item = list.item(cx, item_id, template);
                    let text = match item_id % 4 {
                        1 => format!("At vero eos et accusam et justo duo dolores et ea rebum. Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet. Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua. At vero eos et accusam et justo duo dolores et ea rebum. id: {}", item_id),
                        2 => format!("How are you? Item id: {}", item_id),
                        3 => format!("Stet clita kasd gubergren, no sea takimata sanctus est Lorem ipsum dolor sit amet. id: {}", item_id),
                        _ => format!("Lorem ipsum dolor sit amet, consetetur sadipscing elitr, sed diam nonumy eirmod tempor invidunt ut labore et dolore magna aliquyam erat, sed diam voluptua. 4 id {}", item_id),
                    };
                    item.label(id!(content.text)).set_text(cx, &text);
                    item.button(id!(likes)).set_text(cx, &format!("{}", item_id % 23));
                    item.button(id!(comments)).set_text(cx, &format!("{}", item_id % 6));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
    fn handle_event(&mut self, cx:&mut Cx, event:&Event, scope:&mut Scope){
        self.view.handle_event(cx, event, scope)
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx:&mut Cx){
    }
    fn handle_actions(&mut self, _cx:&mut Cx, actions:&Actions){
        if self.ui.button(id!(find)).clicked(&actions) {
            
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
