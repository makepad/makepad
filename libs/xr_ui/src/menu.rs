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
    pub haptics: Haptics,
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
        if let Some(card) = self.card {
            if let Some(a) = input::aim(rc, rh) {
                let (o, d) = input::ray(&a);
                out.hover = self.panel.hit(&card, o, d);
            }
            if out.hover.is_none() && !rc.active() {
                if let Some(tip) = rh.tip_pos_checked(XrHand::INDEX_TIP) {
                    out.hover = self.panel.poke(&card, tip);
                }
            }
        }
        if out.hover.is_some() && out.hover != self.panel.hover() {
            out.haptics.add(1, HOVER);
        }
        self.panel.set_hover(out.hover);
        if self.trigger.pressed(rc, rh) && out.hover.is_some() {
            out.pressed = out.hover;
            out.haptics.add(1, PRESS);
        }
        out
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
