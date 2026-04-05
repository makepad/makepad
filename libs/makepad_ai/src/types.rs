/// Role in a conversation
#[derive(Clone, Debug, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(MessageRole::System),
            "user" => Some(MessageRole::User),
            "assistant" => Some(MessageRole::Assistant),
            "tool" => Some(MessageRole::Tool),
            "model" => Some(MessageRole::Assistant), // Gemini uses "model"
            _ => None,
        }
    }
}

/// Content block - supports text, images, tool calls
#[derive(Clone, Debug)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// A message in the conversation
#[derive(Clone, Debug)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn assistant(text: &str) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn system(text: &str) -> Self {
        Self {
            role: MessageRole::System,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Tool definition for function calling
#[derive(Clone, Debug)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: String,
}

/// Request to send to AI backend
#[derive(Clone, Debug)]
pub struct AiRequest {
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub tools: Vec<ToolDefinition>,
    pub stream: bool,
}

impl Default for AiRequest {
    fn default() -> Self {
        Self {
            messages: vec![],
            system_prompt: None,
            max_tokens: 4096,
            temperature: None,
            tools: vec![],
            stream: true,
        }
    }
}

/// Streaming delta from AI
#[derive(Clone, Debug)]
pub enum StreamDelta {
    TextDelta {
        text: String,
    },
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolUseDelta {
        partial_json: String,
    },
    ToolUseEnd,
    Done {
        stop_reason: StopReason,
        usage: Option<Usage>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default)]
pub enum StopReason {
    #[default]
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Complete response (non-streaming)
#[derive(Clone, Debug)]
pub struct AiResponse {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
}
