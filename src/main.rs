use widgets::*;

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
    quad:Quad,
    triangle:Triangle
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
            quad:Quad{
                ..Style::style(cx)
            },
            triangle:Triangle{
                ..Style::style(cx)
            },
            dock:Dock{
                splitters:Elements::new(Splitter{
                    ..Style::style(cx)
                }),
                dock_items:DockItem::Split{
                    axis:Axis::Vertical,
                    align:SplitterAlign::First,
                    pos:150.0,
                    first:Box::new(DockItem::Single(
                        MyItem::Color(color("red"))
                    )),
                    last:Box::new(DockItem::Split{
                        axis:Axis::Horizontal,
                        align:SplitterAlign::Last,
                        pos:150.0,
                        first:Box::new(DockItem::Single(
                            MyItem::Color(color("purple"))
                        )),
                        last:Box::new(DockItem::Single(
                            MyItem::Color(color("blue"))
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
        while let Some(item) = dock_walker.walk_handle_dock(cx, event){
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
        while let Some(item) = dock_walker.walk_draw_dock(cx){
            match item{
                MyItem::Color(color)=>{
                    self.quad.color = *color;
                    self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                }
            }
        }
        /*

        self.quad.color = color("pink");
        self.quad.draw_quad(cx, 250.,250.,100.,100.);

        self.triangle.color = color("orange");
        self.triangle.draw_triangle(cx, 70.,70.,70.,150.,150.,150.);
      */

        // draw scroll bars
        self.view.end_view(cx);
    }
}

main_app!(App, "My App");