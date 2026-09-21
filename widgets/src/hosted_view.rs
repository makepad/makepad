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

/// The host's open or close of a tile app whose compact face shows the
/// same content as its full face: a location-accurate crossfade. `tile`
/// is the tile's rect relative to the full viewport (x, y, w, h) and `app`
/// that viewport's size, in the app's points. Opening, the app places its
/// tile subject at `tile` on its first full frame and glides it to rest
/// over `seconds`; closing, it glides the subject back to `tile` so the
/// tile face takes over in place.
#[derive(Clone, Copy, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedTransition {
    pub tile: [f64; 4],
    pub app: [f64; 2],
    pub opening: bool,
    pub seconds: f64,
}

impl HostedTransition {
    pub fn tile_rect(&self) -> Rect {
        Rect { pos: dvec2(self.tile[0], self.tile[1]), size: dvec2(self.tile[2], self.tile[3]) }
    }
}

/// One face message: the face, and the transition it arrives with, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostedFace {
    pub mode: HostedViewMode,
    pub transition: Option<HostedTransition>,
}

#[derive(SerJson, DeJson)]
struct ViewEnvelope { makepad_view: HostedViewMode, makepad_view_transition: Option<HostedTransition> }

impl HostedViewMode {
    pub fn to_json(self) -> String { HostedFace { mode: self, transition: None }.to_json() }
    pub fn parse(json: &str) -> Option<Self> {
        HostedFace::parse(json).map(|face| face.mode)
    }
}

impl HostedFace {
    pub fn to_json(self) -> String {
        ViewEnvelope { makepad_view: self.mode, makepad_view_transition: self.transition }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_view\"") { return None; }
        if let Ok(e) = ViewEnvelope::deserialize_json(json) {
            return Some(HostedFace { mode: e.makepad_view, transition: e.makepad_view_transition });
        }
        // A face alone, from a sender that knows no transition.
        BareEnvelope::deserialize_json(json).ok().map(|e| HostedFace { mode: e.makepad_view, transition: None })
    }
}

#[derive(SerJson, DeJson)]
struct BareEnvelope { makepad_view: HostedViewMode }

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
        // A bare face still parses (no transition), and a transition rides along.
        let bare = BareEnvelope { makepad_view: HostedViewMode::Tile }.serialize_json();
        assert!(!bare.contains("transition"));
        assert_eq!(HostedFace::parse(&bare), Some(HostedFace { mode: HostedViewMode::Tile, transition: None }));
        let face = HostedFace {
            mode: HostedViewMode::Full,
            transition: Some(HostedTransition { tile: [16.0, 70.0, 370.0, 178.0], app: [402.0, 778.0], opening: true, seconds: 0.25 }),
        };
        let parsed = HostedFace::parse(&face.to_json()).unwrap();
        assert_eq!(parsed, face);
        assert_eq!(parsed.transition.unwrap().tile_rect(), Rect { pos: dvec2(16.0, 70.0), size: dvec2(370.0, 178.0) });
        assert_eq!(HostedViewMode::parse(&face.to_json()), Some(HostedViewMode::Full), "older readers see the face alone");
    }
}
