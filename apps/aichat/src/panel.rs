//! The chat panel: the one widget every host shows. It owns the engine —
//! the service registry, the model, the transcript — and draws
//! `EngineState` as a transcript of user lines, assistant text, tool cards
//! (running, done, waiting for a confirm), and system lines, over a
//! composer.
//!
//! Hosts talk to it in two ways: they hand it service links
//! (`registry()`) or bus frames (`on_custom`), and they listen for
//! [`AiChatPanelAction`]s. The engine runs on the panel's own events: every
//! event pumps it, and while a turn is in flight the panel asks for the
//! next frame so streaming, deadlines and the cloud provider's polling all
//! advance without a host timer.

use crate::bus::ServiceBus;
use crate::settings::AiSettings;
#[cfg(feature = "engine")]
use makepad_ai_services::engine::models::{build_model, provider_rows};
use makepad_ai_services::engine::NoModelWithReason;
use makepad_ai_services::engine::{EngineCore, EngineEvent, ServiceRegistry};
use makepad_ai_services::state::*;
use makepad_ai_services::wire::ToolOutcome;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiChatPanelBase = #(AiChatPanel::register_widget(vm))

    let Line = Label{
        width: Fill
        height: Fit
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let Row = View{
        width: Fill
        height: Fit
        flow: Down
        padding: Inset{left: 14 right: 14 top: 4 bottom: 4}
    }

    mod.widgets.AiChatPanel = set_type_default() do mod.widgets.AiChatPanelBase{
        width: Fill
        height: Fill
        flow: Down
        draw_bg +: { color: theme.color_bg_app }

        header := SolidView{
            width: Fill
            height: 36
            flow: Right
            spacing: 10
            padding: Inset{left: 14 right: 8}
            align: Align{y: 0.5}
            draw_bg +: { color: theme.color_bg_container }
            title := Label{
                text: "AI"
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_bold{font_size: 10.5}
                }
            }
            provider := Label{
                width: Fill
                text: ""
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
            clear_button := ButtonFlatter{ text: "Clear" }
        }

        apps_row := Label{
            width: Fill
            height: Fit
            padding: Inset{left: 14 right: 14 top: 6 bottom: 2}
            max_lines: 2
            text: ""
            draw_text +: {
                color: theme.color_text_meta
                text_style: theme.font_regular{font_size: 8.5}
            }
        }

        transcript := PortalList{
            width: Fill
            height: Fill
            auto_tail: true

            UserRow := Row{
                padding: Inset{left: 14 right: 14 top: 10 bottom: 4}
                user_text := Line{
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_bold{font_size: 9.5}
                    }
                }
            }
            AssistantRow := Row{
                assistant_md := Markdown{
                    width: Fill
                    height: Fit
                    body: ""
                }
            }
            StreamRow := Row{
                stream_text := Line{}
            }
            ToolRow := Row{
                padding: Inset{left: 22 right: 14 top: 3 bottom: 3}
                tool_head := View{
                    width: Fill
                    height: Fit
                    cursor: MouseCursor.Hand
                    tool_title := Line{
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                }
                tool_note := Line{
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
                tool_bar := SolidView{
                    width: Fill
                    height: 2
                    margin: Inset{top: 3}
                    draw_bg +: { color: theme.color_text_hl }
                }
                tool_detail := Line{
                    visible: false
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
            }
            ConfirmRow := Row{
                padding: Inset{left: 22 right: 14 top: 6 bottom: 6}
                confirm_title := Line{
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 9.0}
                    }
                }
                confirm_buttons := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 8
                    margin: Inset{top: 4}
                    run_button := Button{ text: "Run" }
                    deny_button := ButtonFlat{ text: "Cancel" }
                }
            }
            SystemRow := Row{
                system_text := Line{
                    draw_text +: {
                        color: theme.color_text_hl
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
            }
        }

        status := Label{
            width: Fill
            height: Fit
            padding: Inset{left: 14 right: 14 top: 4 bottom: 2}
            max_lines: 2
            text: ""
            draw_text +: {
                color: theme.color_text_meta
                text_style: theme.font_regular{font_size: 8.5}
            }
        }

        composer := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            padding: Inset{left: 12 right: 12 top: 6 bottom: 10}
            align: Align{y: 0.5}
            input := TextInput{
                width: Fill
                height: Fit
                empty_text: "Ask AI"
                // The prompt is a hint, not text: a dark grey in every state,
                // never the typed colour (the composer is always focused).
                draw_text +: {
                    color_empty: #666666
                    color_empty_hover: #777777
                    color_empty_focus: #666666
                }
            }
            send_button := Button{ text: "Send" }
        }
    }
}

/// What the panel tells its host.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AiChatPanelAction {
    /// Esc with an empty composer and no turn in flight: the host may hide
    /// the pane.
    Close,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    engine: Option<EngineCore>,
    #[rust]
    registry: ServiceRegistry,
    #[rust]
    bus: ServiceBus,
    #[rust]
    settings: Option<AiSettings>,
    #[rust]
    drawn_generation: u64,
    #[rust]
    next_frame: NextFrame,
    /// The composer took the keyboard once it existed on screen — a
    /// focus set before the first draw lands on no area at all.
    #[rust]
    composer_focused: bool,
}

impl AiChatPanel {
    /// The registry a host plugs in-process links into.
    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    /// A studio `Custom` frame that may be a bus frame from the WM.
    pub fn on_custom(&mut self, json: &str) -> bool {
        self.bus.on_custom(&self.registry, json)
    }

    pub fn state(&self) -> Option<&EngineState> {
        self.engine.as_ref().map(|e| e.state())
    }

    fn settings(&mut self) -> &AiSettings {
        if self.settings.is_none() {
            self.settings = Some(AiSettings::load());
        }
        self.settings.as_ref().unwrap()
    }

    /// The engine comes up on first use — a local model is a long load
    /// and no host pays it before the person opens the pane.
    fn ensure_engine(&mut self) -> &mut EngineCore {
        if self.engine.is_none() {
            let settings = self.settings().clone();
            // The real models ride the `engine` feature; a build without a
            // runtime (the web page) says so and keeps the tool console.
            #[cfg(feature = "engine")]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) = (
                match build_model(&settings.provider, settings.local_only) {
                    Ok(m) => m,
                    Err(reason) => Box::new(NoModelWithReason::new(reason)),
                },
                provider_rows(settings.local_only),
            );
            #[cfg(not(feature = "engine"))]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) =
                (Box::new(NoModelWithReason::new("this build has no model runtime")), Vec::new());
            let mut core = EngineCore::new(self.registry.clone(), model, None);
            // The state is the panel's window into the core; the core owns
            // it, so provider facts go in through the core.
            core.set_provider_facts(settings.provider.clone(), rows, settings.local_only);
            self.engine = Some(core);
        }
        self.engine.as_mut().unwrap()
    }

    fn now(cx: &Cx) -> f64 {
        cx.seconds_since_app_start()
    }

    /// A line as if typed and sent — the bridge's `/ai?say=`.
    pub fn say(&mut self, cx: &mut Cx, text: String) {
        self.send(cx, text);
    }

    fn send(&mut self, cx: &mut Cx, text: String) {
        let now = Self::now(cx);
        self.ensure_engine().send(&text, now);
        let input = self.view.text_input(cx, ids!(input));
        input.set_text(cx, "");
        // The widget drops the keyboard on submit; a chat composer keeps
        // it, so the next line can be typed straight away.
        input.take_key_focus(cx);
        self.view.redraw(cx);
    }

    fn apps_line(&self, state: &EngineState) -> String {
        if state.services.is_empty() {
            return "No apps connected.".into();
        }
        state
            .services
            .iter()
            .map(|s| if s.connected { s.label.clone() } else { format!("{} (not running)", s.label) })
            .collect::<Vec<_>>()
            .join("  ·  ")
    }

    fn status_line(state: &EngineState) -> String {
        let rate = state.rate.map(|r| format!("  ·  {r:.0} tok/s")).unwrap_or_default();
        match &state.status {
            Status::Idle => rate.trim_start_matches("  ·  ").to_string(),
            Status::Loading { phase, fraction } => format!("loading {phase} {:.0}%", fraction * 100.0),
            Status::Thinking => {
                if state.thinking.is_empty() {
                    "thinking…".to_string()
                } else {
                    let tail: String = state.thinking.chars().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("thinking… {tail}")
                }
            }
            Status::Streaming => format!("writing{rate}"),
            Status::WaitingForTool => "waiting for the app…".to_string(),
            Status::Error(e) => e.clone(),
        }
    }
}

impl Widget for AiChatPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Custom(json) = event {
            self.on_custom(json);
        }
        // Esc: stop a running turn; with nothing running and an empty
        // composer, ask the host to hide the pane.
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape {
                let busy = self.engine.as_ref().map(|e| e.state().status.is_busy()).unwrap_or(false);
                let empty = self.view.text_input(cx, ids!(input)).text().trim().is_empty();
                if busy {
                    let now = Self::now(cx);
                    self.ensure_engine().cancel(now);
                    self.view.redraw(cx);
                } else if empty {
                    cx.widget_action(self.widget_uid(), AiChatPanelAction::Close);
                }
            }
        }
        // Drive the engine on every event; the bus relays what it sent.
        let now = Self::now(cx);
        let mut changed = false;
        let mut busy = false;
        if let Some(engine) = self.engine.as_mut() {
            for ev in engine.pump(now) {
                match ev {
                    EngineEvent::Changed => changed = true,
                    EngineEvent::Confirm { .. } => changed = true,
                }
            }
            busy = engine.state().status.is_busy() || matches!(engine.state().status, Status::Loading { .. });
        }
        self.bus.relay_down(&self.registry);
        if changed {
            self.view.redraw(cx);
        }
        if busy {
            self.next_frame = cx.new_next_frame();
        }
        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The engine exists once the panel is on screen.
        let _ = self.ensure_engine();
        let state = self.engine.as_ref().unwrap().state().clone();
        self.drawn_generation = state.generation;
        self.view.label(cx, ids!(provider)).set_text(cx, &format!("{}{}", state.provider_label, if state.local_only { "  ·  local only" } else { "" }));
        self.view.label(cx, ids!(apps_row)).set_text(cx, &self.apps_line(&state));
        self.view.label(cx, ids!(status)).set_text(cx, &Self::status_line(&state));
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else { continue };
            let total = state.entries.len();
            list.set_item_range(cx, 0, total);
            while let Some(index) = list.next_visible_item(cx) {
                if index >= total {
                    continue;
                }
                let entry = &state.entries[index];
                let row = match entry {
                    Entry::User { text } => {
                        let row = list.item(cx, index, id!(UserRow));
                        row.label(cx, ids!(user_text)).set_text(cx, text);
                        row
                    }
                    Entry::Assistant { text, streaming: false } => {
                        let row = list.item(cx, index, id!(AssistantRow));
                        // No text, no row: a blank block above a card is a gap.
                        row.set_visible(cx, !text.trim().is_empty());
                        if let Some(mut md) = row.widget(cx, ids!(assistant_md)).borrow_mut::<Markdown>() {
                            md.set_text(cx, text);
                        }
                        row
                    }
                    Entry::Assistant { text, streaming: true } => {
                        let row = list.item(cx, index, id!(StreamRow));
                        row.set_visible(cx, !text.trim().is_empty());
                        row.label(cx, ids!(stream_text)).set_text(cx, text);
                        row
                    }
                    Entry::Tool(t) if matches!(t.status, ToolStatus::Confirm) => {
                        let row = list.item(cx, index, id!(ConfirmRow));
                        row.label(cx, ids!(confirm_title)).set_text(cx, &format!("{}  —  this changes things outside the app. Run it?", t.title));
                        row
                    }
                    Entry::Tool(t) => {
                        let row = list.item(cx, index, id!(ToolRow));
                        let (glyph, note, permille, detail) = match &t.status {
                            ToolStatus::Running { note, permille } => ("›", note.clone(), *permille, String::new()),
                            ToolStatus::Done { outcome, note, text } => {
                                let glyph = if outcome.is_ok() { "✓" } else { "✗" };
                                let note = if note.is_empty() { outcome.slug().to_string() } else { note.clone() };
                                (glyph, note, 1000, text.clone())
                            }
                            ToolStatus::Confirm => ("?", String::new(), 0, String::new()),
                        };
                        row.label(cx, ids!(tool_title)).set_text(cx, &format!("{glyph} {}", t.title));
                        row.label(cx, ids!(tool_note)).set_text(cx, &note);
                        let bar_visible = matches!(t.status, ToolStatus::Running { .. });
                        row.view(cx, ids!(tool_bar)).set_visible(cx, bar_visible);
                        if bar_visible {
                            // The bar is a fraction of the row's inner width
                            // (the row's rect is last frame's; the first
                            // frame of a card draws it at a token width).
                            let inner = (row.area().rect(cx).size.x - 36.0).max(24.0);
                            let px = inner * (permille as f64 / 1000.0).max(0.05);
                            let mut bar = row.view(cx, ids!(tool_bar));
                            script_apply_eval!(cx, bar, { width: #(px) });
                        }
                        let detail_label = row.label(cx, ids!(tool_detail));
                        detail_label.set_visible(cx, t.expanded && !detail.is_empty());
                        if t.expanded {
                            detail_label.set_text(cx, &format!("{}\n{}", t.args, detail));
                        }
                        let _ = matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. });
                        row
                    }
                    Entry::System { text } => {
                        let row = list.item(cx, index, id!(SystemRow));
                        row.label(cx, ids!(system_text)).set_text(cx, text);
                        row
                    }
                };
                row.draw_all(cx, &mut Scope::empty());
            }
        }
        if !self.composer_focused {
            self.composer_focused = true;
            self.view.text_input(cx, ids!(input)).take_key_focus(cx);
        }
        DrawStep::done()
    }
}

impl WidgetMatchEvent for AiChatPanel {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let input = self.view.text_input(cx, ids!(input));
        let returned = input.returned(actions).map(|(text, _)| text);
        if let Some(text) = returned {
            self.send(cx, text);
        } else if self.view.button(cx, ids!(send_button)).clicked(actions) {
            let text = input.text();
            self.send(cx, text);
        }
        if self.view.button(cx, ids!(clear_button)).clicked(actions) {
            let now = Self::now(cx);
            self.ensure_engine().clear(now);
            self.view.redraw(cx);
        }
        // Cards: confirm buttons and click-to-expand.
        let list = self.view.portal_list(cx, ids!(transcript));
        let items = list.items_with_actions(actions);
        if !items.is_empty() {
            let now = Self::now(cx);
            let call_ids: Vec<Option<String>> = {
                let state = self.ensure_engine().state();
                items
                    .iter()
                    .map(|(index, _)| match state.entries.get(*index) {
                        Some(Entry::Tool(t)) => Some(t.call_id.clone()),
                        _ => None,
                    })
                    .collect()
            };
            for ((_, item), call_id) in items.iter().zip(call_ids) {
                let Some(call_id) = call_id else { continue };
                if item.button(cx, ids!(run_button)).clicked(actions) {
                    self.ensure_engine().confirm(&call_id, true, now);
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(deny_button)).clicked(actions) {
                    self.ensure_engine().confirm(&call_id, false, now);
                    self.view.redraw(cx);
                } else if item.view(cx, ids!(tool_head)).finger_up(actions).is_some() {
                    self.ensure_engine().toggle_tool(&call_id);
                    self.view.redraw(cx);
                }
            }
        }
        let _ = scope;
    }
}
