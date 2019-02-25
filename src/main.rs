use shui::*;
mod button;
use crate::button::*;

struct App{
    view:View,
    ok:Button,
    //oks:Elements<Button>,
    //rc:Quad,
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
            //oks:Elements::new(),
            //rc:Quad{..Style::style(cx)},
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

        self.ok.draw_with_label(cx, "OK");
        /*self.oks.reset();
        for i in 0..500{
            //self.rc.draw_sized(cx,Fixed(5.0),Fixed(5.0),Margin::zero());
            self.oks.add(&self.ok).draw_with_label(cx, &format!("OK{}",rand::random::<f32>()));
        }*/

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

//TODO do this with a macro to generate both entrypoints for App and Cx
pub fn main() {
    let mut cx = Cx{
        title:"Hi World".to_string(),
        ..Default::default()
    };

    let mut app = App{
        ..Style::style(&mut cx)
    };

    cx.event_loop(|cx, ev|{
        app.handle(cx, &ev);
    });
}

#[export_name = "init_wasm"]
pub extern "C" fn init_wasm()->u32{
    let mut cx = Box::new(
         Cx{
            title:"Hi JS!".to_string(),
            ..Default::default()
        }
    );
    let app = Box::new(
        App{
            ..Style::style(&mut cx)
        }
    );
    Box::into_raw(Box::new((Box::into_raw(app),Box::into_raw(cx)))) as u32
}

#[export_name = "to_wasm"]
pub unsafe extern "C" fn to_wasm(appcx:u32, msg:u32)->u32{
    let appcx = &*(appcx as *mut (*mut App,*mut Cx));
    (*appcx.1).to_wasm(msg,|cx, ev|{
        (*appcx.0).handle(cx, &ev);
    })
}