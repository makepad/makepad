use shui::*;
mod button;
use crate::button::*;

struct App{
    view:View,
    ok:Elements<Button, usize>,
}
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(),
            ok:Elements::new(Button{
                ..Style::style(cx)  
            }),
        }
    }
}

impl App{
    fn handle(&mut self, cx:&mut Cx, event:&Event){
        for ok in self.ok.all(){
            if let ButtonEvent::Clicked = ok.handle(cx, event){
                // we got clicked!
            }
        }
    }

    fn draw(&mut self, cx:&mut Cx){
        self.view.begin(cx, &Layout{
            ..Layout::filled_padded(10.0)
        });

        //self.ok.mark();
        for i in 0..2000{
            self.ok.get(cx, i).draw_with_label(cx, &format!("{}",(i as u64 + cx.frame_id)%5000));
        }
        //self.ok.sweep(cx);

        self.view.end(cx);
    }
}

main_app!(App, "My App");