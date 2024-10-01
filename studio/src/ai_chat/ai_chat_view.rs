
use {
    crate::{
        app::{AppData},
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
                code_block = <CodeView>{}
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
        if self.button(id!(send_button)).clicked(actions) || 
        self.text_input(id!(message_input)).returned(actions).is_some()
        {
            let user_prompt = self.text_input(id!(message_input)).text();
            // alright we got a user prompt. lets send it
             data.ai_chat_manager.send_message(cx, chat_id, user_prompt);
        }
        if self.button(id!(clear_button)).clicked(actions){
            data.ai_chat_manager.clear_chat(chat_id);
            self.redraw(cx);
        }
    }
}
impl Widget for AiChatView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk:Walk)->DrawStep{
        // alright we have a scope, and an id, so now we can properly draw the editor.
        // alright lets fetch the chat-id from the scope
        let data = scope.data.get_mut::<AppData>().unwrap();
        let chat_id = scope.path.from_end(0);
        if let Some(chat_data) = data.ai_chat_manager.open_chats.get(&chat_id){
            // alright we have a chat_data.. now what.
            // now we need to update the text on the markdown object
            self.markdown(id!(md)).set_text(&chat_data.chat);
        }
        
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
        let data = scope.data.get_mut::<AppData>().unwrap();
        // alright we can now access our AiChatManager object
        let chat_id = scope.path.from_end(1);
        if let Some(_chat_data) = data.ai_chat_manager.open_chats.get(&chat_id){
            // alright we have a chat_data..
            
        }
        /*
        if let Some(session) = data.file_system.get_session_mut(session_id){
            for action in self.editor.handle_event(cx, event, &mut Scope::empty(), session){
                cx.widget_action(uid, &scope.path, action);
            }
            data.file_system.handle_sessions();
        }*/
    }
}