//! The wrist menu: a panel on a card at the left controller (or the left wrist of a tracked
//! hand). It opens with a button on the left controller (by default Y) or, with tracked hands,
//! while the left palm is open and turned up. The right hand's aim points at it (a tracked
//! hand's index tip touches it), its trigger (index pinch) presses.

use crate::card::Card;
use crate::haptic::{Haptics, HOVER, PRESS};
use crate::input::{self, Buttons, Trigger};
use crate::panel::XrPanel;
use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrWristMenu = set_type_default() do #(XrWristMenu::script_component(vm)){
        panel: mod.widgets.XrPanel{}
    }
}

/// What opens the menu on a controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOpen {
    /// A left controller button toggles it (`XrController::CLICK_*`).
    Button(u16),
    /// Turning the left controller's side up toward the eyes, as one looks at a watch, shows
    /// it while held (for apps whose buttons all belong to the game).
    WristTurn,
}

/// One frame of the menu.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuStep {
    /// The menu shows.
    pub open: bool,
    /// The pressable cell under the right hand's pointer.
    pub hover: Option<u32>,
    /// A cell pressed this frame.
    pub pressed: Option<u32>,
    /// A captured slider, including the final value on release.
    pub slider: Option<SliderDrag>,
    pub haptics: Haptics,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderDrag {
    pub id: u32,
    pub value: f32,
    pub ended: bool,
}

struct SliderCapture {
    drag: SliderDrag,
    card: Card,
    poke: bool,
    tick: f32,
}

#[derive(Script, ScriptHook)]
pub struct XrWristMenu {
    #[live]
    pub panel: XrPanel,
    #[rust(MenuOpen::Button(XrController::CLICK_Y))]
    pub open_with: MenuOpen,
    #[rust]
    pub open: bool,
    #[rust]
    card: Option<Card>,
    #[rust]
    trigger: Trigger,
    #[rust]
    turned_since: Option<f64>,
    #[rust]
    slider: Option<SliderCapture>,
    #[rust]
    pointer: Option<Vec2f>,
    #[rust]
    poke: bool,
}

/// How long the wrist must stay turned before the menu shows (s).
const WRIST_DWELL: f64 = 0.3;

impl XrWristMenu {
    /// Where it stands this frame (None: closed, or no left hand to stand on).
    pub fn card(&self) -> Option<Card> {
        self.card
    }

    /// One headset frame: opens or closes, places the card, finds the hover and the press.
    /// `buttons`: this frame's press edges (shared with the app's own button handling).
    pub fn step(&mut self, s: &XrState, buttons: &Buttons, now: f64) -> MenuStep {
        let mut out = self.prepare(s, buttons, now);
        self.interact(s, buttons, &mut out);
        out
    }

    /// Place and point without activating. Apps with draggables call this first, let
    /// touched draggables claim the press, then call `interact` with those claims.
    pub fn prepare(&mut self, s: &XrState, buttons: &Buttons, now: f64) -> MenuStep {
        let (lc, lh) = (&s.left_controller, &s.left_hand);
        let (rc, rh) = (&s.right_controller, &s.right_hand);
        if lc.active() {
            match self.open_with {
                MenuOpen::Button(bit) => {
                    if buttons.hit(0, bit) {
                        self.open = !self.open;
                    }
                }
                MenuOpen::WristTurn => {
                    // the controller's outer side (-x) up toward the eyes
                    let side = lc.grip_pose.orientation.rotate_vec3(&vec3f(-1.0, 0.0, 0.0));
                    let to_head = (s.head_pose.position - lc.grip_pose.position).normalize();
                    let turned = side.dot(to_head) > 0.7;
                    let since = if turned { *self.turned_since.get_or_insert(now) } else { now };
                    if !turned {
                        self.turned_since = None;
                    }
                    self.open = turned && now - since >= WRIST_DWELL;
                }
            }
        } else if lh.in_view() {
            self.open = lh.is_open() && !lh.is_palm_down(true);
        }
        let size = self.panel.size();
        self.card = if self.open { Card::wrist(lc, lh, size) } else { None };
        let mut out = MenuStep { open: self.card.is_some(), ..Default::default() };
        let mut pointer = None;
        let mut poke = false;
        if let Some(card) = self.card {
            if let Some(a) = input::aim(rc, rh) {
                let (o, d) = input::ray(&a);
                pointer = card.hit(o, d).map(|(uv, _)| uv);
                out.hover = pointer.and_then(|uv| self.panel.cell_at(uv));
            }
            if out.hover.is_none() && !rc.active() {
                if let Some(tip) = rh.tip_pos_checked(XrHand::INDEX_TIP) {
                    pointer = card.poke(tip);
                    out.hover = pointer.and_then(|uv| self.panel.cell_at(uv));
                    poke = out.hover.is_some();
                }
            }
        }
        self.pointer = pointer;
        self.poke = poke;
        // An existing slider owns the hand until its release, even outside the card.
        if let Some(capture) = &self.slider {
            out.slider = Some(capture.drag);
            out.hover = Some(capture.drag.id);
        }
        out
    }

    /// Activate only unclaimed presses. Always advance the trigger edge, including
    /// while another consumer owns it. Existing slider captures continue until release.
    pub fn interact(&mut self, s: &XrState, buttons: &Buttons, out: &mut MenuStep) {
        let (rc, rh) = (&s.right_controller, &s.right_hand);
        let (pointer, poke) = (self.pointer, self.poke);
        let pressed = self.trigger.pressed(rc, rh);
        if pressed && !buttons.claimed(1) && out.hover.is_some() && self.slider.is_none() {
            let id = out.hover.unwrap();
            if let Some(value) = pointer.and_then(|uv| self.panel.slider_at(id, uv)) {
                self.slider = self.card.map(|card| SliderCapture {
                    drag: SliderDrag { id, value, ended: false }, card, poke, tick: value,
                });
            } else {
                out.pressed = Some(id);
            }
            out.haptics.add(1, PRESS);
        }
        out.slider = None;
        if let Some(mut capture) = self.slider.take() {
            // A rewritten menu may have removed the control. Never apply its release to
            // an unrelated row. Losing a pose retains the last value until release.
            if self.panel.slider_at(capture.drag.id, Vec2f::default()).is_some() {
                if let Some(card) = self.card {
                    capture.card = card;
                }
                let uv = if capture.poke {
                    rh.tip_pos_checked(XrHand::INDEX_TIP).map(|p| capture.card.project(p))
                } else {
                    input::aim(rc, rh).and_then(|a| {
                        let (o, d) = input::ray(&a);
                        capture.card.project_ray(o, d).map(|(uv, _)| uv)
                    })
                };
                if let Some(value) = uv.and_then(|uv| self.panel.slider_at(capture.drag.id, uv)).filter(|v| v.is_finite()) {
                    capture.drag.value = value;
                    if (value - capture.tick).abs() >= 0.05 {
                        capture.tick = value;
                        out.haptics.add(1, HOVER);
                    }
                }
                capture.drag.ended = !self.trigger.down();
                self.panel.set_slider_value(capture.drag.id, capture.drag.value);
                out.hover = Some(capture.drag.id);
                out.slider = Some(capture.drag);
                if !capture.drag.ended {
                    self.slider = Some(capture);
                }
            }
        }
        if out.hover.is_some() && out.hover != self.panel.hover() {
            out.haptics.add(1, HOVER);
        }
        self.panel.set_hover(out.hover);
    }

    /// Its texture (inside the XR pass, before the scene's draws), when it shows.
    pub fn render(&mut self, cx: &mut Cx2d) {
        if self.card.is_some() {
            self.panel.render(cx);
        }
    }

    pub fn draw(&mut self, cx: &mut Cx3d) {
        if let Some(card) = self.card {
            self.panel.draw(cx, &card);
        }
    }

    /// As [`XrWristMenu::draw`] in a pass whose scene is not the room (`room_to_scene`).
    pub fn draw_in(&mut self, cx: &mut Cx3d, room_to_scene: &Mat4f) {
        if let Some(card) = self.card {
            self.panel.draw_in(cx, &card, room_to_scene);
        }
    }
}
