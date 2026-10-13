//! Child windows and domains, as a parent session sees them (HAL.md 5c / 5d). A parent that
//! started other apps (domains) in the same process gets one `ChildWindow` per window they open:
//! a surface it draws like any texture (`Texture::from_hal_lent`) and the size it must lay out.
//! Only the HAL client produces these events; the plain-data types are shared.

/// An id the HAL chose (index and generation): a child's window, or the surface lent to the
/// parent for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LentId {
    pub index: u32,
    pub gen: u32,
}

/// A child's window became (or changed): the surface to draw, in pixels, and its title.
/// Sent again with a new `texture` when the parent resized the window.
#[derive(Clone, Debug)]
pub struct ChildWindowEvent {
    /// the child domain
    pub child: u32,
    /// the window in the child's own id space (what `Forward` and `ChildGeom` name)
    pub window: LentId,
    /// the surface, valid until `ChildWindowClosed` or the next `ChildWindow` of the window
    pub texture: LentId,
    pub width: u32,
    pub height: u32,
    pub dpi: f64,
    pub title: String,
}

/// A child presented a frame into the lent texture; the parent can redraw its view of it.
#[derive(Clone, Copy, Debug)]
pub struct ChildFrameEvent {
    pub child: u32,
    pub window: LentId,
    pub texture: LentId,
    pub frame: u64,
}

#[derive(Clone, Debug)]
pub struct ChildWindowClosedEvent {
    pub child: u32,
    pub window: LentId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainStatus {
    /// the host is compiling / loading the app, `phase` and `percent` say how far
    Compiling,
    Running,
    Crashed,
    Exited,
}

/// A domain this session started changed state.
#[derive(Clone, Debug)]
pub struct DomainStateEvent {
    pub domain: u32,
    pub status: DomainStatus,
    pub phase: u32,
    /// 0..=1
    pub percent: f32,
    /// the `start_domain` request this answers (0 = none)
    pub request: u32,
    pub app: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct DomainEntry {
    pub domain: u32,
    pub status: DomainStatus,
    pub phase: u32,
    pub percent: f32,
    pub app: String,
}

/// The answer to `list_domains`.
#[derive(Clone, Debug)]
pub struct DomainListEvent {
    pub request: u32,
    pub domains: Vec<DomainEntry>,
}

/// Window-management information from a child's window to its embedding parent.
#[derive(Clone, Debug)]
pub enum ChildWindowAction {
    OwnChrome(bool),
    DragAnswer { request: u32, response: super::WindowDragQueryResponse },
    /// The HAL's window_state value; the parent owns the layout policy.
    State(u32),
}

#[derive(Clone, Debug)]
pub struct ChildWindowActionEvent {
    pub child: u32,
    pub window: LentId,
    pub action: ChildWindowAction,
}
