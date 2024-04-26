use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget_match_event::*,
    outline_tree::*,
    turtle_step::*,
    view::View,
    widget::*,
};
use std::collections::HashMap;
use std::fmt::Write;

live_design!{
    DesignerBase = {{Designer}} {
    }
    
    DesignerOutlineBase = {{DesignerOutline}}{
    }
    
    DesignerViewBase = {{DesignerView}}{
    }
    
    DesignerContainerBase = {{DesignerContainer}}{
    }    
}

#[allow(dead_code)]

enum OutlineNode{
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
        ptr: LivePtr,
        children: SmallVec<[LiveId;4]>
    }
}


#[derive(Live, Widget, LiveHook)]
pub struct DesignerContainer {
    #[deref] view: View
}

impl Widget for DesignerContainer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
    }
                
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk: Walk) -> DrawStep {
        let data = scope.props.get::<ContainerData>().unwrap();
        // alright lets draw the container, then the child
        let _turtle_step = self.view.turtle_step(id!(inner));
        self.walk = Walk{
            abs_pos: Some(data.rect.pos),
            width: Size::Fixed(data.rect.size.x),
            height: Size::Fixed(data.rect.size.y),
            margin: Default::default()
        };
        while let Some(_next) = self.view.draw(cx, &mut Scope::empty()).step() {
            data.component.draw_all(cx, &mut Scope::empty());
        }
        
       DrawStep::done()
    }
}

struct FingerMove{
    start_pan: DVec2,
}

struct ContainerData{
    component: WidgetRef,
    container: WidgetRef,
    rect: Rect
}

enum Edge{
    Left,
    Right,
    Bottom,
    Top,
}

impl ContainerData{
    fn get_edge(&self, rel:DVec2, zoom:f64, pan: DVec2)->Option<Edge>{
        let cp = rel * zoom + pan;
        let edge_outer:f64 = 3.0 * zoom ;
        let edge_inner:f64  = 3.0 * zoom ;
        
        if cp.x >= self.rect.pos.x - edge_outer && 
        cp.x <= self.rect.pos.x + edge_inner && 
        cp.y >= self.rect.pos.y && 
        cp.y <= self.rect.pos.y + self.rect.size.y{
            // left edge
            return Some(Edge::Left);
        }
        if cp.x >= self.rect.pos.x + self.rect.size.x- edge_outer && 
        cp.x <= self.rect.pos.x + self.rect.size.x+ edge_inner && 
        cp.y >= self.rect.pos.y && 
        cp.y <= self.rect.pos.y + self.rect.size.y{
            return Some(Edge::Right);
        }
        else if cp.y >= self.rect.pos.y - edge_outer && 
        cp.y <= self.rect.pos.y + edge_inner &&
        cp.x >= self.rect.pos.x && 
        cp.x <= self.rect.pos.x + self.rect.size.x{
            // top edge
            return Some(Edge::Top);
        }
        else if cp.y >= self.rect.pos.y + self.rect.size.y- edge_outer && 
        cp.y <= self.rect.pos.y + self.rect.size.y + edge_inner &&
        cp.x >= self.rect.pos.x && 
        cp.x <= self.rect.pos.x + self.rect.size.x{
            // bottom edge
            return Some(Edge::Bottom);
        }
        None
    }
}

#[derive(Live, Widget)]
pub struct DesignerView {
    #[walk] walk:Walk,
    #[rust] area:Area,
    #[rust] reapply: bool,
    #[rust(1.5)] zoom: f64,
    #[rust] pan: DVec2,
    #[rust] finger_move: Option<FingerMove>,
    #[live] container: Option<LivePtr>,
    #[live] draw_bg: DrawColor,
    #[rust] containers: ComponentMap<LivePtr, ContainerData>,
    #[redraw] #[rust(DrawList2d::new(cx))] draw_list: DrawList2d,
    #[rust(Pass::new(cx))] pass: Pass,
    #[rust] color_texture: Option<Texture>,
}

impl LiveHook for DesignerView {
    fn after_apply(&mut self, _cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]){
        
        // hmm. we might need to re-apply the data
        self.reapply = true;
    }
}
        
impl Widget for DesignerView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        
        match event.hits(cx, self.area) {
            Hit::FingerHoverOver(fh) =>{
                let cp = (fh.abs -fh.rect.pos) * self.zoom + self.pan;
                println!("{:?}", cp);
                // alright so we hover over. lets determine the mouse cursor
                //let corner_inner:f64  = 10.0 * self.zoom;
                //let corner_outer:f64  = 10.0 * self.zoom;
                cx.set_cursor(MouseCursor::Default);
                for cd in self.containers.values(){
                    match cd.get_edge((fh.abs -fh.rect.pos), self.zoom, self.pan){
                        Some(Edge::Left)=>{
                            cx.set_cursor(MouseCursor::EwResize);
                        }
                        Some(Edge::Right)=>{
                            cx.set_cursor(MouseCursor::EwResize);
                        }
                        Some(Edge::Top)=>{
                            cx.set_cursor(MouseCursor::NsResize);
                        }
                        Some(Edge::Bottom)=>{
                            cx.set_cursor(MouseCursor::NsResize);
                        }
                        None=>{
                        }
                    }
                    
                }
            }
            Hit::FingerHoverOut(_fh)=>{
                
            }
            Hit::FingerDown(_fe) => {
              self.finger_move = Some(FingerMove{
                  start_pan: self.pan
              });
            },
            Hit::KeyDown(_k)=>{
               
            }
            Hit::FingerScroll(fs)=>{
                let last_zoom = self.zoom;
                if fs.scroll.y < 0.0{
                    self.zoom *= 0.9;
                }
                else{
                    self.zoom *= 1.1;
                }
                // we should shift the pan to stay in the same place
                let pan1 = (fs.abs - fs.rect.pos) * last_zoom;
                let pan2 = (fs.abs - fs.rect.pos) * self.zoom;
                // we should keep it in the same place
                
                self.pan += pan1 - pan2;
                
                self.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let fm = self.finger_move.as_ref().unwrap();
                self.pan= fm.start_pan - (fe.abs - fe.abs_start) * self.zoom;
                self.redraw(cx);
            }
            Hit::FingerUp(_) => {
                self.finger_move = None;
            }
            _ => ()
        }
    }
        
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
       
        if self.color_texture.is_none(){
            self.color_texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                },
            ));
            self.pass.add_color_texture(
                cx,
                self.color_texture.as_ref().unwrap(),
                PassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
            )
        }
        
        if cx.will_redraw(&mut self.draw_list, walk) {
            
            cx.make_child_pass(&self.pass);
            cx.begin_pass(&self.pass, None);
            
            self.draw_list.begin_always(cx);
    
            cx.begin_pass_sized_turtle_no_clip(Layout::flow_down());
            
            let data = scope.props.get::<DesignerData>().unwrap();
            
            // lets draw the component container windows and components
            
            if let Some(selected) = &data.selected{
                if let Some(OutlineNode::Component{ptr,..}) = data.node_map.get(selected){
                    
                    let container_ptr = self.container.unwrap();
                    let cd = self.containers.get_or_insert(cx, *ptr, | cx | {
                        ContainerData{
                            component:WidgetRef::new_from_ptr(cx, Some(*ptr)),
                            container: WidgetRef::new_from_ptr(cx, Some(container_ptr)),
                            rect: rect(50.0,50.0,800.0,600.0)
                        }
                    });
                    
                    if self.reapply{
                        self.reapply = false;
                        cd.container.apply_from_ptr(cx, Some(self.container.unwrap()));
                        cd.component.apply_from_ptr(cx, Some(*ptr));
                    }
                    // ok so we're going to draw the container with the widget inside
                    cd.container.draw_all(cx, &mut Scope::with_props(cd))
                }
            }
            
            cx.end_pass_sized_turtle_no_clip();
            self.draw_list.end(cx);
            cx.end_pass(&self.pass);
        }
        
        self.draw_bg.draw_vars.set_texture(0, self.color_texture.as_ref().unwrap());
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.draw_bg.draw_abs(cx, rect);
            
        cx.set_pass_area_with_origin(
            &self.pass,
            self.area,
            dvec2(0.0,0.0)
        );
        cx.set_pass_shift_scale(&self.pass, self.pan, dvec2(self.zoom,self.zoom));
        
        DrawStep::done()
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
        let file_tree = self.view.outline_tree(id!(outline_tree));
        let data = scope.props.get::<DesignerData>().unwrap();
        let mut buf = String::new();
        while let Some(next) = self.view.draw(cx, &mut Scope::empty()).step() {
            if let Some(mut file_tree) = file_tree.borrow_mut_if_eq(&next) {
                if let OutlineNode::Virtual{children,..} = &data.node_map.get(&data.root).as_ref().unwrap(){
                    recur_nodes(&mut buf, cx,  &mut *file_tree, &data.node_map, children);
                }
            }
        }
        
        fn recur_nodes(buf:&mut String, cx: &mut Cx2d, outline_tree: &mut OutlineTree,map:&HashMap<LiveId,OutlineNode>, children:&[LiveId]) {
            for child in children{
                match map.get(&child).unwrap(){
                    OutlineNode::Folder{name, children}=>{
                        if outline_tree.begin_folder(cx, *child, &name).is_ok(){
                            recur_nodes(buf, cx, outline_tree, map, children);
                            outline_tree.end_folder();
                        }            
                    }
                    OutlineNode::File{name,  file_id:_, children}=>{
                        if outline_tree.begin_folder(cx, *child, &name).is_ok(){
                            recur_nodes(buf, cx, outline_tree, map, children);
                            outline_tree.end_folder();
                        }            
                    }
                    OutlineNode::Virtual{name, children}=>{
                        if outline_tree.begin_folder(cx, *child, &name).is_ok(){
                            recur_nodes(buf, cx, outline_tree, map, children);
                            outline_tree.end_folder();
                        }            
                    }
                    OutlineNode::Component{children, name, prop_type, class, ..}=>{
                        buf.clear();
                        if !name.is_unique(){
                            if let LivePropType::Field = prop_type {
                                write!(buf, "{}: <{}>", name, class).unwrap();
                            }
                            else {
                                write!(buf, "{}=<{}>", name, class).unwrap();
                            }
                        }
                        else {
                            write!(buf, "<{}>", class).unwrap();
                        }
                        
                        if outline_tree.begin_folder(cx, *child, &buf).is_ok() {
                            recur_nodes(buf, cx, outline_tree, map, children);
                            outline_tree.end_folder();
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }
}

#[derive(Default)]
pub struct DesignerData{
    root: LiveId,
    node_map: HashMap<LiveId, OutlineNode>,
    selected: Option<LiveId>
}

impl DesignerData{
    fn remove_child(&mut self, find_node:LiveId){
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
    
    fn find_component_by_path(&self, path:&[LiveId])->Option<LiveId>{
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

#[derive(Live, Widget)]
pub struct Designer {
    #[deref] ui: View,
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
        }
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
        let outline_tree = self.ui.outline_tree(id!(outline_tree));
        if let Some(file_id) = outline_tree.folder_clicked(&actions) {
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
