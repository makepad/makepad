use crate::text_runtime::{
    GemmaTextGenerationOutput, GemmaTextModel, GemmaTextSamplingOptions, MlxTextSamplingRng,
};
use crate::MlxTokenizerConfig;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemmaChatRole {
    User,
    Assistant,
}

impl GemmaChatRole {
    pub fn as_prompt_label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "model",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GemmaChatMessage {
    pub role: GemmaChatRole,
    pub content: Arc<str>,
}

impl GemmaChatMessage {
    pub fn new(role: GemmaChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Arc::<str>::from(content.into()),
        }
    }
}

pub fn format_gemma4_chat_prompt(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[GemmaChatMessage],
) -> Result<String, Box<dyn Error>> {
    if messages.is_empty() {
        return Err("chat prompt requires at least one message".into());
    }

    let mut prompt = String::new();
    prompt.push_str(&tokenizer_config.bos_token);
    for message in messages {
        prompt.push_str(&tokenizer_config.sot_token);
        prompt.push_str(message.role.as_prompt_label());
        prompt.push('\n');
        prompt.push_str(message.content.as_ref());
        prompt.push_str(&tokenizer_config.eot_token);
        prompt.push('\n');
    }
    prompt.push_str(&tokenizer_config.sot_token);
    prompt.push_str(GemmaChatRole::Assistant.as_prompt_label());
    prompt.push('\n');
    prompt.push_str(&tokenizer_config.soc_token);
    prompt.push_str("thought\n");
    prompt.push_str(&tokenizer_config.eoc_token);
    Ok(prompt)
}

fn erase_all_substrings(text: &mut String, needle: &str) {
    if needle.is_empty() {
        return;
    }
    while let Some(pos) = text.find(needle) {
        text.replace_range(pos..pos + needle.len(), "");
    }
}

fn erase_all_spans(text: &mut String, start: &str, end: &str) {
    if start.is_empty() || end.is_empty() {
        return;
    }
    while let Some(start_pos) = text.find(start) {
        let after_start = start_pos + start.len();
        let Some(rel_end) = text[after_start..].find(end) else {
            text.truncate(start_pos);
            break;
        };
        let end_pos = after_start + rel_end + end.len();
        text.replace_range(start_pos..end_pos, "");
    }
}

pub fn extract_gemma4_assistant_response_text(
    tokenizer_config: &MlxTokenizerConfig,
    raw_text: &str,
) -> String {
    let mut text = raw_text.to_owned();
    if let Some(tool_call_start) = text.find(&tokenizer_config.stc_token) {
        text.truncate(tool_call_start);
    }
    erase_all_spans(&mut text, &tokenizer_config.soc_token, &tokenizer_config.eoc_token);

    erase_all_substrings(&mut text, &tokenizer_config.bos_token);
    erase_all_substrings(&mut text, &tokenizer_config.sot_token);
    erase_all_substrings(&mut text, &tokenizer_config.eot_token);
    erase_all_substrings(&mut text, &tokenizer_config.soc_token);
    erase_all_substrings(&mut text, &tokenizer_config.eoc_token);
    erase_all_substrings(&mut text, "[multimodal]");

    text.trim().to_owned()
}

#[derive(Clone)]
pub struct GemmaChatSession {
    model: GemmaTextModel,
    max_new_tokens: Option<usize>,
    rng: MlxTextSamplingRng,
    sampling_options: GemmaTextSamplingOptions,
    messages: Vec<GemmaChatMessage>,
}

impl GemmaChatSession {
    pub fn load(
        model_path: impl AsRef<Path>,
        max_new_tokens: Option<usize>,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            model: GemmaTextModel::load(model_path)?,
            max_new_tokens,
            rng: MlxTextSamplingRng::new(0),
            sampling_options: GemmaTextSamplingOptions::mlx_vlm_chat_default(),
            messages: Vec::new(),
        })
    }

    pub fn max_new_tokens(&self) -> Option<usize> {
        self.max_new_tokens
    }

    pub fn messages(&self) -> &[GemmaChatMessage] {
        &self.messages
    }

    pub fn reset(&mut self) {
        self.messages.clear();
    }

    pub fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::Assistant, content));
    }

    pub fn send_user_message(
        &mut self,
        content: impl Into<String>,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let content = content.into();
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::User, content));
        let formatted_prompt =
            format_gemma4_chat_prompt(self.model.tokenizer_config(), &self.messages)?;
        let output = self
            .model
            .generate_preformatted_with_rng_and_sampling(
                formatted_prompt,
                self.max_new_tokens,
                &self.sampling_options,
                &mut self.rng,
            )?;
        self.messages.push(GemmaChatMessage::new(
            GemmaChatRole::Assistant,
            output.generated_text.as_ref(),
        ));
        Ok(output)
    }

    pub fn send_user_message_streaming<F>(
        &mut self,
        content: impl Into<String>,
        on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let content = content.into();
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::User, content));
        let formatted_prompt =
            format_gemma4_chat_prompt(self.model.tokenizer_config(), &self.messages)?;
        let output = self
            .model
            .stream_generate_preformatted_with_rng_and_sampling(
                formatted_prompt,
                self.max_new_tokens,
                &self.sampling_options,
                &mut self.rng,
                on_text_delta,
            )?;
        self.messages.push(GemmaChatMessage::new(
            GemmaChatRole::Assistant,
            output.generated_text.as_ref(),
        ));
        Ok(output)
    }
}
