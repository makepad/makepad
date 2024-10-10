use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    designer_outline_tree::*,
    designer_data::*,
    view::View,
    widget::*,
};
use std::collections::HashMap;


live_design!{
    DesignerOutlineBase = {{DesignerOutline}}{
    }
}

#[derive(Live, Widget, LiveHook)]
pub struct DesignerOutline {
    #[deref] view: View
} 

impl Widget for DesignerOutline {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk: Walk) -> DrawStep {
        let file_tree = self.view.designer_outline_tree(id!(outline_tree));
        let data = scope.data.get::<DesignerData>().unwrap();
        
        while let Some(next) = self.view.draw(cx, &mut Scope::empty()).step() {
            if let Some(mut file_tree) = file_tree.borrow_mut_if_eq(&next) {
                if let OutlineNode::Virtual{children,..} = &data.node_map.get(&data.root).as_ref().unwrap(){
                    recur_nodes(cx,  &mut *file_tree, &data.node_map, children);
                }
            }
        }
        
        fn recur_nodes(cx: &mut Cx2d, outline_tree: &mut DesignerOutlineTree,map:&HashMap<LiveId,OutlineNode>, children:&[LiveId]) {
            for child in children{
                match map.get(&child).unwrap(){
                    OutlineNode::Folder{name, children}=> {
                        if outline_tree.begin_node(cx, *child, &name, live_id!(Folder)).is_ok(){
                            recur_nodes(cx, outline_tree, map, children);
                            outline_tree.end_node();
                        }            
                    }
                    OutlineNode::File{name,  file_id:_, children}=>{
                        if children.len() > 0{
                            if outline_tree.begin_node(cx, *child, &name, live_id!(File)).is_ok(){
                                recur_nodes(cx, outline_tree, map, children);
                                outline_tree.end_node();
                            }
                        }
                    }
                    OutlineNode::Virtual{name, children}=>{
                        if outline_tree.begin_node(cx, *child, &name, live_id!(Folder)).is_ok(){
                            recur_nodes(cx, outline_tree, map, children);
                            outline_tree.end_node();
                        }            
                    }
                    OutlineNode::Component{children, name, ..}=>{
                        
                        if children.len() > 0{
                            if outline_tree.begin_node(cx, *child, &name, live_id!(Layout)).is_ok() {
                                recur_nodes(cx, outline_tree, map, children);
                                outline_tree.end_node();
                            }
                        }
                        else{
                            outline_tree.node(cx, *child, name, live_id!(Widget))
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }
}