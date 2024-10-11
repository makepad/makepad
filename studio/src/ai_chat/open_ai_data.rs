use crate::makepad_micro_serde::*;

#[derive(Debug, SerJson, DeJson)]
pub struct ChatPrompt {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    pub max_tokens: i32,
    pub stream: bool
}

#[derive(Debug, SerJson, DeJson)]
pub struct ChatMessage {
    pub content: Option<String>,
    pub role: Option<String>,
    pub refusal: Option<JsonValue>
} 

#[allow(unused)]
#[derive(Debug, DeJson)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i32,
    pub model: String,
    pub system_fingerprint: Option<JsonValue>,
    pub usage: Option<ChatUsage>,
    pub choices: Vec<ChatChoice>,
}

#[allow(unused)]
#[derive(Debug, DeJson)]
pub struct CompletionDetails {
    pub reasoning_tokens: i32,
}

#[allow(unused)]
#[derive(Debug, DeJson)]
pub struct ChatUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
    pub completion_tokens_details: CompletionDetails
}

#[allow(unused)]
#[derive(Debug, DeJson)]
pub struct ChatChoice {
    pub message: Option<ChatMessage>,
    pub delta: Option<ChatMessage>,
    pub finish_reason: Option<String>,
    pub logprobs: Option<JsonValue>,
    pub index: i32,
}
/*
#[derive(SerJson, DeJson)]
pub struct LLamaCppQuery {
    pub stream: bool,
    pub repeat_last_n: i32,
    pub repeat_penalty:f32,
    pub top_k:f32,
    pub top_p:f32,
    pub min_p:f32,
    pub tfs_z:f32,
    pub n_predict:i32,
    pub temperature:f32,
    pub stop: Vec<i32>,
    pub typical_p: f32,
    pub presence_penalty: f32,
    pub frequency_penalty: f32,
    pub mirostat:f32,
    pub mirostat_tau:f32,
    pub mirostat_eta:f32,
    pub grammar:String,
    pub n_probs:i32,
    pub min_keep:f32,
    pub image_data:Vec<i32>,
    pub cache_prompt:bool,
    pub api_key:String,
    pub slot_id:i32,
    pub prompt: String,
}

#[derive(SerJson, DeJson)]
pub struct LLamaCppStream {
    pub content: String,
    pub stop: bool,
    pub id_slot: u32,
    pub multimodal: bool,
    pub index: i32,
}
AiBackend::LlamaLocal=>{
    for data in data.split("\n\n"){
        if let Some(data) = data.strip_prefix("data: "){
            if data != "[DONE]"{
                match LLamaCppStream::deserialize_json(data){
                    Ok(chat_response)=>{
                        if let Some(AiChatMessage::Ai(s)) = self.chat.last_mut(){
                            s.push_str(&chat_response.content);
                        }
                        else{
                            self.chat.push(AiChatMessage::Ai(chat_response.content))
                        }
                        // triggre a save to disk as well
                        changed = true;
                    }
                    Err(e)=>{
                        println!("JSon parse error {:?} {}", e, data);
                    }
                }
            }
        }
    }
}
AiBackend::LlamaLocal=>{
    let url = format!("{}/completion", self.config.llama_url);
    let mut request = HttpRequest::new(url, HttpMethod::POST);
    request.set_is_streaming();
    request.set_header("Content-Type".to_string(), "application/json".to_string());
    request.set_metadata_id(chat_id); 
    let mut prompt = String::new();
    prompt.push_str("<|begin_of_text|>\n");
    prompt.push_str("<|start_header_id|>system<|stop_header_id|>");
    prompt.push_str("You are a helpful programming assitant<|eot_id|>");
    for msg in &doc.chat{
        match msg{
            AiChatMessage::User(v)=>{
                prompt.push_str("<|start_header_id|>user<|end_header_id|>");
                prompt.push_str(v);
                prompt.push_str("<|eot_id|>");
            }
            AiChatMessage::Ai(v)=>{
                prompt.push_str("<|start_header_id|>assistant<|end_header_id|>");
                prompt.push_str(v);
                prompt.push_str("<|eot_id|>");
            }
        }
    }
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
    cx.http_request(request_id, request);
}*/