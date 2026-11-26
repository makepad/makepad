use makepad_desk_link::{self as proto, ClientMsg};

/// What this desktop already has, for the life of one session.
#[derive(Default)]
pub struct ClipboardLink {
    session: u64,
    peer: proto::ClipboardPeer,
}

impl ClipboardLink {
    fn peer(&mut self, session: u64) -> &mut proto::ClipboardPeer {
        if self.session != session {
            self.peer = proto::ClipboardPeer::default();
            self.session = session;
        }
        &mut self.peer
    }

    pub fn received(&mut self, session: u64, text: &str) {
        self.peer(session).received(text);
    }

    pub fn offer(&mut self, session: u64, desktop_has_clipboard: bool, text: &str) -> Vec<ClientMsg> {
        let peer = self.peer(session);
        if desktop_has_clipboard && peer.offer(text) {
            ClientMsg::clipboard(text)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_offers_follow_the_session() {
        let mut link = ClipboardLink::default();
        assert!(link.offer(1, false, "first").is_empty(), "an older desktop gets nothing");
        link.received(2, "second");
        assert!(link.offer(2, true, "second").is_empty(), "a copy from the desktop never echoes");
        assert_eq!(link.offer(2, true, "first"), ClientMsg::clipboard("first"), "a local copy goes");
        assert!(link.offer(2, true, "first").is_empty(), "the desktop already has the text");
        assert_eq!(link.offer(3, true, "first"), ClientMsg::clipboard("first"), "a new session starts fresh");
    }

    #[test]
    fn clipboard_offer_in_parts_ends_with_the_remaining_text() {
        let mut text = "a".repeat(proto::CLIPBOARD_PART - 1);
        text.push('é');
        let messages = ClipboardLink::default().offer(1, true, &text);
        assert!(matches!(messages.as_slice(),
            [ClientMsg::ClipboardPart { text: first }, ClientMsg::Clipboard { text: last }]
            if first == &text[..proto::CLIPBOARD_PART - 1] && last == "é"
        ), "the two messages carry the whole text in order");
    }
}
