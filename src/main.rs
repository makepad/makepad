use shui::*;
mod button;
use crate::button::*;

struct App{
    view:View,
    ok:Button,
    oks:Elements<Button>,
}
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(),
            ok:Button{
                ..Style::style(cx)  
            }, 
            oks:Elements::new(),
        }
    }
}

impl App{
    fn handle(&mut self, cx:&mut Cx, event:&Event){
        if let Event::Redraw = event{return self.draw(cx);}

        if let ButtonEvent::Clicked = self.ok.handle(cx, event){
            // we got clicked!
        }
    } 

    fn draw(&mut self, cx:&mut Cx){
        self.view.begin(cx, &Layout{
            w:Percent(100.0),
            ..Layout::filled_padded(10.0)
        });

        //self.ok.draw_with_label(cx, "Live Rust");
        self.oks.reset();
        for i in 0..500{
            self.oks.add(&self.ok).draw_with_label(cx, &format!("OK{}",i));
        }

        self.view.end(cx);
    }
}

main_app!(App, "My Application");