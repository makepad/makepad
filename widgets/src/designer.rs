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
            align: FromStart(200),
            a: <Frame> {
                outline = <FileTree> {
                }
            },
            b: <Solid> {draw_bg: {color: #4}},
        }
    }
}

#[allow(dead_code)]
enum OutlineNode{
    Global{
        name: LiveId,
        ptr: LivePtr
    },
    Component{
        name: LiveId,
        ptr: LivePtr,
        children: Vec<OutlineNode>
    }
}

#[derive(Live)]
pub struct Designer {
    #[rust] _outline: Vec<OutlineNode>,
    #[deref] ui: Frame,
}

impl LiveHook for Designer {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, Designer)
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // lets take the doc we need (app_mobile for instance)
        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        let file_id = live_registry.file_name_to_file_id("examples/ironfish/src/app_mobile.rs").unwrap();
        // now we fetch the unexpanded nodes
        // and build a list
        let _file = live_registry.file_id_to_file(file_id);
        
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
                if outline.begin_folder(cx, live_id!(root).into(), "ROOT").is_ok() {
                    for i in 0..10 {
                        outline.file(cx, LiveId(i as u64).into(), &format!("FILE {i}"))
                    }
                    outline.end_folder();
                }
            }
        }
        WidgetDraw::done()
    }
}
