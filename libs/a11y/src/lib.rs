//! Makepad's semantic UI tree.
//!
//! Every widget describes itself (role, name, value, states, actions, text) in one place; Cx collects
//! those descriptions while drawing into one tree per window and sends changes as a [`TreeUpdate`]. The
//! platform side keeps a [`mirror::Mirror`] of each tree. Its consumers are all equal: the OS
//! accessibility adapters (screen readers), the remote `ui` commands (agents, tests, devhub, Desk Link)
//! and the automation scripts. They read the same tree and act through the same [`ActionRequest`] path.
//!
//! No dependencies; plain data. Text offsets on the wire are UTF-8 byte offsets.

pub mod mirror;
pub mod query;
pub mod render;
pub mod roles;
pub mod wire;
pub mod wire_js;

pub use roles::Role;

/// A node's id: the widget uid in the low 48 bits, a widget-chosen part index (tab, row, dock tab) in
/// the high 16. 0 is "none". Stable for the widget's life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const NONE: NodeId = NodeId(0);
    pub const UID_BITS: u32 = 48;
    pub fn widget(uid: u64) -> Self {
        NodeId(uid & ((1 << Self::UID_BITS) - 1))
    }
    /// A sub-element of a widget. `part` 0 is the widget itself, so parts start at 1.
    pub fn part(uid: u64, part: u16) -> Self {
        NodeId((uid & ((1 << Self::UID_BITS) - 1)) | ((part as u64) << Self::UID_BITS))
    }
    pub fn uid(self) -> u64 {
        self.0 & ((1 << Self::UID_BITS) - 1)
    }
    pub fn part_index(self) -> u16 {
        (self.0 >> Self::UID_BITS) as u16
    }
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
    /// The short hex form used by `ui` output and queries (`#1a2`, `#1a2.3` for parts).
    pub fn short(self) -> String {
        if self.part_index() == 0 {
            format!("{:x}", self.uid())
        } else {
            format!("{:x}.{}", self.uid(), self.part_index())
        }
    }
    pub fn parse_short(s: &str) -> Option<NodeId> {
        let s = s.strip_prefix('#').unwrap_or(s);
        match s.split_once('.') {
            Some((u, p)) => Some(NodeId::part(u64::from_str_radix(u, 16).ok()?, p.parse().ok()?)),
            None => Some(NodeId::widget(u64::from_str_radix(s, 16).ok()?)),
        }
    }
}

/// Window-local layout points (the same space as hit testing and remote input).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        !(self.w > 0.0 && self.h > 0.0)
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

macro_rules! bitset {
    ($(#[$m:meta])* $name:ident : $ty:ty { $( $flag:ident = $bit:literal, $short:literal; )* }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub struct $name(pub $ty);
        impl $name {
            $( pub const $flag: $name = $name(1 << $bit); )*
            pub const NAMES: &'static [($name, &'static str)] = &[$( ($name::$flag, $short), )*];
            pub fn empty() -> Self { $name(0) }
            pub fn contains(self, other: $name) -> bool { self.0 & other.0 == other.0 && other.0 != 0 }
            pub fn intersects(self, other: $name) -> bool { self.0 & other.0 != 0 }
            pub fn insert(&mut self, other: $name) { self.0 |= other.0 }
            pub fn remove(&mut self, other: $name) { self.0 &= !other.0 }
            pub fn set(&mut self, other: $name, on: bool) { if on { self.insert(other) } else { self.remove(other) } }
            pub fn is_empty(self) -> bool { self.0 == 0 }
            pub fn from_short(name: &str) -> Option<$name> {
                Self::NAMES.iter().find(|(_, n)| *n == name).map(|(f, _)| *f)
            }
            /// Comma-separated short names of the set flags.
            pub fn list(self) -> String {
                let mut out = String::new();
                for (f, n) in Self::NAMES {
                    if self.contains(*f) {
                        if !out.is_empty() { out.push(','); }
                        out.push_str(n);
                    }
                }
                out
            }
        }
        impl std::ops::BitOr for $name {
            type Output = $name;
            fn bitor(self, o: $name) -> $name { $name(self.0 | o.0) }
        }
    };
}

bitset! {
    /// Node states.
    States: u64 {
        FOCUSABLE = 0, "focusable";
        FOCUSED = 1, "focused";
        DISABLED = 2, "disabled";
        HIDDEN = 3, "hidden";
        SELECTED = 4, "selected";
        SELECTABLE = 5, "selectable";
        EXPANDED = 6, "expanded";
        COLLAPSED = 7, "collapsed";
        PRESSED = 8, "pressed";
        READ_ONLY = 9, "read_only";
        REQUIRED = 10, "required";
        BUSY = 11, "busy";
        MODAL = 12, "modal";
        MULTISELECTABLE = 13, "multiselectable";
        OFFSCREEN = 14, "offscreen";
        EDITABLE = 15, "editable";
        DEFAULT = 16, "default";
        VISITED = 17, "visited";
        INVALID = 18, "invalid";
        HAS_POPUP = 19, "has_popup";
        MULTILINE = 20, "multiline";
        PROTECTED = 21, "protected";
        VERTICAL = 22, "vertical";
        HORIZONTAL = 23, "horizontal";
        ACTIVE = 24, "active";
    }
}

bitset! {
    /// What a node can be asked to do.
    Actions: u32 {
        FOCUS = 0, "focus";
        PRESS = 1, "press";
        TOGGLE = 2, "toggle";
        EXPAND = 3, "expand";
        COLLAPSE = 4, "collapse";
        INCREMENT = 5, "increment";
        DECREMENT = 6, "decrement";
        SET_VALUE = 7, "set_value";
        SELECT = 8, "select";
        ADD_SELECTION = 9, "add_selection";
        REMOVE_SELECTION = 10, "remove_selection";
        SCROLL_INTO_VIEW = 11, "scroll_into_view";
        SCROLL = 12, "scroll";
        SHOW_MENU = 13, "show_menu";
        DISMISS = 14, "dismiss";
        SET_TEXT_SELECTION = 15, "set_text_selection";
        REPLACE_TEXT = 16, "replace_text";
        CUSTOM = 17, "custom";
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Toggle {
    #[default]
    None = 0,
    Off = 1,
    On = 2,
    Mixed = 3,
}

impl Toggle {
    pub fn from_u8(v: u8) -> Toggle {
        match v {
            1 => Toggle::Off,
            2 => Toggle::On,
            3 => Toggle::Mixed,
            _ => Toggle::None,
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Toggle::None => "",
            Toggle::Off => "off",
            Toggle::On => "on",
            Toggle::Mixed => "mixed",
        }
    }
}

/// Live-region politeness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Live {
    #[default]
    Off = 0,
    Polite = 1,
    Assertive = 2,
}

impl Live {
    pub fn from_u8(v: u8) -> Live {
        match v {
            1 => Live::Polite,
            2 => Live::Assertive,
            _ => Live::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RangeValue {
    pub min: f64,
    pub max: f64,
    pub now: f64,
    /// 0 = continuous.
    pub step: f64,
}

/// One laid-out line of a text node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineBox {
    /// UTF-8 byte offset of the line's first character.
    pub start: u32,
    pub rect: Rect,
    pub baseline: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextInfo {
    /// The node's full text (masked for passwords). UTF-8.
    pub content: String,
    /// Selection as (anchor, cursor) UTF-8 byte offsets; equal = caret.
    pub selection: (u32, u32),
    pub lines: Vec<LineBox>,
    /// x of every char boundary per line, flattened in line order (line i has one entry per char boundary
    /// from its start up to and including its end). Only sent for editable or focused text; empty
    /// otherwise, and then per-character bounds are approximated from the line box.
    pub glyph_x: Vec<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TableInfo {
    pub rows: u32,
    pub cols: u32,
    pub row: u32,
    pub col: u32,
    pub row_span: u32,
    pub col_span: u32,
}

macro_rules! relations {
    ($( $f:ident = $tag:literal, $short:literal; )*) => {
        /// Relations to other nodes.
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
        pub struct Relations { $( pub $f: Vec<NodeId>, )* }
        impl Relations {
            pub const NAMES: &'static [(u8, &'static str)] = &[$( ($tag, $short), )*];
            pub fn get(&self, tag: u8) -> Option<&Vec<NodeId>> {
                match tag { $( $tag => Some(&self.$f), )* _ => None }
            }
            pub fn get_mut(&mut self, tag: u8) -> Option<&mut Vec<NodeId>> {
                match tag { $( $tag => Some(&mut self.$f), )* _ => None }
            }
            pub fn is_empty(&self) -> bool { true $( && self.$f.is_empty() )* }
        }
    };
}

relations! {
    labelled_by = 0, "labelled_by";
    described_by = 1, "described_by";
    controls = 2, "controls";
    flows_to = 3, "flows_to";
    active_descendant = 4, "active_descendant";
    error_message = 5, "error_message";
}

/// One node of the semantic tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub role: Role,
    pub states: States,
    pub actions: Actions,
    pub bounds: Rect,
    pub name: String,
    pub description: String,
    pub value: String,
    pub placeholder: String,
    pub key_shortcut: String,
    pub role_description: String,
    /// The widget type name (registry name), for tools; adapters may expose it as a class/automation id.
    pub class: String,
    /// The widget's dotted path of named ancestors (window-relative), for tools and source mapping.
    pub path: String,
    pub range: Option<RangeValue>,
    pub toggle: Toggle,
    pub level: u32,
    pub pos_in_set: u32,
    pub set_size: u32,
    pub live: Live,
    pub relations: Relations,
    /// App-specific verbs offered as named actions.
    pub custom_actions: Vec<String>,
    pub children: Vec<NodeId>,
    pub text: Option<TextInfo>,
    pub table: Option<TableInfo>,
}

impl Default for Role {
    fn default() -> Self {
        Role::Generic
    }
}

/// What the platform should tell assistive technology about a change. Derived by the builder while
/// diffing (focus/value/name/state/children/text/caret), plus explicit announcements.
#[derive(Clone, Debug, PartialEq)]
pub enum Notification {
    FocusChanged(NodeId),
    NameChanged(NodeId),
    ValueChanged(NodeId),
    StatesChanged { node: NodeId, old: States, new: States },
    ChildrenChanged(NodeId),
    TextChanged { node: NodeId, start: u32, removed: u32, inserted: u32 },
    CaretMoved(NodeId),
    LiveRegionChanged(NodeId),
    Announce { text: String, live: Live },
}

/// The change of one window's tree since the previous update. The first update after activation is a
/// full tree (`full = true`): the receiver drops what it had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeUpdate {
    pub full: bool,
    pub root: NodeId,
    pub focus: NodeId,
    /// Changed or new nodes, sent whole.
    pub nodes: Vec<Node>,
    pub removed: Vec<NodeId>,
    pub notifications: Vec<Notification>,
}

/// A request to act on a node: from an OS adapter, a remote client or a test. They all take this path.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionRequest {
    /// Caller's id, echoed in the [`ActionResult`] event (0 = nobody waits).
    pub req: u64,
    pub node: NodeId,
    pub action: Actions,
    pub arg: ActionArg,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ActionArg {
    #[default]
    None,
    Number(f64),
    Text(String),
    /// UTF-8 byte range.
    Range(u32, u32),
    /// Replace a byte range with text.
    Replace(u32, u32, String),
    /// Scroll by (dx, dy) layout points.
    Scroll(f32, f32),
    /// A custom action's name.
    Custom(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ActionResult {
    Handled = 0,
    /// No widget handled it; a generic default ran (focus, scroll into view, tap at the centre).
    Fallback = 1,
    NoSuchNode = 2,
    Disabled = 3,
    Unsupported = 4,
    /// Internal widget dispatch only; resolved before sending a completion on the wire.
    Unhandled = 5,
}

impl ActionResult {
    pub fn from_u8(v: u8) -> ActionResult {
        match v {
            0 => ActionResult::Handled,
            1 => ActionResult::Fallback,
            2 => ActionResult::NoSuchNode,
            3 => ActionResult::Disabled,
            _ => ActionResult::Unsupported,
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            ActionResult::Handled => "handled",
            ActionResult::Fallback => "fallback",
            ActionResult::NoSuchNode => "no such node",
            ActionResult::Disabled => "disabled",
            ActionResult::Unsupported => "unsupported",
            ActionResult::Unhandled => "unhandled",
        }
    }
}

/// Which consumers want the tree (the builder runs while any bit is set).
pub mod consumer {
    pub const OS: u32 = 1;
    pub const REMOTE: u32 = 2;
    pub const TEST: u32 = 4;
    pub const AUTOMATION: u32 = 8;
    pub const HOST: u32 = 16;
}
