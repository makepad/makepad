use widgets::*;

#[derive(Clone)]
enum Panel{
    Color(Vec4),
    FileTree,
    Editor(String)
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Element<Dock<Panel>>,
    ok:Elements<Button, u64>,
    file_tree:Element<FileTree>,
    editors:Elements<Editor,String>,
    tree_load_id:u64,
    quad:Quad
}

main_app!(App, "My App!");
 
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
            editors:Elements::new(Editor{
                ..Style::style(cx)
            }),
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
                                    title:"RedTab".to_string(),
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

fn path_file_name(path:&str)->String{
    if let Some(pos) =  path.rfind('/'){
        path[pos+1..path.len()].to_string()
    }
    else{
        path.to_string()
    }
}

impl App{
    fn handle_app(&mut self, cx:&mut Cx, event:&mut Event){
        match event{
            Event::Construct=>{
                self.tree_load_id = cx.read_file("./index.json");
            },
            Event::FileRead(fr)=>{
                if fr.id == self.tree_load_id{
                    if let Ok(str_data) = &fr.data{
                        if let Ok(json_data) = std::str::from_utf8(&str_data){
                            if let Some(file_tree) = &mut self.file_tree.element{
                                file_tree.load_from_json(cx, json_data);
                            }
                        }
                    }
                }   
            }
            _=>()
        }

        self.view.handle_scroll_bars(cx, event);
        
        if let Some(dock) = self.dock.get(){
            let mut dock_walker = dock.walker();
            let mut file_tree_event = FileTreeEvent::None;
            while let Some(item) = dock_walker.walk_handle_dock(cx, event){
                match item{
                    Panel::Color(_)=>{}
                    Panel::FileTree=>{
                        if let Some(file_tree) = &mut self.file_tree.element{
                            file_tree_event = file_tree.handle_file_tree(cx, event);
                        }
                    },
                    Panel::Editor(path)=>{
                        if let Some(editor) = &mut self.editors.get(path.clone()){
                            editor.handle_editor(cx, event);
                        }
                    }
                }
            }
            match file_tree_event{
                FileTreeEvent::DragMove{fe, ..}=>{
                    dock.dock_drag_move(cx, fe);
                },
                FileTreeEvent::DragOut=>{
                    dock.dock_drag_out(cx);
                },
                FileTreeEvent::DragEnd{fe, paths}=>{
                    let mut tabs = Vec::new();
                    for path in paths{
                        tabs.push(DockTab{
                            title:path_file_name(&path),
                            item:Panel::Editor(path)
                        })
                    }
                    dock.dock_drag_end(cx, fe, tabs);
                    // ok so.. what now. we have this set of paths
                    // that will end up being tabs.
                    // so how do we do those things
                },
                FileTreeEvent::SelectFile{path}=>{
                    // search for the tabcontrol with the maximum amount of editors
                    let mut max_editors = 0;
                    let mut max_ctrl_id = 0;
                    let mut dock_walker = dock.walker();
                    let mut ctrl_id = 1;
                    'outer: while let Some(dock_item) = dock_walker.walk_dock_item(){
                        match dock_item{
                            DockItem::TabControl{current, tabs}=>{
                                let mut editor_count = 0;
                                for (id,tab) in tabs.iter().enumerate(){
                                    match &tab.item{
                                        Panel::Editor(edit_path)=>{
                                            if *edit_path == path{ 
                                                *current = id; // focus this one
                                                max_ctrl_id = 0; // already open
                                                cx.redraw_area(Area::All);
                                                break 'outer;
                                            }
                                            else{
                                                editor_count += 1;
                                            }
                                        },
                                        _=>()
                                    }
                                }
                                if editor_count >= max_editors{
                                    max_editors = editor_count;
                                    max_ctrl_id = ctrl_id;
                                }
                            },
                            _=>()
                        }
                        ctrl_id += 1;
                    }
                   
                    if max_ctrl_id != 0{ // found a control to append to
                        let mut dock_walker = dock.walker();
                        let mut ctrl_id = 1;
                        while let Some(dock_item) = dock_walker.walk_dock_item(){
                            if ctrl_id == max_ctrl_id{
                                if let DockItem::TabControl{current, tabs} = dock_item{
                                    tabs.push(DockTab{
                                        title:path_file_name(&path),
                                        item:Panel::Editor(path.clone())
                                    });
                                    *current = tabs.len() - 1;
                                    cx.redraw_area(Area::All);
                                }
                            }
                            ctrl_id += 1;
                        }
                    }
                },
                _=>{}
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
                },
                Panel::Editor(path)=>{
                    self.editors.get_draw(cx, path.clone()).draw_editor(cx);
                }
            }
        }

        //self.ok.get(cx, 0).draw_button_with_label(cx, "HI");
        
        // draw scroll bars
        self.view.end_view(cx);
    }
}