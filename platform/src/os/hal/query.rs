//! App-only queries. Widget inspection lives entirely in the semantic mirror.
use crate::cx::Cx;

impl Cx {
    pub(crate) fn hal_answer_query(&mut self, req: u32, request: &[u8]) {
        let request = String::from_utf8_lossy(request);
        if let Some(text) = request.strip_prefix("a11y_type\n") {
            self.a11y_type_fallback(text.to_owned());
            self.hal.frames.w.query_reply(req, b"ok");
            return;
        }
        if let Some(index) = request.strip_prefix("a11y_present ").and_then(|s| s.parse::<usize>().ok()) {
            let target = self.windows.id_iter().find(|id| id.id() == index && self.hal.window_ready(*id));
            if let Some(window) = target {
                let id = crate::event::hal_intake::hal_window_id(window);
                let submitted = self.hal.frames.window_submission(id);
                self.hal.a11y_present_waiters.push((req, id, submitted));
                // Same forced present as remote input wait=1, including hidden and idle windows.
                self.request_remote_window_present(window);
                return;
            }
        }
        let reply = match request.trim() {
            "draw_owners" => self.draw_owners_answer(),
            "a11y_user_seq" => self.remote_activity.user_seq().to_string(),
            other => format!("error: unknown request `{other}` (draw_owners, a11y_present, a11y_user_seq)"),
        };
        self.hal.frames.w.query_reply(req, reply.as_bytes());
    }

    pub(crate) fn hal_a11y_presented(&mut self) {
        let user_seq = self.remote_activity.user_seq();
        let frames = &mut self.hal.frames;
        self.hal.a11y_host_waiters.retain(|(req, window, submitted)| {
            if frames.window_completion(*window) > *submitted {
                Self::send_studio_message(crate::studio::AppToStudio::A11yReadDone { req: *req, user_seq });
                false
            } else { true }
        });
        self.hal.a11y_present_waiters.retain(|(req, window, submitted)| {
            if frames.window_completion(*window) > *submitted {
                frames.w.query_reply(*req, b"ok");
                false
            } else { true }
        });
    }
}
