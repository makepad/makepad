use crate::{
    delta::Delta,
    text::Text,
    tokenizer::{Tokenizer, TokensByLine},
};

pub struct Document {
    revision: usize,
    text: Text,
    tokenizer: Tokenizer,
}

impl Document {
    pub fn new(revision: usize, text: Text) -> Document {
        let tokenizer = Tokenizer::new(&text);
        Document {
            revision,
            text,
            tokenizer,
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn tokens_by_line(&self) -> TokensByLine<'_> {
        self.tokenizer.tokens_by_line()
    }

    pub fn apply_delta(&mut self, delta: Delta) {
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta);
        self.tokenizer.refresh_cache(&self.text);
    }
}
