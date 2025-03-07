use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

    IMGVIEWER_IMG_DUMMY = dep("crate://self/resources/hassaan-here-Ype8P9pAjXQ-unsplash.jpg");
    IMGVIEWER_ICO_LARR = dep("crate://self/resources/icon_larr.svg");
    IMGVIEWER_ICO_RARR = dep("crate://self/resources/icon_rarr.svg");
    IMGVIEWER_ICO_SEARCH = dep("crate://self/resources/Icon_Search.svg");
    IMGVIEWER_ICO_FOLDER = dep("crate://self/resources/icon_folder.svg");
    IMGVIEWER_FONT_REGULAR = dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf");
    IMGVIEWER_FONT_BOLD = dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf");

    Filler = <View> { width: Fill, height: Fill }

    Thumb = <View> {
        width: Fit,

        <Image> {
            width: 100.,
            fit: Biggest,
            source: (IMGVIEWER_IMG_DUMMY)
        }
    }

    Search = <View> {
        width: Fit, height: Fit,
        align: { y: 0.5} 
        spacing: 5.

        <Icon> {
            icon_walk: { width: 12.0 }
            draw_icon: { svg_file: (IMGVIEWER_ICO_SEARCH) }
        }

        query = <TextInput> {
            height: Fit,
            empty_message: "Search"
            draw_text: {
                text_style: { font_size: 10. }
                color: #8,
            }
        }
    }

    FolderInfos = <Label> {
        draw_text: {
            text_style: {
                font: { path: (IMGVIEWER_FONT_REGULAR) }
                font_size: 10., 
            }
            color: #8
        }

        text: "43 Images"
    }

    CheckboxFolderInfos = <CheckBox> {
        draw_text: {
            text_style: { font_size: 10. }
            color: #B,
        }

        text: "Show infos",
    }

    RadiosAspectRatio = <View> {
        width: Fit, height: Fit,
        spacing: 10.,

        <Label> {
            draw_text: {
                text_style: {
                    font: { path: (IMGVIEWER_FONT_BOLD) }
                    font_size: 10.
                }
                color: #B
            }

            text: "Preview",
        }
        <RadioButton> { text: "Square" }
        <RadioButton> { text: "Original" }
    }

    SliderThumbCount = <SliderAlt1> {
        width: 200.,
        step: 1.,
        min: 1.,
        max: 10.
        precision: 0,
        text: "Pics / line",
    }

    ButtonFolder = <Button> {
        margin: { left: 55. }

        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }

        icon_walk: { width: 12., margin: { left: 3.0 }}
        draw_icon: { svg_file: (IMGVIEWER_ICO_FOLDER) }

        text: ""
    }

    ButtonSlideshow = <Button> {
        draw_bg: {
            color: #06F,
            color_hover: #09F,
            color_down: #03A,
        }

        icon_walk: { width: 8.5, margin: { left: 3.0 }}

        draw_icon: {
            color: #D
            color_hover: #F
            color_down: #A
            svg_file: (IMGVIEWER_ICO_RARR),
        }

        draw_text: {
            color: #D
            color_hover: #F
            color_down: #A
        }

        text: "Slideshow"
    }

    LabelFolder = <Label> {
        draw_text: {
            text_style: {
                font: { path: (IMGVIEWER_FONT_BOLD) }
                font_size: 10.
            }
            color: #B
        }

        text: "../vacation/italy_2023",
    }

    SlideshowPrev = <Button> {
        height: Fill, width: 50.,
        icon_walk: { width: 9. }
        draw_bg: {
            color: #fff0,
            color_down: #fff2,
        }

        draw_icon: { svg_file: (IMGVIEWER_ICO_LARR) }
        text: ""
    }

    SlideshowNext = <SlideshowPrev> { draw_icon: { svg_file: (IMGVIEWER_ICO_RARR) } }

    SlideshowMenu = <View> {
        height: Fill, width: Fill,
        align: { x: 0., y: 0.92}
        spacing: 10.,

        <SlideshowPrev> {}
        <Filler> {}
        <RectShadowView> {
            visible: true; // TODO: make this an animated property so that the thumbs only show up on mouse over.
            width: Fit, height: 100.,
            spacing: 1.,
            align: { x: 0., y: 0.5}
            draw_bg: { shadow_radius: 20. }

            <Thumb> {}
            <Thumb> {}
            <Thumb> {}
            <Thumb> {}

        }
        <SlideshowNext> {}
    }

    Slideshow = <View> {
        flow: Overlay,

        <Image> {
            width: Fill, height: Fill,
            fit: Biggest,

            source: (IMGVIEWER_IMG_DUMMY) 
        }

        <SlideshowMenu> {}
    }

    ImgPlaceholder = <View> {
        height: Fit, width: Fill,
        flow: Down, 

        <Image> {
            width: Fill, height: Fit,
            fit: Biggest,

            source: (IMGVIEWER_IMG_DUMMY)
        }

        <View> {
            width: Fill, height: Fit,
            padding: 5.,

            <Label> {
                width: Fill,
                draw_text: {
                    wrap: Ellipsis,
                    text_style: { font_size: 10. }
                    color: #B,
                }

                text: "filename.jpg",
            }
            <Label> {
                width: Fit,
                draw_text: {
                    wrap: Ellipsis;
                    text_style: { font_size: 10. }
                    color: #8,
                }

                text: "03-03-23",
            }
        }
    }

    MenuDesktop = <View> {
        width: Fill, height: Fit,
        padding: { top: 5., right: 10, left: 10.}
        spacing: 10.0
        align: { y: 0.5 } 

        <ButtonFolder> {}
        <LabelFolder> {}
        <Filler> {}
        <Search> {}
        <ButtonSlideshow> {}
    }

    MenuMobile = <View> {
        width: Fill, height: Fit,
        flow: Down,
        padding: {top: 10., left: 10.0, right: 10. }

        <View> {
            height: Fit, width: Fill,
            align: { y: 0.5} 
            spacing: 10.0

            <ButtonFolder> {}
            <LabelFolder> {}
            <Filler> {}
            <ButtonSlideshow> {}
        }

        <Hr> {}

        <View> {
            height: Fit, width: Fill,
            spacing: 10.0
            <Search> { 
                width: Fill
                query = { width: Fill }
            }
        }
    }

    ImgGridMenuDesktop = <View> {
        width: Fill, height: Fit,
        flow: Right,
        spacing: 10.,
        padding: { right: 10, bottom: 5., left: 10.},
        align: { y: 0.5 },

        <SliderThumbCount> {}
        <Vr> {}
        <RadiosAspectRatio> {}
        <Vr> {}
        <CheckboxFolderInfos> {}
        <Filler> {}
        <FolderInfos> {}
    }

    ImgGridMenuMobile = <View> {
        width: Fill, height: Fit,
        flow: Down,
        padding: { right: 10, bottom: 10., left: 10.}

        <View> {
            height: Fit, width: Fill,
            align: { y: 0.5 }
            spacing: 10.,

            <SliderThumbCount> {}
            <Filler> {}
            <RadiosAspectRatio> {}
        }

        <Hr> {}

        <View> {
            height: Fit, width: Fill,
            align: { x: 0.0, y: 0.5 }
            spacing: 10.,

            <CheckboxFolderInfos> {}
            <Filler> {}
            <FolderInfos> {}
        }
    }

    ImgPlaceholderRowDesktop = <View> {
        width: Fill, height: Fit,
        flow: Right
        spacing: 1.,

        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
    }

    ImgPlaceholderRowMobile = <View> {
        width: Fill, height: Fit,
        flow: Right
        spacing: 1.,

        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
        <ImgPlaceholder> {}
    }

    ImgGridDesktop = <View> {
        width: Fill, height: Fill,
        flow: Down,
        padding: 10.0,
        spacing: 1.0,
        scroll_bars: <ScrollBars> {}

        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
        <ImgPlaceholderRowDesktop> {}
    } 

    ImgGridMobile = <View> {
        width: Fill, height: Fill,
        flow: Down,
        padding: 10.0,
        spacing: 1.0,
        scroll_bars: <ScrollBars> {}

        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
        <ImgPlaceholderRowMobile> {}
    } 

    ImgBrowserDesktop = <View> {
        width: Fill, height: Fill,
        flow: Down,

        <Hr> {}
        <ImgGridDesktop> {}
        <Hr> {}
        <ImgGridMenuDesktop> {}
    }

    ImgBrowserMobile = <View> {
        width: Fill, height: Fill,
        flow: Down,

        <Hr> {}
        <ImgGridMobile> {}
        <Hr> {}
        <ImgGridMenuMobile> {}
    }

    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <AdaptiveView> {
                    Desktop = {
                        flow: Down,

                        // <MenuDesktop> {}
                        // <ImgBrowserDesktop> {}
                        <Slideshow> {}
                    } 

                    Mobile = {
                        flow: Down,

                        // <MenuMobile> {}
                        // <ImgBrowserMobile> {}
                        <Slideshow> {}
                    } 

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