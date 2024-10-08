
use {
    crate::{
        app::{AppData, AppAction},
        ai_chat::ai_chat_manager::*,
        file_system::file_system::{EditSession,OpenDocument},
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_code_editor::code_view::CodeView;
    import makepad_widgets::base::*;
    import makepad_draw::shader::std::*;
        
    import makepad_widgets::theme_desktop_dark::*;
    
    User = <RoundedView>{
        height: Fit
        draw_bg:{color:#5}
        padding: {left:5,top:5,right:5},
        flow:Down
        <View>{
            height:Fit
            width:Fill
            <Label>{margin:{top:4.5},text:"Project:"}
            project_dropdown = <DropDown>{ width: Fit,popup_menu_position:BelowInput}
            <Label>{margin:{top:4.5,left:20},text:"Context:"}
            context_dropdown = <DropDown>{ width: Fit,popup_menu_position:BelowInput}
            <Label>{margin:{top:4.5, left:20},text:"Model:"}
            model_dropdown = <DropDown>{ width: Fit,popup_menu_position:BelowInput}
            auto_run = <CheckBox>{ margin:{left:20}, text:"Autorun", width: Fit}
            run_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "Run"
            }
        }
        <View>{
            height:Fit
            width:Fill
            message_input = <TextInput> {
                text: ""
                empty_message:"..."
                width: Fill,
                height: Fit,
                draw_bg: {
                    color: #1
                }
            }
            send_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: ">"
            }
                    
            clear_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "X"
            }
        }
        
        
    }
    
    Assistant = <RoundedView>{
        draw_bg:{color:#4}
        flow: Down
        md = <Markdown>{
            code_block = <View>{
                
                width:Fill,
                height:Fit,
                flow: Overlay
                code_view = <CodeView>{
                    editor:{
                        draw_bg: { color: (#3) }
                    }
                }
                <View>{
                    //show_bg: true,
                    //draw_bg:{color:#7}
                    width:Fill,
                    height:Fit,
                    align:{x:1.0}
                    copy_button = <Button> {
                        icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                        text: "Copy"
                    }
                    
                }
                
            }
            use_code_block_widget: true,
            body:""
        }
        busy = <View>{
            margin:{top:5, bottom:5}
            width: 50,
            height: 10
            show_bg: true,
            draw_bg:{
                fn pixel(self)->vec4{
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                    let x = 5.0;
                    for i in 0..4{
                        x = x + 8.0;
                        sdf.circle(x,5.,3.);
                        sdf.fill(#a);
                    }
                    return sdf.result
                }
            }
        }
    }
    
    AiChatView = {{AiChatView}}{
        height: Fill, width: Fill,
        flow: Down
        spacing: 3
        
        tb = <DockToolbar> {
            
            content = {
                align: { x: 0., y: 0.5}
                height: Fit, width: Fill,
                spacing: (THEME_SPACE_1)
                flow: Right,
                margin: {left: (THEME_SPACE_1), right: (THEME_SPACE_1) },
                history_left = <ButtonFlat> { width: Fit, text: "<"}
                history_right = <ButtonFlat> { width: Fit, text: ">"}
                slot = <Label> { width: Fit, text: "0"}
                <View>{width:Fill}
                history_delete = <ButtonFlat> { width: Fit, text: "Delete"}
            }
        }
        // lets make portal list with User and Assistant components
        // and lets fix the portal lists scroll
        list = <PortalList>{
            drag_scrolling: false
            //auto_tail: true
            User = <User>{}
            Assistant = <Assistant>{}
        }
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct AiChatView{
    #[deref] view:View,
    #[rust] initialised: bool,
    #[rust] history_slot: usize,
}

impl AiChatView{
    fn handle_own_actions(&mut self, cx: &mut Cx, actions:&Actions, scope: &mut Scope){
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get_mut(&chat_id){
                if let Some(wa) = actions.widget_action(id!(copy_button)){
                    if wa.widget().as_button().pressed(actions){
                        let code_view = wa.widget_nth(2).widget(id!(code_view));
                        log!("COPY! {}", code_view.text());
                    }
                }
                
                if self.view.button(id!(history_left)).pressed(actions){
                    // first we check if our messages are the same as 'slot'.
                    // if not, we should create an undo item first
                    self.history_slot = self.history_slot.saturating_sub(1);
                    cx.action(AppAction::RedrawAiChat{chat_id});
                }
                if self.view.button(id!(history_right)).pressed(actions){
                    self.history_slot = (self.history_slot+ 1).min(doc.file.history.len().saturating_sub(1));
                    cx.action(AppAction::RedrawAiChat{chat_id});
                }
                if self.view.button(id!(history_delete)).pressed(actions){
                    doc.file.remove_slot(cx, &mut self.history_slot);
                    cx.action(AppAction::RedrawAiChat{chat_id});
                    cx.action(AppAction::SaveAiChat{chat_id});
                }
                                
                let list = self.view.portal_list(id!(list));
                for (item_id,item) in list.items_with_actions(actions){
                    let message_input = item.text_input(id!(message_input));
                    if let Some(text) = message_input.changed(actions){
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        cx.action(AppAction::RedrawAiChat{chat_id});
                        cx.action(AppAction::SaveAiChat{chat_id});
                    }
                    if message_input.escape(actions){
                        cx.action(AppAction::CancelAiGeneration{chat_id});
                    }
                    
                    if let Some(ctx_id) = item.drop_down(id!(context_dropdown)).selected(actions){
                        let ctx_name = &data.ai_chat_manager.contexts[ctx_id].name;
                        doc.file.set_base_context(self.history_slot, item_id, ctx_name);
                    }
                    
                    if let Some(model_id) = item.drop_down(id!(model_dropdown)).selected(actions){
                        let model = &data.ai_chat_manager.models[model_id].name;
                        doc.file.set_model(self.history_slot, item_id, model);
                    }
                    
                    if let Some(project_id) = item.drop_down(id!(project_dropdown)).selected(actions){
                        let model = &data.ai_chat_manager.projects[project_id].name;
                        doc.file.set_project(self.history_slot, item_id, model);
                    }
                    if let Some(value) = item.check_box(id!(auto_run)).changed(actions){
                        doc.file.set_auto_run(self.history_slot, item_id, value);
                    }
                    
                    if item.button(id!(run_button)).pressed(actions){
                        cx.action(AppAction::RunAiChat{chat_id, history_slot: self.history_slot, item_id});
                    }
                    
                    if item.button(id!(send_button)).pressed(actions) || 
                    item.text_input(id!(message_input)).returned(actions).is_some(){
                        // we'd already be forked
                        let text = message_input.text();
                        
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, text);
                        // alright so we press send/enter now what
                        // we now call 'setaichatlen' this will 'fork' our current index
                        // what if our chat is empty? then we dont fork
                        doc.file.clamp_slot(&mut self.history_slot);
                        // lets fetch the context
                        // println!("{}", dd.selected_item());
                        // alright lets collect the context
                        cx.action(AppAction::SendAiChatToBackend{chat_id, history_slot: self.history_slot});
                        cx.action(AppAction::SaveAiChat{chat_id});
                        cx.action(AppAction::RedrawAiChat{chat_id});
                    }
                    // lets clear the messages
                    if item.button(id!(clear_button)).pressed(actions){
                        doc.file.fork_chat_at(cx, &mut self.history_slot, item_id, "".to_string());
                        cx.action(AppAction::SaveAiChat{chat_id});
                        cx.action(AppAction::RedrawAiChat{chat_id});
                    }
                }
            }
        }
       
    }
}
impl Widget for AiChatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get(&chat_id){
                if !self.initialised{
                    self.initialised = true;
                    self.history_slot = doc.file.history.iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.last_time.total_cmp(&b.last_time))
                    .map(|(index, _)| index).unwrap_or(0);
                }
                                
                
                let history_len = doc.file.history.len(); 
                self.view.label(id!(slot)).set_text_with(|v| fmt_over!(v, "{}/{}", self.history_slot+1, history_len));
                
                while let Some(item) =  self.view.draw_walk(cx, &mut Scope::empty(), walk).step(){
                    
                    if let Some(mut list) = item.as_portal_list().borrow_mut() {
                        doc.file.clamp_slot(&mut self.history_slot);
                        list.set_item_range(cx, 0,doc.file.history[self.history_slot].messages.len());
                        while let Some(item_id) = list.next_visible_item(cx) {
                            match doc.file.history[self.history_slot].messages.get(item_id){
                                Some(AiChatMessage::Assistant(val))=>{
                                    let item = list.item(cx, item_id, live_id!(Assistant));
                                    // alright we got the assistant. lets set the markdown stuff
                                    item.widget(id!(md)).set_text(&val);
                                    item.view(id!(busy)).set_visible(
                                        item_id + 1 == doc.file.history[self.history_slot].messages.len() && 
                                        doc.in_flight.is_some()
                                    );
                                    item.draw_all_unscoped(cx);
                                }
                                Some(AiChatMessage::User(val))=>{
                                    // lets set the value to the text input
                                    let item = list.item(cx, item_id, live_id!(User));
                                    
                                    // model dropdown
                                    let cb = item.check_box(id!(auto_run));
                                    cb.set_selected(cx, val.auto_run);
                                    
                                    // model dropdown
                                    let dd = item.drop_down(id!(model_dropdown));
                                    // ok how do we set these dropdown labels without causing memory changes
                                    let mut i = data.ai_chat_manager.models.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                    if let Some(pos) = data.ai_chat_manager.models.iter().position(|b| b.name == val.model){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    
                                    let dd = item.drop_down(id!(context_dropdown));
                                    let mut i = data.ai_chat_manager.contexts.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                    
                                    if let Some(pos) = data.ai_chat_manager.contexts.iter().position(|ctx| ctx.name == val.base_context){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    let dd = item.drop_down(id!(project_dropdown));
                                    let mut i = data.ai_chat_manager.projects.iter();
                                    dd.set_labels_with(|label|{i.next().map(|m| label.push_str(&m.name));});
                                                                        
                                    if let Some(pos) = data.ai_chat_manager.projects.iter().position(|ctx| ctx.name == val.base_context){
                                        dd.set_selected_item(pos);
                                    }
                                    
                                    item.widget(id!(message_input)).set_text(&val.message);
                                    item.draw_all_unscoped(cx);
                                }
                                _=>()
                            }
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let ac = cx.capture_actions(|cx|{
            self.view.handle_event(cx, event, scope);
        });
        if ac.len()>0{
            self.handle_own_actions(cx, &ac, scope)
        }
    }
}