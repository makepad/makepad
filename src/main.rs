#![allow(dead_code)]

use shui::*;
mod button;
use crate::button::*;

struct App{
    view:View,
    ok:Button,
    debug_qd:Quad,
    debug_tx:Text
}

impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(),
            ok:Button{
                ..Style::style(cx)
            },
            debug_qd:Quad{
                ..Style::style(cx)
            },
            debug_tx:Text{
                font_size:5.0,
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
        self.view.begin(cx, &Layout::filled_padded(10.0));
        //self.text.draw_text(cx, "Hey world");
        self.ok.draw_with_label(cx, "OK");

        self.view.end(cx);

        debug_pts_store.with(|c|{
            let mut store = c.borrow_mut();
            for (x,y,col,s) in store.iter(){
                self.debug_qd.color = match col{
                    0=>color("red"),
                    1=>color("green"),
                    2=>color("blue"),
                    _=>color("yellow")
                };
                self.debug_qd.draw_abs(cx, false, *x, *y,2.0,2.0);
                if s.len() != 0{
                    self.debug_tx.draw_text(cx, Fixed(*x), Fixed(*y), s);
                }
            }
            store.truncate(0);
        })
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