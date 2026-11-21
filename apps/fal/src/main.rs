//! fal — a window onto Fal (fal.ai), the hosted generative-media service.
//!
//! Paste an API key (kept in `~/.makepad/fal/key`, or read from `FAL_KEY`),
//! pick a model or type any Fal endpoint id, write a prompt and Generate: the
//! run goes through Fal's queue API, its queue position, log lines and elapsed
//! time show live, and the pictures land in the list and on disk. The Sweep
//! tab runs every text-to-image model Fal lists once, same prompt and seed,
//! and ranks them by latency.
//!
//! The UI thread never waits: HTTP replies arrive as events, one timer drives
//! the polls and retries, pictures decode on the task pool, and files are
//! written there too. The key only ever travels in the Authorization header
//! to Fal's own hosts; it is never logged or shown.

pub use makepad_widgets;

mod engine;
mod fal;
mod store;
mod sweep;
mod ui;

use engine::{Engine, JobEvent, Origin};
use makepad_strict_json::{obj, s, Value};
use makepad_widgets::*;
use store::KeySource;
use std::collections::HashMap;
use sweep::Sweep;
use ui::Intent;

app_main!(App);

/// The models the picker offers (all listed by Fal's model search under
/// text-to-image); any other endpoint id can be typed in the field.
/// The Generate tab's models, in the dropdown's order (its `labels` in the
/// UI below match). Each runs from a prompt alone at its own default size.
const MODELS: &[(&str, &str)] = &[
    ("Nano Banana Lite — Google", "google/nano-banana-lite"),
    ("Grok Imagine Image — xAI", "xai/grok-imagine-image"),
    ("Recraft V4.1 Flash", "recraft/v4.1/flash/text-to-image"),
    ("FLUX.1 [schnell] — fast", "fal-ai/flux/schnell"),
    ("FLUX.1 [dev] — quality", "fal-ai/flux/dev"),
    ("Stable Diffusion XL — fast", "fal-ai/fast-sdxl"),
    ("FLUX1.1 [pro] — quality", "fal-ai/flux-pro/v1.1"),
];

const SWEEP_INTRO: &str = "One picture from every text-to-image model Fal lists, the same prompt and seed for each, the smallest common size, three at a time. Load the model list first (free): it shows the count and the cost. Run sweep then spends credits on your Fal account.";

/// Fal's image size presets; the first entry sends none (the model's default).
const SIZES: &[&str] = &["model default", "square_hd", "square", "portrait_4_3", "portrait_16_9", "landscape_4_3", "landscape_16_9"];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Caption = Label{
        text: ""
        draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 9}}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Fal"
                window.inner_size: vec2(1280, 860)
                body +: {
                    flow: Overlay
                    padding: 0
                    spacing: 0
                    page := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        show_bg: true
                        draw_bg +: {color: theme.color_bg_app}

                        top := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 10
                            padding: Inset{left: 16 right: 16 top: 10 bottom: 10}
                            align: Align{y: 0.5}
                            H3{width: Fit text: "Fal"}
                            tab_generate := ButtonFlat{text: "Generate"}
                            tab_sweep := ButtonFlat{text: "Sweep"}
                            Filler{}
                            key_status := Label{
                                text: ""
                                draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10}}
                            }
                            key_edit := ButtonFlat{text: "Key"}
                        }

                        key_panel := RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 6
                            margin: Inset{left: 16 right: 16 bottom: 8}
                            padding: 12
                            show_bg: true
                            draw_bg +: {color: theme.color_bg_container border_radius: 6.0}
                            Caption{text: "Fal API key. It is saved in ~/.makepad/fal/key (readable only by you) and sent only to Fal."}
                            key_row := View{
                                width: Fill
                                height: Fit
                                flow: Right{wrap: true}
                                spacing: 8
                                align: Align{y: 0.5}
                                key_input := TextInput{
                                    width: Fill
                                    is_password: true
                                    empty_text: "Paste your Fal API key"
                                }
                                key_show := ButtonFlat{text: "Show"}
                                key_save := Button{text: "Save"}
                                key_forget := ButtonFlat{text: "Forget"}
                            }
                            key_note := Caption{width: Fill}
                        }

                        generate_page := View{
                            width: Fill
                            height: Fill
                            flow: Right
                            controls := ScrollYView{
                                width: 380
                                height: Fill
                                flow: Down
                                spacing: 6
                                padding: Inset{left: 16 right: 12 top: 4 bottom: 16}
                                Caption{text: "Model"}
                                model_pick := DropDown{
                                    width: Fill
                                    labels: ["Nano Banana Lite — Google" "Grok Imagine Image — xAI" "Recraft V4.1 Flash" "FLUX.1 [schnell] — fast" "FLUX.1 [dev] — quality" "Stable Diffusion XL — fast" "FLUX1.1 [pro] — quality"]
                                }
                                Caption{text: "Endpoint id (any Fal model)"}
                                endpoint := TextInput{width: Fill text: "google/nano-banana-lite"}
                                Caption{text: "Prompt"}
                                prompt := TextInput{
                                    width: Fill
                                    height: 120
                                    is_multiline: true
                                    empty_text: "Describe the picture"
                                }
                                options := View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 8
                                    size_box := View{
                                        width: Fill
                                        height: Fit
                                        flow: Down
                                        Caption{text: "Image size"}
                                        size_pick := DropDown{
                                            width: Fill
                                            labels: ["model default" "square_hd" "square" "portrait_4_3" "portrait_16_9" "landscape_4_3" "landscape_16_9"]
                                        }
                                    }
                                    count_box := View{
                                        width: 70
                                        height: Fit
                                        flow: Down
                                        Caption{text: "Images"}
                                        count_pick := DropDown{width: Fill labels: ["1" "2" "3" "4"]}
                                    }
                                    seed_box := View{
                                        width: 100
                                        height: Fit
                                        flow: Down
                                        Caption{text: "Seed"}
                                        seed := TextInput{width: Fill empty_text: "random"}
                                    }
                                }
                                size_hint := Caption{width: Fill visible: false text: ""}
                                Caption{text: "Advanced input (JSON): its fields are merged into the request"}
                                advanced := TextInput{
                                    width: Fill
                                    height: 100
                                    is_multiline: true
                                    empty_text: "{\"num_inference_steps\": 4}"
                                }
                                generate := Button{width: Fill text: "Generate"}
                                generate_note := Label{
                                    width: Fill
                                    text: ""
                                    draw_text +: {color: #xe5534b text_style: theme.font_regular{font_size: 10}}
                                }
                            }
                            runs := FalRunList{}
                        }

                        sweep_page := View{
                            visible: false
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 6
                            sweep_controls := View{
                                width: Fill
                                height: Fit
                                flow: Right{wrap: true}
                                spacing: 8
                                padding: Inset{left: 16 right: 16 top: 4}
                                align: Align{y: 0.5}
                                sweep_prompt := TextInput{
                                    width: Fill
                                    text: "a lighthouse on a rocky coast at sunset, photograph"
                                }
                                Caption{text: "Seed"}
                                sweep_seed := TextInput{width: 90 text: "42"}
                                sweep_load := Button{text: "Load models"}
                                sweep_run := Button{text: "Run sweep"}
                                sweep_stop := ButtonFlat{text: "Stop"}
                            }
                            sweep_summary := Label{
                                width: Fill
                                padding: Inset{left: 16 right: 16}
                                text: ""
                                draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10}}
                            }
                            sweep_error := Label{
                                width: Fill
                                padding: Inset{left: 16 right: 16}
                                text: ""
                                draw_text +: {color: #xe5534b text_style: theme.font_regular{font_size: 10}}
                            }
                            sweep_table := FalSweepTable{}
                        }
                    }

                    viewer := View{
                        visible: false
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 8
                        padding: 24
                        align: Align{x: 0.5 y: 0.5}
                        cursor: MouseCursor.Hand
                        show_bg: true
                        draw_bg +: {color: #x000000e0}
                        viewer_image := Image{width: Fill height: Fill fit: ImageFit.Smallest}
                        viewer_caption := Label{
                            width: Fill
                            text: ""
                            draw_text +: {color: #xffffffd0 text_style: theme.font_regular{font_size: 10}}
                        }
                    }
                }
            }
        }
    }
}

/// What the lists read while drawing, and the clicks they hand back.
#[derive(Default)]
pub struct State {
    pub engine: Engine,
    pub sweep: Sweep,
    pub intents: Vec<Intent>,
    /// Whether a key is set (the empty list's hint says what is missing).
    pub has_key: bool,
}

impl State {
    /// This session's single runs, newest first.
    pub fn single_runs(&self) -> Vec<usize> {
        let mut runs: Vec<usize> = (0..self.engine.jobs.len()).filter(|&i| self.engine.jobs[i].origin == Origin::Single).collect();
        runs.reverse();
        runs
    }
}

/// What the Generate tab knows of models' inputs, from Fal's catalog: it
/// sends a size preset in the form each model takes.
#[derive(Default)]
pub struct Schemas {
    /// Per endpoint: its input schema, or None when Fal has none for it.
    known: HashMap<String, Option<fal::InputSchema>>,
    asking: HashMap<LiveId, String>,
    /// A Generate press waiting for its model's schema.
    waiting: Option<String>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
    /// The key and where it came from. Never logged, never drawn.
    #[rust]
    key: Option<(String, KeySource)>,
    #[rust]
    key_visible: bool,
    #[rust]
    tick: Timer,
    /// What the lists last drew (engine, sweep generations).
    #[rust]
    drawn: (u64, u64),
    #[rust]
    sweep_tab: bool,
    #[rust]
    schemas: Schemas,
}

impl App {
    fn key(&self) -> Option<&str> {
        self.key.as_ref().map(|(key, _)| key.as_str())
    }

    fn show_key_state(&mut self, cx: &mut Cx, note: &str) {
        self.state.has_key = self.key.is_some();
        self.ui.widget(cx, ids!(runs)).redraw(cx);
        let status = match &self.key {
            Some((_, source)) => format!("Key set ({})", source.describe()),
            None => "No key yet".to_string(),
        };
        self.ui.label(cx, ids!(key_status)).set_text(cx, &status);
        self.ui.label(cx, ids!(key_note)).set_text(cx, note);
        self.ui.button(cx, ids!(key_forget)).set_visible(cx, matches!(self.key, Some((_, KeySource::File))));
    }

    fn set_tab(&mut self, cx: &mut Cx, sweep: bool) {
        self.sweep_tab = sweep;
        self.ui.view(cx, ids!(generate_page)).set_visible(cx, !sweep);
        self.ui.view(cx, ids!(sweep_page)).set_visible(cx, sweep);
        self.ui.redraw(cx);
    }

    /// The endpoint field's id, when it reads as one.
    fn endpoint(&self, cx: &mut Cx) -> Option<String> {
        fal::normalize_endpoint_id(&self.ui.text_input(cx, ids!(endpoint)).text()).ok()
    }

    /// The size preset picked, None for the model's own.
    fn size_preset(&self, cx: &mut Cx) -> Option<&'static str> {
        let index = self.ui.drop_down(cx, ids!(size_pick)).selected_item();
        (index > 0).then(|| SIZES.get(index).copied()).flatten()
    }

    /// Ask Fal's catalog what a model takes (free), once per model.
    fn ask_schema(&mut self, cx: &mut Cx, endpoint: &str) {
        if self.schemas.known.contains_key(endpoint) || self.schemas.asking.values().any(|asked| asked == endpoint) {
            return;
        }
        let id = LiveId::unique();
        cx.http_request(id, fal::model_request(self.key(), endpoint));
        self.schemas.asking.insert(id, endpoint.to_string());
    }

    /// A catalog reply for the Generate tab; false when the response is not one.
    fn schema_response(&mut self, cx: &mut Cx, response: &NetworkResponse) -> bool {
        let (request_id, reply) = match response {
            NetworkResponse::HttpResponse { request_id, response } => (*request_id, Ok(response)),
            NetworkResponse::HttpError { request_id, error } => (*request_id, Err(error.message.clone())),
            _ => return false,
        };
        let Some(endpoint) = self.schemas.asking.remove(&request_id) else { return false };
        let page = match reply {
            Ok(response) => fal::parse_models_page(response),
            Err(message) => Err(fal::FalError::Network(message)),
        };
        match page {
            // Fal lists the model or not; either way that is what it takes.
            Ok(page) => {
                let schema = page.models.into_iter().find(|model| model.endpoint_id == endpoint).and_then(|model| model.input);
                self.schemas.known.insert(endpoint.clone(), schema);
            }
            // An id Fal's catalog does not list: nothing to learn about it.
            Err(fal::FalError::NotFound(_)) => {
                self.schemas.known.insert(endpoint.clone(), None);
            }
            // Not known this time (rate limited, offline): a later press asks again.
            Err(error) => log!("fal: the inputs of {endpoint} are unknown ({})", error.message()),
        }
        if self.schemas.waiting.as_deref() == Some(endpoint.as_str()) {
            self.schemas.waiting = None;
            self.generate_now(cx);
        }
        self.show_size_hint(cx);
        true
    }

    /// How the picked size goes to this model, when that is not plain.
    fn show_size_hint(&mut self, cx: &mut Cx) {
        let hint = match (self.size_preset(cx), self.endpoint(cx)) {
            (Some(preset), Some(endpoint)) => match self.schemas.known.get(&endpoint) {
                Some(Some(schema)) => match fal::size_input(schema, preset) {
                    Some((name, value)) if name == "image_size" && value.as_str() == Some(preset) => String::new(),
                    Some((name, value)) => format!("Sent as {}: the form this model takes.", fal::describe_size(&name, &value)),
                    None => "This model takes no size; it makes its own.".to_string(),
                },
                Some(None) => "Fal lists no inputs for this model; the size goes as image_size.".to_string(),
                None if self.schemas.asking.values().any(|asked| *asked == endpoint) => "Reading this model's inputs…".to_string(),
                None => String::new(),
            },
            _ => String::new(),
        };
        self.ui.label(cx, ids!(size_hint)).set_text(cx, &hint);
        self.ui.widget(cx, ids!(size_hint)).set_visible(cx, !hint.is_empty());
    }

    /// Keep the timer running only while something is in flight.
    fn ensure_tick(&mut self, cx: &mut Cx) {
        let wanted = self.state.engine.any_live() || self.state.sweep.busy();
        if wanted && self.tick.is_empty() {
            self.tick = cx.start_interval(0.1);
        } else if !wanted && !self.tick.is_empty() {
            cx.stop_timer(self.tick);
            self.tick = Timer::empty();
        }
    }

    /// The single run's request: the fields above, then the advanced JSON on top.
    fn build_input(&self, cx: &mut Cx) -> Result<(String, String, Value), String> {
        let endpoint = fal::normalize_endpoint_id(&self.ui.text_input(cx, ids!(endpoint)).text())?;
        let prompt = self.ui.text_input(cx, ids!(prompt)).text().trim().to_string();
        let mut input = obj(vec![]);
        if let Value::Obj(fields) = &mut input {
            if !prompt.is_empty() {
                fields.push(("prompt".to_string(), s(prompt.clone())));
            }
            // A size preset in the model's own form (`aspect_ratio` for some);
            // a model Fal has no schema for gets Fal's usual `image_size`.
            if let Some(preset) = self.size_preset(cx) {
                match self.schemas.known.get(&endpoint) {
                    Some(Some(schema)) => fields.extend(fal::size_input(schema, preset)),
                    _ => fields.push(("image_size".to_string(), s(preset))),
                }
            }
            let count = self.ui.drop_down(cx, ids!(count_pick)).selected_item() + 1;
            if count > 1 {
                fields.push(("num_images".to_string(), Value::Int(count as i64)));
            }
            let seed = self.ui.text_input(cx, ids!(seed)).text();
            let seed = seed.trim();
            if !seed.is_empty() {
                let seed: i64 = seed.parse().map_err(|_| format!("The seed must be a whole number, not \"{seed}\"."))?;
                fields.push(("seed".to_string(), Value::Int(seed)));
            }
        }
        let advanced = self.ui.text_input(cx, ids!(advanced)).text();
        if !advanced.trim().is_empty() {
            let overlay = fal::parse_json(advanced.trim().as_bytes()).map_err(|err| format!("The advanced input is not valid JSON ({err})."))?;
            fal::merge_input(&mut input, &overlay).map_err(|err| format!("Advanced input: {err}"))?;
        }
        if input.get("prompt").is_none() {
            return Err("Write a prompt (or give one in the advanced input).".to_string());
        }
        Ok((endpoint, prompt, input))
    }

    /// The Generate press: a size preset waits for the model's schema first
    /// (one catalog call, usually answered already).
    fn generate(&mut self, cx: &mut Cx) {
        if let (Some(_), Some(endpoint)) = (self.size_preset(cx), self.endpoint(cx)) {
            if !self.schemas.known.contains_key(&endpoint) {
                self.ask_schema(cx, &endpoint);
                self.schemas.waiting = Some(endpoint);
                self.show_size_hint(cx);
                return;
            }
        }
        self.generate_now(cx);
    }

    fn generate_now(&mut self, cx: &mut Cx) {
        let Some(key) = self.key().map(str::to_string) else {
            self.ui.label(cx, ids!(generate_note)).set_text(cx, "Paste your Fal API key first.");
            self.ui.view(cx, ids!(key_panel)).set_visible(cx, true);
            return;
        };
        match self.build_input(cx) {
            Ok((endpoint, prompt, input)) => {
                self.ui.label(cx, ids!(generate_note)).set_text(cx, "");
                self.state.engine.start(cx, &key, Origin::Single, endpoint, prompt, input);
                self.ensure_tick(cx);
                self.refresh(cx, true);
            }
            Err(why) => self.ui.label(cx, ids!(generate_note)).set_text(cx, &why),
        }
    }

    fn sweep_seed(&self, cx: &mut Cx) -> Result<i64, String> {
        let text = self.ui.text_input(cx, ids!(sweep_seed)).text();
        text.trim().parse().map_err(|_| format!("The seed must be a whole number, not \"{}\".", text.trim()))
    }

    /// The sweep's summary line and its buttons, from its state.
    fn show_sweep(&mut self, cx: &mut Cx) {
        let sweep = &self.state.sweep;
        let mut summary = sweep.message.clone();
        let estimate = sweep.estimate(&self.state.engine);
        let cost = estimate.usd;
        match sweep.phase {
            sweep::Phase::Idle => summary = SWEEP_INTRO.to_string(),
            sweep::Phase::Ready | sweep::Phase::Stopped => {
                summary.push_str(&format!(" One picture from each costs about ${cost:.2} at this size by Fal's prices"));
                if estimate.unpriced > 0 {
                    summary.push_str(&format!(", plus {} models billed in other units or unpriced", estimate.unpriced));
                }
                summary.push_str(". Press Run sweep to start; it spends credits on your Fal account.");
            }
            sweep::Phase::Running => {
                let done = sweep.rows.iter().filter_map(|row| row.job).filter(|&job| !self.state.engine.jobs[job].stage.is_live()).count();
                summary = format!("Running: {done} of {} models finished, three at a time.", sweep.runnable());
            }
            sweep::Phase::Finished => {
                if let Some(path) = &sweep.file {
                    summary = format!("Sweep finished. Saved to {}", path.display());
                }
            }
            _ => {}
        }
        let run_label = match sweep.phase {
            sweep::Phase::Ready if cost > 0.0 => format!("Run sweep: {} models, about ${cost:.2}", sweep.runnable()),
            sweep::Phase::Ready => format!("Run sweep: {} models", sweep.runnable()),
            sweep::Phase::Stopped => "Resume sweep".to_string(),
            _ => "Run sweep".to_string(),
        };
        let can_run = matches!(sweep.phase, sweep::Phase::Ready | sweep::Phase::Stopped);
        let error = sweep.error.clone().unwrap_or_default();
        let running = sweep.phase == sweep::Phase::Running;
        let loading = matches!(sweep.phase, sweep::Phase::Listing | sweep::Phase::Pricing | sweep::Phase::Running);
        self.ui.label(cx, ids!(sweep_summary)).set_text(cx, &summary);
        self.ui.label(cx, ids!(sweep_error)).set_text(cx, &error);
        let run = self.ui.button(cx, ids!(sweep_run));
        run.set_text(cx, &run_label);
        run.set_visible(cx, can_run);
        self.ui.button(cx, ids!(sweep_stop)).set_visible(cx, running);
        self.ui.button(cx, ids!(sweep_load)).set_visible(cx, !loading);
    }

    /// Redraw the lists when something they show changed (or, while runs
    /// are live, for their clocks).
    fn refresh(&mut self, cx: &mut Cx, force: bool) {
        let now = (self.state.engine.generation, self.state.sweep.generation);
        let live = self.state.engine.any_live();
        if force || now != self.drawn || live {
            self.drawn = now;
            self.ui.widget(cx, ids!(runs)).redraw(cx);
            self.ui.widget(cx, ids!(sweep_table)).redraw(cx);
            self.show_sweep(cx);
        }
    }

    fn drain_job_events(&mut self, cx: &mut Cx) {
        let events = std::mem::take(&mut self.state.engine.events);
        for event in events {
            match event {
                JobEvent::Finished(index) => {
                    if let Origin::Sweep(_) = self.state.engine.jobs[index].origin {
                        self.state.sweep.job_finished(&self.state.engine, index);
                        self.state.sweep.record(cx, &self.state.engine);
                    }
                }
            }
        }
    }

    fn open_viewer(&mut self, cx: &mut Cx, job: usize, shot: usize) {
        let Some(job) = self.state.engine.jobs.get(job) else { return };
        let Some(picture) = job.shots.get(shot) else { return };
        let Some(bytes) = picture.bytes.as_ref() else { return };
        let image = self.ui.image(cx, ids!(viewer_image));
        ui::show_image(cx, &image, &picture.key, bytes);
        let mut caption = format!("{} · {}", job.endpoint_id, job.prompt);
        if let (Some(w), Some(h)) = (picture.media.width, picture.media.height) {
            caption.push_str(&format!(" · {w}×{h}"));
        }
        if let Some(path) = &picture.saved {
            caption.push_str(&format!("\n{}", path.display()));
        }
        caption.push_str("\nClick anywhere to close.");
        self.ui.label(cx, ids!(viewer_caption)).set_text(cx, &caption);
        self.ui.view(cx, ids!(viewer)).set_visible(cx, true);
        self.ui.redraw(cx);
    }

    fn handle_intents(&mut self, cx: &mut Cx) {
        let intents = std::mem::take(&mut self.state.intents);
        for intent in intents {
            match intent {
                Intent::View(job, shot) => self.open_viewer(cx, job, shot),
                Intent::Cancel(job) => {
                    if let Some(key) = self.key.as_ref().map(|(key, _)| key.clone()) {
                        self.state.engine.cancel(cx, &key, job);
                    }
                }
                Intent::Refetch(job) => {
                    self.state.engine.refetch(job);
                    self.ensure_tick(cx);
                    self.refresh(cx, true);
                }
                Intent::CopyResult(job) => {
                    if let Some(result) = self.state.engine.jobs.get(job).and_then(|job| job.result.as_ref()) {
                        let text = fal::pretty(result);
                        cx.copy_to_clipboard(&text);
                    }
                }
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let (key, problem) = store::load_key();
        self.key = key;
        let note = problem.unwrap_or_default();
        self.show_key_state(cx, &note);
        // The key panel stays open until there is a key.
        self.ui.view(cx, ids!(key_panel)).set_visible(cx, self.key.is_none());
        self.set_tab(cx, std::env::args().any(|arg| arg == "--sweep"));
        self.show_sweep(cx);
        if let Some(endpoint) = self.endpoint(cx) {
            self.ask_schema(cx, &endpoint);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(saved) = action.downcast_ref::<store::Saved>() {
                for (path, error) in &saved.results {
                    if let Some(error) = error {
                        log!("fal: could not save {}: {error}", path.display());
                    }
                }
            }
        }
        if self.ui.button(cx, ids!(tab_generate)).clicked(actions) {
            self.set_tab(cx, false);
        }
        if self.ui.button(cx, ids!(tab_sweep)).clicked(actions) {
            self.set_tab(cx, true);
        }
        if self.ui.button(cx, ids!(key_edit)).clicked(actions) {
            let panel = self.ui.view(cx, ids!(key_panel));
            panel.set_visible(cx, !panel.visible());
            self.ui.redraw(cx);
        }
        if self.ui.button(cx, ids!(key_show)).clicked(actions) {
            self.key_visible = !self.key_visible;
            self.ui.text_input(cx, ids!(key_input)).set_is_password(cx, !self.key_visible);
            self.ui.button(cx, ids!(key_show)).set_text(cx, if self.key_visible { "Hide" } else { "Show" });
        }
        let save = self.ui.button(cx, ids!(key_save)).clicked(actions) || self.ui.text_input(cx, ids!(key_input)).returned(actions).is_some();
        if save {
            let input = self.ui.text_input(cx, ids!(key_input));
            match fal::parse_key_text(&input.text()) {
                Some(key) => match store::save_key(&key) {
                    Ok(()) => {
                        self.key = Some((key, KeySource::File));
                        input.set_text(cx, "");
                        self.show_key_state(cx, "Saved.");
                    }
                    Err(why) => self.show_key_state(cx, &why),
                },
                None => self.show_key_state(cx, "That does not look like a key: paste the whole key, without spaces."),
            }
        }
        if self.ui.button(cx, ids!(key_forget)).clicked(actions) {
            match store::forget_key() {
                Ok(()) => {
                    let (key, _) = store::load_key();
                    self.key = key;
                    let note = if self.key.is_some() { "The saved key is gone; FAL_KEY is still set." } else { "The saved key is gone." };
                    self.show_key_state(cx, note);
                }
                Err(why) => self.show_key_state(cx, &why),
            }
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(model_pick)).changed(actions) {
            if let Some((_, endpoint)) = MODELS.get(index) {
                self.ui.text_input(cx, ids!(endpoint)).set_text(cx, endpoint);
                self.ask_schema(cx, endpoint);
            }
            self.show_size_hint(cx);
        }
        if self.ui.text_input(cx, ids!(endpoint)).returned(actions).is_some() {
            if let Some(endpoint) = self.endpoint(cx) {
                self.ask_schema(cx, &endpoint);
            }
            self.show_size_hint(cx);
        }
        if self.ui.drop_down(cx, ids!(size_pick)).changed(actions).is_some() {
            if let (Some(_), Some(endpoint)) = (self.size_preset(cx), self.endpoint(cx)) {
                self.ask_schema(cx, &endpoint);
            }
            self.show_size_hint(cx);
        }
        // Typing: the hint follows what is known (asked on Enter or Generate).
        if self.ui.text_input(cx, ids!(endpoint)).changed(actions).is_some() {
            self.show_size_hint(cx);
        }
        if self.ui.button(cx, ids!(generate)).clicked(actions) {
            self.generate(cx);
        }
        if self.ui.button(cx, ids!(sweep_load)).clicked(actions) {
            match (self.key().map(str::to_string), self.sweep_seed(cx)) {
                (None, _) => {
                    self.state.sweep.error = Some("Paste your Fal API key first (the price list needs it).".to_string());
                    self.ui.view(cx, ids!(key_panel)).set_visible(cx, true);
                }
                (Some(_), Err(why)) => self.state.sweep.error = Some(why),
                (Some(key), Ok(seed)) => {
                    let prompt = self.ui.text_input(cx, ids!(sweep_prompt)).text().trim().to_string();
                    if prompt.is_empty() {
                        self.state.sweep.error = Some("Write a prompt for the sweep.".to_string());
                    } else {
                        self.state.sweep.load(cx, &key, prompt, seed);
                    }
                }
            }
            self.ensure_tick(cx);
            self.refresh(cx, true);
        }
        if self.ui.button(cx, ids!(sweep_run)).clicked(actions) {
            let estimate = self.state.sweep.estimate(&self.state.engine);
            self.state.sweep.run(estimate);
            self.ensure_tick(cx);
            self.refresh(cx, true);
        }
        if self.ui.button(cx, ids!(sweep_stop)).clicked(actions) {
            self.state.sweep.stop();
            self.refresh(cx, true);
        }
        if self.ui.view(cx, ids!(viewer)).finger_up(actions).is_some_and(|f| f.is_over) {
            self.ui.view(cx, ids!(viewer)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        ui::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::NetworkResponses(responses) = event {
            for response in responses {
                if self.schema_response(cx, response) {
                    continue;
                }
                let key = self.key.as_ref().map(|(key, _)| key.as_str());
                if !self.state.engine.handle_response(cx, key, response) {
                    self.state.sweep.handle_response(cx, key, response);
                }
            }
            self.drain_job_events(cx);
            self.ensure_tick(cx);
            self.refresh(cx, false);
        }
        if self.tick.is_event(event).is_some() {
            let key = self.key.as_ref().map(|(key, _)| key.as_str());
            self.state.engine.tick(cx, key);
            self.state.sweep.tick(cx, key, &mut self.state.engine);
            self.drain_job_events(cx);
            self.ensure_tick(cx);
            self.refresh(cx, false);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.state));
        self.handle_intents(cx);
    }
}
