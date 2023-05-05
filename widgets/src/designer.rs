use crate::{
    makepad_draw::*,
    file_tree::*,
    frame::Frame,
    widget::*,
};

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_widgets::splitter::Splitter;
    import makepad_widgets::file_tree::FileTree,
    import makepad_draw::shader::std::*;
    
    Designer = {{Designer}} {
        layout: {flow: Right},
        <Splitter> {
            align: FromStart(300),
            a: <Frame> {
                outline = <FileTree> {
                }
            },
            b: <Solid> {draw_bg: {color: #4}},
        }
    }
}

#[allow(dead_code)]
enum OutlineNode {
    Global {
        uid: FileNodeId,
        name: LiveId,
        ptr: LivePtr
    },
    Component {
        uid: FileNodeId,
        name: LiveId,
        class: LiveId,
        prop_type: LivePropType,
        ptr: LivePtr,
        children: Vec<OutlineNode>
    }
}

#[derive(Live)]
pub struct Designer {
    #[rust] outline_nodes: Vec<OutlineNode>,
    #[deref] ui: Frame,
}

impl LiveHook for Designer {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Designer)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // lets take the doc we need (app_mobile for instance)
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = &*live_registry_rc.borrow();
        let file_id = live_registry.file_name_to_file_id("examples/ironfish/src/app_mobile.rs").unwrap();
        // now we fetch the unexpanded nodes
        // and build a list
        let file = live_registry.file_id_to_file(file_id);
        let nodes = &file.expanded.nodes;
        // lets run over the file
        fn recur_walk(live_registry: &LiveRegistry, base_ptr: LivePtr, mut index: usize, nodes: &[LiveNode], out: &mut Vec<OutlineNode>) -> usize {
            while index < nodes.len() - 1 {
                if let LiveValue::Class {class_parent, ..} = &nodes[index].value {
                    // lets emit a class at our level
                    let mut children = Vec::new();
                    let name = nodes[index].id;
                    let class = live_registry.ptr_to_node(class_parent.unwrap()).id;
                    /*if !name.is_ident(){ // no name.. 
                        // ok so we have to find the origin 'class' name now
                        let parent_node = live_registry.ptr_to_node(class_parent.unwrap());
                        //live_registry.file_id_to_file_name()
                        log!("Got class parent {:?} {:?}",class_parent, parent_node.value);
                        name = parent_node.id;
                    }*/
                    let ptr = base_ptr.with_index(index);
                    index = recur_walk(live_registry, base_ptr, index + 1, nodes, &mut children);
                    out.insert(0, OutlineNode::Component {
                        uid: LiveId::unique().into(),
                        name,
                        prop_type: nodes[index].origin.prop_type(),
                        class,
                        ptr,
                        children
                    });
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
        let base_ptr = live_registry.file_id_index_to_live_ptr(file_id, 0);
        recur_walk(live_registry, base_ptr, 1, nodes, &mut self.outline_nodes);
        
    }
}

impl Widget for Designer {
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let _actions = self.ui.handle_widget_event(cx, event);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        let outline = self.ui.get_file_tree(id!(outline));
        while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
            if let Some(mut outline) = outline.pick(next).borrow_mut() {
                fn recur_walk(cx: &mut Cx2d, outline: &mut FileTree, children: &[OutlineNode]) {
                    for child in children {
                        match child {
                            OutlineNode::Global {..} => {}
                            OutlineNode::Component {name, children, uid, class, prop_type, ..} => {
                                name.as_string( | s | {
                                    if outline.begin_folder(cx, *uid, &if let Some(s) = s {
                                        if let LivePropType::Field = prop_type{
                                            format!("{}: <{}>", s, class)
                                        }
                                        else{
                                            format!("{}=<{}>", s, class)
                                        }
                                    }else {
                                        format!("<{}>", class)
                                    }).is_ok() {
                                        recur_walk(cx, outline, children);
                                        outline.end_folder();
                                    }
                                });
                            }
                        }
                    }
                }
                recur_walk(cx, &mut *outline, &self.outline_nodes);
            }
        }
        WidgetDraw::done()
    }
}
