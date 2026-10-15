//! Child windows and domains for a parent app (HAL.md 5c / 5d). Everything here produces
//! `CxOsOp`s (turned into HAL commands by `ops.rs`) or is a reading helper; the answers come back
//! as `Event::ChildWindow`, `ChildWindowClosed`, `DomainState` and `DomainList`.
//!
//! A window manager draws a child window's `ChildWindowEvent::texture` through a `Texture` made
//! with [`Texture::from_hal_lent`] (re-bound with [`Texture::set_hal_lent`] when the window
//! changes), forwards the input it receives in the child window's coordinates, and grants focus.
//! The domain commands work for a privileged session only: the HAL rejects and counts them otherwise.

use crate::{
    cx::Cx,
    cx_api::CxOsOp,
    event::{LentId, ScrollPhase},
    makepad_math::*,
    texture::{Texture, TextureFormat},
};
use makepad_hal::{buffers::pack_forward, input, RawEvent};

pub(crate) fn hal_id(id: LentId) -> makepad_hal::Id {
    makepad_hal::Id::new(id.index, id.gen)
}

pub(crate) fn lent_id(id: makepad_hal::Id) -> LentId {
    LentId { index: id.index, gen: id.gen }
}

/// A parent-message endpoint tied to one session, so ports can send without retaining Cx.
#[derive(Clone)]
pub struct HalParentSender { session: u32 }

impl HalParentSender {
    pub fn send(&self, message: &str) {
        let mut writer = makepad_hal::CmdWriter::new(self.session);
        writer.parent_message(message.as_bytes());
        makepad_hal::hal_submit(&writer.buf());
    }
}

impl Texture {
    /// A texture that shows a surface the HAL lent (a child's window): draws sample it like any
    /// other texture; Cx never allocates or uploads it.
    #[track_caller]
    pub fn from_hal_lent(cx: &mut Cx, surface: LentId) -> Texture {
        let texture = Texture::new_with_format(cx, TextureFormat::Unknown);
        texture.set_hal_lent(cx, surface);
        texture
    }

    /// Points the texture at another lent surface (the child's window was resized).
    pub fn set_hal_lent(&self, cx: &mut Cx, surface: LentId) {
        let id = self.texture_id();
        cx.hal.frames.lend(id.0, id.generation(), hal_id(surface));
    }
}

impl Cx {
    /// The host's grant for this session, not a command-line assertion by the application.
    pub fn hal_can_host_domains(&self) -> bool {
        self.hal.info.is_some_and(|i| i.flags & makepad_hal::info_flags::PRIVILEGED != 0)
    }

    pub fn hal_is_embedded(&self) -> bool {
        self.hal.info.is_some_and(|i| i.flags & makepad_hal::info_flags::EMBEDDED != 0)
    }

    /// Send an application message without a socket or a process-global routing slot.
    pub fn hal_send_parent_message(&self, message: &str) -> bool {
        let Some(sender) = self.hal_parent_sender() else { return false };
        sender.send(message);
        true
    }

    pub fn hal_parent_sender(&self) -> Option<HalParentSender> {
        self.hal_is_embedded().then(|| HalParentSender { session: self.hal.info.unwrap().session })
    }

    /// A correlated caption query; a later press cannot consume an earlier answer.
    pub fn hal_query_child_drag(&mut self, child: u32, window: LentId, request: u32, pos: Vec2d) {
        self.hal_forward(child, window, &RawEvent::parent_drag_query(makepad_hal::Id::NONE, request, pos.x, pos.y), &[]);
    }

    /// Resizes the child's window to `size` points at `dpi`; the child gets a `WindowGeomChange`,
    /// this app a `ChildWindow` with the new surface.
    pub fn hal_child_geom(&mut self, child: u32, window: LentId, size: Vec2d, dpi: f64) {
        self.platform_ops.push_back(CxOsOp::ChildGeom { child, window, size, dpi });
    }

    /// Delivers a raw event to the child's window (coordinates in that window's space).
    pub fn hal_forward(&mut self, child: u32, window: LentId, event: &RawEvent, payload: &[u8]) {
        self.platform_ops.push_back(CxOsOp::ForwardEvent { child, window, packed: pack_forward(event, payload) });
    }

    pub fn hal_forward_mouse_down(&mut self, child: u32, window: LentId, pos: Vec2d, button: u32, modifiers: u32) {
        self.hal_forward(child, window, &RawEvent::mouse_down(makepad_hal::Id::NONE, pos.x, pos.y, button, modifiers), &[]);
    }

    pub fn hal_forward_mouse_move(&mut self, child: u32, window: LentId, pos: Vec2d, modifiers: u32) {
        self.hal_forward(child, window, &RawEvent::mouse_move(makepad_hal::Id::NONE, pos.x, pos.y, 0.0, 0.0, modifiers), &[]);
    }

    pub fn hal_forward_mouse_up(&mut self, child: u32, window: LentId, pos: Vec2d, button: u32, modifiers: u32) {
        self.hal_forward(child, window, &RawEvent::mouse_up(makepad_hal::Id::NONE, pos.x, pos.y, button, modifiers), &[]);
    }

    /// Forwards a scroll with its gesture phase (`ScrollPhase::None` is a wheel), so the
    /// child's scrollers follow this host's momentum stream like local ones.
    pub fn hal_forward_scroll(&mut self, child: u32, window: LentId, pos: Vec2d, delta: Vec2d, phase: ScrollPhase, modifiers: u32) {
        let phase = match phase {
            ScrollPhase::None => input::scroll::NONE,
            ScrollPhase::Began => input::scroll::BEGAN,
            ScrollPhase::Touched => input::scroll::TOUCHED,
            ScrollPhase::Changed => input::scroll::CHANGED,
            ScrollPhase::Ended => input::scroll::ENDED,
            ScrollPhase::Momentum => input::scroll::MOMENTUM,
            ScrollPhase::MomentumEnded => input::scroll::MOMENTUM_ENDED,
        };
        let precise = phase != input::scroll::NONE;
        self.hal_forward(child, window, &RawEvent::scroll(makepad_hal::Id::NONE, pos.x, pos.y, delta.x, delta.y, modifiers, precise, phase), &[]);
    }

    /// The child owns the keyboard, IME, cursor and clipboard; zero returns them to the parent.
    pub fn hal_focus_child(&mut self, child: u32) {
        self.platform_ops.push_back(CxOsOp::FocusChild(child));
    }

    /// Asks the child to close its window (it decides, then the window closes).
    pub fn hal_close_child_window(&mut self, child: u32, window: LentId) {
        self.platform_ops.push_back(CxOsOp::CloseChildWindow { child, window });
    }

    /// Starts `app` (a package or path) as a child domain; `Event::DomainState` reports it with
    /// the returned request id. Privileged sessions only.
    pub fn hal_start_domain(&mut self, app: &str, args: &[String]) -> u32 {
        // An argv is not a shell command: preserve spaces, quotes and empty arguments.
        let args: String = args.iter().flat_map(|arg| arg.chars().chain(std::iter::once('\0'))).collect();
        self.hal.next_domain_request += 1;
        let request = self.hal.next_domain_request;
        self.platform_ops.push_back(CxOsOp::StartDomain { request, app: app.to_string(), args });
        request
    }

    /// Asks the domain to quit, or frees it at once with `kill`.
    pub fn hal_stop_domain(&mut self, domain: u32, kill: bool) {
        self.platform_ops.push_back(CxOsOp::StopDomain { domain, kill });
    }

    /// Frees the domain's session and starts its app again under the same id.
    pub fn hal_relaunch_domain(&mut self, domain: u32) {
        self.platform_ops.push_back(CxOsOp::RelaunchDomain(domain));
    }

    /// Asks for the domains this app started; answered by `Event::DomainList` with the returned id.
    pub fn hal_list_domains(&mut self) -> u32 {
        self.hal.next_domain_request += 1;
        let request = self.hal.next_domain_request;
        self.platform_ops.push_back(CxOsOp::ListDomains(request));
        request
    }
}
