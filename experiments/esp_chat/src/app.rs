use crate::makepad_live_id::*;
use makepad_widgets::*;
   
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;
    import crate::app_ui::AppUI;
    App = {{App}} { 
        ui: <Root> {
            <Window> {
                window: {inner_size: vec2(800, 600)},
                caption_bar = {visible: true, caption_label = {label = {text: "Esp chat"}}},
                hide_caption_on_fullscreen: true,
                body = <AppUI>{}
            }
        }
    }
}

app_main!(App);

 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] llm_chat: Vec<(LLMMsg,String)>,
    
    #[rust] delay_timer: Timer,
}

enum LLMMsg{
    AI,
    Human,
    Progress
}

impl LiveRegister for App{
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::app_ui::live_design(cx);
    }
}
     
impl App {
   
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx:&mut Cx){
        
    }
    
    fn handle_signal(&mut self, _cx: &mut Cx){
        
    }
            
    fn handle_draw_2d(&mut self, cx:&mut Cx2d){
        let llm_chat = self.ui.portal_list(id!(llm_chat));
                
        while let Some(next) = self.ui.draw(cx, &mut Scope::empty()).step() {
           if let Some(mut llm_chat) = llm_chat.has_widget(&next).borrow_mut() {
                llm_chat.set_item_range(cx, 0, self.llm_chat.len());
                while let Some(item_id) = llm_chat.next_visible_item(cx) {
                    if item_id >= self.llm_chat.len(){
                        continue
                    }
                    let (is_llm, msg) = &self.llm_chat[item_id];
                    let template = match is_llm{
                        LLMMsg::AI=>live_id!(AI),
                        LLMMsg::Human=>live_id!(Human),
                        LLMMsg::Progress=>live_id!(AI)
                    };
                    let item = llm_chat.item(cx, item_id, template).unwrap();
                    item.set_text(msg);
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
    }
    
    
    fn handle_actions(&mut self, cx:&mut Cx, actions:&Actions){
        
        let chat = self.ui.text_input(id!(chat));
        if let Some(val) = chat.returned(&actions){
            chat.set_text_and_redraw(cx, "");
            chat.set_cursor(0,0);
            self.llm_chat.push((LLMMsg::Human, val));
            self.llm_chat.push((LLMMsg::Progress, "... Thinking ...".into()));
            self.ui.widget(id!(llm_chat)).redraw(cx);
        }
                  
        if self.ui.button(id!(trim_button)).clicked(&actions) {
            if self.llm_chat.len()>2{
                let last = self.llm_chat.len().max(2)-1;
                self.llm_chat.drain(2..last);
                self.ui.widget(id!(llm_chat)).redraw(cx);
            }
        }
        
        if self.ui.button(id!(clear_button)).clicked(&actions) {
            self.llm_chat.clear();
            self.ui.widget(id!(llm_chat)).redraw(cx);
        }    
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.match_event_with_draw_2d(cx, event).is_ok(){
            return
        }
        
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
