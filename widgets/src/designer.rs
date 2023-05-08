use crate::{
    makepad_draw::*,
    file_tree::*,
    frame::Frame,
    widget::*,
    label::*,
};

live_design!{
    import makepad_widgets::theme::*
    import makepad_widgets::frame::*
    import makepad_widgets::splitter::Splitter
    import makepad_widgets::file_tree::FileTree
    import makepad_widgets::hook_widget::HookWidget
    import makepad_widgets::label::Label
    import makepad_draw::shader::std::*
    
    Designer = {{Designer}} {
        layout: {flow: Right},
        container: <Box> {
            draw_bg: {color: #3}
            walk: {width: Fill, height: 400},
            layout: {flow: Down, spacing: 10, padding:10}
            <Box>{
                walk: {width: Fill, height: Fit},
                layout:{padding:5}
                draw_bg:{color:#5}
                label = <Label> {label: "HI", draw_label:{color:#f}}
            }
            inner = <HookWidget> {}
        }
        <Splitter> {
            align: FromStart(300),
            a: <Frame> {
                outline = <FileTree> {
                }
            },
            b: <CachedScrollXY> {
                dpi_factor: 1.0
                draw_bg: {color: #4}
                walk: {width: Fill, height: Fill}
                layout: {flow: Down},
                design = <HookWidget> {}
            },
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
    #[live] container: Option<LivePtr>,
    #[rust] outline_nodes: Vec<OutlineNode>,
    #[rust] components: ComponentMap<LivePtr, (WidgetRef, WidgetRef)>,
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
        let file_id = live_registry.file_name_to_file_id("examples/ironfish/src/app_desktop.rs").unwrap();
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
    // ok now we can iterate our top level components
    // and instance them
}

impl Designer {
    
    fn draw_design(&mut self, cx: &mut Cx2d) {
        // alrigh so. lets draw the designs
        for node in &self.outline_nodes {
            if let OutlineNode::Component {ptr, name, class, ..} = node {
                let container_ptr = self.container;
                let (widget, container) = self.components.get_or_insert(cx, *ptr, | cx | {
                    (
                        WidgetRef::new_from_ptr(cx, Some(*ptr)),
                        WidgetRef::new_from_ptr(cx, container_ptr),
                    )
                });
                container.get_label(id!(label)).set_label(&format!("{}=<{}>", name, class));
                // lets draw this thing in a neat little container box with a title bar
                while let Some(_) = container.draw_widget(cx).hook_widget() {
                    widget.draw_widget_all(cx);
                }
            }
        }
        
    }
    
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
    
}

impl Widget for Designer {
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let _actions = self.ui.handle_widget_event(cx, event);
        for (component, container) in self.components.values_mut() {
            component.handle_widget_event(cx, event);
            container.handle_widget_event(cx, event);
        }
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        let outline = self.ui.get_file_tree(id!(outline));
        while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
            if let Some(mut outline) = outline.has_widget(&next).borrow_mut() {
                self.draw_outline(cx, &mut *outline);
            }
            else if next == self.ui.get_widget(id!(design)) {
                self.draw_design(cx);
            }
        }
        WidgetDraw::done()
    }
}
