use {
    crate::makepad_widgets::{text::{non_nan::NonNanF32, geom::Point, text::{Baseline, Color, Text, Span, Style}}, *},
    std::rc::Rc,
};

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    pub MyWidget = {{MyWidget}} {
        width: Fill, height: Fill,
        spacing: 7.5,
        align: {x: 0.5, y: 0.5},
        padding: <THEME_MSPACE_2> {}
        label_walk: { width: Fit, height: Fit },
                        
        draw_text: {
            color: (THEME_COLOR_TEXT_DEFAULT)
            text_style: <THEME_FONT_REGULAR> {
                font_size: (THEME_FONT_SIZE_P)
            }
            fn get_color(self) -> vec4 {
                return self.color
            }
        }
                        
        draw_bg: {
            fn pixel(self) -> vec4 {
                return #0
            }
        }
    }
    
    App = {{App}} {
        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill,
            window: {
                inner_size: vec2(400, 300)
            },
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return #4;
                }
            }
            body = <View> {
                padding: 20
                flow: Down
                widget = <MyWidget> {}
            }
        }
    }
}

app_main!(App);

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App {}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct MyWidget {
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText2,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    label_walk: Walk,
}

impl Widget for MyWidget {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let mut text = Text::default();
        /*
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap()
            },
            text: "繁😊😔 The quick brown fox ".into()
        });
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(20.0).unwrap()
            },
            text: "Averylongwordtoshowthatdesperatebreakswork jumps ".into()
        });
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap()
            },
            text: "繁😊😔 over the lazy dog.".into()
        });
        */
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
                color: Color::BRIGHT_RED,
                baseline: Baseline::Alphabetic,
            },
            text: "XBaseline alphabetic ".into()
        });
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
                color: Color::BRIGHT_GREEN,
                baseline: Baseline::Top,
            },
            text: "XBaseline top ".into()
        });
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
                color: Color::BRIGHT_BLUE,
                baseline: Baseline::Bottom,
            },
            text: "XBaseline bottom ".into()
        });
        self.draw_text.draw(cx, Point::new(50.0, 50.0), Rc::new(text));
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}
