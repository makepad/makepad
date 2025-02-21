use { 
    crate::{
        makepad_widgets::*,
    },
};

// We dont have a UI yet 

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    pub Drawer = {{Drawer}} {
        width: Fit, height: Fit,
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
                return #6
            }
        }
    }
    
    App = {{App}} {
        ui: <Window>{
            show_bg: true
            width: Fill,
            height: Fill,
            window: {inner_size: vec2(400, 300)},
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return #4;
                }
            }
            body = <View>{
                padding:20
                flow:Down
                drawer = <Drawer>{}
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

impl MatchEvent for App{
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct Drawer {
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    label_walk: Walk,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout
}

impl Widget for Drawer {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text
        .draw_walk(cx, self.label_walk, Align::default(), "Hello world");
        self.draw_bg.end(cx);
        DrawStep::done()
    }
}
