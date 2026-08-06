//! Routing a keyless client's typed request to the host's agent
//! (game.md §"AI tiers": "in a room with a host, creation requests can still
//! route through the host's agent, so BYO-key is only needed for standalone
//! creation").
//!
//! The host owns the mic, the key and the agent. A client types a request, it
//! arrives as `Intent::Authoring`, and the host queues it for its own agent.
//! The resulting edit reloads the game host-side and reaches everyone through
//! the ordinary replication path — clients never need the source, or a key.

use makepad_game_net::endpoint::HostEvent;
use makepad_game_net::protocol::{Intent, PlayerId, MAX_AUTHORING_TEXT};

/// One client's request, ready to hand to the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoringRequest {
    pub player: PlayerId,
    pub text: String,
}

/// Why a request was refused. Kept explicit so the host can tell the room
/// something true rather than dropping requests silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentRefusal {
    Empty,
    TooLong,
    TooManyPending,
}

/// Collects authoring requests arriving from clients.
///
/// Bounded on purpose: this queue is filled by unauthenticated-in-spirit peers
/// (they hold the lobby key, but a kid's tablet is not a trusted terminal), and
/// every entry it holds eventually costs a paid agent call.
pub struct AuthoringInbox {
    pending: Vec<AuthoringRequest>,
    max_pending: usize,
    refusals: Vec<(PlayerId, IntentRefusal)>,
}

impl Default for AuthoringInbox {
    fn default() -> Self {
        Self::new(8)
    }
}

impl AuthoringInbox {
    pub fn new(max_pending: usize) -> Self {
        Self {
            pending: Vec::new(),
            max_pending,
            refusals: Vec::new(),
        }
    }

    /// Feed one pump's host events; non-authoring events are ignored.
    pub fn absorb(&mut self, events: &[HostEvent]) {
        for event in events {
            if let HostEvent::Intent {
                player,
                intent: Intent::Authoring { text },
            } = event
            {
                self.push(*player, text);
            }
        }
    }

    fn push(&mut self, player: PlayerId, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.refusals.push((player, IntentRefusal::Empty));
            return;
        }
        if text.len() > MAX_AUTHORING_TEXT {
            self.refusals.push((player, IntentRefusal::TooLong));
            return;
        }
        if self.pending.len() >= self.max_pending {
            self.refusals.push((player, IntentRefusal::TooManyPending));
            return;
        }
        self.pending.push(AuthoringRequest {
            player,
            text: trimmed.to_string(),
        });
    }

    /// Take everything queued. The caller hands these to the agent.
    pub fn drain(&mut self) -> Vec<AuthoringRequest> {
        std::mem::take(&mut self.pending)
    }

    pub fn drain_refusals(&mut self) -> Vec<(PlayerId, IntentRefusal)> {
        std::mem::take(&mut self.refusals)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_event(player: PlayerId, text: &str) -> HostEvent {
        HostEvent::Intent {
            player,
            intent: Intent::Authoring {
                text: text.to_string(),
            },
        }
    }

    #[test]
    fn authoring_intents_are_queued_for_the_agent() {
        let mut inbox = AuthoringInbox::default();
        inbox.absorb(&[
            intent_event(PlayerId(2), "  make the cars faster  "),
            HostEvent::Intent {
                player: PlayerId(3),
                intent: Intent::Respawn,
            },
        ]);
        let queued = inbox.drain();
        assert_eq!(queued.len(), 1, "Respawn must not reach the agent");
        assert_eq!(queued[0].player, PlayerId(2));
        assert_eq!(queued[0].text, "make the cars faster");
        assert!(inbox.drain().is_empty(), "drain must consume");
    }

    #[test]
    fn empty_and_oversized_requests_are_refused_not_forwarded() {
        let mut inbox = AuthoringInbox::default();
        let huge = "x".repeat(MAX_AUTHORING_TEXT + 1);
        inbox.absorb(&[intent_event(PlayerId(1), "   "), intent_event(PlayerId(1), &huge)]);
        assert!(inbox.drain().is_empty());
        let refusals: Vec<_> = inbox.drain_refusals().into_iter().map(|(_, r)| r).collect();
        assert_eq!(
            refusals,
            vec![IntentRefusal::Empty, IntentRefusal::TooLong]
        );
    }

    #[test]
    fn a_flooding_client_cannot_grow_the_queue_without_bound() {
        let mut inbox = AuthoringInbox::new(2);
        for i in 0..50 {
            inbox.absorb(&[intent_event(PlayerId(9), &format!("request {i}"))]);
        }
        assert_eq!(inbox.pending_len(), 2);
        assert_eq!(inbox.drain_refusals().len(), 48);
    }
}
