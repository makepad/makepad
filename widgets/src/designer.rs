use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    multi_window::*,
    widget_match_event::*,
    
    designer_data::*,
    designer_outline_tree::*,
    
    widget::*,
};
use std::collections::HashMap;

live_design!{
    DesignerBase = {{Designer}} {
    }
}

#[derive(Live, Widget)]
pub struct Designer {
    #[deref] ui: MultiWindow,
    #[rust] data: DesignerData,
}

impl LiveHook for Designer {
    
    fn before_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){
        
        self.data.node_map.clear();
        
        // insert the root
        let root_uid = live_id!(designer_root).into();
        self.data.root = root_uid;
        self.data.node_map.insert(root_uid, OutlineNode::Virtual{
            name: "root".into(),
            children: SmallVec::new()
        });
        
        // lets take the doc we need (app_mobile for instance)
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = &*live_registry_rc.borrow();
        let main_module_lti = live_registry.main_module.as_ref().unwrap().clone();
        let mut path_hash = String::new();
        for (file_path, file_id) in live_registry.file_ids(){
            // now we fetch the unexpanded nodes
            // and build a list
            let file = live_registry.file_id_to_file(*file_id);
            let nodes = &file.expanded.nodes;
            // lets run over the file
            
            fn recur_walk_components(
                main_module_lti:&LiveTypeInfo, 
                live_registry: &LiveRegistry, 
                hash_id:LiveId, 
                base_ptr: LivePtr, 
                mut index: usize, 
                nodes: &[LiveNode],
                map: &mut HashMap<LiveId, OutlineNode>,
                parent_children: &mut SmallVec<[LiveId;4]>) -> usize {
                    
                while index < nodes.len() - 1 { 
                    if let LiveValue::Class {live_type, class_parent, ..} = &nodes[index].value {
                        // lets check if its a widget
                        let wr = live_registry.components.get::<WidgetRegistry>();
                        if main_module_lti.live_type == *live_type || wr.map.get(live_type).is_some(){
                            
                            // lets emit a class at our level
                            let name = nodes[index].id;
                            let class = live_registry.ptr_to_node(class_parent.unwrap()).id;
                            let ptr = base_ptr.with_index(index);
                            let prop_type =  nodes[index].origin.prop_type();
                            let token_id = nodes[index].origin.token_id();
                            let uid = hash_id.bytes_append(&name.0.to_be_bytes());
                            let mut children = SmallVec::new();
                            
                            index = recur_walk_components(
                                main_module_lti, 
                                live_registry, 
                                uid, 
                                base_ptr, 
                                index + 1, 
                                nodes, 
                                map, 
                                &mut children
                            );
                            
                            let uid = uid.into();
                            parent_children.push(uid);
                            map.insert(uid, OutlineNode::Component {
                                name,
                                token_id: token_id.unwrap(), 
                                prop_type,
                                class,
                                ptr,
                                children
                            });
                        }
                        else{
                         //   log!("NOT A WIDGET {}", nodes[index].id);
                           index = nodes.skip_node(index);
                        }
                    }
                    else if nodes[index].value.is_close() {
                        return index + 1;
                    }
                    else {
                        index = nodes.skip_node(index);
                    }
                }
                index
            }
            // alright lets iterate over the files
            
            let base_ptr = live_registry.file_id_index_to_live_ptr(*file_id, 0);
            
            path_hash.clear();
            
            let parent_id = path_split(self.data.root, &mut path_hash, file_path, &mut self.data.node_map, *file_id);
            let base_id = LiveId(0).bytes_append(&file_id.0.to_be_bytes());
            
            let mut children_out = SmallVec::new();
            recur_walk_components(
                &main_module_lti, 
                live_registry, 
                base_id, 
                base_ptr, 
                1, 
                nodes,
                &mut self.data.node_map, 
                &mut children_out
            );
            if let Some(OutlineNode::File{children,..}) = self.data.node_map.get_mut(&parent_id){
                *children = children_out;
            }
            
            fn path_split<'a>(parent_id: LiveId, path_hash:&mut String, name:&str, map:&mut HashMap<LiveId, OutlineNode>, file_id:LiveFileId)->LiveId{
                if let Some((folder,rest)) = name.split_once("/"){
                    path_hash.push_str(folder);
                    path_hash.push_str("/");
                    
                    if folder == "src"{ // flatten this
                        return path_split(parent_id, path_hash, rest, map, file_id)
                    }
                                        
                    let what_uid =  LiveId::from_str(&path_hash).into();
                    
                    // add node to pareht
                    if let Some(OutlineNode::Folder{children, ..}) | Some(OutlineNode::Virtual{children, ..})= map.get_mut(&parent_id){
                        if !children.contains(&what_uid){
                            children.push(what_uid);
                        }
                    }
                    
                    if map.get_mut(&what_uid).is_some(){
                        return path_split(what_uid, path_hash, rest, map, file_id)
                    }
                    
                    map.insert(what_uid, OutlineNode::Folder{
                        name: folder.to_string(),
                        children: SmallVec::new()
                    });
                    
                    return path_split(what_uid, path_hash, rest, map, file_id)
                }
                else{ // we're a file
                    path_hash.push_str(name);
                    path_hash.push_str("/");
                    let what_uid =  LiveId::from_str(&path_hash).into();
                    if let Some(OutlineNode::Folder{children, ..}) | Some(OutlineNode::Virtual{children, ..})= map.get_mut(&parent_id){
                        if !children.contains(&what_uid){
                            children.push(what_uid);
                        }
                    }
                    map.insert(what_uid, OutlineNode::File{
                        file_id,
                        name: name.to_string(),
                        children: SmallVec::new()
                    });
                    
                    return what_uid
                }
            }
          
        }
        /*
        // alright lets move some nodes
        // we should move theme_desktop_dark to the root
        // and we should move main_window / body to the root
        if let Some(node_id) = self.data.find_component_by_path(id!(main_window.body)){
            self.data.remove_child(node_id);
            let mut children = SmallVec::new();
            children.push(node_id);
            let app_uid = LiveId::from_str("app").into();
            self.data.node_map.insert(app_uid, OutlineNode::Virtual{
                name: "app".into(),
                children
            });
            if let Some(OutlineNode::Virtual{children, ..})= self.data.node_map.get_mut(&self.data.root){
                children.insert(0, app_uid)
            }
        }*/
    }
    
    // ok now we can iterate our top level components
    // and instance them
}

impl Designer {
    /*
    fn draw_design(&mut self, cx: &mut Cx2d) {
        // alrigh so. lets draw the designs
        let mut count = 0;
        for node in &self.outline_nodes {
            if let OutlineNode::Component {ptr, name, class, ..} = node {
                count += 1;
                if count > 5{
                    break;
                }
                let container_ptr = self.container;
                let (widget, container) = self.components.get_or_insert(cx, *ptr, | cx | {
                    (
                        WidgetRef::new_from_ptr(cx, Some(*ptr)),
                        WidgetRef::new_from_ptr(cx, container_ptr),
                    )
                });
                container.widget(id!(label)).set_text(&format!("{}=<{}>", name, class));
                // lets draw this thing in a neat little container box with a title bar
                while let Some(_) = container.draw(cx, &mut Scope::empty()).step() {
                    widget.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        
    }
    */
    /*
    fn draw_outline(&mut self, cx: &mut Cx2d, outline: &mut FileTree) {
        fn recur_walk(cx: &mut Cx2d, outline: &mut FileTree, children: &[OutlineNode]) {
            for child in children {
                match child {
                    OutlineNode::Global {..} => {}
                    OutlineNode::Component {name, children, uid, class, prop_type, ..} => {
                        if outline.begin_folder(cx, *uid, &if !name.is_unique(){
                            if let LivePropType::Field = prop_type {
                                format!("{}: <{}>", name, class)
                            }
                            else {
                                format!("{}=<{}>", name, class)
                            }
                        }else {
                            format!("<{}>", class)
                        }).is_ok() {
                            recur_walk(cx, outline, children);
                            outline.end_folder();
                        }
                    }
                }
            }
        }
        recur_walk(cx, outline, &self.outline_nodes);
    }
    */
}

impl WidgetMatchEvent for Designer{
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope){
        let outline_tree = self.ui.designer_outline_tree(id!(outline_tree));
        if let Some(file_id) = outline_tree.folder_clicked(&actions) {
            // alright we have a folder clicked
            // lets get a file/line number out of it so we can open it in the code editor.
            if let Some(node) = self.data.node_map.get(&file_id){
                match node{
                    OutlineNode::File{file_id:_,..}=>{
                        //let live_registry = cx.live_registry.borrow();
                        //let file_name = live_registry.file_id_to_file(file_id).file_name.clone();
                    }
                    OutlineNode::Component{token_id:_,..}=>{
                        /*
                        let file_id = token_id.file_id().unwrap();
                        let live_registry = cx.live_registry.borrow();
                        let tid = live_registry.token_id_to_token(*token_id).clone();
                        let span = tid.span.start;
                        let file_name = live_registry.file_id_to_file(file_id).file_name.clone();
                            
                        Cx::send_studio_message(AppToStudio::JumpToFile(JumpToFile{
                            file_name,
                            line: span.line,
                            column: span.column
                        }));*/
                    }
                    _=>()
                }
            }
            self.data.selected = Some(file_id);
            self.ui.widget(id!(designer_view)).redraw(cx);
        }
    }
}

impl Widget for Designer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.widget_match_event(cx, event, scope);
        let mut scope = Scope::with_props(&self.data);
        self.ui.handle_event(cx, event, &mut scope);
        /*
        for (component, container) in self.components.values_mut() {
            component.handle_event(cx, event, &mut scope);
            container.handle_event(cx, event, &mut scope);
        }*/
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, _walk: Walk) -> DrawStep {
        let mut scope = Scope::with_props(&self.data);
        self.ui.draw(cx, &mut scope)
    }
}
