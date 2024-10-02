use crate::makepad_micro_serde::*;

#[derive(SerJson, DeJson)]
pub struct ChatPrompt {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    pub max_tokens: i32,
    pub stream: bool
}

#[derive(SerJson, DeJson)]
pub struct ChatMessage {
    pub content: Option<String>,
    pub role: Option<String>,
    pub refusal: Option<JsonValue>
} 

#[allow(unused)]
#[derive(DeJson)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i32,
    pub model: String,
    pub system_fingerprint: JsonValue,
    pub usage: Option<ChatUsage>,
    pub choices: Vec<ChatChoice>,
}

#[allow(unused)]
#[derive(DeJson)]
pub struct CompletionDetails {
    pub reasoning_tokens: i32,
}

#[allow(unused)]
#[derive(DeJson)]
pub struct ChatUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
    pub completion_tokens_details: CompletionDetails
}

#[allow(unused)]
#[derive(DeJson)]
pub struct ChatChoice {
    pub message: Option<ChatMessage>,
    pub delta: Option<ChatMessage>,
    pub finish_reason: Option<String>,
    pub logprobs: JsonValue,
    pub index: i32,
}

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
