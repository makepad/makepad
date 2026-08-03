//! Turning an agent's file edit into a coedit transaction.
//!
//! The agent writes `game.splash` on disk (a CLI agent does it with its own
//! tools; the HTTP backends do it through the adapter). That edit is **not**
//! the source of truth: it is a *proposal* against whatever generation the
//! turn started from. This module reads the file back, submits it to the
//! intent log, and hands out whatever the merge decided.
//!
//! Going through the log even for the local agent is the whole point — a
//! shortcut here would leave the merge path exercised only by remote authors,
//! which is exactly backwards (game.md §"Collaborative editing").

use crate::coedit::{CoeditBridge, PendingEval};
use makepad_game_net::protocol::CoeditResponse;
use std::path::{Path, PathBuf};

/// What the host should do after a proposal was merged.
#[derive(Debug, PartialEq)]
pub enum Applied {
    /// The head moved: evaluate this source and hot-reload.
    Reload(PendingEval),
    /// Nothing to do (the edit was refused, rebased, or changed nothing) —
    /// the responses say why.
    Nothing,
}

pub struct Authoring {
    bridge: CoeditBridge,
    path: PathBuf,
    /// The generation the running turn was written against. Captured when the
    /// turn starts, so an edit that lands after somebody else's is correctly
    /// treated as stale rather than silently clobbering them.
    turn_base: u64,
}

impl Authoring {
    /// Start from whatever is on disk; a missing file is an empty game.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let bridge = CoeditBridge::new(source);
        Self {
            bridge,
            path,
            turn_base: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bridge(&mut self) -> &mut CoeditBridge {
        &mut self.bridge
    }

    pub fn head_generation(&self) -> u64 {
        self.bridge.head_generation()
    }

    /// Called when a turn begins: everything the agent writes during it is a
    /// proposal against this generation.
    pub fn begin_turn(&mut self) {
        self.turn_base = self.bridge.head_generation();
    }

    /// Read the agent's edit off disk and submit it as a transaction.
    ///
    /// Returns `Nothing` when the file is unreadable or unchanged — an agent
    /// that replies without editing is normal, not an error.
    pub fn submit_from_disk(&mut self, intent: &str) -> Applied {
        let Ok(source) = std::fs::read_to_string(&self.path) else {
            return Applied::Nothing;
        };
        self.bridge.submit_local(intent, self.turn_base, &source);
        match self.bridge.process() {
            Some(pending) => {
                // The merge may have produced a source that differs from what
                // the agent wrote (someone else's hunk landed too). Disk must
                // match the head, or the next mtime poll would re-propose the
                // agent's stale text as if it were new.
                let _ = std::fs::write(&self.path, &pending.source);
                Applied::Reload(pending)
            }
            None => Applied::Nothing,
        }
    }

    /// A remote author's transactions arrive through the same queue; process
    /// them and reload if the head moved.
    pub fn process_remote(&mut self) -> Applied {
        match self.bridge.process() {
            Some(pending) => {
                let _ = std::fs::write(&self.path, &pending.source);
                Applied::Reload(pending)
            }
            None => Applied::Nothing,
        }
    }

    pub fn note_eval_ok(&mut self, generation: u64) {
        self.bridge.note_eval_ok(generation);
    }

    pub fn note_eval_error(&mut self, generation: u64, message: impl Into<String>) {
        self.bridge.note_eval_error(generation, message);
    }

    /// Responses addressed to the local agent — eval errors it must fix,
    /// rebases it must re-derive.
    pub fn drain_local(&mut self) -> Vec<CoeditResponse> {
        self.bridge.drain_local()
    }

    /// Turn one response into a line for the chat. `None` for responses that
    /// are pure bookkeeping (an accept needs no announcement).
    pub fn describe(response: &CoeditResponse) -> Option<String> {
        match response {
            CoeditResponse::Accepted { .. } => None,
            CoeditResponse::EvalError {
                message,
                last_good_generation,
                ..
            } => Some(format!(
                "That edit didn't load, so the game is still running the last \
                 version that worked (v{last_good_generation}):\n{message}"
            )),
            CoeditResponse::Rebase { generation, .. } => Some(format!(
                "Someone else changed the game while you were working \
                 (now at v{generation}) — re-read it and try that edit again."
            )),
            CoeditResponse::Refused { reason } => {
                Some(format!("That edit was refused: {reason:?}"))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_game_net::endpoint::HostEvent;
    use makepad_game_net::protocol::{CoeditRequest, PlayerId};

    const GAME: &str = "cars {\n  count: 4\n}\nrules {\n  laps: 3\n}\n";

    fn temp_game(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arcade-authoring-{}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.splash");
        std::fs::write(&path, GAME).unwrap();
        path
    }

    /// The load-bearing test: a typed request must reach the intent log, not
    /// a private path that skips merging.
    #[test]
    fn an_agent_edit_becomes_a_transaction_in_the_intent_log() {
        let path = temp_game("submit");
        let mut authoring = Authoring::new(&path);
        assert_eq!(authoring.head_generation(), 0);

        authoring.begin_turn();
        // The agent edits the file, exactly as a CLI agent would.
        std::fs::write(&path, GAME.replace("count: 4", "count: 8")).unwrap();

        let applied = authoring.submit_from_disk("more cars");
        let Applied::Reload(pending) = applied else {
            panic!("an edit must move the head, got {applied:?}");
        };
        assert_eq!(pending.generation, 1, "the log advanced by one generation");
        assert!(pending.source.contains("count: 8"));
        assert_eq!(
            authoring.head_generation(),
            1,
            "the head is the log's, not the file's"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_reply_with_no_edit_proposes_nothing() {
        let path = temp_game("noedit");
        let mut authoring = Authoring::new(&path);
        authoring.begin_turn();
        // Agent answered a question without touching the file.
        assert_eq!(authoring.submit_from_disk("what is this game?"), Applied::Nothing);
        assert_eq!(authoring.head_generation(), 0);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// A remote author landing mid-turn must rebase the local agent, and the
    /// file on disk must end up matching the head — otherwise the next mtime
    /// poll would re-propose the agent's stale text.
    #[test]
    fn a_local_edit_that_loses_a_race_is_rebased_and_disk_follows_the_head() {
        let path = temp_game("race");
        let mut authoring = Authoring::new(&path);
        authoring.begin_turn();

        // A remote Claude edits the same region first.
        authoring.bridge().absorb(
            &[HostEvent::Coedit {
                player: PlayerId(5),
                req: CoeditRequest::Submit {
                    intent: "eight cars".into(),
                    base_generation: 0,
                    source: GAME.replace("count: 4", "count: 8"),
                },
            }],
            0.0,
        );
        let applied = authoring.process_remote();
        assert!(matches!(applied, Applied::Reload(_)), "remote edit lands");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            GAME.replace("count: 4", "count: 8"),
            "disk follows the head so the watcher does not re-propose"
        );

        // Now the local agent's conflicting edit, written against generation 0.
        std::fs::write(&path, GAME.replace("count: 4", "count: 2")).unwrap();
        authoring.submit_from_disk("two cars");

        let responses = authoring.drain_local();
        let rebase = responses
            .iter()
            .find(|r| matches!(r, CoeditResponse::Rebase { .. }))
            .expect("the local agent must be rebased, not silently merged");
        let text = Authoring::describe(rebase).expect("a rebase is worth saying out loud");
        assert!(text.contains("try that edit again"), "{text}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_eval_error_comes_back_as_a_line_for_the_chat() {
        let path = temp_game("evalerr");
        let mut authoring = Authoring::new(&path);
        authoring.note_eval_ok(0);
        authoring.begin_turn();
        std::fs::write(&path, GAME.replace("laps: 3", "laps: oops")).unwrap();

        let Applied::Reload(pending) = authoring.submit_from_disk("longer race") else {
            panic!("the edit should have been accepted before it failed to eval");
        };
        authoring.drain_local();
        authoring.note_eval_error(pending.generation, "game.splash:5:9: expected a number");

        let responses = authoring.drain_local();
        let text = responses
            .iter()
            .find_map(Authoring::describe)
            .expect("an eval error must reach the proposer");
        assert!(text.contains("expected a number"), "{text}");
        assert!(text.contains("last version that worked"), "{text}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
