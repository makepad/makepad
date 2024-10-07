use {
    self::super::open_ai_data::*,
    crate::{
        app::AppAction,
        file_system::file_system::{FileSystem,OpenDocument},
        makepad_widgets::*,
        makepad_micro_serde::*
    },
};

pub struct AiChatManager{
    pub backends: Vec<(String, AiBackend)>,
    pub contexts: Vec<BaseContext>,
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
                    "OpenAI gpt-4o-mini".to_string(),
                    AiBackend::OpenAI{
                        url: OPENAI_DEFAULT_URL.to_string(),
                        model: "gpt-4o-mini".to_string(),
                        key: std::fs::read_to_string("OPENAI_KEY").unwrap_or("".to_string())
                    }
                ),
                (
                    "Llama Local".to_string(),
                    AiBackend::OpenAI{
                        url:"http://127.0.0.1:8080/v1/chat/completions".to_string(),
                        model:"".to_string(),
                        key:"".to_string()
                    }
                ),
            ],
            contexts: vec![
                BaseContext{
                    name: "Empty".to_string(),
                    system: "You are a Rust programming assistant. Please answer with code examples only and very little explanation".to_string(),
                    files: vec![]
                },
                BaseContext{
                    name: "Makepad UI".to_string(),
                    system: "You are a Rust programming assistant for writing Makepad applications. You have been given 2 components and one example application. Please answer with code only and don't give explanations".to_string(),
                    files: vec![
                        "widgets/src/button.rs".to_string(),
                        "widgets/src/slider.rs".to_string(),
                        "examples/simple/src/app.rs".to_string(),
                    ]
                }
            ]
        }
    }
}

impl AiChatManager{
    pub fn model_strings(&self)->Vec<String>{
        let mut out = Vec::new();
        for backend in &self.backends{
            out.push(backend.0.clone())    
        }
        out
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

pub struct BaseContext{
    pub name: String,
    pub system: String,
    pub files: Vec<String>
}

#[derive(Debug, SerRon, DeRon, Clone)]
pub enum AiContext{
    Snippet{name:String, language:String, content:String},
    File{file_id:LiveId}
}

#[derive(Default, Debug, SerRon, DeRon, Clone)]
pub struct AiUserMessage{
    pub base_context: String,
    pub context: Vec<AiContext>,
    pub message:String
}

#[derive(Debug, SerRon, DeRon, Clone)]
pub enum AiChatMessage{
    User(AiUserMessage),
    Assistant(String)
}


#[derive(Debug, SerRon, DeRon, Clone)]
pub struct AiChatMessages{
    pub messages: Vec<AiChatMessage>
}

impl AiChatMessages{
    fn new()->Self{
        AiChatMessages{
            messages: vec![AiChatMessage::User(AiUserMessage::default())],
        }
    }
}

#[derive(Debug, SerRon, DeRon)]
pub struct AiChatFile{
    pub history: Vec<AiChatMessages>,
}

#[derive(Debug)]
pub struct AiInFlight{
    request_id: LiveId,
    history_slot: usize
}

#[derive(Debug)]
pub struct AiChatDocument{
    pub in_flight: Option<AiInFlight>,
    pub file: AiChatFile
}

impl AiChatDocument{
    pub fn load_or_empty(data: &str)->AiChatDocument{
        match AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e)){
            Err(e)=>{
                error!("Error parsing AiChatDocument {e}");
                Self{
                    in_flight: None,
                    file: AiChatFile::new()
                }
            }
            Ok(file)=>{
                Self{
                    in_flight: None,
                    file
                }
            }
        }
    }
}



impl AiChatFile{
    pub fn new()->Self{
        Self{
            history:vec![
                AiChatMessages::new()
            ],
        }
    }
    pub fn load(data: &str)->Result<AiChatFile,String>{
        AiChatFile::deserialize_ron(data).map_err(|e| format!("{:?}", e))
    }
    
    pub fn to_string(&self)->String{
        self.serialize_ron()
    }
    
    pub fn clamp_slot(&self, slot:&mut usize){
        *slot = self.history.len().saturating_sub(1).min(*slot);
    }
    
    pub fn remove_slot(&mut self,  _cx:&mut Cx, history_slot:&mut usize){
        self.clamp_slot(history_slot);
        self.history.remove(*history_slot);
        self.clamp_slot(history_slot);
        if self.history.len() == 0{
            self.history.push(AiChatMessages::new());
        }
    }
        // ok what happens. 
    pub fn fork_chat_at(&mut self, _cx:&mut Cx, history_slot:&mut usize, at:usize, data:String ) {
        // alriught so first we clamp the history slot
        self.clamp_slot(history_slot);
        if at + 1 != self.history[*history_slot].messages.len() { // fork it first
            let mut clone = self.history[*history_slot].clone();
            clone.messages.truncate(at + 1);
            *history_slot += 1;
            self.history.insert(*history_slot, clone);
        }
        if let AiChatMessage::User(s) = &mut self.history[*history_slot].messages[at]{
            s.message = data
        }
        else{
            error!("fork_chat_at: last message is not user")
        }
        // 
    }
    
    pub fn set_base_context(&mut self, history_slot:usize, at:usize, base_context:&str){ 
        if let AiChatMessage::User(s) = &mut self.history[history_slot].messages[at]{
            s.base_context = base_context.to_string()
        }
        else{panic!()}
    }
}


impl AiChatManager{
    pub fn init(&mut self, fs:&mut FileSystem) {
        // lets load up all our context files
        for ctx in &self.contexts{
            for file in &ctx.files{
                if let Some(file_id) = fs.path_to_file_node_id(&file){
                    fs.request_open_file(LiveId(0), file_id);
                }
                else{
                    println!("Cant find {} in context {}",file,ctx.name);
                }
            }
        }
    }
    
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, fs:&mut FileSystem) {
        // lets handle the 
        
        // alright. lets see if we have any incoming Http things
        match event{
            Event::NetworkResponses(e)=>for e in e{
                // lets check our in flight queries
                if let Some((chat_id,OpenDocument::AiChat(doc))) = fs.open_documents.iter_mut().find(
                    |(_,v)| if let OpenDocument::AiChat(v) = v {if let Some(v) = &v.in_flight{v.request_id == e.request_id}else{false}} else{false}){
                    let chat_id = *chat_id;
                    let in_flight = doc.in_flight.as_ref().unwrap();
                    match &e.response{
                        NetworkResponse::HttpRequestError(_err)=>{
                        }
                        NetworkResponse::HttpStreamResponse(res)=>{
                            let data = res.get_string_body().unwrap();
                            let mut changed = false;
                            for data in data.split("\n\n"){
                                if let Some(data) = data.strip_prefix("data: "){
                                    if data != "[DONE]"{
                                        match ChatResponse::deserialize_json(data){
                                            Ok(chat_response)=>{
                                                if let Some(content) = &chat_response.choices[0].delta.as_ref().unwrap().content{
                                                    if let Some(msg) = doc.file.history.get_mut(in_flight.history_slot){
                                                        if let Some(AiChatMessage::Assistant(s)) = msg.messages.last_mut(){
                                                            s.push_str(&content);
                                                        }
                                                        else{
                                                            msg.messages.push(AiChatMessage::Assistant(content.clone()))
                                                        }
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
                                cx.action(AppAction::RedrawAiChat{chat_id});
                                //fs.request_save_file_for_file_node_id(chat_id, false);
                            }
                        }
                        NetworkResponse::HttpStreamComplete(_res)=>{
                            // done?..
                           //let chat_id = res.metadata_id;
                            // alright lets fetch the chat object
                            if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
                                if let Some(in_flight) = doc.in_flight.take(){
                                    doc.in_flight = None;
                                    doc.file.history[in_flight.history_slot].messages.push(AiChatMessage::User(AiUserMessage::default()));
                                    cx.action(AppAction::RedrawAiChat{chat_id});
                                    cx.action(AppAction::SaveAiChat{chat_id});
                                    //self.redraw_ai_chat_by_id(cx, chat_id, ui, fs);
                                    //fs.request_save_file_for_file_node_id(chat_id, false);
                                }
                            }
                        }
                        _=>{}
                    }
                }
            }
            _=>()
        }
    }
    
    pub fn cancel_chat_generation(&mut self, cx:&mut Cx, ui: &WidgetRef, chat_id:LiveId, fs:&mut FileSystem) {
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            if let Some(in_flight) = doc.in_flight.take(){
                cx.cancel_http_request(in_flight.request_id);
                if let Some(msg) = doc.file.history.get_mut(in_flight.history_slot){
                    msg.messages.push(AiChatMessage::User(AiUserMessage::default()));
                    self.redraw_ai_chat_by_id(cx, chat_id, ui, fs);
                }
            }
        }
    }
            
    pub fn send_chat_to_backend(&mut self, cx: &mut Cx, chat_id:LiveId, backend_index:usize, history_slot:usize, fs:&mut FileSystem) {
        // build the request
        let request = if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get(&chat_id){
            match &self.backends[backend_index].1{
                AiBackend::OpenAI{url, model, key}=>{
                    
                    let mut request = HttpRequest::new(url.clone(), HttpMethod::POST);
                    request.set_is_streaming();
                    request.set_header("Authorization".to_string(), format!("Bearer {key}"));
                    request.set_header("Content-Type".to_string(), "application/json".to_string());
                    request.set_metadata_id(chat_id); 
                    let mut messages = Vec::new();
                    for msg in &doc.file.history[history_slot].messages{
                        match msg{
                            AiChatMessage::User(v)=>{
                                // alright. we have to now collect our base context files
                                // ok lets find these files
                                if let Some(ctx) = self.contexts.iter().find(|ctx| ctx.name == v.base_context){
                                    messages.push(ChatMessage {content: Some(ctx.system.clone()), role: Some("user".to_string()), refusal: Some(JsonValue::Null)});
                                    for file in &ctx.files{
                                        if let Some(file_id) = fs.path_to_file_node_id(file){
                                            if let Some(OpenDocument::Code(doc)) = fs.open_documents.get(&file_id){
                                                let mut content = String::new();
                                                let text = doc.as_text().to_string();
                                                content.push_str("\n File:");
                                                content.push_str(file);
                                                content.push_str("\n```rust\n");
                                                content.push_str(&text);
                                                content.push_str("```\n");
                                                 messages.push(ChatMessage {content: Some(content), role: Some("user".to_string()), refusal: Some(JsonValue::Null)})
                                            }
                                        }
                                    }
                                }
                                //content.push_str(&v.message);
                                //println!("{:?}", content);
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
                    request
                }
            }
        }
        else{
            panic!()
        };
        
        if let Some(OpenDocument::AiChat(doc)) = fs.open_documents.get_mut(&chat_id){
            let request_id = LiveId::unique();
            if let Some(in_flight) = doc.in_flight.take(){
                cx.cancel_http_request(in_flight.request_id);
            }
            doc.file.history[history_slot].messages.push(AiChatMessage::Assistant("".to_string()));
            doc.in_flight = Some(AiInFlight{
                history_slot,
                request_id
            });
            cx.http_request(request_id, request);
        }
    }
    
    pub fn redraw_ai_chat_by_id(&mut self, cx: &mut Cx, chat_id: LiveId, ui: &WidgetRef, fs:&mut FileSystem) {
        // lets fetch all the sessions
        let dock = ui.dock(id!(dock));
        for (tab_id, file_node_id) in &fs.tab_id_to_file_node_id{
            if *file_node_id == chat_id{
                dock.item(*tab_id).redraw(cx);
            }
        }
    }
}