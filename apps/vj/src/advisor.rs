//! What an advisor is allowed to say, and what the engine will accept.
//!
//! The optional brain proposes; the deterministic planner disposes. This is
//! the seam between them, and it is deliberately the whole of the contract:
//! nothing here talks to a model, loads anything or blocks on anything, so
//! the rules a proposal has to survive can be argued with in a test rather
//! than in front of a room.
//!
//! Every surveyed system that put a language model near a DJ engine failed
//! the same handful of ways — records that were never in the library,
//! positions past the end of the track, techniques the engine could not
//! perform, and answers that never arrived. So the shape here is narrow on
//! purpose. A proposal names things by the ids it was GIVEN and picks from
//! options it was OFFERED; it computes nothing. Anything else is dropped,
//! and dropping it means the deterministic plan simply stands, which is
//! also what a missing model, a refused licence, a timeout and a garbled
//! answer all mean. There is exactly one failure mode, and it is silence.

use crate::blend::Route;

/// What an advisor put forward, before anyone checked it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proposal {
    /// The record it wants next, by the key it was handed.
    pub pick: Option<String>,
    /// The shape it wants the transition to take.
    pub route: Option<Route>,
    /// Its own words, for the panel and the log.
    pub reason: String,
}

/// A proposal that survived checking: every field here is something the
/// engine was already prepared to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Accepted {
    pub pick: Option<String>,
    pub route: Option<Route>,
    pub reason: String,
}

/// How much of an answer is read before giving up. An advisor that has not
/// said anything useful in this many lines is not going to.
const MOST_LINES: usize = 24;

/// The longest reason worth showing. A panel line, not an essay.
const REASON_CHARS: usize = 120;

/// Read a proposal out of whatever the model actually said.
///
/// Tolerant on purpose: `key: value` a line at a time, case-insensitive,
/// ignoring everything it does not recognise. Models wrap answers in prose,
/// in code fences and in apologies, and a parser that demanded strict JSON
/// would throw away good answers over their packaging. What it will NOT do
/// is guess — an unreadable line is skipped, never interpreted.
pub fn parse(text: &str) -> Proposal {
    let mut out = Proposal::default();
    for line in text.lines().take(MOST_LINES) {
        let line = line.trim().trim_start_matches(['-', '*', '#', '`', ' ']);
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'', ',', '`']).trim();
        if value.is_empty() {
            continue;
        }
        match field.trim().to_ascii_lowercase().as_str() {
            "pick" | "track" | "next" => out.pick = Some(value.to_string()),
            "route" | "shape" => {
                out.route = match value.to_ascii_lowercase().as_str() {
                    "handover" | "blend" => Some(Route::Handover),
                    "pickup" | "filter" => Some(Route::Pickup),
                    "short" | "cut" => Some(Route::Short),
                    _ => out.route,
                }
            }
            "reason" | "why" => {
                out.reason = value.chars().take(REASON_CHARS).collect();
            }
            _ => {}
        }
    }
    out
}

/// Keep only what the engine was already prepared to do.
///
/// `offered` is the set of record keys the planner put in front of the
/// advisor, and `eligible` the routes it said were available for this pair.
/// A proposal naming anything else is not a difference of opinion, it is an
/// answer about a different world, and the deterministic plan stands.
///
/// `None` when nothing survived — which is the same answer as no advisor at
/// all, and is why the model can be absent, slow, wrong or unlicensed
/// without any of those being a separate case to handle.
pub fn validate(
    proposal: &Proposal,
    offered: &[String],
    eligible: &[Route],
) -> Option<Accepted> {
    let pick = proposal
        .pick
        .as_ref()
        .filter(|key| offered.iter().any(|offer| offer == *key))
        .cloned();
    let route = proposal.route.filter(|route| eligible.contains(route));
    if pick.is_none() && route.is_none() {
        return None;
    }
    Some(Accepted {
        pick,
        route,
        // A reason for something that was thrown out would be a lie about
        // what is going to happen, so it travels only with what survived.
        reason: proposal.reason.clone(),
    })
}

// ---------------------------------------------------------------------------
// the model, when there is one
// ---------------------------------------------------------------------------

/// A question for the advisor, and the state it is about.
///
/// Everything measured is already computed and simply stated: the model is
/// asked to CHOOSE, never to work anything out. A number it invented is a
/// number nobody checked.
#[derive(Clone, Debug)]
pub struct Question {
    /// The records on offer, in the planner's own order, each already
    /// described by its measurements.
    pub offered: Vec<String>,
    /// The lines describing them, one per offered record.
    pub described: Vec<String>,
    /// The shapes the engine says it can perform for this pair.
    pub eligible: Vec<Route>,
    /// What the operator asked for, in their words. Empty when they have
    /// not said anything.
    pub mood: String,
}

impl Question {
    /// The prompt, in the plainest form that can be answered wrongly and
    /// still be safe. Every id it may name is in front of it, every shape
    /// it may pick is listed, and it is told what "no opinion" looks like.
    pub fn prompt(&self) -> String {
        let mut out = String::from(
            "You are helping choose the next record in a DJ set.\n\n",
        );
        if !self.mood.is_empty() {
            out.push_str("The operator asks for: ");
            out.push_str(&self.mood);
            out.push_str("\n\n");
        }
        out.push_str("Records on offer:\n");
        for (key, described) in self.offered.iter().zip(&self.described) {
            out.push_str("  ");
            out.push_str(key);
            out.push_str("  ");
            out.push_str(described);
            out.push('\n');
        }
        out.push_str("\nTransition shapes available: ");
        for (index, route) in self.eligible.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            out.push_str(match route {
                Route::Handover => "handover",
                Route::Pickup => "pickup",
                Route::Short => "short",
            });
        }
        out.push_str(
            "\n\nAnswer with these lines and nothing else:\n\
             pick: <one id from the list above>\n\
             route: <one shape from the list above>\n\
             reason: <one short sentence>\n\
             Leave out any line you have no opinion about.\n",
        );
        out
    }
}

#[cfg(feature = "advisor")]
mod runner {
    use super::*;
    use makepad_widgets::log;
    use std::sync::mpsc::{Receiver, Sender};

    /// Context the model is given. Small on purpose: the whole question
    /// fits in a page, and a big window costs load time and memory for
    /// nothing.
    const CONTEXT: u32 = 2048;
    /// A sentence, not an essay.
    const ANSWER_TOKENS: usize = 96;

    /// The advisor on its own thread.
    ///
    /// The model is `!Send`, so it is built on the worker and never leaves
    /// it. A failure to load is latched: a missing file, a card that cannot
    /// run it or a refused licence are asked about once and then never
    /// again, because retrying every transition would be a stall a room
    /// can hear.
    pub struct Advisor {
        ask: Sender<(u64, String)>,
        answered: Receiver<(u64, Proposal)>,
    }

    impl Advisor {
        /// Start the worker. Cheap: nothing is loaded until the first
        /// question, so an advisor that is never asked costs a thread.
        pub fn start(model: std::path::PathBuf) -> Advisor {
            let (ask, questions) = std::sync::mpsc::channel::<(u64, String)>();
            let (replies, answered) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("vj-advisor".to_string())
                .spawn(move || {
                    let mut session = None;
                    let mut failed = false;
                    for (gen, prompt) in questions {
                        if failed {
                            continue;
                        }
                        if session.is_none() {
                            match makepad_ai_llm::LlamaSession::load(
                                &model,
                                makepad_ai_llm::LlamaSessionConfig {
                                    max_context: Some(CONTEXT),
                                    ..Default::default()
                                },
                            ) {
                                Ok(loaded) => session = Some(loaded),
                                Err(error) => {
                                    log!("auto dj advisor: no model ({error})");
                                    failed = true;
                                    continue;
                                }
                            }
                        }
                        let Some(session) = session.as_mut() else { continue };
                        // Every question stands alone: the state of the room
                        // has moved on since the last one, and carrying it
                        // over would only invite an answer about the past.
                        if session.reset().is_err() {
                            continue;
                        }
                        let Ok(tokens) = session.vocab().tokenize(&prompt, true, true)
                        else {
                            continue;
                        };
                        if session.append_tokens(&tokens).is_err() {
                            continue;
                        }
                        match session.continue_greedy(ANSWER_TOKENS) {
                            Ok(generated) => {
                                let _ = replies.send((gen, super::parse(&generated.text)));
                            }
                            Err(error) => {
                                log!("auto dj advisor: no answer ({error})");
                            }
                        }
                    }
                })
                .ok();
            Advisor { ask, answered }
        }

        /// Ask, and carry on. Never waits: the answer is wanted minutes
        /// before it is needed, and an advisor that has not replied by the
        /// time the fire point arrives has simply said nothing.
        pub fn ask(&self, gen: u64, question: &Question) {
            let _ = self.ask.send((gen, question.prompt()));
        }

        /// Whatever has come back since last time, oldest first.
        pub fn poll(&self) -> Vec<(u64, Proposal)> {
            self.answered.try_iter().collect()
        }
    }
}

#[cfg(feature = "advisor")]
pub use runner::Advisor;

#[cfg(test)]
mod tests {
    use super::*;

    fn offered() -> Vec<String> {
        vec!["ast_one".to_string(), "ast_two".to_string()]
    }

    #[test]
    fn a_plain_answer_is_read() {
        let p = parse("pick: ast_two\nroute: pickup\nreason: it arrives hotter\n");
        assert_eq!(p.pick.as_deref(), Some("ast_two"));
        assert_eq!(p.route, Some(Route::Pickup));
        assert_eq!(p.reason, "it arrives hotter");
    }

    #[test]
    fn the_packaging_a_model_wraps_it_in_does_not_matter() {
        // Fences, bullets, capitals, quotes and a paragraph of throat
        // clearing. Throwing a good answer away over its wrapping is how a
        // parser makes a model look worse than it is.
        let p = parse(
            "Sure! Here is my suggestion:\n\
             ```\n\
             - Pick: \"ast_one\"\n\
             - ROUTE: Handover\n\
             ```\n\
             Hope that helps.\n",
        );
        assert_eq!(p.pick.as_deref(), Some("ast_one"));
        assert_eq!(p.route, Some(Route::Handover));
    }

    #[test]
    fn nonsense_is_skipped_rather_than_guessed_at() {
        let p = parse("route: interpretive dance\npick:\n\nmumble\n");
        assert_eq!(p.route, None, "an unknown shape is not a shape");
        assert_eq!(p.pick, None, "an empty value is not a value");
        assert_eq!(validate(&p, &offered(), &[Route::Handover]), None);
    }

    #[test]
    fn a_record_that_was_never_offered_is_refused() {
        // The failure every surveyed system had: a confident answer about a
        // library that does not exist.
        let p = parse("pick: ast_invented\nreason: trust me\n");
        assert_eq!(validate(&p, &offered(), &[Route::Handover]), None);
    }

    #[test]
    fn a_shape_the_engine_cannot_perform_is_refused() {
        let p = parse("route: pickup\n");
        assert_eq!(validate(&p, &offered(), &[Route::Handover, Route::Short]), None);
        // And is accepted the moment it is on the menu.
        let ok = validate(&p, &offered(), &[Route::Pickup]).expect("offered");
        assert_eq!(ok.route, Some(Route::Pickup));
    }

    #[test]
    fn half_an_answer_is_still_an_answer() {
        // A record it is sure about and a shape it is not: take the half
        // that survives rather than throwing the turn away.
        let p = parse("pick: ast_one\nroute: nonsense\nreason: it follows well\n");
        let ok = validate(&p, &offered(), &[Route::Handover]).expect("the pick stands");
        assert_eq!(ok.pick.as_deref(), Some("ast_one"));
        assert_eq!(ok.route, None);
        assert_eq!(ok.reason, "it follows well");
    }

    #[test]
    fn silence_and_rubbish_are_the_same_answer_as_no_advisor() {
        for text in ["", "   \n\n", "I'm sorry, I can't help with that.", "{}"] {
            let p = parse(text);
            assert_eq!(validate(&p, &offered(), &Route::ALL), None, "{text:?}");
        }
    }

    #[test]
    fn the_question_names_every_id_and_shape_it_will_accept() {
        // The model can only get a choice wrong if it was offered it, so
        // the prompt and the gate have to agree about what is on the table.
        let q = Question {
            offered: vec!["ast_one".to_string(), "ast_two".to_string()],
            described: vec!["128 BPM, 8A".to_string(), "126 BPM, 9A".to_string()],
            eligible: vec![Route::Handover, Route::Short],
            mood: "keep it moving".to_string(),
        };
        let prompt = q.prompt();
        for id in &q.offered {
            assert!(prompt.contains(id), "the id has to be in front of it: {id}");
        }
        assert!(prompt.contains("128 BPM"), "and what is known about it");
        assert!(prompt.contains("keep it moving"), "and what was asked for");
        assert!(prompt.contains("handover") && prompt.contains("short"));
        assert!(!prompt.contains("pickup"), "a shape not on offer is not named");
        // An answer in the shape the prompt asks for has to survive the gate
        // it will be judged by — the two cannot drift apart.
        let answer = parse("pick: ast_two\nroute: short\nreason: it keeps moving\n");
        let ok = validate(&answer, &q.offered, &q.eligible).expect("its own form");
        assert_eq!(ok.pick.as_deref(), Some("ast_two"));
        assert_eq!(ok.route, Some(Route::Short));
    }

    #[test]
    fn a_long_reason_is_cut_to_something_a_panel_can_show() {
        let p = parse(&format!("pick: ast_one\nreason: {}\n", "x".repeat(400)));
        assert!(p.reason.chars().count() <= REASON_CHARS);
    }

    #[test]
    fn an_answer_that_never_stops_is_not_read_forever() {
        let flood = "noise: x\n".repeat(10_000);
        let p = parse(&format!("pick: ast_one\n{flood}"));
        assert_eq!(p.pick.as_deref(), Some("ast_one"), "the useful line came first");
    }
}
