
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    Thumbnail = <View> {
        width: Fit,

        <Image> {
            width: 100.,
            fit: Biggest,
            source: dep("crate://self/resources/hassaan-here-Ype8P9pAjXQ-unsplash.jpg")
        }
    }

    SlideshowPrev = <Button> {
        height: Fill, width: 50.,
        icon_walk: { width: 9. }
        draw_bg: {
            bodytop: #fff0,
            bodybottom: #fff2,
        }
        draw_icon: {
            svg_file: dep("crate://self/resources/icon_larr.svg")
        }

        text: ""
    }

    SlideshowNext = <SlideshowPrev> {
        draw_icon: { svg_file: dep("crate://self/resources/icon_rarr.svg") }
    }

    SlideshowMenu = <View> {
        height: Fill,
        width: Fill,
        padding: { left: 0., right: 0.}
        align: { x: 0., y: 0.92}
        spacing: 10.,

        <SlideshowPrev> {}
        <Filler> {}
        <RectShadowView> {
            visible: true; // make this an animated property so that the thumbs only show up on mouse over.

            width: Fit, height: 100.,
            spacing: 1.,
            align: { x: 0., y: 0.5}
            draw_bg: {
                shadow_radius: 20.,
            }

            <Thumbnail> {}
            <Thumbnail> {}
            <Thumbnail> {}
            <Thumbnail> {}

        }
        <SlideshowNext> {}
    }

    Slideshow = <View> {
        flow: Overlay,

        <Image> {
            width: Fill, height: Fill,
            fit: Biggest,
            source: dep("crate://self/resources/hassaan-here-Ype8P9pAjXQ-unsplash.jpg")
        }
        <SlideshowMenu> {}

    }

    ImgPlaceholder = <View> {
        height: Fit, width: Fill,
        flow: Down, 
        <Image> {
            width: Fill, height: Fit,
            fit: Biggest,
            source: dep("crate://self/resources/hassaan-here-Ype8P9pAjXQ-unsplash.jpg")
        }
        <View> {
            width: Fill, height: Fit,
            padding: { top: 5.0, right: 5.0, bottom: 5.0, left: 5.0 },
            <Label> {
                width: Fill,
                draw_text: {
                    wrap: Ellipsis,
                    text_style: {
                        font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf") }
                        font_size: 10., 
                    }
                    fn get_color(self) -> vec4 {
                        return #B;
                    }
                }

                text: "filename.jpg",
            }
            <Label> {
                draw_text: {
                    wrap: Ellipsis;
                    text_style: {
                        font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf") }
                        font_size: 10., 
                    }
                    fn get_color(self) -> vec4 {
                        return #8;
                    }
                }
                width: Fit,

                text: "03-03-23",
            }
        }
    }

    Filler = <View> {
        width: Fill, height: Fill
    }

    Menu = <View> {
        width: Fill, height: Fit,
        padding: { top: 5., right: 10, bottom: 0., left: 10.}
        margin: 0.0,
        spacing: 10.0
        align: { x: 0.0, y: 0.5} 

        <Button> {
            margin: { top: 0., right: 0., bottom: 0., left: 65. }

            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    return sdf.result
                }
            }

            draw_icon: {
                svg_file: dep("crate://self/resources/icon_folder.svg"),
            }
            icon_walk: { width: 12., margin: { left: 3.0 }}

            text: ""
        }

        <Label> {
            width: Fit,
            margin: 0.,
            draw_text: {
                text_style: {
                    font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf") }
                    font_size: 10.
                }
                color: #B
            }

            text: "../vacation/italy_2023",
        }
        <Filler> {}
        <Icon> {
            icon_walk: { width: 12.0 }
            draw_icon: {
                svg_file: dep("crate://self/resources/Icon_Search.svg"),
            }
        }

        <TextInput> {
            height: Fit,
            empty_message: "Search"
            draw_text: {
                text_style: {
                    font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf") }
                    font_size: 10., 
                }
                fn get_color(self) -> vec4 {
                    return #8
                }
            }
        }

        <Button> {
            draw_bg: {
                bodytop: #06f,
                bodybottom: #08F,
            }

            icon_walk: { width: 8.5, margin: { left: 3.0 }}
            draw_icon: {
                color: #fff,
                svg_file: dep("crate://self/resources/icon_rarr.svg"),
            }

            draw_text: {
                color: #fff,
            }
            text: "Slideshow"
        }
    }

    ImgGridMenu = <View> {
        width: Fill, height: Fit,
        margin: 0.,
        spacing: 10.,
        padding: { top: 0., right: 10, bottom: 5., left: 10.}
        align: { x: 0.0, y: 0.5 }
        flow: Right,
        <SliderAlt1> {
            width: 200.,
            step: 1.,
            min: 1.,
            max: 10.
            precision: 0,

            text: "Pics / line",
        }
        <Vr> {}
        <Label> {
            width: Fit,
            margin: 0.,
            draw_text: {
                text_style: {
                    font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf") }
                    font_size: 10.
                }
                color: #B
            }

            text: "Preview",
        }

        <RadioButton> { text: "Square" }
        <RadioButton> { text: "Original" }
        <Vr> {}
        <CheckBox> {
            width: Fit,
            margin: 0.,
            draw_text: {
                text_style: {
                    font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf") }
                    font_size: 10., 
                }
                fn get_color(self) -> vec4 {
                    return #B;
                }
            }

            text: "Show infos",
        }
        <Filler> {}
        <Label> {
            width: Fit,
            margin: 0.,
            draw_text: {
                text_style: {
                    font: { path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf") }
                    font_size: 10., 
                }
                color: #8
            }

            text: "43 Images"
        }
    }

    ImgPlaceholderRow = <View> {
        width: Fill, height: Fit,
        spacing: 1.,
        flow: Right
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
    }

    ImgGrid = <View> {
        width: Fill, height: Fill,
        flow: Down,
        margin: 0.0,
        padding: 10.0,
        spacing: 1.0,
        scroll_bars: <ScrollBars> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
        <ImgPlaceholderRow> {}
    } 

    ImgBrowser = <View> {
        width: Fill, height: Fill,
        flow: Down,
        spacing: 0.0,

        <Hr> {}
        <ImgGrid> {}
        <Hr> {}
        <ImgGridMenu> {}
    }

    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View>{
                    flow: Down,
                    spacing: 0.,
                    <Menu> {}
                    <ImgBrowser> {}
                    // <Slideshow> {}
                }
            }
        }
    }
}

app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            self.counter += 1;
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}