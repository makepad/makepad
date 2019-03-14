use widgets::*;

#[derive(Clone)]
enum MyItem{
    Color(Vec4)
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Element<Dock<MyItem>>,
    ok:Elements<Button, usize>,
    quad:Quad
}
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        set_dark_style(cx);
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
            dock:Element::new(Dock{
                dock_items:Some(DockItem::Splitter{
                    axis:Axis::Vertical,
                    align:SplitterAlign::First,
                    pos:150.0,
                    first:Box::new(DockItem::TabControl{
                        current:0,
                        tabs:vec![
                            DockTab{
                                title:"OrangeTab".to_string(),
                                item:MyItem::Color(color("orange"))
                            }
                        ]
                    }),
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
                        last:Box::new(DockItem::TabControl{
                            current:0,
                            tabs:vec![
                                DockTab{
                                    title:"PurpleTab".to_string(),
                                    item:MyItem::Color(color("purple"))
                                }
                            ]
                        })
                    })
                }),
                ..Style::style(cx)
            })
        }
    }
}

impl App{
    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        self.view.handle_scroll_bars(cx, event);
        
        for dock in self.dock.handle(){
            let mut dock_walker = dock.walker();
            while let Some(item) = dock_walker.walk_handle_dock(cx, event){
                match item{
                    MyItem::Color(_)=>{}
                }
            }

            // handle the dock events        
            match dock.handle_dock(cx, event){
                DockEvent::DockChanged=>{
                },         
                _=>()
            }
        }

        for (id,ok) in self.ok.handle_ids(){
            if let ButtonEvent::Clicked = ok.handle_button(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }
        
    }

    fn draw_app(&mut self, cx:&mut Cx){
        
        //cx.debug_area = Area::Instance(InstanceArea{draw_list_id:0,draw_call_id:0,instance_offset:0,instance_count:0});

        self.view.begin_view(cx, &Layout{..Default::default()});
        // recursive item iteration       
        let dock = self.dock.draw(cx);

        dock.draw_dock(cx);

        let mut dock_walker = dock.walker();
        while let Some(item) = dock_walker.walk_draw_dock(cx){
            match item{
                MyItem::Color(color2)=>{
                    self.quad.color = *color2;
                    self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                }
            }
        }

        //self.ok.get(cx, 0).draw_button_with_label(cx, "HI");
        
        // draw scroll bars
        self.view.end_view(cx);
    }
}

main_app!(App, "My App");