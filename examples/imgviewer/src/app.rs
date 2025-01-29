
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

    SlideshowMenu = <View> {
            align: { x: 0., y: 0.9}
            height: Fill, width: Fill,
            padding: { left: 0., right: 0.}
            spacing: 10.,
            <ButtonFlat> {
                text: ""
                height: Fill, width: 50.,
                icon_walk: { width: 9. }
                draw_icon: {
                    svg_file: dep("crate://self/resources/icon_larr.svg"),
                }
            }
            <Filler> {}
            <RoundedShadowView> {
                align: { x: 0., y: 0.5}
                flow: Right,
                spacing: 1.,
                width: Fit, height: 100.,
                <Thumbnail> {}
                <Thumbnail> {}
                <Thumbnail> {}
                <Thumbnail> {}
            }
            <ButtonFlat> {
                text: ""
                height: Fill, width: 50.,
                icon_walk: { width: 9. }
                draw_icon: {
                    svg_file: dep("crate://self/resources/icon_rarr.svg"),
                }
            }
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
            padding: { top: 5.0, right: 5.0, bottom: 0.0, left: 5.0 },
            <Pbold> {
                text: "filename.jpg",
                draw_text: {
                    wrap: Ellipsis;
                }
            }
            <P> {
                width: Fit,
                text: "03-03-23",
                draw_text: {
                    color: #8,
                }
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
        <ButtonFlatter> {
            text: ""
            padding: 0.,
            margin: { top: 0., right: 0., bottom: 0., left: 65. }
            icon_walk: { width: 12., margin: { left: 3.0 }}
            draw_icon: {
                svg_file: dep("crate://self/resources/icon_folder.svg"),
            }
        }
        <Pbold> {
            width: Fit, height: Fit,
            margin: 0.,
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
        }
        <ButtonIcon> {
            text: "Slideshow"
            icon_walk: { width: 8.5, margin: { left: 3.0 }}
            draw_bg: {
                bodytop: #06f,
                bodybottom: #08F,
            }

            draw_text: {
                color: #fff,
            }
            draw_icon: {
                color: #fff,
                svg_file: dep("crate://self/resources/icon_rarr.svg"),
            }
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
            text: "Pics / line",
            width: 200.,
            step: 1.,
            min: 1.,
            max: 10.
            precision: 0,
        }
        <Vr> {}
        <Pbold> {
            width: Fit,
            margin: 0.
            text: "Preview",
        }
        <RadioButton> { text: "Square" }
        <RadioButton> { text: "Original" }
        <Vr> {}
        <CheckBox> { text: "Show infos" }
        <Filler> {}
        <P> {
            width: Fit,
            margin: 0.
            draw_text: {
                color: #8,
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
        padding: 5.0,
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
