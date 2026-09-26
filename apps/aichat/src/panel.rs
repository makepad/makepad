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

use crate::attach::{self, Attachment, Source};
use crate::bus::ServiceBus;
#[cfg(feature = "gen")]
use crate::gen::GenService;
use crate::settings::AiSettings;
#[cfg(feature = "engine")]
use makepad_ai_services::engine::models::{build_model, fallback_choice, provider_rows};
use makepad_ai_services::engine::NoModelWithReason;
use makepad_ai_services::engine::{EngineCore, EngineEvent, ModelImage, ServiceRegistry};
use makepad_ai_services::mcp::host::{McpActivity, McpHost};
use makepad_ai_services::mcp::mcpb;
use makepad_ai_services::state::*;
use makepad_ai_services::wire::ToolOutcome;
use makepad_widgets::ai_slot::AiSlotRequests;
use makepad_widgets::makepad_platform::mcp_relay::{self, CLAUDE_DESKTOP};
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
            // Which model answers: the choices that exist on this machine.
            provider_pick := DropDown{
                width: 150
                labels: ["Model"]
            }
            clear_button := ButtonFlatter{ text: "Clear" }
        }

        // Claude Desktop is the provider: this app's tools are served to it.
        mcp_row := View{
            visible: false
            width: Fill
            height: Fit
            flow: Down
            spacing: 4
            padding: Inset{left: 14 right: 14 top: 8 bottom: 4}
            mcp_connect := Button{ text: "Connect to Claude Desktop" }
            mcp_status := Label{
                width: Fill
                text: ""
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
            mcp_hint := Label{
                width: Fill
                text: "Type in Claude Desktop; its calls show here."
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
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
            EventRow := Row{
                margin: Inset{left: 10 right: 10 top: 5 bottom: 5}
                padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                draw_bg +: { color: theme.color_bg_container }
                event_title := Line{
                    draw_text +: {
                        color: theme.color_text_hl
                        text_style: theme.font_bold{font_size: 8.5}
                    }
                }
                event_text := Line{}
                event_meta := Line{
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: 8.0}
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

        // Pictures dropped on the panel, sent with the next line.
        attach_row := View{
            visible: false
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            padding: Inset{left: 12 right: 12 top: 4 bottom: 0}
            align: Align{y: 0.5}
            attach_thumb := Image{width: 44 height: 32 fit: ImageFit.Smallest}
            attach_label := Label{
                width: Fill
                text: ""
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
            attach_clear := ButtonFlatter{ text: "Remove" }
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
                    color_empty: mod.theme.color_text_disabled
                    color_empty_hover: mod.theme.color_text_disabled
                    color_empty_focus: mod.theme.color_text_disabled
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
    /// The host's model for this panel, by provider slug (`claude-cli`,
    /// `codex-cli`, `local`, …); empty keeps the person's saved choice.
    /// `MAKEPAD_AI_PROVIDER` overrides both.
    #[live]
    provider: String,
    /// Pictures waiting for the next line.
    #[rust]
    attachments: Vec<Attachment>,
    /// Dropped pictures being decoded on the task pool.
    #[rust]
    loading: Vec<TaskHandle<Result<Attachment, String>>>,
    /// The attachment chip shows this many pictures.
    #[rust]
    attach_shown: Option<(usize, usize)>,
    /// The choices the provider menu lists, in its order.
    #[rust]
    provider_rows: Vec<ProviderRow>,
    /// The provider menu shows these rows with this one picked.
    #[rust]
    pick_shown: Option<(usize, usize)>,
    /// A line for the status row until the next send (a picture that
    /// could not be read, a provider that is not installed).
    #[rust]
    notice: Option<String>,
    #[rust]
    engine: Option<EngineCore>,
    #[rust]
    registry: ServiceRegistry,
    #[rust]
    bus: ServiceBus,
    /// The assistant's own `gen` service (pictures from the fleet), joined
    /// to the registry with the engine.
    #[cfg(feature = "gen")]
    #[rust]
    gen: Option<GenService>,
    #[rust]
    settings: Option<AiSettings>,
    /// This app's tools served to Claude Desktop, while it is the provider.
    #[rust]
    mcp: Option<McpHost>,
    /// The connection as the panel last drew it.
    #[rust]
    mcp_shown: McpActivity,
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
            let lease_id = self.widget_uid().0;
            let mut settings = self.settings().clone();
            // A host (or the environment) naming its model picks it for
            // this panel: that is the person's explicit choice for the app.
            let host = std::env::var("MAKEPAD_AI_PROVIDER").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.provider.clone());
            if !host.trim().is_empty() {
                settings.provider = ProviderChoice::from_slug(&host);
                settings.local_only = settings.provider.is_local() || settings.provider.slug() == "none";
            } else {
                // The saved (or default) choice is not built into this app
                // (Local without `localai`): start on the first provider
                // that can answer here. Only this panel's copy changes, so
                // a `localai` build still starts on Local; the lock guards
                // a local model this build does not have.
                #[cfg(feature = "engine")]
                if let Some(choice) = fallback_choice(&settings.provider) {
                    settings.provider = choice;
                    settings.local_only = false;
                }
            }
            // The real models ride the `engine` feature; a build without a
            // runtime (the web page) says so and keeps the tool console.
            #[cfg(feature = "engine")]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) = (
                match build_model(&settings.provider, settings.local_only) {
                    Ok(m) => m,
                    Err(reason) => Box::new(NoModelWithReason::new(reason)),
                },
                provider_rows(false),
            );
            #[cfg(not(feature = "engine"))]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) =
                (Box::new(NoModelWithReason::new("this build has no model runtime")), Vec::new());
            let mut core = EngineCore::new(self.registry.clone(), model, None, lease_id);
            // The state is the panel's window into the core; the core owns
            // it, so provider facts go in through the core.
            self.provider_rows = rows.clone();
            core.set_provider_facts(settings.provider.clone(), rows, settings.local_only);
            self.engine = Some(core);
            #[cfg(feature = "gen")]
            {
                self.gen = GenService::open(&self.registry);
            }
            self.apply_mcp();
        }
        self.engine.as_mut().unwrap()
    }

    /// Serve this app's tools to Claude Desktop exactly while it is the
    /// provider: started on the switch to it, stopped (and the discovery
    /// file removed) on the switch away.
    fn apply_mcp(&mut self) {
        let wanted = self.engine.as_ref().is_some_and(|e| e.state().provider.slug() == CLAUDE_DESKTOP);
        if !wanted {
            if self.mcp.take().is_some() {
                // Its waiting clients are answered before the server stops.
                if let Some(engine) = self.engine.as_mut() {
                    engine.cancel_external();
                }
            }
            return;
        }
        if self.mcp.is_none() {
            match McpHost::start(self.registry.clone(), &self.app_title()) {
                Ok(host) => self.mcp = Some(host),
                Err(e) => self.notice = Some(format!("the Claude Desktop connection could not start: {e}")),
            }
        }
    }

    /// The app as Claude Desktop names it: its own service's label (not
    /// the assistant's built-in ones), else the executable's name.
    fn app_title(&self) -> String {
        self.registry
            .services()
            .into_iter()
            .find(|s| s.connected && !Self::own_service(&s.id))
            .map(|s| s.label)
            .unwrap_or_else(mcp_relay::app_id)
    }

    fn own_service(id: &str) -> bool {
        #[cfg(feature = "gen")]
        if id == crate::gen::SERVICE_ID {
            return true;
        }
        let _ = id;
        false
    }

    /// Hand Claude Desktop's waiting calls to the engine; keep the title
    /// in the discovery file current. True when the panel should redraw.
    fn pump_mcp(&mut self, cx: &mut Cx, now: f64) -> bool {
        if self.mcp.is_none() {
            return false;
        }
        let title = self.app_title();
        let Some(mcp) = self.mcp.as_mut() else { return false };
        if let Err(e) = mcp.set_title(&title) {
            error!("aichat: the Claude Desktop discovery file: {e}");
        }
        let calls = mcp.take_calls();
        let activity = mcp.activity();
        let mut changed = activity != self.mcp_shown;
        for call in calls {
            if let Some(EngineEvent::Confirm { .. }) = self.ensure_engine().call_external(&call.name, &call.args, call.reply, now) {
                // The person confirms in the pane: bring it up.
                cx.global::<AiSlotRequests>().open = Some(true);
                cx.new_next_frame();
            }
            changed = true;
        }
        changed
    }

    /// Write this app's Claude Desktop extension and open it: Claude
    /// Desktop shows its install dialog.
    fn connect_claude_desktop(&mut self, cx: &mut Cx) {
        let Some(mcp) = self.mcp.as_ref() else { return };
        let written = std::env::current_exe()
            .map_err(|e| format!("cannot find this app's executable: {e}"))
            .and_then(|exe| mcpb::write(&mcp_relay::discovery_dir(), mcp.app_id(), &mcp.title(), &exe, &mcp.tools()))
            .and_then(|path| mcpb::open_in_claude_desktop(&path).map(|_| path));
        match written {
            Ok(_) => self.note(cx, "Claude Desktop asks to install it: click Install, then type in Claude Desktop".into()),
            Err(e) => self.note(cx, e),
        }
    }

    fn mcp_status(activity: &McpActivity) -> String {
        if activity.requests == 0 {
            return "Waiting for Claude Desktop…".into();
        }
        let calls = match activity.calls {
            0 => "no calls yet".to_string(),
            1 => "1 call".to_string(),
            n => format!("{n} calls"),
        };
        if activity.recent.is_empty() {
            format!("Claude Desktop connected · {calls}")
        } else {
            format!("Claude Desktop connected · {calls} · last: {}", activity.recent.join(", "))
        }
    }

    fn now(cx: &Cx) -> f64 {
        cx.seconds_since_app_start()
    }

    /// A line as if typed and sent — the bridge's `/ai?say=`.
    pub fn say(&mut self, cx: &mut Cx, text: String) {
        self.send(cx, text);
    }

    /// Images the model sees with its next input — for a host's tool that
    /// captured something while a tool round is in flight (its results
    /// carry the pictures). `Err` when the model cannot see images.
    pub fn attach_for_model(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
        self.ensure_engine().model_mut().attach_images(images)
    }

    /// A picture for the person's next line, as if dropped on the panel.
    pub fn attach(&mut self, cx: &mut Cx, source: Source) {
        match cx.task_pool().submit(Lane::Light, move || attach::load(source)) {
            Ok(handle) => self.loading.push(handle),
            Err(e) => self.note(cx, format!("could not read the picture ({e:?})")),
        }
    }

    /// Switch the model; the conversation restarts.
    pub fn set_provider(&mut self, cx: &mut Cx, slug: &str) {
        let choice = ProviderChoice::from_slug(slug);
        let _ = self.ensure_engine();
        let _ = &choice;
        #[cfg(feature = "engine")]
        {
            let model: Box<dyn makepad_ai_services::Model> = match build_model(&choice, false) {
                Ok(m) => m,
                Err(reason) => Box::new(NoModelWithReason::new(reason)),
            };
            let rows = self.provider_rows.clone();
            let engine = self.engine.as_mut().unwrap();
            engine.set_model(model);
            engine.set_provider_facts(choice, rows, false);
        }
        self.apply_mcp();
        self.view.redraw(cx);
    }

    fn note(&mut self, cx: &mut Cx, text: String) {
        self.notice = Some(text);
        self.view.redraw(cx);
    }

    fn poll_loading(&mut self, cx: &mut Cx) {
        let mut i = 0;
        while i < self.loading.len() {
            match self.loading[i].try_take() {
                Some(result) => {
                    self.loading.remove(i).detach();
                    match result {
                        Ok(Ok(picture)) => {
                            self.attachments.push(picture);
                            // At most four pictures ride with one line.
                            if self.attachments.len() > 4 {
                                self.attachments.remove(0);
                            }
                        }
                        Ok(Err(e)) => self.note(cx, e),
                        Err(e) => self.note(cx, format!("could not read the picture ({e:?})")),
                    }
                    self.view.redraw(cx);
                }
                None => i += 1,
            }
        }
    }

    fn handle_drop(&mut self, cx: &mut Cx, event: &Event) {
        if !matches!(event, Event::Drag(_) | Event::Drop(_)) {
            return;
        }
        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(drag) => {
                let accept = drag.items.iter().any(|item| Source::from_drag_item(item).is_some());
                if let Ok(mut response) = drag.response.try_lock() {
                    *response = if accept { DragResponse::Copy } else { DragResponse::None };
                }
            }
            DragHit::Drop(drop) => {
                let sources: Vec<Source> = drop.items.iter().filter_map(Source::from_drag_item).collect();
                if sources.is_empty() {
                    self.note(cx, "drop a PNG, JPEG or WebP picture".into());
                }
                for source in sources.into_iter().take(4) {
                    self.attach(cx, source);
                }
            }
            _ => {}
        }
    }

    fn send(&mut self, cx: &mut Cx, text: String) {
        let now = Self::now(cx);
        self.notice = None;
        // A pasted path to a picture is an attachment, not a line.
        if let Some(source) = Source::from_pasted_text(&text) {
            self.attach(cx, source);
            self.view.text_input(cx, ids!(input)).set_text(cx, "");
            return;
        }
        let mut text = text;
        if !self.attachments.is_empty() && !text.trim().starts_with('/') {
            let pictures = std::mem::take(&mut self.attachments);
            let names: Vec<String> = pictures.iter().map(|p| p.name.clone()).collect();
            let images = pictures.into_iter().map(|p| ModelImage { label: format!("reference: {}", p.name), png: p.png }).collect();
            match self.ensure_engine().model_mut().attach_images(images) {
                Ok(()) => {
                    if text.trim().is_empty() {
                        text = "Use the attached picture as the reference.".into();
                    }
                    text.push_str(&format!("\n[attached: {}]", names.join(", ")));
                }
                Err(e) => self.note(cx, format!("the pictures were not sent: {e}")),
            }
            self.attach_shown = None;
        }
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

impl Drop for AiChatPanel {
    fn drop(&mut self) {
        // The engine only enqueues Unsubscribe frames. Flush them while the
        // hosted bus and registry still exist; field destruction is too late.
        if let Some(engine) = self.engine.as_mut() {
            // Claude Desktop's waiting calls are answered before the MCP
            // server (a field, dropped after this) stops.
            engine.cancel_external();
            engine.shutdown();
        }
        self.bus.relay_down(&self.registry);
    }
}

impl Widget for AiChatPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Custom(json) = event {
            self.on_custom(json);
        }
        self.handle_drop(cx, event);
        if !self.loading.is_empty() {
            self.poll_loading(cx);
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
        // The built-in services answer before the engine pumps, so a
        // finished picture lands in this same event.
        #[cfg(feature = "gen")]
        if let Some(gen) = self.gen.as_mut() {
            gen.handle_event(cx, event);
        }
        // An app started for Claude Desktop runs its engine from the start,
        // with the pane closed.
        if self.engine.is_none() && cx.global::<AiSlotRequests>().engine_at_start {
            self.ensure_engine();
        }
        // Drive the engine on every event; the bus relays what it sent.
        let now = Self::now(cx);
        let mut changed = self.pump_mcp(cx, now);
        let mut busy = false;
        if let Some(engine) = self.engine.as_mut() {
            for ev in engine.pump(now) {
                match ev {
                    EngineEvent::Changed => changed = true,
                    EngineEvent::Confirm { .. } => changed = true,
                }
            }
            busy = engine.needs_pump()
                || engine.state().status.is_busy()
                || matches!(engine.state().status, Status::Loading { .. });
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
        let mcp_row = self.view.view(cx, ids!(mcp_row));
        mcp_row.set_visible(cx, self.mcp.is_some());
        if let Some(mcp) = &self.mcp {
            self.mcp_shown = mcp.activity();
            self.view.label(cx, ids!(mcp_status)).set_text(cx, &Self::mcp_status(&self.mcp_shown));
        }
        let status = match &self.notice {
            Some(notice) => notice.clone(),
            None => Self::status_line(&state),
        };
        self.view.label(cx, ids!(status)).set_text(cx, &status);
        let shown = (self.attachments.len(), self.loading.len());
        if self.attach_shown != Some(shown) {
            self.attach_shown = Some(shown);
            self.view.view(cx, ids!(attach_row)).set_visible(cx, shown.0 + shown.1 > 0);
            let mut names: Vec<String> = self.attachments.iter().map(|a| format!("{} ({}×{})", a.name, a.width, a.height)).collect();
            if !self.loading.is_empty() {
                names.push("reading…".into());
            }
            self.view.label(cx, ids!(attach_label)).set_text(cx, &names.join(", "));
            if let Some(thumb) = self.attachments.last().and_then(|a| a.thumb.clone()) {
                let _ = self.view.image(cx, ids!(attach_thumb)).load_png_from_data(cx, &thumb);
            }
        }
        if !self.provider_rows.is_empty() {
            let labels: Vec<String> = self
                .provider_rows
                .iter()
                .map(|r| match &r.unavailable {
                    Some(_) => format!("{} (unavailable)", r.label),
                    None => r.label.clone(),
                })
                .collect();
            // A choice with no row (Local in a build without local AI)
            // answers with no model, so the menu shows that row.
            let at = self
                .provider_rows
                .iter()
                .position(|r| r.choice == state.provider)
                .or_else(|| self.provider_rows.iter().position(|r| r.choice.slug() == "none"))
                .unwrap_or(0);
            if self.pick_shown != Some((labels.len(), at)) {
                self.pick_shown = Some((labels.len(), at));
                let pick = self.view.drop_down(cx, ids!(provider_pick));
                pick.set_labels(cx, labels);
                pick.set_selected_item(cx, at);
            }
        }
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
                    Entry::Event(event) => {
                        let row = list.item(cx, index, id!(EventRow));
                        row.label(cx, ids!(event_title))
                            .set_text(cx, &format!("→ {} · {}", event.service_label, event.topic));
                        row.label(cx, ids!(event_text)).set_text(cx, &event.text);
                        let mut meta = format!("sub_id: {}", event.sub_id);
                        if event.dropped != 0 {
                            meta.push_str(&format!(" · dropped: {}", event.dropped));
                        }
                        if event.final_ {
                            meta.push_str(" · final");
                        }
                        if let Some(data) = &event.data {
                            meta.push_str(&format!("\n{data}"));
                        }
                        row.label(cx, ids!(event_meta)).set_text(cx, &meta);
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
        if self.view.button(cx, ids!(attach_clear)).clicked(actions) {
            self.attachments.clear();
            self.attach_shown = None;
            self.view.redraw(cx);
        }
        if let Some(at) = self.view.drop_down(cx, ids!(provider_pick)).changed(actions) {
            if let Some(row) = self.provider_rows.get(at).cloned() {
                if let Some(why) = &row.unavailable {
                    self.note(cx, format!("{}: {why}", row.label));
                } else {
                    self.set_provider(cx, &row.choice.slug());
                }
            }
        }
        if self.view.button(cx, ids!(mcp_connect)).clicked(actions) {
            self.connect_claude_desktop(cx);
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
                // A press taken away (the list scrolled under it) toggles nothing.
                } else if item.view(cx, ids!(tool_head)).finger_up(actions).is_some_and(|e| !e.cancelled) {
                    self.ensure_engine().toggle_tool(&call_id);
                    self.view.redraw(cx);
                }
            }
        }
        let _ = scope;
    }
}
