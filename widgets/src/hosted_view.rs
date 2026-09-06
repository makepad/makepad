//! Two resident presentations of one application. The host selects the compact
//! tile or the full view on the existing window/swapchain; application state,
//! workers and process ownership remain with the app.
use crate::*;
use makepad_micro_serde::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerJson, DeJson)]
pub enum HostedViewMode {
    #[default]
    Full,
    Tile,
}

#[derive(SerJson, DeJson)]
struct ViewEnvelope { makepad_view: HostedViewMode }

impl HostedViewMode {
    pub fn to_json(self) -> String { ViewEnvelope { makepad_view: self }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_view\"") { return None; }
        ViewEnvelope::deserialize_json(json).ok().map(|e| e.makepad_view)
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.HostedViewBase = #(HostedView::register_widget(vm))
    mod.widgets.HostedView = set_type_default() do mod.widgets.HostedViewBase{
        width: Fill height: Fill
        full: View{width: Fill height: Fill}
        tile: View{width: Fill height: Fill}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct HostedView {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[walk] walk: Walk,
    #[find] #[live] full: WidgetRef,
    #[find] #[live] tile: WidgetRef,
    #[redraw] #[rust] area: Area,
    #[rust] mode: HostedViewMode,
}

impl HostedView {
    pub fn mode(&self) -> HostedViewMode { self.mode }
    pub fn set_mode(&mut self, cx: &mut Cx, mode: HostedViewMode) {
        if self.mode != mode {
            self.mode = mode;
            self.full.redraw(cx);
            self.tile.redraw(cx);
            self.area.redraw(cx);
        }
    }
}

impl Widget for HostedView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Custom(json) = event {
            if let Some(mode) = HostedViewMode::parse(json) { self.set_mode(cx, mode); }
        }
        let input = matches!(event, Event::MouseDown(_) | Event::MouseUp(_) | Event::MouseMove(_)
            | Event::TouchUpdate(_) | Event::Scroll(_) | Event::KeyDown(_) | Event::KeyUp(_)
            | Event::TextInput(_) | Event::BackPressed { .. });
        // Non-input events keep the same resident views' asynchronous work
        // alive. Hidden faces never receive a pointer or a text insertion.
        if !input || self.mode == HostedViewMode::Full { self.full.handle_event(cx, event, scope); }
        if !input || self.mode == HostedViewMode::Tile { self.tile.handle_event(cx, event, scope); }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let active = if self.mode == HostedViewMode::Tile { &self.tile } else { &self.full };
        let step = active.draw_walk(cx, scope, walk);
        self.area = active.area();
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presentation_protocol_is_typed_and_ignores_other_custom_messages() {
        for mode in [HostedViewMode::Full, HostedViewMode::Tile] {
            assert_eq!(HostedViewMode::parse(&mode.to_json()), Some(mode));
        }
        assert_eq!(HostedViewMode::parse(r#"{"wm":"CloseRequested"}"#), None);
        assert_eq!(HostedViewMode::parse(r#"{"makepad_view":"other"}"#), None);
    }
}
