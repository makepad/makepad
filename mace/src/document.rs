use {
    crate::{
        delta::Delta,
        text::Text,
        tokenizer::{Tokenizer, TokensByLine},
    },
    std::collections::VecDeque,
};

pub struct Document {
    revision: usize,
    text: Text,
    tokenizer: Tokenizer,
    outstanding_deltas: VecDeque<Delta>,
}

impl Document {
    pub fn new(revision: usize, text: Text) -> Document {
        let tokenizer = Tokenizer::new(&text);
        Document {
            revision,
            text,
            tokenizer,
            outstanding_deltas: VecDeque::new(),
        }
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn tokens_by_line(&self) -> TokensByLine<'_> {
        self.tokenizer.tokens_by_line()
    }

    pub fn apply_delta(
        &mut self,
        delta: Delta,
        send_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta.clone());
        self.tokenizer.refresh_cache(&self.text);
        self.outstanding_deltas.push_back(delta);
        if self.outstanding_deltas.len() == 1 {
            send_apply_delta_request(self.revision, self.outstanding_deltas[0].clone())
        }
    }
}
