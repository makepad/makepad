use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    multi_window::*,
    widget_match_event::*,
    designer_data::*,
    designer_outline_tree::*,
    widget::*,
    makepad_platform::studio::*,
};

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
        self.data.update_from_live_registry(cx);
    }
    
    // ok now we can iterate our top level components
    // and instance them
}

impl WidgetMatchEvent for Designer{
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope){
        let outline_tree = self.ui.designer_outline_tree(id!(outline_tree));
        if let Some((selected_file_id,km)) = outline_tree.selected(&actions) {
            // alright we have a folder clicked
            // lets get a file/line number out of it so we can open it in the code editor.
            
            if let Some(node) = self.data.node_map.get(&selected_file_id){
                match node{
                    OutlineNode::File{file_id,..}=>{
                        if km.control{
                            let file_name = cx.live_registry.borrow().file_id_to_file(*file_id).file_name.clone();
                            Cx::send_studio_message(AppToStudio::JumpToFile(JumpToFile{
                                file_name,
                                line: 0,
                                column: 0
                            }));
                        }
                        else if km.alt{
                            Cx::send_studio_message(AppToStudio::FocusDesign);
                        }
                        else{
                            self.data.selected = Some(selected_file_id);
                            self.ui.widget(id!(designer_view)).redraw(cx);
                        }        
                    }
                    OutlineNode::Component{token_id,..}=>{
                        if km.control{
                            let file_id = token_id.file_id().unwrap();
                            let live_registry = cx.live_registry.borrow();
                            let tid = live_registry.token_id_to_token(*token_id).clone();
                            let span = tid.span.start;
                            let file_name = live_registry.file_id_to_file(file_id).file_name.clone();
                            Cx::send_studio_message(AppToStudio::JumpToFile(JumpToFile{
                                file_name,
                                line: span.line,
                                column: span.column
                            }));
                        }
                        else if km.alt{
                            Cx::send_studio_message(AppToStudio::FocusDesign);
                        }
                        else{
                            self.data.selected = Some(selected_file_id);
                            self.ui.widget(id!(designer_view)).redraw(cx);
                        }
                    }
                    _=>()
                }
            }
        }
    }
}

impl Widget for Designer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.widget_match_event(cx, event, scope);
        let mut scope = Scope::with_props(&self.data);
        self.ui.handle_event(cx, event, &mut scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, _walk: Walk) -> DrawStep {
        let mut scope = Scope::with_props(&self.data);
        self.ui.draw(cx, &mut scope)
    }
}
