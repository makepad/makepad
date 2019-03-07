use shui::*;
mod button;
use crate::button::*;
mod scrollbar;
use crate::scrollbar::*;
mod splitter;
use crate::splitter::*;
mod dock;
use crate::dock::*;

#[derive(Clone)]
enum MyItem{
    Color(Vec4)
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Dock<MyItem, Splitter>,
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
            ok:Elements::new(Button{
                ..Style::style(cx)  
            }),
            fill:Quad{
                ..Style::style(cx)
            },
            dock:Dock{
                splitters:Elements::new(Splitter{
                    ..Style::style(cx)
                }),
                item_root:DockItem::Split{
                    axis:Axis::Vertical,
                    split_mode:SplitterMode::AlignBegin,
                    split_pos:50.0,
                    left:Box::new(DockItem::Single(
                        MyItem::Color(color("red"))
                    )),
                    right:Box::new(DockItem::Split{
                        axis:Axis::Horizontal,
                        split_mode:SplitterMode::AlignBegin,
                        split_pos:150.0,
                        left:Box::new(DockItem::Single(
                            MyItem::Color(color("purple"))
                        )),
                        right:Box::new(DockItem::Single(
                            MyItem::Color(color("orange"))
                        ))
                    })
                }
            }
        }
    }
}

impl App{
    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        
        self.view.handle_scroll_bars(cx, event);

         // recursive item iteration        
        let mut dock_walker = self.dock.walker();
        while let Some(item) = dock_walker.handle_walk(cx, event){
            match item{
                MyItem::Color(_)=>{}
            }
        }

        for (id,ok) in self.ok.ids(){
            if let ButtonEvent::Clicked = ok.handle_button(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }

    }

    fn draw_app(&mut self, cx:&mut Cx){
        self.view.begin_view(cx, &Layout{
            no_wrap:false,
            width:Bounds::Fill,
            height:Bounds::Fill,
            padding:Padding::all(0.0),
            ..Default::default()
        });

        // recursive item iteration        
        let mut dock_walker = self.dock.walker();
        while let Some(item) = dock_walker.draw_walk(cx){
            match item{
                MyItem::Color(color)=>{
                    self.fill.color = *color;
                    self.fill.draw_quad(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                }
            }
        }
/*
        // grab a splitter
        let split = self.splitter.get(cx);
        split.begin_splitter(cx, Axis::Vertical);
        self.fill.color = color("orange");
        self.fill.draw_quad(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
        // do left.
        split.mid_splitter(cx);
        self.fill.color = color("purple");
        self.fill.draw_quad(cx, Bounds::Fill, Bounds::Fill ,Margin::zero());
        // do right
        split.end_splitter(cx);
*/
        // ok so, a 'dock' has a memory state. 


        /*
        for i in 0..200{
            self.ok.get(cx, i).draw_button_with_label(cx, &format!("OK {}",(i as u64 )%5000));
            if i%7 == 6{
                cx.turtle_newline();
            }
        }*/

        // draw scroll bars
        self.view.end_view(cx);
    }
}

main_app!(App, "My App");