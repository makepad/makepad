
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
        padding: 10,
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
    
    Assistant = <RoundedView>{
        draw_bg:{color:#4}
        flow: Down
        md = <Markdown>{
            code_block = <CodeView>{
                editor:{
                    draw_bg: { color: (#3) }
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
                        sdf.fill(#f);
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
        padding:{top:4}
        
        // lets make portal list with User and Assistant components
        // and lets fix the portal lists scroll
        list = <PortalList>{
            User = <User>{}
            Assistant = <Assistant>{}
        }
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct AiChatView{
    #[deref] view:View
}
impl WidgetMatchEvent for AiChatView{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions, scope: &mut Scope){
        let uid = self.widget_uid();
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get_mut(&chat_id){
                
                let list = self.view.portal_list(id!(list));
                for (item_id,item) in list.items_with_actions(actions){
                    let message_input = item.text_input(id!(message_input));
                    if let Some(text) = message_input.changed(actions){
                        // lets write the text to the chat index
                        if let Some(AiChatMessage::User(val)) = doc.file.messages.get_mut(item_id){
                            // update it
                            val.message = text;
                        }
                    }
                    if message_input.escape(actions){
                        cx.action(AppAction::CancelAiGeneration{chat_id});
                    }
                                        
                    if item.button(id!(send_button)).pressed(actions) || 
                    item.text_input(id!(message_input)).returned(actions).is_some(){
                        cx.action(AppAction::SetAiChatLen{chat_id, new_len:item_id+1});
                        cx.action(AppAction::SendAiChatToBackend{chat_id, backend_index:0});
                        self.redraw(cx);
                    }
                    // lets clear the messages
                    if item.button(id!(clear_button)).pressed(actions){
                        cx.action(AppAction::SetAiChatLen{chat_id, new_len:item_id+1});
                        item.text_input(id!(message_input)).set_text_and_redraw(cx,"");
                        if let Some(AiChatMessage::User(val)) = doc.file.messages.get_mut(item_id){
                            val.message.clear();
                        }
                        self.redraw(cx);
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
                while let Some(list) =  self.view.draw_walk(cx, &mut Scope::empty(), walk).step(){
                    if let Some(mut list) = list.as_portal_list().borrow_mut() {
                        list.set_item_range(cx, 0, doc.file.messages.len());
                        while let Some(item_id) = list.next_visible_item(cx) {
                            match doc.file.messages.get(item_id){
                                Some(AiChatMessage::Assistant(val))=>{
                                    let item = list.item(cx, item_id, live_id!(Assistant));
                                    // alright we got the assistant. lets set the markdown stuff
                                    item.widget(id!(md)).set_text(&val);
                                    item.view(id!(busy)).set_visible(doc.in_flight_request_id.is_some());
                                    item.draw_all_unscoped(cx);
                                }
                                Some(AiChatMessage::User(val))=>{
                                   // lets set the value to the text input
                                    let item = list.item(cx, item_id, live_id!(User));
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
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
        // we have an AI connection running on AppData
       /* let data = scope.data.get_mut::<AppData>().unwrap();
        // alright we can now access our AiChatManager object
        let chat_id = scope.path.from_end(1);
        if let Some(_chat_data) = data.ai_chat_manager.open_chats.get(&chat_id){
            // alright we have a chat_data..
        }*/
        /*
        if let Some(session) = data.file_system.get_session_mut(session_id){
            for action in self.editor.handle_event(cx, event, &mut Scope::empty(), session){
                cx.widget_action(uid, &scope.path, action);
            }
            data.file_system.handle_sessions();
        }*/
    }
}