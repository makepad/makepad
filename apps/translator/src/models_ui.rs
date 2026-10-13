//! The Models card in settings and the first-run offer, over a [`ModelStore`].

use super::*;
use crate::models::{self, ModelState, ModelStore, State};

#[derive(Default)]
pub(super) struct Models {
    store: Option<Box<dyn ModelStore>>,
    timer: Option<Timer>,
    /// "Not now" / "Use my hub" was pressed: the offer stays away this session.
    offer_dismissed: bool,
    opened_for: Option<Option<String>>,
    /// Whether the translation model was ready at the last look: the first
    /// time it is, the phone's models become available to the app.
    text_ready: bool,
}

const ROWS: usize = 4;

impl TranslatorView {
    /// Open the store (again when the paired hub changes: its address is the
    /// first place the models are fetched from).
    pub(super) fn start_models(&mut self, cx: &mut Cx) {
        let lan = crate::engine::hub_base(&self.saved.hub).filter(|_| !self.saved.credential.is_empty());
        if let Some(base) = &lan {
            // The LAN front is TLS with the device credential: tell the fleet client.
            makepad_ai_hub::fleet_auth::mark_fleet_endpoint(base.trim_start_matches("https://").trim_start_matches("http://"));
        }
        if self.models.opened_for == Some(lan.clone()) && self.models.store.is_some() {
            return;
        }
        self.models.opened_for = Some(lan.clone());
        let wake: makepad_ai_hub::model_store::Wake = Arc::new(|| SignalToUI::set_ui_signal());
        self.models.store = models::open(lan.as_deref(), Some(wake));
        if self.models.store.is_some() && self.models.timer.is_none() {
            self.models.timer = Some(cx.start_interval(0.5));
        }
        self.refresh_models(cx);
    }

    pub(super) fn on_models_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.models.timer.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.refresh_models(cx);
        }
    }

    pub(super) fn model_list(&self) -> Vec<ModelState> {
        self.models.store.as_ref().map(|s| s.models()).unwrap_or_default()
    }

    pub(super) fn refresh_models(&mut self, cx: &mut Cx) {
        let list = self.model_list();
        let text_ready = list.iter().any(|m| m.capability == models::Capability::Text && m.state == State::Ready);
        if text_ready && !self.models.text_ready {
            // The files are in place: find them, and run on the phone from now on.
            if crate::ondevice::activate().is_ok() && self.saved.model != crate::ondevice::MODEL {
                self.saved.model = crate::ondevice::MODEL.to_string();
                self.view.text_input(cx, ids!(settings.model)).set_text(cx, &self.saved.model);
                self.persist(cx);
                self.hub_ok = None;
                self.probe_id = 0;
                self.probe(cx);
            }
            self.sync_run_on(cx);
        }
        self.models.text_ready = text_ready;
        let present = !list.is_empty();
        self.view.view(cx, ids!(settings.models_card)).set_visible(cx, present);
        // Nothing to offer while a translation model is already on the phone (installed by
        // hand or by an earlier download).
        let offer = present && !self.models.offer_dismissed && !models::all_ready(&list) && !crate::ondevice::available();
        let any_started = list.iter().any(|m| m.state != State::Absent);
        self.view.view(cx, ids!(offer)).set_visible(cx, offer && !any_started && !self.keyboard_up);
        if offer {
            self.view.label(cx, ids!(offer.offer_text)).set_text(cx, &models::offer_text(&list));
        }
        for i in 0..ROWS {
            let id = |name: &str| [live_id!(settings), live_id!(models_card), LiveId::from_str(&format!("{name}{i}"))];
            let row = self.view.widget(cx, &id("m"));
            match list.get(i) {
                Some(m) => {
                    row.set_visible(cx, true);
                    let name = row_path(i, "mname");
                    self.view.label(cx, &name).set_text(cx, m.capability.label());
                    self.view.label(cx, &row_path(i, "mstate")).set_text(cx, &models::state_text(m));
                    let button = self.view.button(cx, &row_path(i, "mbtn"));
                    let text = models::action_text(&m.state);
                    button.set_text(cx, text);
                    button.set_visible(cx, !text.is_empty());
                    // The progress track shows while a file is on its way; the bar is
                    // as wide as the share on the phone.
                    let moving = matches!(m.state, State::Downloading { .. } | State::Paused { .. } | State::Verifying);
                    self.view.widget(cx, &row_path(i, "mtrack")).set_visible(cx, moving);
                    let card = self.view.widget(cx, ids!(settings.models_card)).area().rect(cx).size.x;
                    let track = if card > 64.0 { card - 32.0 } else { 300.0 };
                    let width = models::fraction(&m.state) as f64 * track;
                    let mut bar = self.view.widget(cx, &row_path(i, "mbar"));
                    script_apply_eval!(cx, bar, { width: #(width) });
                }
                None => row.set_visible(cx, false),
            }
        }
        self.view.redraw(cx);
    }

    pub(super) fn handle_models_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.models.store.is_none() {
            return;
        }
        let list = self.model_list();
        for (i, m) in list.iter().enumerate().take(ROWS) {
            if self.view.button(cx, &row_path(i, "mbtn")).clicked(actions) {
                if let Some(store) = self.models.store.as_mut() {
                    match m.state {
                        State::Downloading { .. } => store.pause(m.id),
                        // A failed download starts over (the partial file is not trusted).
                        State::Failed(_) => {
                            store.cancel(m.id);
                            store.start(m.id);
                        }
                        _ => store.start(m.id),
                    }
                }
                self.refresh_models(cx);
            }
        }
        if self.view.button(cx, ids!(offer.offer_row.offer_dl)).clicked(actions) {
            if let Some(store) = self.models.store.as_mut() {
                for m in &list {
                    store.start(m.id);
                }
            }
            self.refresh_models(cx);
        }
        if self.view.button(cx, ids!(offer.offer_row.offer_hub)).clicked(actions) || self.view.button(cx, ids!(offer.offer_row.offer_later)).clicked(actions) {
            self.models.offer_dismissed = true;
            self.refresh_models(cx);
        }
        if self.view.button(cx, ids!(settings.models_card.models_remove)).clicked(actions) {
            if let Some(store) = self.models.store.as_mut() {
                store.remove_all();
            }
            self.models.offer_dismissed = false;
            self.refresh_models(cx);
        }
    }
}

/// A widget inside model row `i` (the rows share one template).
fn row_path(i: usize, name: &str) -> [LiveId; 4] {
    [live_id!(settings), live_id!(models_card), LiveId::from_str(&format!("m{i}")), LiveId::from_str(name)]
}
