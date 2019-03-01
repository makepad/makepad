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

        for i in 0..2000{
            //let f = ((i+cx.frame_id)%250)as f32/250.0;
            // we add a button with a certain ID 
            //self.ok.get(cx, i).draw_with_label(cx, &format!("{}",(i+cx.frame_id)%5000));
            self.ok.get(cx, i).draw_with_label(cx, &format!("{}",(i as u64 + cx.frame_id)%5000));
            //self.oks.elements[i as usize].bg.color = vec4(f,1.0-f,f*0.5,1.0);
        }

        self.view.end(cx);
        //cx.redraw_all();
    }
}

main_app!(App, "My Application");