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

const USE_OPEN_AI: bool = false;
const LLAMA_CPP_BASE_URL: &str = "http://127.0.0.1:8080";
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
                            if USE_OPEN_AI{
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
                            else{
                                match LLamaCppStream::deserialize_json(data){
                                    Ok(chat_response)=>{
                                        if let Some(chat_data) = self.open_chats.get_mut(&chat_id){
                                            chat_data.chat.push_str(&chat_response.content);
                                            // alright lets redraw the UI
                                            self.redraw_ai_chat_by_id(cx, chat_id, ui)
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
        
        let completion_url = if USE_OPEN_AI{
            format!("{}/chat/completions", OPENAI_BASE_URL)
        }
        else{
            format!("{}/completion", LLAMA_CPP_BASE_URL)
        };
        let request_id = live_id!(AiChatMessagae);
        let mut request = HttpRequest::new(completion_url, HttpMethod::POST);
        request.set_is_streaming();
        
        let ai_key = std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string());
        request.set_header("Authorization".to_string(), format!("Bearer {ai_key}"));
        request.set_header("Content-Type".to_string(), "application/json".to_string());
        request.set_metadata_id(chat_id); 
        if USE_OPEN_AI{
            request.set_json_body(ChatPrompt {
                messages: vec![ChatMessage {content: Some(message), role: Some("user".to_string()), refusal: Some(JsonValue::Null)}],
                model: "gpt-4o".to_string(),
                max_tokens: 1000,
                stream: true,
            });
        }
        else{
            request.set_json_body(LLamaCppQuery{
                stream: true,
                n_predict:400,
                temperature:0.7,
                stop:vec![],
                repeat_last_n:256,
                repeat_penalty:1.18,
                top_k:40.0,
                top_p:0.95,
                min_p:0.05,
                tfs_z:1.0,
                typical_p:1.0,
                presence_penalty:0.0,
                frequency_penalty:0.0,
                mirostat:0.0,
                mirostat_tau:5.0,
                mirostat_eta:0.1,
                grammar:"".to_string(),
                n_probs:0,
                min_keep:0.0,
                image_data:vec![],
                cache_prompt:true,
                api_key:"".to_string(),
                slot_id:-1,
                prompt: message.to_string()
            })
        }
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
        