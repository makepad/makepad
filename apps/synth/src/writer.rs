//! The writers: one chat turn with a system text, answered by the model the
//! person picked in the chat (`makepad_app_aichat::AiSettings`: the hub's
//! local model, their own Claude Code or Codex, the Claude API, under the
//! local-only lock). The editor panel's makers, the melody variants and the
//! batch's composers (`music_iter::gen::Writer`) all write through it.

use makepad_ai_services::engine::models::build_model;
use makepad_ai_services::engine::ModelEvent;
use makepad_music_iter::gen::{TurnMonitor, Writer};
use std::sync::atomic::{AtomicBool, Ordering};

/// One turn: `turns` alternate user, assistant, user …; the last is the
/// ask. Each piece of the answer goes to `delta` as it comes.
pub fn turn(system: &str, turns: &[String], cancel: &AtomicBool, monitor: &TurnMonitor, delta: &mut dyn FnMut(&str)) -> Result<String, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("stopped".into());
    }
    let settings = makepad_app_aichat::AiSettings::load();
    let mut model = build_model(&settings.provider, settings.local_only).map_err(|why| format!("no model to write with ({why}): pick one in the chat"))?;
    model.configure(system, &[])?;
    // A stateless turn: the conversation so far is told in the ask.
    let text = match turns.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, before)) => {
            let mut text = String::from("The conversation so far:\n");
            for (i, t) in before.iter().enumerate() {
                text.push_str(if i % 2 == 0 { "\n[you were asked]\n" } else { "\n[you answered]\n" });
                text.push_str(t);
            }
            text.push_str("\n\n[now]\n");
            text.push_str(last);
            text
        }
        None => return Err("nothing to ask".into()),
    };
    let activity = monitor.begin();
    model.send_user(&text, "");
    let mut out = String::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            model.cancel();
            return Err("stopped".into());
        }
        let events = model.poll();
        if !events.is_empty() {
            activity.progress();
        }
        for event in events {
            match event {
                ModelEvent::Delta(piece) => {
                    out.push_str(&piece);
                    delta(&piece);
                }
                // The writers are given no tools.
                ModelEvent::ToolCall { call_id, .. } => model.send_tool_result(&call_id, "no tools here: answer in text", true),
                ModelEvent::TurnDone { .. } => return Ok(out),
                ModelEvent::Error(error) => return Err(error),
                _ => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The batch's and the learner's writer.
#[derive(Default)]
pub struct AiWriter {
    pub monitor: TurnMonitor,
}

impl Writer for AiWriter {
    fn chat(&self, system: &str, turns: &[String], cancel: &AtomicBool) -> Result<String, String> {
        turn(system, turns, cancel, &self.monitor, &mut |_| {})
    }

    fn label(&self) -> String {
        let settings = makepad_app_aichat::AiSettings::load();
        format!("the chat's model ({})", settings.provider.slug())
    }
}
