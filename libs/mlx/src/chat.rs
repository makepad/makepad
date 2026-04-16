use crate::text_runtime::{
    GemmaPromptFormat, GemmaTextBackendConfig, GemmaTextGenerationOutput, GemmaTextModel,
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
    erase_all_spans(
        &mut text,
        &tokenizer_config.soc_token,
        &tokenizer_config.eoc_token,
    );

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
    incremental_prompt_text: Option<String>,
    incremental_prompt_token_ids: Option<Vec<u32>>,
}

impl GemmaChatSession {
    fn active_sampling_options(&self) -> GemmaTextSamplingOptions {
        match self.decode_mode {
            GemmaChatDecodeMode::Sampled => self.sampling_options.clone(),
            GemmaChatDecodeMode::Greedy => self.sampling_options.greedy_variant(),
        }
    }

    fn incremental_chat_sampling_options(&self) -> GemmaTextSamplingOptions {
        let mut options = self.active_sampling_options();
        // The incremental exact path is still sensitive around Gemma4's empty
        // thought prefix for short prompts like "write a poem". Let the model
        // emit its control channel naturally and strip that span from the
        // user-visible output.
        options.allow_thought = true;
        options
    }

    fn prune_oldest_turn(messages: &mut Vec<GemmaChatMessage>) -> bool {
        if messages.is_empty() {
            return false;
        }
        let remove_count = if messages.len() >= 2
            && messages[0].role == GemmaChatRole::User
            && messages[1].role == GemmaChatRole::Assistant
        {
            2
        } else {
            1
        };
        messages.drain(0..remove_count.min(messages.len()));
        true
    }

    fn clear_incremental_prompt_cache(&mut self) {
        self.incremental_prompt_text = None;
        self.incremental_prompt_token_ids = None;
    }

    fn uses_incremental_greedy_chat_path(&self) -> bool {
        self.current_image_path.is_none()
            && self.decode_mode == GemmaChatDecodeMode::Greedy
            && self.max_new_tokens.is_some()
            && self.model.uses_incremental_greedy_backend(
                self.max_new_tokens,
                &self.active_sampling_options(),
            )
    }

    fn format_incremental_chat_turn_suffix(
        tokenizer_config: &MlxTokenizerConfig,
        prompt_format: GemmaPromptFormat,
        content: &str,
        include_previous_assistant_terminator: bool,
    ) -> String {
        let mut prompt = String::new();
        match prompt_format {
            GemmaPromptFormat::Gemma4UserTurn => {
                if include_previous_assistant_terminator {
                    prompt.push_str(&tokenizer_config.eot_token);
                    prompt.push('\n');
                } else {
                    prompt.push_str(&tokenizer_config.bos_token);
                }
                prompt.push_str(&tokenizer_config.sot_token);
                prompt.push_str(GemmaChatRole::User.as_prompt_label());
                prompt.push('\n');
                prompt.push_str(content);
                prompt.push_str(&tokenizer_config.eot_token);
                prompt.push('\n');
                prompt.push_str(&tokenizer_config.sot_token);
                prompt.push_str(GemmaChatRole::Assistant.as_prompt_label());
                prompt.push('\n');
            }
            GemmaPromptFormat::AutoChat | GemmaPromptFormat::RawBos => {
                if include_previous_assistant_terminator {
                    prompt.push('\n');
                } else {
                    prompt.push_str(&tokenizer_config.bos_token);
                }
                prompt.push_str("User: ");
                prompt.push_str(content);
                prompt.push('\n');
                prompt.push_str("Assistant:");
            }
        }
        prompt
    }

    fn format_current_generation_prompt_untrimmed(&self) -> Result<String, Box<dyn Error>> {
        let prompt_format = self.model.default_chat_prompt_format();
        match prompt_format {
            GemmaPromptFormat::AutoChat => {
                format_plain_chat_prompt(self.model.tokenizer_config(), &self.messages)
            }
            GemmaPromptFormat::Gemma4UserTurn => {
                format_gemma4_chat_prompt(self.model.tokenizer_config(), &self.messages)
            }
            GemmaPromptFormat::RawBos => {
                format_plain_chat_prompt(self.model.tokenizer_config(), &self.messages)
            }
        }
    }

    fn incremental_window_prompt(
        &self,
        prompt_text: String,
        prompt_token_ids: Vec<u32>,
        prompt_token_limit: usize,
        reusable_suffix_token_count: Option<usize>,
    ) -> Result<(Arc<str>, Arc<[u32]>, usize), Box<dyn Error>> {
        if prompt_token_ids.len() <= prompt_token_limit {
            let prefill_token_count =
                reusable_suffix_token_count.unwrap_or(prompt_token_ids.len());
            return Ok((
                Arc::<str>::from(prompt_text),
                Arc::<[u32]>::from(prompt_token_ids),
                prefill_token_count,
            ));
        }
        if prompt_token_limit == 0 {
            return Err("incremental exact chat prompt token limit is zero".into());
        }

        let keep_start = prompt_token_ids.len() - prompt_token_limit;
        let windowed_token_ids = prompt_token_ids[keep_start..].to_vec();
        let windowed_prompt_text = self
            .model
            .decode_token_ids(&windowed_token_ids)
            .map_err(|err| err.to_string())?;
        Ok((
            Arc::<str>::from(windowed_prompt_text),
            Arc::<[u32]>::from(windowed_token_ids),
            prompt_token_limit,
        ))
    }

    fn prepare_incremental_generation_prompt(
        &mut self,
        user_content: &str,
    ) -> Result<(Arc<str>, Arc<[u32]>, usize), Box<dyn Error>> {
        let prompt_format = self.model.default_chat_prompt_format();
        let Some(total_limit) = self.model.exact_greedy_supported_total_tokens(
            self.max_new_tokens,
            &self.active_sampling_options(),
        ) else {
            return Err("missing exact greedy token limit".into());
        };
        let max_new_tokens = self.max_new_tokens.unwrap_or(0);
        if max_new_tokens >= total_limit {
            return Err(format!(
                "max_new_tokens {} exceeds exact greedy token limit {}",
                max_new_tokens, total_limit
            )
            .into());
        }
        let prompt_token_limit = total_limit - max_new_tokens;

        if let (Some(raw_prompt_text), Some(raw_prompt_token_ids)) = (
            self.incremental_prompt_text.as_ref(),
            self.incremental_prompt_token_ids.as_ref(),
        ) {
            let suffix_text = Self::format_incremental_chat_turn_suffix(
                self.model.tokenizer_config(),
                prompt_format,
                user_content,
                true,
            );
            let suffix_token_ids = self.model.tokenize_formatted_prompt(&suffix_text)?;
            let next_prompt_token_count = raw_prompt_token_ids
                .len()
                .checked_add(suffix_token_ids.len())
                .ok_or("CUDA chat prompt token count overflow")?;
            let mut next_prompt_text =
                String::with_capacity(raw_prompt_text.len() + suffix_text.len());
            next_prompt_text.push_str(raw_prompt_text);
            next_prompt_text.push_str(&suffix_text);
            let mut next_prompt_token_ids = Vec::with_capacity(next_prompt_token_count);
            next_prompt_token_ids.extend_from_slice(raw_prompt_token_ids);
            next_prompt_token_ids.extend_from_slice(suffix_token_ids.as_ref());
            return self.incremental_window_prompt(
                next_prompt_text,
                next_prompt_token_ids,
                prompt_token_limit,
                Some(suffix_token_ids.len()),
            );
        }

        self.clear_incremental_prompt_cache();
        let formatted_prompt = self.format_current_generation_prompt_untrimmed()?;
        let prompt_token_ids = self.model.tokenize_formatted_prompt(&formatted_prompt)?;
        self.incremental_window_prompt(
            formatted_prompt,
            prompt_token_ids.to_vec(),
            prompt_token_limit,
            None,
        )
    }

    fn finish_incremental_generation(
        &mut self,
        prompt_text: &Arc<str>,
        prompt_token_ids: &Arc<[u32]>,
        output: &Arc<GemmaTextGenerationOutput>,
    ) -> Result<(), Box<dyn Error>> {
        let raw_generated_text = self
            .model
            .decode_token_ids(output.generated_token_ids.as_ref())
            .map_err(|err| err.to_string())?;
        let mut next_prompt_text =
            String::with_capacity(prompt_text.len() + raw_generated_text.len());
        next_prompt_text.push_str(prompt_text.as_ref());
        next_prompt_text.push_str(&raw_generated_text);
        let mut next_prompt_token_ids = Vec::with_capacity(
            prompt_token_ids
                .len()
                .checked_add(output.generated_token_ids.len())
                .ok_or("CUDA chat prompt token count overflow")?,
        );
        next_prompt_token_ids.extend_from_slice(prompt_token_ids.as_ref());
        next_prompt_token_ids.extend_from_slice(output.generated_token_ids.as_ref());
        self.incremental_prompt_text = Some(next_prompt_text);
        self.incremental_prompt_token_ids = Some(next_prompt_token_ids);
        Ok(())
    }

    fn prepare_generation_prompt(&mut self) -> Result<String, Box<dyn Error>> {
        let include_image = self.current_image_path.is_some();
        let prompt_format = self.model.default_chat_prompt_format();
        let sampling_options = self.active_sampling_options();
        let mut messages = self.messages.clone();

        loop {
            let formatted_prompt = match prompt_format {
                GemmaPromptFormat::AutoChat => format_plain_chat_prompt_with_image(
                    self.model.tokenizer_config(),
                    &messages,
                    include_image,
                )?,
                GemmaPromptFormat::Gemma4UserTurn => format_gemma4_chat_prompt_with_image(
                    self.model.tokenizer_config(),
                    &messages,
                    include_image,
                )?,
                GemmaPromptFormat::RawBos => format_plain_chat_prompt_with_image(
                    self.model.tokenizer_config(),
                    &messages,
                    include_image,
                )?,
            };

            let needs_trim = !include_image
                && self
                    .model
                    .uses_incremental_greedy_backend(self.max_new_tokens, &sampling_options)
                && self.max_new_tokens.is_some();
            if !needs_trim {
                self.messages = messages;
                return Ok(formatted_prompt);
            }

            let total_limit = self.model.exact_greedy_supported_total_tokens(
                self.max_new_tokens,
                &sampling_options,
            )
            .ok_or("missing exact greedy token limit")?;
            let max_new_tokens = self.max_new_tokens.unwrap_or(0);
            if max_new_tokens >= total_limit {
                return Err(format!(
                    "max_new_tokens {} exceeds exact greedy token limit {}",
                    max_new_tokens, total_limit
                )
                .into());
            }
            let prompt_token_limit = total_limit - max_new_tokens;
            let prompt_token_count = self
                .model
                .tokenize_formatted_prompt(&formatted_prompt)?
                .len();
            if prompt_token_count <= prompt_token_limit {
                self.messages = messages;
                return Ok(formatted_prompt);
            }
            if messages.len() <= 1 || !Self::prune_oldest_turn(&mut messages) {
                return Err(format!(
                    "chat prompt requires {} tokens but exact greedy allows only {} prompt tokens with max_new_tokens={}",
                    prompt_token_count, prompt_token_limit, max_new_tokens
                )
                .into());
            }
        }
    }

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
            GemmaTextBackendConfig::default(),
        )
    }

    pub fn load_with_mode_and_backend_config(
        model_path: impl AsRef<Path>,
        max_new_tokens: Option<usize>,
        decode_mode: GemmaChatDecodeMode,
        backend_config: GemmaTextBackendConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let model = GemmaTextModel::load_with_backend_config(model_path, backend_config)?;
        let sampling_options = match model.default_chat_prompt_format() {
            GemmaPromptFormat::Gemma4UserTurn => model.chat_sampling_options(),
            GemmaPromptFormat::AutoChat | GemmaPromptFormat::RawBos => {
                model.default_sampling_options()
            }
        };
        let session = Self {
            model,
            max_new_tokens,
            rng: MlxTextSamplingRng::new(0),
            decode_mode,
            sampling_options,
            messages: Vec::new(),
            current_image_path: None,
            incremental_prompt_text: None,
            incremental_prompt_token_ids: None,
        };
        if decode_mode == GemmaChatDecodeMode::Greedy {
            let _ = session.model.prewarm_greedy_backend(max_new_tokens);
        }
        Ok(session)
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
        let sampling_options = self.active_sampling_options();
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
        self.clear_incremental_prompt_cache();
    }

    pub fn set_image(&mut self, image_path: impl Into<PathBuf>) {
        self.current_image_path = Some(image_path.into());
        self.clear_incremental_prompt_cache();
    }

    pub fn clear_image(&mut self) {
        self.current_image_path = None;
        self.clear_incremental_prompt_cache();
    }

    pub fn current_image_path(&self) -> Option<&Path> {
        self.current_image_path.as_deref()
    }

    pub fn push_assistant_message(&mut self, content: impl Into<String>) {
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::Assistant, content));
        self.clear_incremental_prompt_cache();
    }

    pub fn send_user_message(
        &mut self,
        content: impl Into<String>,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let content = content.into();
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::User, content));
        if !self.uses_incremental_greedy_chat_path() {
            self.clear_incremental_prompt_cache();
        }
        let greedy_sampling_options = self.active_sampling_options();
        if self.uses_incremental_greedy_chat_path() {
            let exact_sampling_options = self.incremental_chat_sampling_options();
            let user_content = self
                .messages
                .last()
                .ok_or("missing user message")?
                .content
                .clone();
            let (prompt_text, prompt_token_ids, prompt_prefill_token_count) =
                self.prepare_incremental_generation_prompt(user_content.as_ref())?;
            if let Some(output) = self
                .model
                .generate_pretokenized_cuda_exact_greedy_with_callback(
                    prompt_text.clone(),
                    prompt_text.clone(),
                    prompt_token_ids.clone(),
                    prompt_prefill_token_count,
                    self.max_new_tokens,
                    &exact_sampling_options,
                    |_| Ok(()),
                )?
            {
                self.finish_incremental_generation(&prompt_text, &prompt_token_ids, &output)?;
                self.messages.push(GemmaChatMessage::new(
                    GemmaChatRole::Assistant,
                    output.generated_text.as_ref(),
                ));
                return Ok(output);
            }
        }
        let formatted_prompt = self.prepare_generation_prompt()?;
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
            (GemmaChatDecodeMode::Sampled, None) => {
                self.model.generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &self.sampling_options,
                    &mut self.rng,
                )?
            }
            (GemmaChatDecodeMode::Greedy, Some(image_path)) => self
                .model
                .generate_preformatted_multimodal_with_rng_and_sampling(
                    image_path,
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                )?,
            (GemmaChatDecodeMode::Greedy, None) => {
                self.model.generate_preformatted_with_rng_and_sampling(
                    formatted_prompt,
                    self.max_new_tokens,
                    &greedy_sampling_options,
                    &mut self.rng,
                )?
            }
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
        mut on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let content = content.into();
        self.messages
            .push(GemmaChatMessage::new(GemmaChatRole::User, content));
        let greedy_sampling_options = self.active_sampling_options();
        if !self.uses_incremental_greedy_chat_path() {
            self.clear_incremental_prompt_cache();
        }
        if self.uses_incremental_greedy_chat_path() {
            let exact_sampling_options = self.incremental_chat_sampling_options();
            let user_content = self
                .messages
                .last()
                .ok_or("missing user message")?
                .content
                .clone();
            let (prompt_text, prompt_token_ids, prompt_prefill_token_count) =
                self.prepare_incremental_generation_prompt(user_content.as_ref())?;
            let model = self.model.clone();
            let tokenizer_config = self.model.tokenizer_config().clone();
            let mut streamed_text = String::new();
            if let Some(output) = self
                .model
                .generate_pretokenized_cuda_exact_greedy_with_callback(
                    prompt_text.clone(),
                    prompt_text.clone(),
                    prompt_token_ids.clone(),
                    prompt_prefill_token_count,
                    self.max_new_tokens,
                    &exact_sampling_options,
                    |generated_token_ids| {
                        let raw_text = model.decode_token_ids(generated_token_ids)?;
                        let partial_text = extract_gemma4_assistant_response_text(
                            &tokenizer_config,
                            &raw_text,
                        );
                        if let Some(delta) = partial_text.strip_prefix(&streamed_text) {
                            if !delta.is_empty() {
                                on_text_delta(delta).map_err(|err| err.to_string())?;
                                streamed_text.push_str(delta);
                            }
                        }
                        Ok(())
                    },
                )?
            {
                if let Some(delta) = output.generated_text.strip_prefix(&streamed_text) {
                    if !delta.is_empty() {
                        on_text_delta(delta)?;
                    }
                }
                self.finish_incremental_generation(&prompt_text, &prompt_token_ids, &output)?;
                self.messages.push(GemmaChatMessage::new(
                    GemmaChatRole::Assistant,
                    output.generated_text.as_ref(),
                ));
                return Ok(output);
            }
        }
        let formatted_prompt = self.prepare_generation_prompt()?;
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
