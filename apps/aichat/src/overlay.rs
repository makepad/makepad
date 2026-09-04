//! The chat's module root: what a host seats in-process.
//!
//! `mod.widgets.AiChatOverlay{}` is what the Window's AI slot
//! (`widgets/src/ai_slot.rs`) instantiates by name on F10, and what the
//! superbuild seats in its pane. It is a `View` with the panel in it and
//! three duties around it: adopt the in-process service links the apps
//! parked on `Cx` ([`PendingServiceLinks`]) into the panel's registry,
//! send the lines the bridge asked to say ([`AiSlotRequests::say`]),
//! and publish the transcript as JSON ([`AiTranscript`]) after each draw
//! so `/ai/transcript` answers without touching a widget. The panel's
//! Escape (an idle, empty composer) asks the slot to close.

use crate::panel::{AiChatPanel, AiChatPanelAction};
use makepad_ai_services::port::PendingServiceLinks;
use makepad_ai_services::state::{Entry, EngineState, Status, ToolStatus};
use makepad_widgets::ai_slot::AiSlotRequests;
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiChatOverlayBase = #(AiChatOverlay::register_widget(vm))
    mod.widgets.AiChatOverlay = set_type_default() do mod.widgets.AiChatOverlayBase{
        width: Fill
        height: Fill
        panel := AiChatPanel{
            width: Fill
            height: Fill
        }
    }
}

/// The transcript as the bridge reads it: a `Cx` global the overlay
/// rewrites after every draw of the panel.
#[derive(Default)]
pub struct AiTranscript {
    pub json: String,
}

#[derive(SerJson)]
struct TranscriptRow {
    kind: String,
    text: String,
    title: String,
    status: String,
    note: String,
}

#[derive(SerJson)]
struct Transcript {
    status: String,
    provider: String,
    apps: Vec<String>,
    entries: Vec<TranscriptRow>,
    generation: u64,
}

/// The transcript JSON for a state: `status`, `provider`, the connected
/// apps, and one row per entry with what its card says.
pub fn transcript_json(state: &EngineState) -> String {
    let status = match &state.status {
        Status::Idle => "idle".to_string(),
        Status::Loading { phase, fraction } => format!("loading {phase} {:.0}%", fraction * 100.0),
        Status::Thinking => "thinking".to_string(),
        Status::Streaming => "streaming".to_string(),
        Status::WaitingForTool => "waiting_for_tool".to_string(),
        Status::Error(e) => format!("error: {e}"),
    };
    let entries = state
        .entries
        .iter()
        .map(|entry| match entry {
            Entry::User { text } => TranscriptRow {
                kind: "user".into(),
                text: text.clone(),
                title: String::new(),
                status: String::new(),
                note: String::new(),
            },
            Entry::Event(event) => TranscriptRow {
                kind: "event".into(),
                text: event.text.clone(),
                title: format!("{} · {}", event.service_label, event.topic),
                status: if event.final_ { "final".into() } else { "message".into() },
                note: if event.dropped == 0 {
                    format!("sub_id: {}", event.sub_id)
                } else {
                    format!("sub_id: {} · dropped: {}", event.sub_id, event.dropped)
                },
            },
            Entry::Assistant { text, streaming } => TranscriptRow {
                kind: "assistant".into(),
                text: text.clone(),
                title: String::new(),
                status: if *streaming { "streaming".into() } else { "done".into() },
                note: String::new(),
            },
            Entry::Tool(t) => {
                let (status, note, text) = match &t.status {
                    ToolStatus::Confirm => ("confirm".to_string(), String::new(), String::new()),
                    ToolStatus::Running { note, permille } => ("running".to_string(), format!("{note} {permille}‰"), String::new()),
                    ToolStatus::Done { outcome, note, text } => (outcome.slug().to_string(), note.clone(), text.clone()),
                };
                TranscriptRow { kind: "tool".into(), text, title: t.title.clone(), status, note }
            }
            Entry::System { text } => TranscriptRow {
                kind: "system".into(),
                text: text.clone(),
                title: String::new(),
                status: String::new(),
                note: String::new(),
            },
        })
        .collect();
    Transcript {
        status,
        provider: state.provider_label.clone(),
        apps: state.services.iter().filter(|s| s.connected).map(|s| s.label.clone()).collect(),
        entries,
        generation: state.generation,
    }
    .serialize_json()
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatOverlay {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The state generation last published as JSON.
    #[rust]
    published: Option<u64>,
}

impl AiChatOverlay {
    /// The transcript JSON, rewritten when the engine's state moved — on
    /// every event as well as every draw, so a closed slot (no draws) still
    /// answers `/ai/transcript` with the turn that finished behind it.
    fn publish_transcript(&mut self, cx: &mut Cx) {
        let json = self
            .view
            .widget(cx, ids!(panel))
            .borrow::<AiChatPanel>()
            .and_then(|panel| {
                let state = panel.state()?;
                if self.published == Some(state.generation) {
                    return None;
                }
                Some((state.generation, transcript_json(state)))
            });
        if let Some((generation, json)) = json {
            self.published = Some(generation);
            cx.global::<AiTranscript>().json = json;
        }
    }

    /// The links the apps parked and the lines the bridge asked for, into
    /// the panel.
    fn adopt_requests(&mut self, cx: &mut Cx) {
        let links = cx.global::<PendingServiceLinks>().take();
        let says = std::mem::take(&mut cx.global::<AiSlotRequests>().say);
        if links.is_empty() && says.is_empty() {
            return;
        }
        let panel = self.view.widget(cx, ids!(panel));
        let Some(mut panel) = panel.borrow_mut::<AiChatPanel>() else {
            return;
        };
        for link in links {
            let label = link.manifest.label.clone();
            match panel.registry().register(link, "in this window", None) {
                Ok(endpoint) => log!("aichat: {label} joined in-process as {}", endpoint.as_str()),
                Err(e) => log!("aichat: {label} refused: {e}"),
            }
        }
        for text in says {
            panel.say(cx, text);
        }
    }
}

impl Widget for AiChatOverlay {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.adopt_requests(cx);
        self.view.handle_event(cx, event, scope);
        self.publish_transcript(cx);
        if let Event::Actions(actions) = event {
            for action in actions {
                if let Some(widget_action) = action.as_widget_action() {
                    if let AiChatPanelAction::Close = widget_action.cast() {
                        cx.global::<AiSlotRequests>().open = Some(false);
                        cx.redraw_all();
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if step.is_done() {
            self.publish_transcript(cx);
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::state::{EventEntry, ServiceInfo, ToolEntry};
    use makepad_ai_services::wire::ToolOutcome;

    #[test]
    fn the_transcript_reads_as_json_rows() {
        let mut state = EngineState::default();
        state.provider_label = "No model".into();
        state.services.push(ServiceInfo {
            id: "sheets".into(),
            endpoint: "e1".into(),
            label: "Sheets".into(),
            parent: None,
            location: "in this window".into(),
            connected: true,
            launchable: false,
            tool_count: 1,
        });
        state.entries.push(Entry::User { text: "/sheets.summary {}".into() });
        state.entries.push(Entry::Event(EventEntry {
            sub_id: "s1".into(),
            service_label: "Sheets".into(),
            topic: "changes".into(),
            text: "A1 changed".into(),
            data: None,
            dropped: 2,
            final_: false,
        }));
        state.entries.push(Entry::Tool(ToolEntry {
            call_id: "c1".into(),
            service: "sheets".into(),
            service_label: "Sheets".into(),
            tool: "summary".into(),
            title: "Sheets · summary".into(),
            args: "{}".into(),
            status: ToolStatus::Done { outcome: ToolOutcome::Ok, note: "3 × 4".into(), text: "Sheet1, 3 rows".into() },
            preview: false,
            expanded: false,
        }));
        state.generation = 7;
        let json = transcript_json(&state);
        assert!(json.contains(r#""status":"idle""#), "{json}");
        assert!(json.contains(r#""apps":["Sheets"]"#), "{json}");
        assert!(json.contains(r#""kind":"user""#) && json.contains(r#""kind":"tool""#), "{json}");
        assert!(json.contains(r#""kind":"event""#) && json.contains("sub_id: s1") && json.contains("dropped: 2"), "{json}");
        assert!(json.contains(r#""title":"Sheets · summary""#) && json.contains(r#""status":"ok""#), "{json}");
        assert!(json.contains(r#""generation":7"#), "{json}");
    }
}
