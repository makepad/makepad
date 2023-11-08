use crate::{makepad_live_id::*};
use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    
    App = {{App}} {
        ui: <Window> {
            window: {inner_size: vec2(2000, 1024)},
            caption_bar = {visible: true, caption_label = {label = {text: "Graph"}}},
            body = <Dock> {
                height: Fill,
                width: Fill
                        
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(300.0),
                    a: list,
                    b: split1
                }
                        
                split1 = Splitter {
                    axis: Vertical,
                    align: FromB(200.0),
                    a: graph_view,
                    b: log_view
                }
                        
                list = Tab {
                    name: ""
                    kind: ListView
                }
                        
                graph_view = Tab {
                    name: ""
                    kind: GraphView
                }
                        
                log_view = Tab {
                    name: ""
                    kind: LogView
                }
                        
                LogView = <RectView> {
                    draw_bg: {color: #2}
                    height: Fill,
                    width: Fill
                }
                        
                GraphView = <RectView> {
                    height: Fill,
                    width: Fill
                    draw_bg: {color: #2}
                }
                        
                ListView = <RectView> {
                    draw_bg: {color: #2}
                    height: Fill,
                    width: Fill
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
    
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
       
    }
}

impl App {
    
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(_next) = self.ui.draw_widget(cx).hook_widget() {
                
            }
            return
        }
        
        let _actions = self.ui.handle_widget_event(cx, event);
    }
}