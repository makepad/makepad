use {
    crate::{
        delta::Delta,
        session::{Session, SessionId},
        text::Text,
        tokenizer::{Tokenizer, TokensByLine},
    },
    std::collections::{HashMap, HashSet},
};

pub struct Document {
    session_ids: HashSet<SessionId>,
    revision: usize,
    text: Text,
    tokenizer: Tokenizer,
    outstanding_delta: Option<Delta>,
    queued_delta: Option<Delta>,
}

impl Document {
    pub fn new(revision: usize, text: Text) -> Document {
        let tokenizer = Tokenizer::new(&text);
        Document {
            session_ids: HashSet::new(),
            revision,
            text,
            tokenizer,
            outstanding_delta: None,
            queued_delta: None,
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

    pub fn apply_local_delta(
        &mut self,
        delta: Delta,
        post_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta.clone());
        self.tokenizer.refresh_cache(&self.text);
        match self.outstanding_delta {
            Some(_) => match self.queued_delta.take() {
                Some(queued_delta) => self.queued_delta = Some(queued_delta.compose(delta)),
                None => self.queued_delta = Some(delta),
            },
            None => {
                self.outstanding_delta = Some(delta.clone());
                post_apply_delta_request(self.revision, delta);
            }
        }
    }

    pub fn handle_apply_delta_response(
        &mut self,
        post_apply_delta_request: &mut dyn FnMut(usize, Delta),
    ) {
        self.revision += 1;
        self.outstanding_delta = self.queued_delta.take();
        if let Some(outstanding_delta) = &self.outstanding_delta {
            post_apply_delta_request(self.revision, outstanding_delta.clone())
        }
    }

    pub fn handle_delta_was_applied_notification(
        &mut self,
        sessions_by_session_id: &mut HashMap<SessionId, Session>,
        delta: Delta,
    ) {
        let mut delta = delta;
        if let Some(outstanding_delta) = self.outstanding_delta.take() {
            let (new_outstanding_delta, new_delta) = outstanding_delta.transform(delta);
            self.outstanding_delta = Some(new_outstanding_delta);
            delta = new_delta;
            if let Some(queued_delta) = self.queued_delta.take() {
                let (new_queued_delta, new_delta) = queued_delta.transform(delta);
                self.queued_delta = Some(new_queued_delta);
                delta = new_delta;
            }
        }
        for session_id in &self.session_ids {
            let session = sessions_by_session_id.get_mut(&session_id).unwrap();
            session.handle_delta_was_applied_notification(&delta);
        }
        self.tokenizer.invalidate_cache(&delta);
        self.text.apply_delta(delta.clone());
        self.tokenizer.refresh_cache(&self.text);
    }
}
