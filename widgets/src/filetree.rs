use render::*;
use crate::scrollbar::*;
use serde_json::{Result, Value};
use serde::*;

#[derive(Clone, Element)]
pub struct FileTree{
    pub view:View<ScrollBar>,
    pub node_bg:Quad,
    pub filler:Quad,
    pub file_text:Text,
    pub folder_text:Text,
    pub root_node:FileNode,
    pub animator:Animator,
    pub row_height:f32,
    pub row_padding:Padding
}

#[derive(Clone, PartialEq)]
pub enum FileTreeEvent{
    None,
    Select{id:usize}
}

#[derive(Clone)]
pub enum NodeState{
    Open,
    Opening(f64),
    Closing(f64),
    Closed
}

#[derive(Clone)]
pub struct NodeDraw{
    hit_state:HitState,
    animator:Animator
}

#[derive(Clone)]
pub enum FileNode{
    File{name:String, draw:Option<NodeDraw>},
    Folder{name:String, draw:Option<NodeDraw>, state:NodeState, folder:Vec<FileNode>}
}

impl FileNode{
    fn get_draw<'a>(&'a mut self)->&'a mut Option<NodeDraw>{
        match self{
            FileNode::File{draw,..}=>draw,
            FileNode::Folder{draw,..}=>draw
        }
    }
}

struct StackEntry<'a>{
    counter:usize,
    index:usize,
    len:usize,
    node:&'a mut FileNode
}

pub struct FileWalker<'a>
{
    stack:Vec<StackEntry<'a>>,
}

// this flattens out recursion into an iterator. unfortunately needs unsafe. come on. thats not nice
impl<'a> FileWalker<'a>{
    pub fn new(root_node: &'a mut FileNode)->FileWalker<'a>{
        return FileWalker{stack:vec![StackEntry{counter:1, index:0, len:0, node:root_node}]};
    }

    pub fn walk(&mut self)->Option<(usize, usize, usize, &mut FileNode)>{
        // lets get the current item on the stack
        let stack_len = self.stack.len() - 1;
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut(){
            // return item 'count'
            match stack_top.node{
                FileNode::File{..}=>{
                    stack_top.counter += 1;
                    if stack_top.counter == 1{
                        return Some((stack_len, stack_top.index, stack_top.len, unsafe{std::mem::transmute(&mut *stack_top.node)}));
                    }
                    None // pop stack
                },
                FileNode::Folder{folder, state, ..}=>{
                    stack_top.counter += 1;
                    if stack_top.counter == 1{ // return self
                        return Some((stack_len, stack_top.index, stack_top.len, unsafe{std::mem::transmute(&mut *stack_top.node)}));
                    }
                    else{
                        let child_index = stack_top.counter - 2;
                        let closed = if let NodeState::Closed = state{true} else {false};
                        if !closed && child_index < folder.len(){ // child on stack
                            Some(StackEntry{counter:0, index:child_index, len:folder.len(), node: unsafe{std::mem::transmute(&mut folder[child_index])}})
                        }
                        else{
                            None // pop stack
                        }
                    }
                }
            }
        }
        else {
            None
        };
        if let Some(item) = push_or_pop{
            self.stack.push(item);
            return self.walk();
        }
        else if self.stack.len() > 0{
            self.stack.pop();
            return self.walk();
        }
        return None;
    }
}

impl Style for FileTree{
    fn style(cx:&mut Cx)->Self{
        let filler_sh = Self::def_filler_shader(cx);
        Self{
            row_height:20.,
            row_padding:Padding{l:5.,t:0.,r:0.,b:1.},
            root_node:FileNode::Folder{name:"".to_string(), state:NodeState::Open, draw:None, folder:vec![
                FileNode::File{name:"loading...".to_string(), draw:None},
            ]},
            node_bg:Quad{
                ..Style::style(cx)
            },
            filler:Quad{
                color:cx.color("icon_color"),
                shader_id:cx.add_shader(filler_sh, "FileTree.filler"),
                ..Style::style(cx)
            },
            file_text:Text{
                color:cx.color("text_deselected_focus"),
                font_size:11.0,
                ..Style::style(cx)
            },
            folder_text:Text{
                color:cx.color("text_selected_focus"),
                font_size:11.0,
                boldness:0.0,
                ..Style::style(cx)
            },
            view:View{
                scroll_h:Some(Element::new(ScrollBar{
                    ..Style::style(cx)
                })),
                scroll_v:Some(Element::new(ScrollBar{
                    ..Style::style(cx)
                })),
                ..Style::style(cx)
            },
            animator:Animator::new(Anim::empty()),
        }
    }
}

#[derive(Deserialize, Debug)]
struct JsonFolder{
    name:String,
    open:bool,
    files:Vec<JsonFile>,
    folders:Vec<JsonFolder>
}

#[derive(Deserialize, Debug)]
struct JsonFile{
    name:String
}

impl FileTree{

    fn json_to_file_node(node:JsonFolder)->FileNode{
        let mut out = Vec::new();
        for folder in node.folders{
            out.push(Self::json_to_file_node(folder));
        }
        for file in node.files{
            out.push(FileNode::File{
                name:file.name,
                draw:None
            })
        };
        FileNode::Folder{
            name:node.name,
            state:if node.open{NodeState::Open} else {NodeState::Closed},
            draw:None,
            folder:out
        }
    } 

    pub fn load_from_json(&mut self, cx:&mut Cx, json_data:&str){
        let value:Result<JsonFolder> = serde_json::from_str(json_data); 
        if let Ok(value) = value{
            self.root_node = Self::json_to_file_node(value);
        }
        self.view.redraw_view_area(cx);
    }

    pub fn def_filler_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            let line_vec:vec2<Instance>;
            let anim_pos:float<Instance>;

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                if anim_pos<-0.5{
                    df_move_to(0.5*w,line_vec.x*h);
                    df_line_to(0.5*w,line_vec.y*h);
                    return df_stroke(color, 1.);
                }
                else{ // its a folder
                    df_box(0.*w, 0.39*h, 0.87*w, 0.39*h, 0.75);
                    df_box(0.*w, 0.32*h, 0.5*w, 0.3*h, 1.);
                    df_union();
                    // ok so. 
                    return df_fill(color);
                }
            }
        }));
        sh
    }

    pub fn get_default_anim(cx:&Cx, counter:usize)->Anim{
        Anim::new(AnimMode::Cut{duration:0.05}, vec![
            AnimTrack::to_vec4("bg.color", 
                if counter&1==0{cx.color("bg_selected")}else{cx.color("bg_odd")}
            )
        ])
    }

    pub fn get_over_anim(cx:&Cx, counter:usize)->Anim{
        Anim::new(AnimMode::Cut{duration:0.05}, vec![
            AnimTrack::to_vec4("bg.color", 
                if counter&1==0{cx.color("bg_selected_over")}else{cx.color("bg_odd_over")}
            )
        ])
    }

    pub fn handle_file_tree(&mut self, cx:&mut Cx, event:&mut Event)->FileTreeEvent{
        // alright. someone clicking on the tree items.
        let mut file_walker = FileWalker::new(&mut self.root_node);
        let mut counter = 0;

        self.view.handle_scroll_bars(cx, event);
        // todo, optimize this so events are not passed through 'all' of our tree elements
        // but filtered out somewhat based on a bounding rect
        while let Some((_depth, _index, _len, node)) = file_walker.walk(){
            // alright we haz a node. so now what.
            let node_draw = if let Some(node_draw) = node.get_draw(){node_draw}else{continue};

            match event.hits(cx, node_draw.animator.area, &mut node_draw.hit_state){
                Event::Animate(ae)=>{
                    node_draw.animator.calc_area(cx, "bg.color", ae.time, node_draw.animator.area);
                },
                Event::FingerDown(_fe)=>{
                    if let FileNode::Folder{state,..} = node{
                        *state = match state{
                            NodeState::Opening(fac)=>{
                                NodeState::Closing(1.0 - *fac)
                            },
                            NodeState::Closing(fac)=>{
                                NodeState::Opening(1.0 - *fac)
                            },
                            NodeState::Open=>{
                                NodeState::Closing(1.0)
                            },
                            NodeState::Closed=>{
                                NodeState::Opening(1.0)
                            }
                        };
                        // start the redraw loop
                        self.view.redraw_view_area(cx);
                    }
                },
                Event::FingerHover(fe)=>{
                    cx.set_hover_mouse_cursor(MouseCursor::Hand);
                    match fe.hover_state{
                        HoverState::In=>{
                            node_draw.animator.play_anim(cx, Self::get_over_anim(cx, counter));
                        },
                        HoverState::Out=>{
                            node_draw.animator.play_anim(cx, Self::get_default_anim(cx, counter));
                        },
                        _=>()
                    }
                },
                _=>()
            }
            counter += 1;
        }   
        FileTreeEvent::None
    }

    pub fn draw_file_tree(&mut self, cx:&mut Cx){
        self.view.begin_view(cx, &Layout{..Default::default()});
        let mut file_walker = FileWalker::new(&mut self.root_node);
        
        // lets draw the filetree
        let mut counter = 0;
        
        let mut scale_stack = Vec::new();
        scale_stack.push(1.0f64);
        let mut last_stack = Vec::new();

        while let Some((depth, index, len, node)) = file_walker.walk(){

            let is_first = index == 0;
            let is_last = index == len - 1;

            while depth < scale_stack.len(){
                scale_stack.pop();
                last_stack.pop();
            }
            let scale = scale_stack[depth - 1];

            // lets store the bg area in the tree
            let node_draw = node.get_draw();
            if node_draw.is_none(){
                *node_draw = Some(NodeDraw{
                    hit_state:HitState{..Default::default()},
                    animator:Animator::new(Self::get_default_anim(cx, index))
                })
            }
            let node_draw = node_draw.as_mut().unwrap();
            self.node_bg.color = node_draw.animator.last_vec4("bg.color");

            let area = self.node_bg.begin_quad(cx, &Layout{
                width:Bounds::Fill,
                height:Bounds::Fix(self.row_height*scale as f32),
                align:Align::left_center(),
                padding:self.row_padding,
                ..Default::default()
            });

            node_draw.animator.set_area(cx, area); 

            for i in 0..(depth-1){
                let quad_margin = Margin{l:1.,t:0.,r:4.,b:0.};
                if i == depth - 2 { // our own thread. 
                    let area = self.filler.draw_quad_walk(cx, Bounds::Fix(10.), Bounds::Fill, quad_margin);
                    if is_last { 
                        if is_first{
                            area.push_vec2(cx, "line_vec", vec2(0.3,0.7))
                        }
                        else{
                            area.push_vec2(cx, "line_vec", vec2(-0.2,0.7))
                        }
                    }
                    else if is_first{
                        area.push_vec2(cx, "line_vec", vec2(-0.3,1.2))
                    }
                    else{
                        area.push_vec2(cx, "line_vec", vec2(-0.2,1.2));
                    }
                    area.push_float(cx, "anim_pos", -1.);
                }
                else{
                    let here_last = if last_stack.len()>1{ last_stack[i+1] } else {false};
                    if here_last{
                        cx.walk_turtle(Bounds::Fix(10.), Bounds::Fill, quad_margin, None);
                    }
                    else{
                        let area = self.filler.draw_quad_walk(cx, Bounds::Fix(10.), Bounds::Fill, quad_margin);
                        area.push_vec2(cx, "line_vec", vec2(-0.2,1.2));
                        area.push_float(cx, "anim_pos", -1.);
                    }
                }
            };

            match node{
                FileNode::Folder{name, state, ..}=>{
                    // draw the folder icon
                    let area = self.filler.draw_quad_walk(cx, Bounds::Fix(14.), Bounds::Fill, Margin{l:0.,t:0.,r:2.,b:0.});
                    area.push_vec2(cx, "line_vec", vec2(0.,0.));
                    area.push_float(cx, "anim_pos", 1.);
                    cx.realign_turtle(Align::left_center(), false);
                    self.folder_text.font_size = 11. * scale as f32;
                    self.folder_text.draw_text(cx, name);

                    let (new_scale, new_state) = match state{
                        NodeState::Opening(fac)=>{
                            self.view.redraw_view_area(cx);
                            if *fac < 0.001{
                                (1.0, NodeState::Open)
                            }
                            else{
                                (1.0 - *fac, NodeState::Opening(*fac * 0.8))
                            }
                        },
                        NodeState::Closing(fac)=>{
                            self.view.redraw_view_area(cx);
                            if *fac < 0.001{
                                (0.0, NodeState::Closed)
                            }
                            else{
                                (*fac, NodeState::Closing(*fac * 0.8))
                            }
                        },
                        NodeState::Open=>{
                            (1.0, NodeState::Open)
                        },
                        NodeState::Closed=>{
                            (1.0, NodeState::Closed)
                        }
                    };
                    *state = new_state;
                    last_stack.push(is_last);
                    scale_stack.push(scale * new_scale);
                },
                FileNode::File{name,..}=>{
                    cx.realign_turtle(Align::left_center(), false);
                    self.file_text.font_size = 11. * scale as f32;;
                    self.file_text.draw_text(cx, name);
                }
            }

            self.node_bg.end_quad(cx);
            cx.turtle_new_line();
            if scale >= 0.99{
                counter += 1;
            }
        }

        // draw filler nodes
        let view_total = cx.turtle_bounds();   
        let rect_now =  cx.turtle_rect();
        let bg_even = cx.color("bg_selected");
        let bg_odd = cx.color("bg_odd");
        let mut y = view_total.y;
        while y < rect_now.h{
            self.node_bg.color = if counter&1 == 0{bg_even}else{bg_odd};
            self.node_bg.draw_quad_walk(cx,
                Bounds::Fill,
                Bounds::Fix( (rect_now.h - y).min(self.row_height) ),
                Margin::zero()
            );
            cx.turtle_new_line();
            y += self.row_height;
            counter += 1;
        } 
        self.view.end_view(cx);
    }
}

