#![allow(dead_code)]

use shui::*;

struct App{
    view:View,
    lay:Lay,
    ok:Button
}

impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(),
            lay:Lay::dynamic(),
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
        self.view.begin(cx, &self.lay);
        //self.text.draw_text(cx, "Hey world");
        self.ok.draw_with_label(cx, "OK");

        self.view.end(cx);
    }
}

fn main() {
    let mut cx = Cx{
        title:"Hi World".to_string(),
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