//! Bridging the room's Claudes to the intent log.
//!
//! The host's own agent is **not** privileged: `submit_local` and a remote
//! `CoeditRequest::Submit` both land in the same queue and are merged by the
//! same rules. A shortcut for local edits would leave the merge path exercised
//! only by the rarer case, which is exactly backwards — the local agent is the
//! one editing every day (game.md §"Collaborative editing").
//!
//! Routing is decided in one place, [`CoeditBridge::route`]: a response for
//! [`AuthorId::LOCAL`] goes to the local agent, anything else goes out on the
//! wire addressed to its author. Nothing about an edit is broadcast to the room.

use makepad_game_coedit::{AuthorId, CoeditHost, LeaseOutcome, Limits, Outcome, Refusal, Transaction};
use makepad_game_net::endpoint::HostEvent;
use makepad_game_net::protocol::{
    CoeditChange, CoeditRefusal, CoeditRequest, CoeditResponse, PlayerId,
};

/// Remote players map to authors above [`AuthorId::LOCAL`] so a client can
/// never impersonate the host's agent by holding player id 0.
fn author_of(player: PlayerId) -> AuthorId {
    AuthorId(player.0.wrapping_add(1))
}

fn player_of(author: AuthorId) -> Option<PlayerId> {
    (author != AuthorId::LOCAL).then(|| PlayerId(author.0.wrapping_sub(1)))
}

fn refusal_wire(reason: Refusal) -> CoeditRefusal {
    match reason {
        Refusal::EmptyIntent => CoeditRefusal::EmptyIntent,
        Refusal::IntentTooLong => CoeditRefusal::IntentTooLong,
        Refusal::SourceTooLong => CoeditRefusal::SourceTooLong,
        Refusal::UnknownBase => CoeditRefusal::UnknownBase,
        Refusal::QueueFull => CoeditRefusal::QueueFull,
        Refusal::NoChange => CoeditRefusal::NoChange,
    }
}

/// A generation the host should evaluate and hot-reload.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingEval {
    pub generation: u64,
    pub source: String,
}

pub struct CoeditBridge {
    host: CoeditHost,
    outbox: Vec<(PlayerId, CoeditResponse)>,
    local: Vec<CoeditResponse>,
    to_eval: Option<PendingEval>,
}

impl CoeditBridge {
    pub fn new(initial_source: impl Into<String>) -> Self {
        Self::with_limits(initial_source, Limits::default())
    }

    pub fn with_limits(initial_source: impl Into<String>, limits: Limits) -> Self {
        Self {
            host: CoeditHost::with_limits(initial_source, limits),
            outbox: Vec::new(),
            local: Vec::new(),
            to_eval: None,
        }
    }

    pub fn head_generation(&self) -> u64 {
        self.host.head().number
    }

    pub fn head_source(&self) -> &str {
        &self.host.head().source
    }

    /// The source the world should be running: the newest generation that
    /// actually evaluated.
    pub fn last_good_source(&self) -> &str {
        self.host.last_good_source()
    }

    /// Send one response to whoever it belongs to.
    fn route(&mut self, author: AuthorId, res: CoeditResponse) {
        match player_of(author) {
            Some(player) => self.outbox.push((player, res)),
            None => self.local.push(res),
        }
    }

    /// The host's own agent proposing an edit — same queue, same merge.
    pub fn submit_local(&mut self, intent: &str, base_generation: u64, source: &str) {
        self.enqueue(AuthorId::LOCAL, intent, base_generation, source);
    }

    fn enqueue(&mut self, author: AuthorId, intent: &str, base_generation: u64, source: &str) {
        let tx = Transaction {
            author,
            intent: intent.to_string(),
            base_generation,
            source: source.to_string(),
        };
        if let Err(reason) = self.host.enqueue(tx) {
            self.route(
                author,
                CoeditResponse::Refused {
                    reason: refusal_wire(reason),
                },
            );
        }
    }

    /// Feed one pump's host events.
    pub fn absorb(&mut self, events: &[HostEvent], now: f64) {
        for event in events {
            match event {
                HostEvent::Coedit { player, req } => {
                    let author = author_of(*player);
                    match req {
                        CoeditRequest::GetBase => {
                            let head = self.host.head();
                            let res = CoeditResponse::Base {
                                generation: head.number,
                                source: head.source.clone(),
                            };
                            self.route(author, res);
                        }
                        CoeditRequest::Submit {
                            intent,
                            base_generation,
                            source,
                        } => self.enqueue(author, intent, *base_generation, source),
                        CoeditRequest::AcquireLease { region, ttl } => {
                            let outcome = self.host.leases().acquire(author, region, *ttl, now);
                            let res = match outcome {
                                LeaseOutcome::Granted { expires_at } => {
                                    CoeditResponse::LeaseGranted {
                                        region: region.clone(),
                                        expires_at,
                                    }
                                }
                                LeaseOutcome::Held { by, expires_at } => {
                                    CoeditResponse::LeaseHeld {
                                        region: region.clone(),
                                        by: by.0,
                                        expires_at,
                                    }
                                }
                                LeaseOutcome::TooMany => CoeditResponse::LeaseRefused {
                                    region: region.clone(),
                                },
                            };
                            self.route(author, res);
                        }
                        CoeditRequest::ReleaseLease { region } => {
                            self.host.leases().release(author, region);
                        }
                    }
                }
                HostEvent::Left { player, .. } => {
                    self.host.forget_author(author_of(*player));
                }
                _ => {}
            }
        }
    }

    /// Merge everything queued. Returns the generation to evaluate, if the head
    /// moved — one eval per pump, because the world can only run one source.
    pub fn process(&mut self) -> Option<PendingEval> {
        let outcomes = self.host.process_queue();
        for (author, outcome) in outcomes {
            match outcome {
                Outcome::Accepted { generation, source } => {
                    self.route(author, CoeditResponse::Accepted { generation });
                    self.to_eval = Some(PendingEval { generation, source });
                }
                Outcome::Rebase {
                    generation,
                    base_source,
                    intervening,
                    conflict_regions,
                } => {
                    let intervening = intervening
                        .into_iter()
                        .map(|change| CoeditChange {
                            generation: change.generation,
                            author: change.author.0,
                            intent: change.intent,
                            hunks: change
                                .hunks
                                .into_iter()
                                .map(|h| (h.base_start as u32, h.removed as u32, h.added as u32))
                                .collect(),
                        })
                        .collect();
                    self.route(
                        author,
                        CoeditResponse::Rebase {
                            generation,
                            base_source,
                            intervening,
                            conflict_regions: conflict_regions as u32,
                        },
                    );
                }
                Outcome::Refused { reason } => self.route(
                    author,
                    CoeditResponse::Refused {
                        reason: refusal_wire(reason),
                    },
                ),
            }
        }
        self.to_eval.take()
    }

    pub fn note_eval_ok(&mut self, generation: u64) {
        self.host.note_eval_ok(generation);
    }

    /// The generation failed to load. The world stays on last-good and the
    /// author that proposed it hears about it — the room does not.
    pub fn note_eval_error(&mut self, generation: u64, message: impl Into<String>) {
        let Some(report) = self.host.note_eval_error(generation, message) else {
            return;
        };
        self.route(
            report.author,
            CoeditResponse::EvalError {
                generation: report.generation,
                message: report.message,
                last_good_generation: report.last_good_generation,
            },
        );
    }

    /// Responses for remote authors, ready for `Host::send_coedit`.
    pub fn drain_outbox(&mut self) -> Vec<(PlayerId, CoeditResponse)> {
        std::mem::take(&mut self.outbox)
    }

    /// Responses for the host's own agent.
    pub fn drain_local(&mut self) -> Vec<CoeditResponse> {
        std::mem::take(&mut self.local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "cars {\n  count: 4\n}\nrules {\n  laps: 3\n}\n";

    fn submit_event(player: u64, base: u64, intent: &str, source: &str) -> HostEvent {
        HostEvent::Coedit {
            player: PlayerId(player),
            req: CoeditRequest::Submit {
                intent: intent.to_string(),
                base_generation: base,
                source: source.to_string(),
            },
        }
    }

    fn edited(source: &str, from: &str, to: &str) -> String {
        source.replace(from, to)
    }

    #[test]
    fn a_remote_submission_is_accepted_and_answered_to_its_author() {
        let mut bridge = CoeditBridge::new(GAME);
        let source = edited(GAME, "count: 4", "count: 8");
        bridge.absorb(&[submit_event(5, 0, "more cars", &source)], 0.0);

        let pending = bridge.process().expect("head moved, so the host evaluates");
        assert_eq!(pending.generation, 1);
        assert_eq!(pending.source, source);

        let outbox = bridge.drain_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].0, PlayerId(5), "addressed, not broadcast");
        assert_eq!(outbox[0].1, CoeditResponse::Accepted { generation: 1 });
        assert!(bridge.drain_local().is_empty());
    }

    #[test]
    fn the_local_agent_uses_the_same_queue_and_merge_as_a_remote_one() {
        let mut bridge = CoeditBridge::new(GAME);

        // Remote lands first; the local agent wrote against the same old base.
        bridge.absorb(
            &[submit_event(5, 0, "more cars", &edited(GAME, "count: 4", "count: 8"))],
            0.0,
        );
        bridge.submit_local("longer race", 0, &edited(GAME, "laps: 3", "laps: 5"));
        bridge.process();

        // Disjoint regions: both edits are in the head.
        assert!(bridge.head_source().contains("count: 8"));
        assert!(bridge.head_source().contains("laps: 5"));
        assert_eq!(bridge.head_generation(), 2);

        let local = bridge.drain_local();
        assert_eq!(local, vec![CoeditResponse::Accepted { generation: 2 }]);
    }

    #[test]
    fn an_overlapping_local_edit_is_rebased_exactly_like_a_remote_one() {
        let mut bridge = CoeditBridge::new(GAME);
        bridge.absorb(
            &[submit_event(5, 0, "eight cars", &edited(GAME, "count: 4", "count: 8"))],
            0.0,
        );
        bridge.submit_local("two cars", 0, &edited(GAME, "count: 4", "count: 2"));
        bridge.process();

        let local = bridge.drain_local();
        let [CoeditResponse::Rebase {
            generation,
            base_source,
            conflict_regions,
            ..
        }] = &local[..]
        else {
            panic!("the local agent must be rebased too, got {local:?}");
        };
        assert_eq!(*generation, 1);
        assert_eq!(*conflict_regions, 1);
        assert!(base_source.contains("count: 8"), "handed the new base");
    }

    #[test]
    fn an_eval_error_reaches_the_proposer_and_nobody_else() {
        let mut bridge = CoeditBridge::new(GAME);
        bridge.note_eval_ok(0);

        bridge.absorb(
            &[submit_event(5, 0, "break it", &edited(GAME, "laps: 3", "laps: oops"))],
            0.0,
        );
        let pending = bridge.process().unwrap();
        bridge.drain_outbox();

        bridge.note_eval_error(pending.generation, "game.splash:5:9: expected a number");

        let outbox = bridge.drain_outbox();
        assert_eq!(outbox.len(), 1, "one recipient — the author");
        assert_eq!(outbox[0].0, PlayerId(5));
        let CoeditResponse::EvalError {
            generation,
            message,
            last_good_generation,
        } = &outbox[0].1
        else {
            panic!("expected an eval error, got {:?}", outbox[0].1);
        };
        assert_eq!(*generation, 1);
        assert_eq!(*last_good_generation, 0);
        assert!(message.contains("expected a number"));

        assert!(bridge.drain_local().is_empty(), "the room is not told");
        assert_eq!(
            bridge.last_good_source(),
            GAME,
            "the world keeps running the last good source"
        );
    }

    #[test]
    fn a_client_cannot_impersonate_the_local_agent() {
        let mut bridge = CoeditBridge::new(GAME);
        // Player 0 would collide with AuthorId::LOCAL under a naive mapping.
        bridge.absorb(
            &[submit_event(0, 0, "sneaky", &edited(GAME, "count: 4", "count: 9"))],
            0.0,
        );
        bridge.process();

        assert!(
            bridge.drain_local().is_empty(),
            "a remote submission must never be answered as local"
        );
        assert_eq!(bridge.drain_outbox().len(), 1);
    }

    #[test]
    fn leases_are_granted_reported_and_dropped_when_an_author_leaves() {
        let mut bridge = CoeditBridge::new(GAME);
        let acquire = |player: u64| HostEvent::Coedit {
            player: PlayerId(player),
            req: CoeditRequest::AcquireLease {
                region: "vehicles".to_string(),
                ttl: 30.0,
            },
        };

        bridge.absorb(&[acquire(1)], 100.0);
        bridge.absorb(&[acquire(2)], 101.0);
        let outbox = bridge.drain_outbox();
        assert!(matches!(outbox[0].1, CoeditResponse::LeaseGranted { .. }));
        let CoeditResponse::LeaseHeld { by, .. } = outbox[1].1 else {
            panic!("second author must be told who holds it, got {:?}", outbox[1].1);
        };
        assert_eq!(by, author_of(PlayerId(1)).0);

        bridge.absorb(
            &[HostEvent::Left {
                player: PlayerId(1),
                reason: makepad_game_net::protocol::LeaveReason::Explicit,
            }],
            102.0,
        );
        bridge.absorb(&[acquire(2)], 103.0);
        assert!(
            matches!(
                bridge.drain_outbox().last().map(|(_, r)| r),
                Some(CoeditResponse::LeaseGranted { .. })
            ),
            "a departed author's lease must not outlive it"
        );
    }

    #[test]
    fn a_flooding_author_is_refused_rather_than_growing_the_queue() {
        let mut bridge = CoeditBridge::with_limits(
            GAME,
            Limits {
                max_pending_per_author: 2,
                ..Limits::default()
            },
        );
        for i in 0..6 {
            bridge.absorb(
                &[submit_event(5, 0, "spam", &format!("{GAME}// {i}\n"))],
                0.0,
            );
        }
        let refusals = bridge
            .drain_outbox()
            .into_iter()
            .filter(|(_, res)| {
                matches!(
                    res,
                    CoeditResponse::Refused {
                        reason: CoeditRefusal::QueueFull
                    }
                )
            })
            .count();
        assert_eq!(refusals, 4, "two queued, four refused with a reason");
    }
}
