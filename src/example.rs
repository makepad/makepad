use widgets::*;

struct App{
    view:View<ScrollBar>,
    buttons:Elements<u64, Button, Button>,
    text:Text,
    quad:Quad,
    clickety:u64
}

main_app!(App, "Example");
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        set_dark_style(cx);
        Self{
            view:View{
                scroll_h:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                scroll_v:Some(ScrollBar{
                    ..Style::style(cx)
                }),
                ..Style::style(cx)
            },
            quad:Quad{
                ..Style::style(cx)
            },
            text:Text{
                ..Style::style(cx)
            },
            buttons:Elements::new(Button{
                ..Style::style(cx)
            }),
            clickety:0
        }
    }
}

impl App{
    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        self.view.handle_scroll_bars(cx, event);

        for btn in self.buttons.iter(){
            match btn.handle_button(cx, event){
                ButtonEvent::Clicked=>{
                    // boop
                    self.clickety += 1;
                    cx.redraw_area(Area::All);
                },
                _=>()
            }
        }
    }

    fn draw_app(&mut self, cx:&mut Cx){
        
        self.view.begin_view(cx, &Layout{
            padding:Padding{l:10.,t:10.,r:0.,b:0.},
            ..Default::default()
        });


        for i in 0..100{
            self.buttons.get_draw(cx, i, |_cx, templ|{
                templ.clone()
            }).draw_button_with_label(cx, &format!("Btn {}", i));
            if i%10 == 9{
                cx.turtle_new_line()
            }
        }

        cx.turtle_new_line();

        self.text.draw_text(cx, &format!("Hello World {}",self.clickety));
        self.quad.draw_quad_walk(cx, Bounds::Fix(100.),Bounds::Fix(100.), Margin{l:15.,t:0.,r:0.,b:0.});

        self.view.end_view(cx);
    }
}