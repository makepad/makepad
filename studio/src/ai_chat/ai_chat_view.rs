
use {
    crate::{
        app::{AppData},
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
    import makepad_widgets::theme_desktop_dark::*;
    
    AiChatView = {{AiChatView}}{
        height: Fill, width: Fill,
        flow: Down
        spacing: 3
        padding:{top:4}
        <View>{
            height: Fit
            message_input = <TextInput> {
                text: ""
                empty_message:"Chat here"
                width: Fill,
                height: Fit,
                draw_bg: {
                    color: #1
                }
            }
                                                                                    
            send_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "send"
            }
            clear_button = <Button> {
                icon_walk: {margin: {left: 10}, width: 16, height: Fit}
                text: "X"
            }
        }
         <ScrollXYView>{
             show_bg: true,
             draw_bg:{color:#3},
             height:Fill
             padding: 3,
             md = <Markdown>{
                code_block = <CodeView>{
                    editor:{
                        draw_bg: { color: (#3) }
                    }
                }
                use_code_block_widget: true,
                body:""
            }
        } 
        
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct AiChatView{
    #[deref] view:View
}
impl WidgetMatchEvent for AiChatView{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions, scope: &mut Scope){
        let data = scope.data.get_mut::<AppData>().unwrap();
        let chat_id = scope.path.from_end(0);
        // someone pressed send / return
        // alright what do we do when we have a request running?
        // we should cancel it first
        if self.button(id!(send_button)).clicked(actions) || 
        self.text_input(id!(message_input)).returned(actions).is_some()
        {
            let message = self.text_input(id!(message_input)).text();
            // alright so. what happens
            // we can 
            // alright we got a user prompt. lets set it as the first message and send it
            
            data.ai_chat_manager.clear_messages(chat_id, &mut data.file_system);
            data.ai_chat_manager.add_user_message(chat_id, AiUserMessage{message, context:vec![]}, &mut data.file_system);
            data.ai_chat_manager.send_chat_to_backend(cx, chat_id, 0, &mut data.file_system);
        }
        if self.button(id!(clear_button)).clicked(actions){
            data.ai_chat_manager.clear_messages(chat_id, &mut data.file_system);
            self.redraw(cx);
        }
    }
}
impl Widget for AiChatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk:Walk)->DrawStep{
        // alright we have a scope, and an id, so now we can properly draw the editor.
        // alright lets fetch the chat-id from the scope
        let data = scope.data.get_mut::<AppData>().unwrap();
        let session_id = scope.path.from_end(0);
        // lets fetch the document id from our session id
        
        // lets use a custom draw flow on the markdown widget
        // we have our 'input' widget which gets cloned everywhere there is a 'user' field
        // this thing is a combination of a context-selector and a text input
        // i think maybe what we need is a flat list
        
        // it is a direct translation of the chat datastructure. Markdown instances for the AI answer
        // and 'input' instances with a context editor for the user input
        
        if let Some(EditSession::AiChat(chat_id)) = data.file_system.get_session_mut(session_id){
            let chat_id = *chat_id;
            if let Some(OpenDocument::AiChat(doc)) = data.file_system.open_documents.get(&chat_id){
                if let Some(AiChatMessage::Assistant(val)) = doc.file.messages.get(1){
                    self.markdown(id!(md)).set_text(&val);
                }
            }
        }
        // we should hook the markdown flow to be 'custom drawn'
        
        self.view.draw_all_unscoped(cx);
        /*
        let session_id = scope.path.from_end(1);
        let app_scope = scope.data.get_mut::<AppData>().unwrap();
        if let Some(session) = app_scope.file_system.get_session_mut(session_id){
            self.editor.draw_walk_editor(cx, session, walk);
        }*/
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