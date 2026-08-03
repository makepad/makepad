//! The settings panel: pick a provider and model, connect a key, and see
//! which capability tier this device actually got (game.md §"AI tiers").
//!
//! M4 wired the provider/model/key selection as API only; this is the surface
//! for it. Key entry is masked and, on a device with no comfortable keyboard,
//! the "Pair from another device" button starts the `/pair` endpoint so a
//! nearby computer can paste the key in instead.

use crate::capability::{Capabilities, Tier};
use crate::pair_server::{PairEvent, PairServer, DEFAULT_PAIR_PORT};
use crate::pairing::{self, Provider};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ArcadeSettingsBase = #(ArcadeSettings::register_widget(vm))
    mod.widgets.ArcadeSettings = set_type_default() do mod.widgets.ArcadeSettingsBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: 10
        padding: theme.space_2

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{y: 0.5}

            Label {
                text: "AI backend"
                draw_text.text_style: theme.font_regular{font_size: 13}
            }
            View { width: Fill height: 1 }
            provider_dropdown := DropDown {
                width: 130
                labels: ["..."]
                draw_text.text_style.font_size: 11
            }
            model_dropdown := DropDown {
                width: 150
                labels: ["..."]
                draw_text.text_style.font_size: 11
            }
        }

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{y: 0.5}

            key_input := TextInput {
                width: Fill
                height: 38
                is_password: true
                empty_text: "paste API key"
            }
            save_key_button := Button { text: "Save" }
            pair_button := Button { text: "Pair from another device" }
        }

        tier_label := Label {
            text: "detecting..."
            draw_text.text_style: theme.font_regular{font_size: 11}
        }
        pair_label := Label {
            text: ""
            draw_text.text_style: theme.font_regular{font_size: 11}
        }
    }
}

/// Providers offered in the picker, in menu order.
const PROVIDERS: &[Provider] = &[
    Provider::ClaudeCode,
    Provider::Anthropic,
    Provider::OpenAi,
    Provider::Gemini,
];

/// Model choices per provider. First entry is the default.
fn models_for(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::ClaudeCode => &["claude-fable-5", "claude-opus-5", "claude-sonnet-5"],
        Provider::Anthropic => &["claude-fable-5", "claude-sonnet-5", "claude-haiku-4-5-20251001"],
        Provider::OpenAi => &["gpt-5", "gpt-5-mini"],
        Provider::Gemini => &["gemini-3-pro", "gemini-3-flash"],
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ArcadeSettings {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    initialized: bool,
    #[rust]
    provider_index: usize,
    #[rust]
    model_index: usize,
    #[rust]
    caps: Option<Capabilities>,
    /// Live only while pairing; dropped when a key lands, which stops serving.
    #[rust]
    pair_server: Option<PairServer>,
    #[rust]
    next_frame: NextFrame,
}

impl ArcadeSettings {
    fn provider(&self) -> Provider {
        PROVIDERS[self.provider_index.min(PROVIDERS.len() - 1)]
    }

    fn tier(&mut self) -> Tier {
        let caps = self.caps.get_or_insert_with(Capabilities::detect);
        caps.tier()
    }

    fn refresh_labels(&mut self, cx: &mut Cx) {
        let provider = self.provider();
        let models: Vec<String> = models_for(provider).iter().map(|m| m.to_string()).collect();
        self.drop_down(cx, ids!(model_dropdown)).set_labels(cx, models);
        self.drop_down(cx, ids!(model_dropdown))
            .set_selected_item(cx, self.model_index.min(models_for(provider).len() - 1));

        let tier = self.tier();
        let key_state = if !provider.needs_key() {
            "no key needed".to_string()
        } else if pairing::has_key(provider) {
            "key connected".to_string()
        } else {
            "no key — join/play only".to_string()
        };
        let text = format!("{} · {}", tier.label(), key_state);
        self.label(cx, ids!(tier_label)).set_text(cx, &text);
    }

    fn start_pairing(&mut self, cx: &mut Cx) {
        let provider = self.provider();
        // Tick-derived seed: the code only has to be unguessable within a room.
        let seed = (cx.seconds_since_app_start() * 1_000_000.0) as u64;
        match PairServer::start(provider, seed, DEFAULT_PAIR_PORT) {
            Some(server) => {
                let text = format!(
                    "Open http://{}:{}/pair on a computer, then enter code {}",
                    local_ip_hint(),
                    server.addr.port(),
                    server.pairing.code
                );
                self.label(cx, ids!(pair_label)).set_text(cx, &text);
                self.pair_server = Some(server);
                // Poll the endpoint until a key lands.
                self.next_frame = cx.new_next_frame();
            }
            None => {
                self.label(cx, ids!(pair_label))
                    .set_text(cx, "could not start the pairing server");
            }
        }
    }

    fn poll_pairing(&mut self, cx: &mut Cx) {
        let Some(server) = self.pair_server.as_mut() else {
            return;
        };
        let events = server.poll();
        let mut done = false;
        for event in events {
            match event {
                PairEvent::Stored(provider) => {
                    self.label(cx, ids!(pair_label))
                        .set_text(cx, &format!("{} key connected", provider.label()));
                    done = true;
                }
                PairEvent::Rejected(err) => {
                    self.label(cx, ids!(pair_label))
                        .set_text(cx, &format!("rejected: {err:?}"));
                }
            }
        }
        if done {
            // Stop serving the moment we have what we came for.
            self.pair_server = None;
            self.refresh_labels(cx);
        } else {
            self.next_frame = cx.new_next_frame();
        }
    }
}

/// Best-effort LAN address for the instructions. Not authoritative — the user
/// can read it off their router if this guesses wrong.
fn local_ip_hint() -> String {
    use std::net::UdpSocket;
    // Connecting a UDP socket picks the interface without sending anything.
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "<this device>".to_string())
}

impl Widget for ArcadeSettings {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            self.initialized = true;
            let labels: Vec<String> = PROVIDERS.iter().map(|p| p.label().to_string()).collect();
            self.drop_down(cx.cx, ids!(provider_dropdown))
                .set_labels(cx.cx, labels);
            self.refresh_labels(cx.cx);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.poll_pairing(cx);
        }
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}

impl WidgetMatchEvent for ArcadeSettings {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if let Some(index) = self.drop_down(cx, ids!(provider_dropdown)).changed(actions) {
            self.provider_index = index;
            self.model_index = 0;
            self.refresh_labels(cx);
        }
        if let Some(index) = self.drop_down(cx, ids!(model_dropdown)).changed(actions) {
            self.model_index = index;
        }
        if self.button(cx, ids!(save_key_button)).clicked(actions) {
            let key = self.text_input(cx, ids!(key_input)).text();
            let provider = self.provider();
            let message = if key.trim().is_empty() {
                "nothing to save".to_string()
            } else {
                match pairing::store_key(provider, key.trim()) {
                    Ok(()) => {
                        // Never leave the key sitting in a widget.
                        self.text_input(cx, ids!(key_input)).set_text(cx, "");
                        format!("{} key stored", provider.label())
                    }
                    Err(err) => format!("could not store key: {err}"),
                }
            };
            self.label(cx, ids!(pair_label)).set_text(cx, &message);
            self.refresh_labels(cx);
        }
        if self.button(cx, ids!(pair_button)).clicked(actions) {
            self.start_pairing(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_offers_at_least_one_model() {
        for provider in PROVIDERS {
            assert!(
                !models_for(*provider).is_empty(),
                "{} has no models",
                provider.label()
            );
        }
    }

    #[test]
    fn provider_labels_are_distinct() {
        let mut seen = Vec::new();
        for provider in PROVIDERS {
            assert!(!seen.contains(&provider.label()), "duplicate provider label");
            seen.push(provider.label());
        }
    }
}
