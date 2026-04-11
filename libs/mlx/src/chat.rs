use crate::text_runtime::{
    GemmaExactMetalConfig, GemmaPromptFormat, GemmaTextGenerationOutput, GemmaTextModel,
    GemmaTextSamplingOptions, MlxTextSamplingRng,
};
use crate::MlxTokenizerConfig;
use std::error::Error;
use std::path::{Path, PathBuf};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GemmaChatDecodeMode {
    #[default]
    Sampled,
    Greedy,
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
    format_gemma4_chat_prompt_with_image(tokenizer_config, messages, false)
}

pub fn format_gemma4_chat_prompt_with_image(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[GemmaChatMessage],
    include_image_on_last_user_turn: bool,
) -> Result<String, Box<dyn Error>> {
    if messages.is_empty() {
        return Err("chat prompt requires at least one message".into());
    }

    let mut prompt = String::new();
    prompt.push_str(&tokenizer_config.bos_token);
    for (index, message) in messages.iter().enumerate() {
        prompt.push_str(&tokenizer_config.sot_token);
        prompt.push_str(message.role.as_prompt_label());
        prompt.push('\n');
        if include_image_on_last_user_turn
            && index + 1 == messages.len()
            && message.role == GemmaChatRole::User
        {
            prompt.push_str(&tokenizer_config.image_token);
            prompt.push(' ');
        }
        prompt.push_str(message.content.as_ref());
        prompt.push_str(&tokenizer_config.eot_token);
        prompt.push('\n');
    }
    prompt.push_str(&tokenizer_config.sot_token);
    prompt.push_str(GemmaChatRole::Assistant.as_prompt_label());
    prompt.push('\n');
    Ok(prompt)
}

pub fn format_plain_chat_prompt(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[GemmaChatMessage],
) -> Result<String, Box<dyn Error>> {
    format_plain_chat_prompt_with_image(tokenizer_config, messages, false)
}

pub fn format_plain_chat_prompt_with_image(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[GemmaChatMessage],
    include_image_on_last_user_turn: bool,
) -> Result<String, Box<dyn Error>> {
    if messages.is_empty() {
        return Err("chat prompt requires at least one message".into());
    }

    let mut prompt = String::new();
    prompt.push_str(&tokenizer_config.bos_token);
    for (index, message) in messages.iter().enumerate() {
        let role_label = match message.role {
            GemmaChatRole::User => "User",
            GemmaChatRole::Assistant => "Assistant",
        };
        prompt.push_str(role_label);
        prompt.push_str(": ");
        if include_image_on_last_user_turn
            && index + 1 == messages.len()
            && message.role == GemmaChatRole::User
        {
            prompt.push_str(&tokenizer_config.image_token);
            prompt.push(' ');
        }
        prompt.push_str(message.content.as_ref());
        prompt.push('\n');
    }
    prompt.push_str("Assistant:");
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
    decode_mode: GemmaChatDecodeMode,
    sampling_options: GemmaTextSamplingOptions,
    messages: Vec<GemmaChatMessage>,
    current_image_path: Option<PathBuf>,
}

impl GemmaChatSession {
    pub fn load(
        model_path: impl AsRef<Path>,
        max_new_tokens: Option<usize>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::load_with_mode(model_path, max_new_tokens, GemmaChatDecodeMode::Sampled)
    }

    pub fn load_with_mode(
        model_path: impl AsRef<Path>,
        max_new_tokens: Option<usize>,
        decode_mode: GemmaChatDecodeMode,
    ) -> Result<Self, Box<dyn Error>> {
        Self::load_with_mode_and_backend_config(
            model_path,
            max_new_tokens,
            decode_mode,
            GemmaExactMetalConfig::default(),
        )
    }

    pub fn load_with_mode_and_backend_config(
        model_path: impl AsRef<Path>,
        max_new_tokens: Option<usize>,
        decode_mode: GemmaChatDecodeMode,
        backend_config: GemmaExactMetalConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let model = GemmaTextModel::load_with_backend_config(model_path, backend_config)?;
        let sampling_options = match model.default_chat_prompt_format() {
            GemmaPromptFormat::Gemma4UserTurn => model.chat_sampling_options(),
            GemmaPromptFormat::AutoChat | GemmaPromptFormat::RawBos => {
                model.default_sampling_options()
            }
        };
        Ok(Self {
            model,
            max_new_tokens,
            rng: MlxTextSamplingRng::new(0),
            decode_mode,
            sampling_options,
            messages: Vec::new(),
            current_image_path: None,
        })
    }

    pub fn max_new_tokens(&self) -> Option<usize> {
        self.max_new_tokens
    }

    pub fn messages(&self) -> &[GemmaChatMessage] {
        &self.messages
    }

    pub fn decode_mode(&self) -> GemmaChatDecodeMode {
        self.decode_mode
    }

    pub fn backend_label(&self) -> &'static str {
        let sampling_options = match self.decode_mode {
            GemmaChatDecodeMode::Sampled => self.sampling_options.clone(),
            GemmaChatDecodeMode::Greedy => self.sampling_options.greedy_variant(),
        };
        if self.current_image_path.is_some() {
            self.model
                .multimodal_generation_backend_label(self.max_new_tokens, &sampling_options)
        } else {
            self.model
                .generation_backend_label(self.max_new_tokens, &sampling_options)
        }
    }

    pub fn reset(&mut self) {
        self.messages.clear();
    }

    pub fn set_image(&mut self, image_path: impl Into<PathBuf>) {
        self.current_image_path = Some(image_path.into());
    }

    pub fn clear_image(&mut self) {
        self.current_image_path = None;
    }

    pub fn current_image_path(&self) -> Option<&Path> {
        self.current_image_path.as_deref()
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
        let formatted_prompt = match self.model.default_chat_prompt_format() {
            GemmaPromptFormat::AutoChat => format_plain_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
            GemmaPromptFormat::Gemma4UserTurn => format_gemma4_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
            GemmaPromptFormat::RawBos => format_plain_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
        };
        let greedy_sampling_options = self.sampling_options.greedy_variant();
        let output = match (self.decode_mode, self.current_image_path.as_deref()) {
            (GemmaChatDecodeMode::Sampled, Some(image_path)) => self
                .model
                .generate_preformatted_multimodal_with_rng_and_sampling(
                    image_path,
                    formatted_prompt,
                    self.max_new_tokens,
                    &self.sampling_options,
                    &mut self.rng,
                )?,
            (GemmaChatDecodeMode::Sampled, None) => self
                .model
                .generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &self.sampling_options,
                    &mut self.rng,
                )?,
            (GemmaChatDecodeMode::Greedy, Some(image_path)) => self
                .model
                .generate_preformatted_multimodal_with_rng_and_sampling(
                    image_path,
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                )?,
            (GemmaChatDecodeMode::Greedy, None) => self
                .model
                .generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                )?,
        };
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
        let formatted_prompt = match self.model.default_chat_prompt_format() {
            GemmaPromptFormat::AutoChat => format_plain_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
            GemmaPromptFormat::Gemma4UserTurn => format_gemma4_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
            GemmaPromptFormat::RawBos => format_plain_chat_prompt_with_image(
                self.model.tokenizer_config(),
                &self.messages,
                self.current_image_path.is_some(),
            )?,
        };
        let greedy_sampling_options = self.sampling_options.greedy_variant();
        let output = match (self.decode_mode, self.current_image_path.as_deref()) {
            (GemmaChatDecodeMode::Sampled, Some(image_path)) => self
                .model
                .stream_generate_preformatted_multimodal_with_rng_and_sampling(
                    image_path,
                    formatted_prompt,
                    self.max_new_tokens,
                    &self.sampling_options,
                    &mut self.rng,
                    on_text_delta,
                )?,
            (GemmaChatDecodeMode::Sampled, None) => self
                .model
                .stream_generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &self.sampling_options,
                    &mut self.rng,
                    on_text_delta,
                )?,
            (GemmaChatDecodeMode::Greedy, Some(image_path)) => self
                .model
                .stream_generate_preformatted_multimodal_with_rng_and_sampling(
                    image_path,
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                    on_text_delta,
                )?,
            (GemmaChatDecodeMode::Greedy, None) => self
                .model
                .stream_generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                    on_text_delta,
                )?,
        };
        self.messages.push(GemmaChatMessage::new(
            GemmaChatRole::Assistant,
            output.generated_text.as_ref(),
        ));
        Ok(output)
    }
}
