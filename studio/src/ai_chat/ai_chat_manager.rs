use {
    std::collections::{HashMap},
    self::super::open_ai_data::*,
    crate::{
        makepad_widgets::*,
        makepad_micro_serde::*
    },
};

#[derive(Default)]
pub struct AiChatData{
    pub chat: String,
}

#[derive(Default)]
pub struct AiChatManager{
    pub open_chats: HashMap<LiveId, AiChatData>
}

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

impl AiChatManager{
    fn handle_network_response(&mut self, cx: &mut Cx, e:&NetworkResponse, ui: &WidgetRef){
        match &e{
            NetworkResponse::HttpRequestError(_err)=>{
            }
            NetworkResponse::HttpStreamResponse(res)=>{
                // alright we have a http stream response for a certain request id
                let chat_id = res.metadata_id;
                let data = res.get_string_body().unwrap();
                for data in data.split("\n\n"){
                    if let Some(data) = data.strip_prefix("data: "){
                        if data != "[DONE]"{
                            match ChatResponse::deserialize_json(data){
                                Ok(chat_response)=>{
                                    if let Some(content) = &chat_response.choices[0].delta.as_ref().unwrap().content{
                                        if let Some(chat_data) = self.open_chats.get_mut(&chat_id){
                                            chat_data.chat.push_str(&content);
                                            // alright lets redraw the UI
                                            self.redraw_ai_chat_by_id(cx, chat_id, ui)
                                        }
                                    }
                                }
                                Err(e)=>{
                                    println!("JSon parse error {:?} {}", e, data);
                                }
                            }
                        }
                    }
                }
            }
            NetworkResponse::HttpStreamComplete=>{
            }
            _=>{}
        }
    }    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef) {
        // alright. lets see if we have any incoming Http things
        match event{
            Event::NetworkResponses(e)=>for e in e{
                if e.request_id == live_id!(AiChatMessagae){
                    self.handle_network_response(cx, &e.response, ui)
                }
            }
            _=>()
        }
    }
    
    pub fn send_message(&mut self, cx: &mut Cx, chat_id:LiveId, message: String) {
        if self.open_chats.get(&chat_id).is_none(){
            self.open_chats.insert(chat_id, AiChatData::default());
        }
        let completion_url = format!("{}/chat/completions", OPENAI_BASE_URL);
        let request_id = live_id!(AiChatMessagae);
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        request.set_is_streaming();
        let ai_key = std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string());
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_header("Authorization".to_string(), format!("Bearer {ai_key}"));
        request.set_metadata_id(chat_id);        
        request.set_json_body(ChatPrompt {
            messages: vec![ChatMessage {content: Some(message), role: Some("user".to_string()), refusal: Some(JsonValue::Null)}],
            model: "gpt-4o".to_string(),
            max_tokens: 1000,
            stream: true,
        });
        //self.ui.label(id!(message_label)).set_text_and_redraw(cx, "Answering:..\n");
        cx.http_request(request_id, request);
    }
    
    pub fn clear_chat(&mut self, chat_id:LiveId) {
        if let Some(chat) = self.open_chats.get_mut(&chat_id){
            chat.chat.clear();
        }
    }
    
    pub fn redraw_ai_chat_by_id(&mut self, cx: &mut Cx, chat_id: LiveId, ui: &WidgetRef) {
                
        let dock = ui.dock(id!(dock));
        dock.item(chat_id).redraw(cx)
    }
}
        