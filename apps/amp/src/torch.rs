//! The torch: a small light the person picks up and carries over the device.
//! While it is away from its socket (on the rail, beside the controls) it
//! lights the surfaces round it through the relief buffer, like an LED, and
//! swings the window's key light toward itself, so bevels and shadows turn
//! with it. Let go, it rests where it is for a moment, then glides home; at
//! home it is a small lit orb in its socket and nothing repaints.
//!
//! On or off from the View menu, the ≡ menu or ⇧⌘L; a right-click on it
//! picks its colour. Both are kept in `<data>/torch.txt`.

use crate::App;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HwTorchBase = #(HwTorch::register_widget(vm))
    mod.widgets.HwTorch = set_type_default() do mod.widgets.HwTorchBase{
        width: Fill
        height: Fill
        // The orb: a hot core and a soft halo, in its colour.
        draw_orb +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let d = length(p - c)
                let core = 1.0 - smoothstep(5.5, 7.0, d)
                let halo = exp(-d * d / (2.0 * 11.0 * 11.0)) * 0.5
                let hot = 1.0 - smoothstep(0.0, 4.0, d)
                let col = mix(self.color.xyz, vec3(1.0, 1.0, 1.0), hot * 0.75)
                let a = clamp(core + halo, 0.0, 1.0)
                return vec4(col * a, a)
            }
        }
        // What the hand takes hold of: the core, not the halo (the keys
        // beside the socket stay clickable).
        draw_grip +: {
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }
        // Its socket: a small dark recess with a lit lip.
        draw_socket +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let d = length(p - c) - 8.5
                let hole = 1.0 - smoothstep(-0.8, 0.8, d)
                let lip = (1.0 - smoothstep(0.0, 1.2, abs(d - 0.6))) * clamp(0.5 - (p.y - c.y) / 18.0, 0.0, 1.0)
                let a = clamp(hole * 0.55 + lip * 0.35, 0.0, 1.0)
                let col = vec3(0.0, 0.0, 0.0) * hole * 0.55 + vec3(1.0, 1.0, 1.0) * lip * 0.35
                return vec4(col, a)
            }
        }
    }
}

/// How long the torch rests where it was let go before it goes home (s).
const REST_S: f64 = 2.0;
/// How long the glide home takes (s).
const GLIDE_S: f64 = 0.7;
/// The orb's quad (its halo) and its core's radius, points.
const ORB: f64 = 44.0;
const CORE: f64 = 7.0;
/// Away this far from home, the key light turns all the way to the torch.
const SWING_REACH: f64 = 140.0;

#[derive(Clone, Debug, Default)]
pub enum HwTorchAction {
    /// It moved: the app swings the key light and redraws.
    Moved,
    /// A right-click on it, at this point.
    Menu(DVec2),
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct HwTorch {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_orb: DrawColor,
    #[live]
    draw_socket: DrawColor,
    #[live]
    draw_grip: DrawColor,
    #[live(true)]
    visible: bool,
    /// Where it sits: `None` = in its socket.
    #[rust]
    at: Option<DVec2>,
    #[rust]
    home: DVec2,
    /// The area it may be carried over (the window).
    #[rust]
    bounds: Rect,
    #[rust]
    color: Vec4f,
    /// Carried: the hand's offset from the orb's centre.
    #[rust]
    grab: Option<DVec2>,
    #[rust]
    rest: Timer,
    /// Gliding home: since when, from where.
    #[rust]
    glide: Option<(f64, DVec2)>,
    #[rust]
    frame: NextFrame,
}

impl WidgetNode for HwTorch {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.draw_grip.area()
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_orb.redraw(cx);
    }
    fn visible(&self) -> bool {
        self.visible
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.at = None;
                self.grab = None;
                self.glide = None;
                cx.stop_timer(self.rest);
            }
            self.draw_orb.redraw(cx);
        }
    }
}

impl HwTorch {
    fn centre(&self) -> DVec2 {
        self.at.unwrap_or(self.home)
    }

    fn moved(&mut self, cx: &mut Cx) {
        self.draw_orb.redraw(cx);
        cx.widget_action(self.uid, HwTorchAction::Moved);
    }

    /// The key light's swing toward the torch: a direction (x right, y
    /// down, z out) and how far (0 at home).
    pub fn swing(&self) -> Option<(Vec3f, f32)> {
        let at = self.at.filter(|_| self.visible)?;
        let away = ((at - self.home).length() / SWING_REACH).clamp(0.0, 1.0);
        let c = self.bounds.pos + self.bounds.size * 0.5;
        let half = dvec2((self.bounds.size.x * 0.5).max(1.0), (self.bounds.size.y * 0.5).max(1.0));
        let x = ((at.x - c.x) / half.x).clamp(-1.2, 1.2);
        let y = ((at.y - c.y) / half.y).clamp(-1.2, 1.2);
        Some((vec3(x as f32, y as f32, 0.62), away as f32))
    }
}

impl Widget for HwTorch {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible {
            return;
        }
        if self.rest.is_event(event).is_some() && self.grab.is_none() {
            if let Some(at) = self.at {
                self.glide = Some((cx.seconds_since_app_start(), at));
                self.frame = cx.new_next_frame();
            }
        }
        if let Some(ne) = self.frame.is_event(event) {
            if let Some((t0, from)) = self.glide {
                let t = ((ne.time - t0) / GLIDE_S).clamp(0.0, 1.0);
                // Ease in and out: it lifts off gently and settles.
                let e = t * t * (3.0 - 2.0 * t);
                if t >= 1.0 {
                    self.glide = None;
                    self.at = None;
                } else {
                    self.at = Some(from + (self.home - from) * e);
                    self.frame = cx.new_next_frame();
                }
                self.moved(cx);
            }
        }
        match event.hits(cx, self.draw_grip.area()) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerDown(fe) if fe.mouse_button().is_some_and(|b| b.is_secondary()) => {
                cx.widget_action(self.uid, HwTorchAction::Menu(fe.abs));
            }
            Hit::FingerDown(fe) => {
                cx.set_cursor(MouseCursor::Grabbing);
                cx.stop_timer(self.rest);
                self.glide = None;
                self.grab = Some(fe.abs - self.centre());
            }
            Hit::FingerMove(fe) => {
                if let Some(grab) = self.grab {
                    cx.set_cursor(MouseCursor::Grabbing);
                    let b = self.bounds;
                    let p = fe.abs - grab;
                    let p = dvec2(p.x.clamp(b.pos.x + CORE, b.pos.x + b.size.x - CORE), p.y.clamp(b.pos.y + CORE, b.pos.y + b.size.y - CORE));
                    if self.at != Some(p) {
                        self.at = Some(p);
                        self.moved(cx);
                    }
                }
            }
            Hit::FingerUp(_) => {
                if self.grab.take().is_some() {
                    // Dropped in its socket: home at once.
                    if self.at.is_some_and(|at| (at - self.home).length() < 14.0) {
                        self.at = None;
                        self.moved(cx);
                    } else if self.at.is_some() {
                        self.rest = cx.start_timeout(REST_S);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.bounds = cx.walk_turtle(walk);
        let home = self.home;
        self.draw_socket.draw_abs(cx, Rect { pos: home - dvec2(11.0, 11.0), size: dvec2(22.0, 22.0) });
        let c = self.centre();
        let away = self.at.is_some();
        let slot = relief_open(cx, if away { 8.0 } else { 1.0 });
        self.draw_orb.color = self.color;
        self.draw_orb.draw_abs(cx, Rect { pos: c - dvec2(ORB * 0.5, ORB * 0.5), size: dvec2(ORB, ORB) });
        // The light it gives: wider and brighter than the core it draws,
        // so the surfaces round it catch it like an LED's spill.
        let intensity = if away { 7.0 } else { 1.2 };
        let glow = if away { 13.0 } else { CORE };
        let proxy = ReliefProxy {
            inset: ORB * 0.5 - glow,
            radius: glow,
            disc: true,
            emissive: vec4(self.color.x, self.color.y, self.color.z, intensity),
        };
        relief_close(cx, slot, self.draw_orb.area(), Some(proxy));
        let grip = CORE + 5.0;
        self.draw_grip.draw_abs(cx, Rect { pos: c - dvec2(grip, grip), size: dvec2(grip * 2.0, grip * 2.0) });
        DrawStep::done()
    }
}

impl HwTorchRef {
    pub fn set_home(&self, cx: &mut Cx, home: DVec2) {
        if let Some(mut inner) = self.borrow_mut() {
            if (inner.home - home).length() > 0.5 {
                inner.home = home;
                inner.draw_orb.redraw(cx);
            }
        }
    }

    pub fn set_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.color != color {
                inner.color = color;
                inner.draw_orb.redraw(cx);
            }
        }
    }

    pub fn swing(&self) -> Option<(Vec3f, f32)> {
        self.borrow().and_then(|inner| inner.swing())
    }

    pub fn moved(&self, actions: &Actions) -> bool {
        matches!(actions.find_widget_action(self.widget_uid()).map(|a| a.cast::<HwTorchAction>()), Some(HwTorchAction::Moved))
    }

    pub fn menu(&self, actions: &Actions) -> Option<DVec2> {
        match actions.find_widget_action(self.widget_uid()).map(|a| a.cast::<HwTorchAction>()) {
            Some(HwTorchAction::Menu(at)) => Some(at),
            _ => None,
        }
    }
}

// ---- the app's side ----

/// The colours a right-click offers: (id, name, colour; `None` = the style's accent).
const COLOURS: [(&str, &str, Option<[f32; 3]>); 6] = [
    ("accent", "Style accent", None),
    ("warm", "Warm white", Some([1.0, 0.86, 0.66])),
    ("amber", "Amber", Some([1.0, 0.58, 0.12])),
    ("cyan", "Cyan", Some([0.2, 0.9, 1.0])),
    ("magenta", "Magenta", Some([1.0, 0.25, 0.8])),
    ("green", "Green", Some([0.35, 1.0, 0.4])),
];

fn torch_path() -> Option<std::path::PathBuf> {
    Some(crate::worker::data_dir()?.join("torch.txt"))
}

/// What is kept: on or off, the colour's id.
#[derive(Clone, Debug, PartialEq)]
pub struct TorchSettings {
    pub on: bool,
    pub colour: String,
}

impl Default for TorchSettings {
    fn default() -> Self {
        Self { on: false, colour: "accent".into() }
    }
}

impl TorchSettings {
    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("on", v)) => s.on = v.trim() == "1",
                Some(("colour", v)) if COLOURS.iter().any(|(id, _, _)| *id == v.trim()) => s.colour = v.trim().to_string(),
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        format!("on {}\ncolour {}\n", self.on as u8, self.colour)
    }
}

impl App {
    fn torch_settings(&mut self) -> &mut TorchSettings {
        self.torch.get_or_insert_with(|| torch_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| TorchSettings::parse(&t)).unwrap_or_default())
    }

    fn save_torch(&mut self) {
        let text = self.torch_settings().serialize();
        if let Some(path) = torch_path() {
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(path, text);
        }
    }

    pub(crate) fn torch_on(&mut self) -> bool {
        self.torch_settings().on
    }

    pub(crate) fn toggle_torch(&mut self, cx: &mut Cx) {
        let on = !self.torch_settings().on;
        self.torch_settings().on = on;
        self.save_torch();
        self.apply_torch(cx);
    }

    /// Its visibility and colour, after a start, a toggle or a restyle.
    pub(crate) fn apply_torch(&mut self, cx: &mut Cx) {
        let settings = self.torch_settings().clone();
        let torch = self.ui.hw_torch(cx, ids!(torch));
        if torch.visible() != settings.on {
            torch.set_visible(cx, settings.on);
            // Never drawn yet, it has no area of its own to redraw.
            self.ui.redraw(cx);
        }
        let colour = COLOURS.iter().find(|(id, _, _)| *id == settings.colour).and_then(|(_, _, c)| *c);
        let a = self.model.theme.accent;
        let colour = colour.map(|c| vec4(c[0], c[1], c[2], 1.0)).unwrap_or(vec4(a.x, a.y, a.z, 1.0));
        torch.set_color(cx, colour);
        self.torch_swing(cx);
    }

    /// Its socket: on the rail, just before the controls.
    pub(crate) fn place_torch(&mut self, cx: &mut Cx) {
        if !self.torch_settings().on {
            return;
        }
        let controls = self.ui.widget(cx, ids!(controls)).area();
        let rail = self.ui.widget(cx, ids!(rail)).area();
        if !controls.is_valid(cx) || !rail.is_valid(cx) {
            return;
        }
        let (c, r) = (controls.rect(cx), rail.rect(cx));
        self.ui.hw_torch(cx, ids!(torch)).set_home(cx, dvec2(c.pos.x - 20.0, r.pos.y + r.size.y * 0.5));
    }

    fn torch_swing(&mut self, cx: &mut Cx) {
        let Some(window) = self.ui.window(cx, ids!(main_window)).window_id() else { return };
        let swing = self.ui.hw_torch(cx, ids!(torch)).swing();
        set_relief_light_swing(cx, window, swing);
    }

    pub(crate) fn torch_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let torch = self.ui.hw_torch(cx, ids!(torch));
        if torch.moved(actions) {
            self.torch_swing(cx);
            // The surfaces take the swung light as they draw.
            self.ui.redraw(cx);
        }
        if let Some(at) = torch.menu(actions) {
            let current = self.torch_settings().colour.clone();
            let a = self.model.theme.accent;
            let mut rows: Vec<MenuRow> = COLOURS
                .iter()
                .map(|(id, name, c)| {
                    let fill = c.map(|c| vec4(c[0], c[1], c[2], 1.0)).unwrap_or(vec4(a.x, a.y, a.z, 1.0));
                    MenuRow::new(LiveId::from_str(&format!("torch:{id}")), name).radio(*id == current).swatch(fill, fill)
                })
                .collect();
            rows.push(MenuRow::separator());
            rows.push(MenuRow::new(live_id!(torch_off), "Put the torch away").key("⇧⌘L"));
            let anchor = Rect { pos: at, size: dvec2(0.0, 0.0) };
            self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(torch_menu), rows, anchor, MenuPlace::At);
        }
        if let Some(id) = menu_picked(actions, live_id!(torch_menu)) {
            if id == live_id!(torch_off) {
                self.toggle_torch(cx);
            } else if let Some((cid, _, _)) = COLOURS.iter().find(|(cid, _, _)| LiveId::from_str(&format!("torch:{cid}")) == id) {
                self.torch_settings().colour = cid.to_string();
                self.save_torch();
                self.apply_torch(cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_back_and_junk_keeps_defaults() {
        let s = TorchSettings { on: true, colour: "amber".into() };
        assert_eq!(TorchSettings::parse(&s.serialize()), s);
        assert_eq!(TorchSettings::parse("on x\ncolour plaid\n"), TorchSettings::default());
    }
}
