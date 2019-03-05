use shui::*;
mod button;
use crate::button::*;
mod scrollbar;
use crate::scrollbar::*;

struct App{
    view:View<ScrollBar>,
    ok:Elements<Button, usize>,
}
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View::new(cx, ScrollBarsActive::Both),
            ok:Elements::new(Button{
                ..Style::style(cx)  
            }),
        }
    }
}

impl App{
    fn handle(&mut self, cx:&mut Cx, event:&mut Event){

        self.view.handle_scroll(cx, event);

        for (id,ok) in self.ok.ids(){
            if let ButtonEvent::Clicked = ok.handle(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }
    }

    fn draw(&mut self, cx:&mut Cx){
        self.view.begin(cx, &Layout{
            nowrap:true,
            ..Layout::filled_padded(10.0)
        });

        //self.ok.mark();
        for i in 0..200{
            self.ok.get(cx, i).draw_with_label(cx, &format!("OK {}",(i as u64 )%5000));
            if i%6 == 5{
                cx.new_line();
            }
        }

        // draw scroll bars
        self.view.end(cx);
    }
}

main_app!(App, "My App");