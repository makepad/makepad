use shui::*;
mod button;
use crate::button::*;
mod scrollbar;
use crate::scrollbar::*;
mod splitter;
use crate::splitter::*;

struct App{
    view:View<ScrollBar>,
    splitter:Element<Splitter>,
    ok:Elements<Button, usize>,
    fill:Quad
}
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        Self{
            view:View{
                scroll_h:Some(Element::new(ScrollBar{
                    ..Style::style(cx)
                })),
                scroll_v:Some(Element::new(ScrollBar{
                    ..Style::style(cx)
                })),
                ..Style::style(cx)
            },
            splitter:Element::new(Splitter{
                ..Style::style(cx)
            }),
            ok:Elements::new(Button{
                ..Style::style(cx)  
            }),
            fill:Quad{
                ..Style::style(cx)
            }
        }
    }
}

impl App{
    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        
        // what do we do here? we could theoretically remap an event here.
        self.view.handle_scroll_bars(cx, event);

       for (id,ok) in self.ok.ids(){
            if let ButtonEvent::Clicked = ok.handle_button(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }

    }

    fn draw_app(&mut self, cx:&mut Cx){
        self.view.begin_view(cx, &Layout{
            nowrap:true,
            w:Bounds::Fill,
            h:Bounds::Fill,
            padding:Padding::all(10.0),
            ..Layout::new()
        });
        /*
        // grab a splitter
        let split = self.splitter.get(cx);
        split.begin_split(SplitterOrientation::Vertical);
        self.fill.color = color("orange");
        self.fill.draw_sized(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
        // do left.
        split.mid_split();
        self.fill.color = color("purple");
        self.fill.draw_sized(cx, Bounds::Fill, Bounds::Fill ,Margin::zero());
        // do right
        split.end_split();
        */
        for i in 0..200{
            self.ok.get(cx, i).draw_button_with_label(cx, &format!("OK {}",(i as u64 )%5000));
            if i%7 == 6{
                cx.new_line();
            }
        }

        // draw scroll bars
        self.view.end_view(cx);
    }
}

main_app!(App, "My App");