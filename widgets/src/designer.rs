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
        ui: <Frame>{
            layout: {flow: Right},
            <Splitter>{
                align: FromStart(200),
                a:<Frame>{
                    outline = <FileTree>{
                    }
                },
                b:<Solid>{draw_bg:{color:#4}},
            }
        }
    }
}

#[derive(Live)]
pub struct Designer {
    #[live] ui: Frame,
}

impl LiveHook for Designer {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, Designer)
    }
}

impl Widget for Designer{
   fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let _actions = self.ui.handle_widget_event(cx, event);
    }

    fn get_walk(&self)->Walk{Walk::default()}
    
    fn redraw(&mut self, cx:&mut Cx){
        self.ui.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        let outline = self.ui.get_file_tree(id!(outline));
        while let Some(next) = self.ui.draw_widget_hook(cx).hook_widget() {
            if let Some(mut outline) = outline.pick(next).borrow_mut() {
                //outline.set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
                if outline.begin_folder(cx, live_id!(root).into(), "ROOT").is_ok(){
                    for i in 0..10{
                        outline.file(cx, LiveId(i as u64).into(), &format!("FILE {i}"))
                    }
                    outline.end_folder();
                }
                if outline.begin_folder(cx, live_id!(root2).into(), "ROOT2").is_ok(){
                    for i in 0..10{
                        outline.file(cx, LiveId(80+i as u64).into(), &format!("FILE {i}"))
                    }
                    outline.end_folder();
                }
            }
        }
        WidgetDraw::done()
    }
}
