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