use widgets::*;

#[derive(Clone)]
enum Panel{
    Color(Vec4),
    FileTree,
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Element<Dock<Panel>>,
    ok:Elements<Button, u64>,
    file_tree:Element<FileTree>,
    tree_load_id:u64,
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
            file_tree:Element::new(FileTree{
                ..Style::style(cx)
            }),
            tree_load_id:0,
            dock:Element::new(Dock{
                dock_items:Some(DockItem::Splitter{
                    axis:Axis::Vertical,
                    align:SplitterAlign::First,
                    pos:150.0,
                    first:Box::new(DockItem::TabControl{
                        current:0,
                        tabs:vec![
                            DockTab{
                                title:"Files".to_string(),
                                item:Panel::FileTree
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
                                    item:Panel::Color(color("pink"))
                                },
                                DockTab{
                                    title:"BlueTab".to_string(),
                                    item:Panel::Color(color("blue"))
                                },
                                DockTab{
                                    title:"GreenTab".to_string(),
                                    item:Panel::Color(color("green"))
                                }      
                            ],
                        }),
                        last:Box::new(DockItem::TabControl{
                            current:0,
                            tabs:vec![
                                DockTab{
                                    title:"PurpleTab".to_string(),
                                    item:Panel::Color(color("purple"))
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
        match event{
            Event::Construct=>{
                self.tree_load_id = cx.read_file("files.json");
            },
            Event::FileRead(fr)=>{
                if fr.id == self.tree_load_id{
                    // do something with the result.
                }   
            }
            _=>()
        }

        self.view.handle_scroll_bars(cx, event);
        
        if let Some(dock) = self.dock.get(){
            let mut dock_walker = dock.walker();
            while let Some(item) = dock_walker.walk_handle_dock(cx, event){
                match item{
                    Panel::Color(_)=>{}
                    Panel::FileTree=>{
                        if let Some(file_tree) = &mut self.file_tree.element{
                            file_tree.handle_file_tree(cx, event);
                        }
                    }
                }
            }

            // handle the dock events        
            match dock.handle_dock(cx, event){
                DockEvent::DockChanged=>{
                },         
                _=>()
            }
        }

        for (id,ok) in self.ok.enumerate(){
            if let ButtonEvent::Clicked = ok.handle_button(cx, event){
                // we got clicked!
                log!(cx, "GOT CLICKED BY {}", id);
            }
        }
        
    }

    fn draw_app(&mut self, cx:&mut Cx){
        
        use syn::{Expr, Result};
        let code = "assert_eq!(u8::max_value(), 255)";
        let expr = syn::parse_str::<Expr>(code);

        //cx.debug_area = Area::Instance(InstanceArea{draw_list_id:0,draw_call_id:0,instance_offset:0,instance_count:0,redraw_id:0});

        self.view.begin_view(cx, &Layout{..Default::default()});
        // recursive item iteration       
        let dock = self.dock.get_draw(cx);

        dock.draw_dock(cx);

        let mut dock_walker = dock.walker();
        while let Some(item) = dock_walker.walk_draw_dock(cx){
            match item{
                Panel::Color(color2)=>{
                    self.quad.color = *color2;
                    self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                },
                Panel::FileTree=>{
                    self.file_tree.get_draw(cx).draw_file_tree(cx);
                }
            }
        }

        //self.ok.get(cx, 0).draw_button_with_label(cx, "HI");
        
        // draw scroll bars
        self.view.end_view(cx);
    }
}

main_app!(App, "My App!");