use render::*;

use crate::scrollbar::*;

#[derive(Clone)]
pub enum FileNode{
    File{name:String, area:Area},
    Folder{name:String, area:Area, open:bool, folder:Vec<FileNode>}
}

#[derive(Clone, Element)]
pub struct FileTree{
    pub view:View<ScrollBar>,
    pub tree_bg:Quad,
    pub tree_line:Quad,
    pub text:Text,
    pub root_node:FileNode,
    pub animator:Animator
}

#[derive(Clone, PartialEq)]
pub enum FileTreeEvent{
    None,
    Select{id:usize}
}

struct StackEntry<'a>{
    counter:usize,
    node:&'a mut FileNode
}

pub struct FileWalker<'a>
{
    stack:Vec<StackEntry<'a>>,
}

// this flattens out recursion into an iterator. unfortunately needs unsafe. ugh.
impl<'a> FileWalker<'a>{
    pub fn walk(&mut self)->Option<(usize, &'a mut FileNode)>{
        // lets get the current item on the stack
        let stack_len = self.stack.len();
        let push_or_pop = if let Some(stack_top) = self.stack.last_mut(){
            // return item 'count'
            match stack_top.node{
                FileNode::File{..}=>{
                    stack_top.counter += 1;
                    if stack_top.counter == 1{
                        return Some((stack_len, unsafe{std::mem::transmute(stack_top.node)}));
                    }
                    None // pop stack
                },
                FileNode::Folder{folder, ..}=>{
                    stack_top.counter += 1;
                    if stack_top.counter == 1{ // return self
                        return Some((stack_len, unsafe{std::mem::transmute(stack_top.node)}));
                    }
                    else{
                        let child_index = stack_top.counter - 2;
                        if child_index < folder.len(){ // child on stack
                            Some(StackEntry{counter:0, node: unsafe{std::mem::transmute(&mut folder[child_index])}})
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
        Self{
            root_node:FileNode::Folder{name:"".to_string(), open:true, area:Area::Empty, folder:vec![
                FileNode::File{name:"helloworld.jpg".to_string(), area:Area::Empty},
                FileNode::Folder{name:"mydirectory".to_string(), open:true, area:Area::Empty, folder:vec![
                    FileNode::File{name:"helloworld.rs".to_string(), area:Area::Empty},
                    FileNode::File{name:"other.rs".to_string(), area:Area::Empty}
                ]}
            ]},
            tree_bg:Quad{
                color:cx.color("bg_normal"),
                ..Style::style(cx)
            },
            tree_line:Quad{
                color:cx.color("bg_normal"),
                ..Style::style(cx)
            },
            text:Text{
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

impl FileTree{
    pub fn handle_file_tree(&mut self, cx:&mut Cx, event:&mut Event)->FileTreeEvent{
        FileTreeEvent::None
        // check event against container,
        // lets iterate our filetree checking event rects

        // then we modify 'open' and call redraw. thats it.

    }

    pub fn draw_file_tree(&mut self, cx:&mut Cx){
        self.view.begin_view(cx, &Layout{..Default::default()});

        let mut file_walker = FileWalker{stack:vec![StackEntry{counter:1, node:&mut self.root_node}]};
        
        // lets draw the filetree
        while let Some((depth,node)) = file_walker.walk(){
            // lets store the bg area in the tree

        }

        self.view.end_view(cx);
    }
}

