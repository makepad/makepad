
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;

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
            padding: 5.0,
            <Pbold> { text: "filename.jpg" }
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
        padding: 10.0,
        margin: 0.0,
        spacing: 10.0
        align: { x: 0.0, y: 0.5} 
        <H4> {
            width: Fit, height: Fit,
            text: "../vacation/italy_2023",
            margin: { left: 35. }
        }
        <Filler> {}
        <Icon> {
            icon_walk: { width: 15.0 }
            draw_icon: {
                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
            }
        }
        <TextInput> {
            height: Fit,
            empty_message: "Search"
        }
        <ButtonIcon> {
            text: "Slideshow"
            draw_icon: {
                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
            }
        }
    }

    ImgGridMenu = <View> {
        width: Fill, height: Fit,
        spacing: 10.,
        padding: 10.,
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
        <Pbold> { width: Fit, text: "Preview" }
        <RadioButton> { text: "Square" }
        <RadioButton> { text: "Original" }
        <Vr> {}
        <CheckBox> { text: "Info" }
        <Filler> {}
        <P> {
            text: "43 Images"
            width: Fit,
            draw_text: {
                color: #8,
            }
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
    } 

    ImgBrowser = <View> {
        width: Fill, height: Fill,
        flow: Down,
        spacing: 0.0,

        <Hr> {}
        <View> {
            width: Fill, height: Fill,
            show_bg: true,
            draw_bg: { color: #0002 }
            <ImgGrid> {}
        }
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
