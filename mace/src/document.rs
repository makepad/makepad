use {
    crate::{
        delta::Delta,
        session::{Session, SessionId},
        text::Text,
        tokenizer::{Tokenizer, TokensByLine},
    },
    std::collections::{HashMap, HashSet, VecDeque},
};

pub struct Document {
    session_ids: HashSet<SessionId>,
    revision: usize,
    text: Text,
    tokenizer: Tokenizer,
    outstanding_deltas: VecDeque<Delta>,
}

impl Document {
    pub fn new(revision: usize, text: Text) -> Document {
        let tokenizer = Tokenizer::new(&text);
        Document {
            session_ids: HashSet::new(),
            revision,
            text,
            tokenizer,
            outstanding_deltas: VecDeque::new(),
        }
    }

    pub fn session_ids(&self) -> &HashSet<SessionId> {
        &self.session_ids
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn tokens_by_line(&self) -> TokensByLine<'_> {
        self.tokenizer.tokens_by_line()
    }

    pub fn add_session_id(&mut self, session_id: SessionId) {
        self.session_ids.insert(session_id);
    }

    pub fn remove_session_id(&mut self, session_id: SessionId) {
        self.session_ids.remove(&session_id);
    }

    pub fn start_applying_local_delta(
        &mut self,
        delta: Delta,
        post_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta.clone());
        self.tokenizer.refresh_cache(&self.text);
        if self.outstanding_deltas.len() == 2 {
            let last_outstanding_delta = self.outstanding_deltas.pop_back().unwrap();
            self.outstanding_deltas.push_back(last_outstanding_delta.compose(delta));
        } else {
            self.outstanding_deltas.push_back(delta);
            if self.outstanding_deltas.len() == 1 {
                post_apply_delta_request(self.revision, self.outstanding_deltas[0].clone())
            }
        }
    }

    pub fn finish_applying_local_delta(
        &mut self,
        post_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        self.revision += 1;
        self.outstanding_deltas.pop_front();
        if let Some(outstanding_delta) = self.outstanding_deltas.front() {
            post_apply_delta_request(self.revision, outstanding_delta.clone())
        }
    }

    pub fn apply_remote_delta(
        &mut self,
        sessions_by_session_id: &mut HashMap<SessionId, Session>,
        delta: Delta,
    ) {
        // TODO: Transform delta against outstanding deltas
        for session_id in &self.session_ids {
            let session = sessions_by_session_id.get_mut(&session_id).unwrap();
            session.apply_remote_delta(&delta);
        }
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta.clone());
        self.tokenizer.refresh_cache(&self.text);
    }
}
