use widgets::*;
use std::collections::HashMap;

#[derive(Clone)]
enum Panel{
    Color(Vec4),
    FileTree,
    EditorTarget,
    Editor{path:String, editor_id:u64}
    //Editor(String),
    //LogView(String),
}

struct App{
    view:View<ScrollBar>,
    dock:Dock<Panel>,
    file_tree:FileTree,
    editors:Elements<u64, Editor>,
    editor_id_alloc:u64,
    text_buffers:HashMap<String, TextBuffer>,
    tree_load_id:u64,
    quad:Quad
}

main_app!(App, "My App!");
 
impl Style for App{
    fn style(cx:&mut Cx)->Self{
        set_dark_style(cx);
        Self{
            text_buffers:HashMap::new(),
            editor_id_alloc:10,
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
            file_tree:FileTree{
                ..Style::style(cx)
            },
            tree_load_id:0,
            editors:Elements::new(Editor{
                ..Style::style(cx)
            }),
            dock:Dock{
                dock_items:Some(DockItem::Splitter{
                    axis:Axis::Vertical,
                    align:SplitterAlign::First,
                    pos:150.0,
                    first:Box::new(DockItem::TabControl{
                        current:0,
                        tabs:vec![
                            DockTab{
                                closeable:false,
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
                            current:1,
                            tabs:vec![
                                DockTab{
                                    closeable:false,
                                    title:"Edit".to_string(),
                                    item:Panel::EditorTarget
                                },
                                DockTab{
                                    closeable:true,
                                    title:"button.rs".to_string(),
                                    item:Panel::Editor{path:"/widgets/src/button.rs".to_string(), editor_id:1}
                                }
                            ],
                        }),
                        last:Box::new(DockItem::TabControl{
                            current:0,
                            tabs:vec![
                                DockTab{
                                    closeable:true,
                                    title:"Log".to_string(),
                                    item:Panel::Color(color256(30,30,30))
                                }
                            ]
                        })
                    })
                }),
                ..Style::style(cx)
            }
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
                // lets see which file we loaded
                if fr.id == self.tree_load_id{
                    if let Ok(str_data) = &fr.data{
                        if let Ok(utf8_data) = std::str::from_utf8(&str_data){
                            self.file_tree.load_from_json(cx, utf8_data);
                        }
                    }
                }
                for (_path, text_buffer) in &mut self.text_buffers{
                    if text_buffer.load_id == fr.id{
                        text_buffer.load_id = 0;
                        if let Ok(str_data) = &fr.data{
                            if let Ok(utf8_data) = std::str::from_utf8(&str_data){
                                text_buffer.text = utf8_data.to_string();
                                // lets be lazy and redraw all
                                cx.redraw_area(Area::All)
                            }
                        }
                    }
                }
            }
            _=>()
        }

        self.view.handle_scroll_bars(cx, event);
        
        let mut dock_walker =  self.dock.walker();
        let mut file_tree_event = FileTreeEvent::None;
        while let Some(item) = dock_walker.walk_handle_dock(cx, event){
            match item{
                Panel::Color(_)=>{}
                Panel::EditorTarget=>{},
                Panel::FileTree=>{
                    file_tree_event = self.file_tree.handle_file_tree(cx, event);
                },
                Panel::Editor{path, editor_id}=>{
                    if let Some(editor) = &mut self.editors.get(*editor_id){
                        let text_buffer = self.text_buffers.get_mut(path);
                        if let Some(text_buffer) = text_buffer{
                            editor.handle_editor(cx, event, text_buffer);
                        }
                    }
                }
            }
        }
        match file_tree_event{
            FileTreeEvent::DragMove{fe, ..}=>{
                self.dock.dock_drag_move(cx, fe);
            },
            FileTreeEvent::DragOut=>{
                self.dock.dock_drag_out(cx);
            },
            FileTreeEvent::DragEnd{fe, paths}=>{
                let mut tabs = Vec::new();
                for path in paths{
                    // find a free editor id
                    tabs.push(self.new_editor_tab(cx, &path));
                }
                self.dock.dock_drag_end(cx, fe, tabs);
            },
            FileTreeEvent::SelectFile{path}=>{
                // search for the tabcontrol with the maximum amount of editors
                if let Some(target_ctrl_id) = self.focus_editor_or_find_editor_target(cx, &path){ // found a control to append to
                    self.open_new_editor_in_target_ctrl(cx, target_ctrl_id, &path);
                }
            },
            _=>{}
        }

        // handle the dock events        
        match self.dock.handle_dock(cx, event){
            DockEvent::DockChanged=>{ // thats a bit bland event. lets let the thing know which file closed
            },
            _=>()
        }
      
    }

    fn draw_app(&mut self, cx:&mut Cx){
        
       // use syn::{Expr, Result};

        //cx.debug_area = Area::Instance(InstanceArea{draw_list_id:0,draw_call_id:0,instance_offset:0,instance_count:0,redraw_id:0});

        self.view.begin_view(cx, &Layout{..Default::default()});

        self.dock.draw_dock(cx);

        let mut dock_walker = self.dock.walker();
        while let Some(item) = dock_walker.walk_draw_dock(cx){
            match item{
                Panel::Color(color2)=>{
                    self.quad.color = *color2;
                    self.quad.draw_quad_walk(cx, Bounds::Fill, Bounds::Fill, Margin::zero());
                },
                Panel::EditorTarget=>{
                },
                Panel::FileTree=>{
                    self.file_tree.draw_file_tree(cx);
                },
                Panel::Editor{path, editor_id}=>{
                    //let text_buffer = self.text_buffers.get_mut(path).unwrap();
                    let text_buffer = self.text_buffers.entry(path.to_string()).or_insert_with(||{
                        TextBuffer{
                            load_id:cx.read_file(&format!(".{}",path)),
                            ..Default::default()
                        }
                    });
                    self.editors.get_draw_or_insert_with(cx, *editor_id, |cx, template|{
                        Editor{
                            path:path.clone(),
                            ..template.clone()
                        }
                    }).draw_editor(cx, text_buffer);
                }
            }
        }
        self.view.end_view(cx);
    }

    fn new_editor_tab(&mut self, cx:&mut Cx, path:&str)->DockTab<Panel>{
        let editor_id = self.editor_id_alloc;
        self.editor_id_alloc += 1;
        DockTab{
            closeable:true,
            title:path_file_name(&path),
            item:Panel::Editor{path:path.to_string(), editor_id:editor_id}
        }
    }

    fn open_new_editor_in_target_ctrl(&mut self, cx:&mut Cx, target_ctrl_id:usize, path:&str){
        let new_tab = self.new_editor_tab(cx, path);
        let mut dock_walker = self.dock.walker();
        let mut ctrl_id = 1;
        while let Some(dock_item) = dock_walker.walk_dock_item(){
            if ctrl_id == target_ctrl_id{
                if let DockItem::TabControl{current, tabs} = dock_item{
                    tabs.insert(*current+1, new_tab);
                    *current = *current + 1;//tabs.len() - 1;
                    cx.redraw_area(Area::All);
                    break
                }
            }
            ctrl_id += 1;
        }
   }

    fn focus_editor_or_find_editor_target(&mut self, cx:&mut Cx, file_path:&str)->Option<usize>{
        let mut target_ctrl_id = 0;
        let mut only_focus_editor = false;
        let mut dock_walker = self.dock.walker();
        let mut ctrl_id = 1;
        'outer: while let Some(dock_item) = dock_walker.walk_dock_item(){
            match dock_item{
                DockItem::TabControl{current, tabs}=>{
                    for (id,tab) in tabs.iter().enumerate(){
                        match &tab.item{
                            Panel::Editor{path, ..}=>{
                                if *path == file_path{
                                    *current = id; // focus this one
                                    only_focus_editor = true;
                                    cx.redraw_area(Area::All);
                                    //break 'outer;
                                }
                            },
                            Panel::EditorTarget=>{
                                target_ctrl_id = ctrl_id;
                            },
                            _=>()
                        }
                    }
                },
                _=>()
            }
            ctrl_id += 1;
        }
        if target_ctrl_id == 0 || only_focus_editor{
            None
        }
        else{
            Some(target_ctrl_id)
        }
    }
}