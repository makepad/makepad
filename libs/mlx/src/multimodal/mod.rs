mod image;
mod vision;

use crate::{MlxIndexedSafetensors, MlxTokenizer};
use std::path::Path;

pub use image::{load_gemma_image, GemmaImagePixels};
pub use vision::GemmaVisionRuntime;

pub(crate) struct PreparedImagePrompt {
    pub formatted_prompt_text: String,
    pub prompt_token_ids: Vec<u32>,
    pub prompt_embedding_rows: Vec<Vec<u16>>,
}

pub(crate) fn prepare_image_prompt(
    weights: &MlxIndexedSafetensors,
    tokenizer: &MlxTokenizer,
    vision: &mut GemmaVisionRuntime,
    formatted_prompt_text: &str,
    image_path: &Path,
) -> Result<PreparedImagePrompt, String> {
    let image = load_gemma_image(&weights.snapshot, image_path).map_err(|err| err.to_string())?;
    let image_embeddings = vision
        .encode_image_to_text_embeddings(&image)
        .map_err(|err| err.to_string())?;
    let mut prompt_token_ids = tokenizer
        .encode(formatted_prompt_text)
        .map_err(|err| err.to_string())?;
    if prompt_token_ids.is_empty() {
        return Err("formatted multimodal prompt encoded to zero tokens".to_string());
    }

    let config = &weights.snapshot.config;
    let image_token_id = config.image_token_id;
    let image_placeholder_count = prompt_token_ids
        .iter()
        .copied()
        .filter(|token_id| *token_id == image_token_id)
        .count();
    if image_placeholder_count != 1 {
        return Err(format!(
            "expected exactly one image token placeholder in formatted prompt, found {image_placeholder_count}"
        ));
    }

    let boi_token_id = config.boi_token_id;
    let eoi_token_id = config.eoi_token_id;
    let mut expanded_token_ids =
        Vec::with_capacity(prompt_token_ids.len() + image_embeddings.len() + 2);
    for token_id in prompt_token_ids.drain(..) {
        if token_id == image_token_id {
            expanded_token_ids.push(boi_token_id);
            expanded_token_ids.extend(std::iter::repeat_n(image_token_id, image_embeddings.len()));
            expanded_token_ids.push(eoi_token_id);
        } else {
            expanded_token_ids.push(token_id);
        }
    }

    let mut image_embeddings_iter = image_embeddings.into_iter();
    let mut prompt_embedding_rows = Vec::with_capacity(expanded_token_ids.len());
    for &token_id in &expanded_token_ids {
        if token_id == image_token_id {
            let image_row = image_embeddings_iter
                .next()
                .ok_or_else(|| "image soft-token expansion underflow".to_string())?;
            prompt_embedding_rows.push(image_row);
        } else {
            prompt_embedding_rows.push(
                weights
                    .embed_token_bf16_words(token_id)
                    .map_err(|err| err.to_string())?,
            );
        }
    }
    if image_embeddings_iter.next().is_some() {
        return Err("image soft-token expansion overflow".to_string());
    }

    Ok(PreparedImagePrompt {
        formatted_prompt_text: formatted_prompt_text.to_owned(),
        prompt_token_ids: expanded_token_ids,
        prompt_embedding_rows,
    })
}
