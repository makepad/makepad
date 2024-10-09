use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    multi_window::*,
    widget_match_event::*,
    designer_data::*,
    designer_view::*,
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
    
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        Cx::send_studio_message(AppToStudio::DesignerStarted);
    }
}

impl Designer{
    fn studio_jump_to_component(&self, cx:&Cx, component:LiveId){
        if let Some(OutlineNode::Component{token_id,..}) =  self.data.node_map.get(&component){
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
    }
    
    fn studio_jump_to_file(&self, cx:&Cx, file_id:LiveFileId){
        let file_name = cx.live_registry.borrow().file_id_to_file(file_id).file_name.clone();
        Cx::send_studio_message(AppToStudio::JumpToFile(JumpToFile{
            file_name,
            line: 0,
            column: 0
        }));
    }
}

impl WidgetMatchEvent for Designer{
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope){
        let outline_tree = self.ui.designer_outline_tree(id!(outline_tree));
        let designer_view = self.ui.designer_view(id!(designer_view));
        if let Some((outline_id, km, tap_count)) = designer_view.selected(&actions){
            // select the right node in the filetree
            let path_ids = self.data.construct_path_ids(outline_id);
            outline_tree.select_and_show_node(cx, &path_ids);
            // if we click with control
            if km.control || tap_count > 1{
                self.studio_jump_to_component(cx, outline_id)
            }
        }
        // ok lets see if we have a designerselectfile action
        for action in actions{
            if let StudioToApp::DesignerSelectFile{file_name} = action.cast_ref(){
                let path_ids = DesignerData::path_str_to_path_ids(&file_name);
                outline_tree.select_and_show_node(cx, &path_ids);
                designer_view.select_component_and_redraw(cx, None);
                designer_view.view_file_and_redraw(cx, *path_ids.last().unwrap());
            }
        }
        if let Some((outline_id,km)) = outline_tree.selected(&actions) {
            // alright we have a folder clicked
            // lets get a file/line number out of it so we can open it in the code editor.
            
            if let Some(node) = self.data.node_map.get(&outline_id){
                match node{
                    OutlineNode::File{file_id,..}=>{
                        let path_ids = self.data.construct_path_ids(outline_id);
                        let file_name = self.data.path_ids_to_string(&path_ids);
                        Cx::send_studio_message(AppToStudio::DesignerFileSelected{
                            file_name
                        });
                        if km.control{
                            self.studio_jump_to_file(cx, *file_id);
                        }
                        else if km.alt{
                            Cx::send_studio_message(AppToStudio::FocusDesign);
                        }
                        else{
                            designer_view.select_component_and_redraw(cx, None);
                            designer_view.view_file_and_redraw(cx, outline_id);
                        }        
                    }
                    OutlineNode::Component{..}=>{
                        if km.control{
                            self.studio_jump_to_component(cx, outline_id)
                        }
                        else if km.alt{
                            Cx::send_studio_message(AppToStudio::FocusDesign);
                        }
                        else{
                            // only select the file 
                            if let Some(file_id) = self.data.find_file_parent(outline_id){
                                designer_view.select_component_and_redraw(cx, Some(outline_id));
                                designer_view.view_file_and_redraw(cx, file_id);
                            }
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
