use widgets::*;

#[derive(Clone)]
enum MyItem{
    Color(Vec4)
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Dock<MyItem, Splitter, TabControl>,
    ok:Elements<Button, usize>,
    quad:Quad
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
            dock:Dock{
                splitters:Elements::new(Splitter{
                    ..Style::style(cx)
                }),
                tab_controls:Elements::new(TabControl{
                    ..Style::style(cx)
                }),
                dock_items:DockItem::Splitter{
                    axis:Axis::Vertical,
                    align:SplitterAlign::First,
                    pos:150.0,
                    first:Box::new(DockItem::Single(
                        MyItem::Color(color("red"))
                    )),
                    last:Box::new(DockItem::Splitter{
                        axis:Axis::Horizontal,
                        align:SplitterAlign::Last,
                        pos:150.0,
                        first:Box::new(DockItem::TabControl{
                            current:0,
                            tabs:vec![
                                DockTab{
                                    title:"PinkTab".to_string(),
                                    item:MyItem::Color(color("pink"))
                                },
                                DockTab{
                                    title:"BlueTab".to_string(),
                                    item:MyItem::Color(color("blue"))
                                },
                                DockTab{
                                    title:"GreenTab".to_string(),
                                    item:MyItem::Color(color("green"))
                                }      
                            ],
                        }),
                        last:Box::new(DockItem::Single(
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
        
         // recursive item iteration        \
         
        let mut dock_walker = self.dock.walker();
        while let Some(item) = dock_walker.walk_handle_dock(cx, event){
            match item{
                MyItem::Color(_)=>{}
            }
        }
        // lets fetch the docks events
        //self.dock.handle_dock(cx, event)

        for (id,ok) in self.ok.ids(){
            if let ButtonEvent::Clicked = ok.handle_button(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }
        
    }

    fn draw_app(&mut self, cx:&mut Cx){
        self.view.begin_view(cx, &Layout{
            line_wrap:LineWrap::None,
            width:Bounds::Fill,
            height:Bounds::Fill,
            padding:Padding::all(0.0),
            ..Default::default()
        });

/*
        self.view1.begin_view(cx, &Layout{
            width:Bounds::Scale(0.5),
            height:Bounds::Scale(0.5),
            ..Default::default()
        });

            self.quad.color = color("orange");
            self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());

            self.view2.begin_view(cx, &Layout{
                width:Bounds::Scale(0.5),
                height:Bounds::Scale(0.5),
                ..Default::default()
            });
            self.quad.color = color("pink");
            self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());

            self.view2.end_view(cx);
                println!("END");

        self.view1.end_view(cx);
*/
        // recursive item iteration       
        
        let mut dock_walker = self.dock.walker();
        while let Some(item) = dock_walker.walk_draw_dock(cx){
            match item{
                MyItem::Color(color2)=>{
                    self.quad.color = *color2;
                    self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                    //self.triangle.color = color("pink");
                    //self.triangle.draw_triangle(cx, 70.,70.,70.,150.,150.,150.);
                }
            }
        }
        
        

       // self.quad.color = color("pink");
       // self.quad.draw_quad(cx, 250.,250.,100.,100.);

       
      

        // draw scroll bars
        self.view.end_view(cx);
       
    }
}

main_app!(App, "My App");