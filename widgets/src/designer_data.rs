use crate::{
    makepad_draw::*,
    makepad_live_compiler::LiveTokenId,
};
use std::collections::HashMap;

pub enum OutlineNode{
    Virtual{
        name: String,
        children: SmallVec<[LiveId;4]>
    },
    File{
        file_id: LiveFileId,
        name: String,
        children: SmallVec<[LiveId;4]>
    },
    Folder{
        name: String,        
        children: SmallVec<[LiveId;4]>
    },
    Component{
        name: LiveId,
        class: LiveId,
        prop_type: LivePropType,
        token_id: LiveTokenId,
        ptr: LivePtr,
        children: SmallVec<[LiveId;4]>
    }
}


#[derive(Default)]
pub struct DesignerData{
    pub root: LiveId,
    pub node_map: HashMap<LiveId, OutlineNode>,
    pub selected: Option<LiveId>
}

impl DesignerData{
    pub fn _remove_child(&mut self, find_node:LiveId){
        for node in &mut self.node_map.values_mut(){
            match node{
                OutlineNode::Component{children,..} | OutlineNode::Virtual{children,..} | OutlineNode::File{children,..} | OutlineNode::Folder{children, ..} =>{
                    if let Some(i) = children.iter().position(|v| *v == find_node){
                        children.remove(i);
                        break;
                    }
                }
            }
        }
    }
    
    pub fn _find_component_by_path(&self, path:&[LiveId])->Option<LiveId>{
        fn get_node(node:LiveId, path:&[LiveId],  map:&HashMap<LiveId, OutlineNode>)->Option<LiveId>{
            match map.get(&node).as_ref(){
                Some(OutlineNode::Virtual{children,..}) |
                Some(OutlineNode::Folder{children,..}) |
                Some(OutlineNode::File{children,..}) =>{
                    for child in children{
                        if let Some(v) = get_node(*child, path, map){
                            return Some(v)
                        }
                    }
                }
                Some(OutlineNode::Component{children, name, ..}) => {
                    if *name == path[0]{
                        if path.len()>1{
                            for child in children{
                                 if let Some(v) = get_node(*child, &path[1..], map){
                                     return Some(v)
                                 }
                            }
                        }
                        else{
                            return Some(node)
                        }
                    }
                    for child in children{
                        if let Some(v) = get_node(*child, path, map){
                            return Some(v)
                        }
                    }
                }
                _=>()
            }
            None
        }
        get_node(self.root, path, &self.node_map)
    }
}
