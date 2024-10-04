use {
    self::super::open_ai_data::*,
    crate::{
        file_system::file_system::{FileSystem,OpenDocument},
        makepad_widgets::*,
        makepad_micro_serde::*
    },
};

pub struct AiChatManager{
    pub backends: Vec<(String, AiBackend)>,
}
const OPENAI_DEFAULT_URL: &'static str = "https://api.openai.com/v1/chat/completions";

impl Default for AiChatManager{
    fn default()->Self{
        Self{
            backends: vec![
                (
                    "OpenAI gpt-4o".to_string(),
                    AiBackend::OpenAI{
                        url: OPENAI_DEFAULT_URL.to_string(),
                        model: "gpt-4o".to_string(),
                        key: std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string())
                    }
                ),
                (
                    "Llama CPP Local".to_string(),
                    AiBackend::OpenAI{
                        url:"http://127.0.0.1:8080/v1/chat/completions".to_string(),
                        model:"".to_string(),
                        key:"".to_string()
                    }
                ),
            ]
        }
    }
}

#[derive(Debug, SerRon, DeRon)]
pub enum AiBackend{
    OpenAI{
        url:String, 
        model:String,
        key: String,
    }
}

#[derive(Debug, SerRon, DeRon)]
pub struct AiContextFile{
    pub file_id: LiveId,
    pub name: String,
    pub contents: String,
}

#[derive(Default, Debug, SerRon, DeRon)]
pub struct AiUserMessage{
    pub context: Vec<AiContextFile>,
    pub message:String
}

#[derive(Debug, SerRon, DeRon)]
pub enum AiChatMessage{
    User(AiUserMessage),
    Assistant(String)
}

#[derive(Debug, Default, SerRon, DeRon)]
pub struct AiChatFile{
    pub messages: Vec<AiChatMessage>,
}

#[derive(Debug, SerRon, DeRon)]
pub struct AiChatDocument{
    in_flight_request_id: Option<LiveId>,
    pub file: AiChatFile
}

impl AiChatDocument{
    pub fn load_or_empty(data: &str)->AiChatDocument{
        match AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e)){
            Err(e)=>{
                error!("Error parsing AiChatDocument {e}");
                Self{
                    in_flight_request_id: None,
                    file: AiChatFile::default()
                }
            }
            Ok(file)=>{
                Self{
                    in_flight_request_id: None,
                    file
                }
            }
        }
    }
}

impl AiChatFile{
    pub fn new()->Self{
        Self{
            messages: vec![]
        }
    }
    
    pub fn load(data: &str)->Result<AiChatFile,String>{
        AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e))
    }
    
    pub fn to_string(&self)->String{
        self.serialize_ron()
    }
}


impl AiChatManager{
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, ui: &WidgetRef, fs:&mut FileSystem) {
        // lets handle the 
        
        // alright. lets see if we have any incoming Http things
        match event{
            Event::NetworkResponses(e)=>for e in e{
                // lets check our in flight queries
                if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.values_mut().find(|v| if let OpenDocument::AiChat(v) = v {v.in_flight_request_id == Some(e.request_id)} else{false}){
                    match &e.response{
                        NetworkResponse::HttpRequestError(_err)=>{
                        }
                        NetworkResponse::HttpStreamResponse(res)=>{
                            // alright we have a http stream response for a certain request id
                            let chat_id = res.metadata_id;
                            // alright lets fetch the chat object
                            let data = res.get_string_body().unwrap();
                            let mut changed = false;
                            for data in data.split("\n\n"){
                                if let Some(data) = data.strip_prefix("data: "){
                                    if data != "[DONE]"{
                                        match ChatResponse::deserialize_json(data){
                                            Ok(chat_response)=>{
                                                if let Some(content) = &chat_response.choices[0].delta.as_ref().unwrap().content{
                                                    if let Some(AiChatMessage::Assistant(s)) = doc.file.messages.last_mut(){
                                                        s.push_str(&content);
                                                    }
                                                    else{
                                                        doc.file.messages.push(AiChatMessage::Assistant(content.clone()))
                                                    }
                                                    changed = true;
                                                }
                                            }
                                            Err(e)=>{
                                                println!("JSon parse error {:?} {}", e, data);
                                            }
                                        }
                                    }
                                }
                            }
                            if changed{
                                self.redraw_ai_chat_by_id(cx, chat_id, ui);
                                fs.request_save_file_for_file_node_id(chat_id, false);
                            }
                        }
                        NetworkResponse::HttpStreamComplete(res)=>{
                            // done?..
                            let chat_id = res.metadata_id;
                            // alright lets fetch the chat object
                            if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
                                doc.in_flight_request_id = None;
                                doc.file.messages.push(AiChatMessage::User(AiUserMessage::default()));
                                self.redraw_ai_chat_by_id(cx, chat_id, ui);
                                fs.request_save_file_for_file_node_id(chat_id, false);
                            }
                        }
                        _=>{}
                    }
                }
            }
            _=>()
        }
    }
    
    pub fn set_chat_len(&mut self, chat_id:LiveId, new_len:usize, fs:&mut FileSystem) {
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            
            doc.file.messages.truncate(new_len);
        }
    }
    
    pub fn cancel_chat_generation(&mut self, cx:&mut Cx, chat_id:LiveId, fs:&mut FileSystem) {
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            if let Some(in_flight) = doc.in_flight_request_id{
                cx.cancel_http_request(in_flight);
            }
        }
    }
            
    pub fn send_chat_to_backend(&mut self, cx: &mut Cx, chat_id:LiveId, backend_index:usize, fs:&mut FileSystem) {
        // alright so what hapepns here
        // per backend we have a path
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            if let Some(in_flight) = doc.in_flight_request_id.take(){
                cx.cancel_http_request(in_flight);
            }
            // alright. lets append the user message
            /*doc.messages.push(AiChatMessage::User{
                message
            });*/
            match &self.backends[backend_index].1{
                AiBackend::OpenAI{url, model, key}=>{
                    let request_id = LiveId::unique();
                    let mut request = HttpRequest::new(url.clone(), HttpMethod::POST);
                    request.set_is_streaming();
                    request.set_header("Authorization".to_string(), format!("Bearer {key}"));
                    request.set_header("Content-Type".to_string(), "application/json".to_string());
                    request.set_metadata_id(chat_id); 
                    let mut messages = Vec::new();
                    for msg in &doc.file.messages{
                        match msg{
                            AiChatMessage::User(v)=>{
                                messages.push(ChatMessage {content: Some(v.message.clone()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)})
                            }
                            AiChatMessage::Assistant(v)=>{
                                messages.push(ChatMessage {content: Some(v.clone()), role: Some("assistant".to_string()), refusal: Some(JsonValue::Null)})
                            }
                        }
                    }
                    request.set_json_body(ChatPrompt {
                        messages,
                        model: model.to_string(),
                        max_tokens: 1000,
                        stream: true,
                    });
                    doc.in_flight_request_id = Some(request_id);
                    cx.http_request(request_id, request);
                }
            }
        }
    }
    
    pub fn redraw_ai_chat_by_id(&mut self, cx: &mut Cx, chat_id: LiveId, ui: &WidgetRef) {
        let dock = ui.dock(id!(dock));
        dock.item(chat_id).redraw(cx)
    }
}