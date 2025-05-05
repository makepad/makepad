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
    //makepad_platform::makepad_live_compiler::TextSpan,
};

live_design!{
    link designer_real
    use link::widgets::*;
    use link::theme::*;
    use crate::designer_outline::*;
    use crate::designer_outline_tree::*;
    use crate::designer_toolbox::*;
    use crate::designer_view::*;
    use crate::designer_theme::*;
        
    DesignerBase = {{Designer}} {
    }
    
    pub Designer = <DesignerBase>{
        <Window> {
            window: { kind_id: 2 }
            body = <View> {
                designer_outline = <DesignerOutline> {
                    flow: Down,
                    <DockToolbar> {
                        content = {
                            margin: {left: (THEME_SPACE_1), right: (THEME_SPACE_1) },
                            align: { x: 0., y: 0.0 }
                            spacing: (THEME_SPACE_3)
                            <Pbold> {
                                width: Fit,
                                text: "Filter",
                                margin: 0.,
                                padding: <THEME_MSPACE_V_1> {}
                            }
                            
                            <View> {
                                width: Fit
                                flow: Right,
                                spacing: (THEME_SPACE_1)
                                <CheckBoxCustom> {
                                    width: 25,
                                    margin: {left: (THEME_SPACE_1)}
                                    text: ""
                                    draw_bg: { check_type: None }
                                    icon_walk: {width: 13.5 }
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_2),
                                        svg_file: dep("crate://self/resources/icons/icon_widget.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    width: 25,
                                    text: ""
                                    draw_bg: { check_type: None }
                                    icon_walk: {width: 12.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_6),
                                        svg_file: dep("crate://self/resources/icons/icon_layout.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    width: 25,
                                    text: ""
                                    draw_bg: { check_type: None }
                                    icon_walk: {width: 10.5}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_1),
                                        svg_file: dep("crate://self/resources/icons/icon_text.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    width: 25,
                                    text:""
                                    draw_bg: { check_type: None }
                                    icon_walk: {width: 13.}
                                    draw_icon: {
                                        color: (THEME_COLOR_D_3),
                                        color_active: (STUDIO_PALETTE_5),
                                        svg_file: dep("crate://self/resources/icons/icon_image.svg"),
                                    }
                                }
                            }
                            <TextInputFlat> {
                                width: Fill,
                                empty_text: "Filter",
                            }
                        }
                    }
                    outline_tree = <DesignerOutlineTree>{
                        
                    }
                }
            }
        }
        <Window>{
            window:{ kind_id: 1 }
            body = <View>{
                flow: Overlay
                designer_view = <DesignerView> {
                    width: Fill, height: Fill
                }
                toolbox = <DesignerToolbox>{
                }
            }
        }
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
    
    fn after_update_from_doc(&mut self, cx:&mut Cx){
        //let designer_view = self.ui.designer_view(id!(designer_view));
        //designer_view.reload_view(cx);
        let outline_tree = self.ui.designer_outline_tree(id!(outline_tree));
        outline_tree.redraw(cx);
        self.data.pending_revision = false;
    }
    
    fn after_new_from_doc(&mut self, _cx:&mut Cx){
        
        Cx::send_studio_message(AppToStudio::DesignerStarted);
    }
}

impl Designer{
    fn studio_jump_to_component(&self, cx:&Cx, component:LiveId){
        if let Some(OutlineNode::Component{ptr,..}) =  self.data.node_map.get(&component){
            let (file_name,span) = cx.live_registry.borrow().ptr_to_file_name_and_object_span(*ptr);
            Cx::send_studio_message(AppToStudio::JumpToFile(JumpToFile{
                file_name,
                line: span.start.line,
                column: span.start.column
            }));
        }
    }
    
    fn studio_select_component(&self, cx:&Cx, component:LiveId){
        if let Some(OutlineNode::Component{ptr,..}) =  self.data.node_map.get(&component){
            let (file_name,span) = cx.live_registry.borrow().ptr_to_file_name_and_object_span(*ptr);
            println!("{:?}", span);
            Cx::send_studio_message(AppToStudio::SelectInFile(SelectInFile{
                file_name,
                line_start: span.start.line,
                column_start: span.start.column,
                line_end: span.end.line,
                column_end: span.end.column
            }));
        }
    }
    
    fn studio_swap_components(&mut self, cx:&Cx, c1:LiveId, c2:LiveId){
        if self.data.pending_revision{
            return 
        }
        if let Some(OutlineNode::Component{ptr:ptr1,..}) =  self.data.node_map.get(&c1){
            if let Some(OutlineNode::Component{ptr:ptr2,..}) =  self.data.node_map.get(&c2){
                let (s1_file_name,s1_span) = cx.live_registry.borrow().ptr_to_file_name_and_object_span(*ptr1);
                let (s2_file_name,s2_span) = cx.live_registry.borrow().ptr_to_file_name_and_object_span(*ptr2);
                self.data.pending_revision = true;
                Cx::send_studio_message(AppToStudio::SwapSelection(SwapSelection{
                    s1_file_name,
                    s1_line_start: s1_span.start.line,
                    s1_column_start: s1_span.start.column,
                    s1_line_end: s1_span.end.line,
                    s1_column_end: s1_span.end.column,
                    s2_file_name,
                    s2_line_start: s2_span.start.line,
                    s2_column_start: s2_span.start.column,
                    s2_line_end: s2_span.end.line,
                    s2_column_end: s2_span.end.column,
                }));
            }
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
                self.studio_select_component(cx, outline_id)
            }
        }
        
        if let Some((c1, c2)) = designer_view.swap_components(&actions){
            self.studio_swap_components(cx, c1, c2);
            outline_tree.redraw(cx);
        }
        // ok lets see if we have a designerselectfile action
        for action in actions{
            if let StudioToApp::DesignerSelectFile{file_name} = action.cast_ref(){
                let path_ids = DesignerData::path_str_to_path_ids(&file_name);
                if path_ids.len() > 0{
                    outline_tree.select_and_show_node(cx, &path_ids);
                    designer_view.select_component(cx, None);
                    designer_view.view_file(cx, *path_ids.last().unwrap());
                }
            }
             if let StudioToApp::DesignerLoadState{positions, zoom_pan} = action.cast_ref(){
                 self.data.to_widget.positions = positions.clone();
                 designer_view.set_zoom_pan(cx,zoom_pan);
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
                            designer_view.select_component(cx, None);
                            designer_view.view_file(cx, outline_id);
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
                                designer_view.select_component(cx, Some(outline_id));
                                designer_view.view_file(cx, file_id);
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
        let mut scope = Scope::with_data(&mut self.data);
        self.ui.handle_event(cx, event, &mut scope);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, _walk: Walk) -> DrawStep {
        let mut scope = Scope::with_data(&mut self.data);
        self.ui.draw(cx, &mut scope)
    }
}
