#![allow(dead_code)]

use shui::*;

struct App{
    dn:DrawNode,
    ok:Button
}

impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            dn:DrawNode{..Default::default()},
            ok:Button{
                ..Style::style(cx)
            }
        }
    }
}

impl App{
    fn handle(&mut self, cx:&mut Cx, ev:&Ev){
        if self.ok.handle_click(cx, ev){
            // do something!
        }
    }

    fn draw(&mut self, cx:&mut Cx){
        self.dn.begin(cx);

        self.ok.draw_with_label(cx, "OK");

        self.dn.end(cx);
    }
}

fn main() {
    let mut cx = Cx{
        title:"Ho World".to_string(),
        ..Default::default()
    };

    let mut app = App{
        ..Style::style(&mut cx)
    };

    cx.event_loop(|cx, ev|{
        if let Ev::Redraw = ev{
            return app.draw(cx);
        }
        app.handle(cx, &ev);
    });
}