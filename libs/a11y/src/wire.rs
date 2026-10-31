//! The binary form of [`TreeUpdate`] and [`ActionRequest`]: the same bytes travel through the HAL
//! command arena, the studio socket, the remote socket and Desk Link.
//!
//! Little-endian. Strings are `u32 len | UTF-8`. A node is `u64 present-mask | fields...`: fields equal
//! to their default are left out, so a bounds-only change is ~30 bytes. The node layout is generated
//! from the [`node_fields!`] table; the JS decoder of the web mirror is generated from [`NODE_FIELDS`].

use crate::*;

pub const MAGIC: u32 = 0x5931_3141; // "A11Y" little-endian
pub const VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a11y wire: {}", self.0)
    }
}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v)
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes())
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes())
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes())
    }
    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes())
    }
    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes())
    }
    pub fn str(&mut self, v: &str) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v.as_bytes())
    }
}

pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.pos + n > self.buf.len() {
            return Err(WireError(format!("truncated at {} (+{n}) of {}", self.pos, self.buf.len())));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, WireError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, WireError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn str(&mut self) -> Result<String, WireError> {
        let n = self.u32()? as usize;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| WireError("string not UTF-8".into()))
    }
    /// A count that is bounded by the remaining bytes (each element takes at least `min` bytes), so a
    /// corrupt count cannot ask for a huge allocation.
    pub fn count(&mut self, min: usize) -> Result<usize, WireError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min.max(1)) > self.buf.len() - self.pos {
            return Err(WireError(format!("count {n} exceeds the remaining {} bytes", self.buf.len() - self.pos)));
        }
        Ok(n)
    }
    pub fn at_end(&self) -> bool {
        self.pos == self.buf.len()
    }
}

/// A value with a wire form and a default that is left out.
pub trait Field: Sized {
    /// The kind name, for the generated JS decoder.
    const KIND: &'static str;
    fn is_default(&self) -> bool;
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader) -> Result<Self, WireError>;
}

impl Field for String {
    const KIND: &'static str = "str";
    fn is_default(&self) -> bool {
        self.is_empty()
    }
    fn put(&self, w: &mut Writer) {
        w.str(self)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        r.str()
    }
}

impl Field for u32 {
    const KIND: &'static str = "u32";
    fn is_default(&self) -> bool {
        *self == 0
    }
    fn put(&self, w: &mut Writer) {
        w.u32(*self)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        r.u32()
    }
}

impl Field for Role {
    const KIND: &'static str = "u16";
    fn is_default(&self) -> bool {
        *self == Role::Generic
    }
    fn put(&self, w: &mut Writer) {
        w.u16(*self as u16)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        let t = r.u16()?;
        Ok(Role::from_u16(t).unwrap_or(Role::Generic))
    }
}

impl Field for States {
    const KIND: &'static str = "u64";
    fn is_default(&self) -> bool {
        self.0 == 0
    }
    fn put(&self, w: &mut Writer) {
        w.u64(self.0)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(States(r.u64()?))
    }
}

impl Field for Actions {
    const KIND: &'static str = "u32";
    fn is_default(&self) -> bool {
        self.0 == 0
    }
    fn put(&self, w: &mut Writer) {
        w.u32(self.0)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Actions(r.u32()?))
    }
}

impl Field for Rect {
    const KIND: &'static str = "rect";
    fn is_default(&self) -> bool {
        *self == Rect::default()
    }
    fn put(&self, w: &mut Writer) {
        w.f32(self.x);
        w.f32(self.y);
        w.f32(self.w);
        w.f32(self.h);
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Rect { x: r.f32()?, y: r.f32()?, w: r.f32()?, h: r.f32()? })
    }
}

impl Field for Toggle {
    const KIND: &'static str = "u8";
    fn is_default(&self) -> bool {
        *self == Toggle::None
    }
    fn put(&self, w: &mut Writer) {
        w.u8(*self as u8)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Toggle::from_u8(r.u8()?))
    }
}

impl Field for Live {
    const KIND: &'static str = "u8";
    fn is_default(&self) -> bool {
        *self == Live::Off
    }
    fn put(&self, w: &mut Writer) {
        w.u8(*self as u8)
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Live::from_u8(r.u8()?))
    }
}

impl Field for Option<RangeValue> {
    const KIND: &'static str = "range";
    fn is_default(&self) -> bool {
        self.is_none()
    }
    fn put(&self, w: &mut Writer) {
        let v = self.unwrap_or_default();
        w.f64(v.min);
        w.f64(v.max);
        w.f64(v.now);
        w.f64(v.step);
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Some(RangeValue { min: r.f64()?, max: r.f64()?, now: r.f64()?, step: r.f64()? }))
    }
}

impl Field for Vec<NodeId> {
    const KIND: &'static str = "ids";
    fn is_default(&self) -> bool {
        self.is_empty()
    }
    fn put(&self, w: &mut Writer) {
        w.u32(self.len() as u32);
        for id in self {
            w.u64(id.0)
        }
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        let n = r.count(8)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(NodeId(r.u64()?))
        }
        Ok(v)
    }
}

impl Field for Vec<String> {
    const KIND: &'static str = "strs";
    fn is_default(&self) -> bool {
        self.is_empty()
    }
    fn put(&self, w: &mut Writer) {
        w.u32(self.len() as u32);
        for s in self {
            w.str(s)
        }
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        let n = r.count(4)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(r.str()?)
        }
        Ok(v)
    }
}

impl Field for Relations {
    const KIND: &'static str = "relations";
    fn is_default(&self) -> bool {
        self.is_empty()
    }
    fn put(&self, w: &mut Writer) {
        let present: Vec<(u8, &Vec<NodeId>)> =
            Relations::NAMES.iter().filter_map(|(t, _)| self.get(*t).filter(|v| !v.is_empty()).map(|v| (*t, v))).collect();
        w.u8(present.len() as u8);
        for (t, v) in present {
            w.u8(t);
            v.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        let mut rel = Relations::default();
        let n = r.u8()?;
        for _ in 0..n {
            let t = r.u8()?;
            let v = Vec::<NodeId>::get(r)?;
            if let Some(slot) = rel.get_mut(t) {
                *slot = v;
            }
        }
        Ok(rel)
    }
}

impl Field for Option<TextInfo> {
    const KIND: &'static str = "text";
    fn is_default(&self) -> bool {
        self.is_none()
    }
    fn put(&self, w: &mut Writer) {
        let t = self.as_ref().unwrap();
        w.str(&t.content);
        w.u32(t.selection.0);
        w.u32(t.selection.1);
        w.u32(t.lines.len() as u32);
        for l in &t.lines {
            w.u32(l.start);
            l.rect.put(w);
            w.f32(l.baseline);
        }
        w.u32(t.glyph_x.len() as u32);
        for x in &t.glyph_x {
            w.f32(*x)
        }
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        let content = r.str()?;
        let selection = (r.u32()?, r.u32()?);
        let n = r.count(24)?;
        let mut lines = Vec::with_capacity(n);
        for _ in 0..n {
            lines.push(LineBox { start: r.u32()?, rect: Rect::get(r)?, baseline: r.f32()? });
        }
        let n = r.count(4)?;
        let mut glyph_x = Vec::with_capacity(n);
        for _ in 0..n {
            glyph_x.push(r.f32()?)
        }
        Ok(Some(TextInfo { content, selection, lines, glyph_x }))
    }
}

impl Field for Option<TableInfo> {
    const KIND: &'static str = "table";
    fn is_default(&self) -> bool {
        self.is_none()
    }
    fn put(&self, w: &mut Writer) {
        let t = self.unwrap();
        for v in [t.rows, t.cols, t.row, t.col, t.row_span, t.col_span] {
            w.u32(v)
        }
    }
    fn get(r: &mut Reader) -> Result<Self, WireError> {
        Ok(Some(TableInfo { rows: r.u32()?, cols: r.u32()?, row: r.u32()?, col: r.u32()?, row_span: r.u32()?, col_span: r.u32()? }))
    }
}

macro_rules! node_fields {
    ($( $bit:literal $field:ident : $ty:ty; )*) => {
        /// (presence bit, field name, wire kind) per optional node field, in wire order.
        pub const NODE_FIELDS: &[(u32, &str, &str)] = &[$( ($bit, stringify!($field), <$ty as Field>::KIND), )*];

        pub fn put_node(w: &mut Writer, n: &Node) {
            w.u64(n.id.0);
            let mut mask = 0u64;
            $( if !n.$field.is_default() { mask |= 1 << $bit; } )*
            w.u64(mask);
            $( if mask & (1 << $bit) != 0 { n.$field.put(w); } )*
        }

        pub fn get_node(r: &mut Reader) -> Result<Node, WireError> {
            let mut n = Node::default();
            n.id = NodeId(r.u64()?);
            let mask = r.u64()?;
            $( if mask & (1 << $bit) != 0 { n.$field = <$ty as Field>::get(r)?; } )*
            Ok(n)
        }
    };
}

node_fields! {
    0 role: Role;
    1 states: States;
    2 actions: Actions;
    3 bounds: Rect;
    4 name: String;
    5 description: String;
    6 value: String;
    7 placeholder: String;
    8 key_shortcut: String;
    9 role_description: String;
    10 class: String;
    11 path: String;
    12 range: Option<RangeValue>;
    13 toggle: Toggle;
    14 level: u32;
    15 pos_in_set: u32;
    16 set_size: u32;
    17 live: Live;
    18 relations: Relations;
    19 custom_actions: Vec<String>;
    20 children: Vec<NodeId>;
    21 text: Option<TextInfo>;
    22 table: Option<TableInfo>;
}

fn put_notification(w: &mut Writer, n: &Notification) {
    match n {
        Notification::FocusChanged(id) => {
            w.u8(0);
            w.u64(id.0)
        }
        Notification::NameChanged(id) => {
            w.u8(1);
            w.u64(id.0)
        }
        Notification::ValueChanged(id) => {
            w.u8(2);
            w.u64(id.0)
        }
        Notification::StatesChanged { node, old, new } => {
            w.u8(3);
            w.u64(node.0);
            w.u64(old.0);
            w.u64(new.0)
        }
        Notification::ChildrenChanged(id) => {
            w.u8(4);
            w.u64(id.0)
        }
        Notification::TextChanged { node, start, removed, inserted } => {
            w.u8(5);
            w.u64(node.0);
            w.u32(*start);
            w.u32(*removed);
            w.u32(*inserted)
        }
        Notification::CaretMoved(id) => {
            w.u8(6);
            w.u64(id.0)
        }
        Notification::LiveRegionChanged(id) => {
            w.u8(7);
            w.u64(id.0)
        }
        Notification::Announce { text, live } => {
            w.u8(8);
            w.str(text);
            w.u8(*live as u8)
        }
    }
}

fn get_notification(r: &mut Reader) -> Result<Notification, WireError> {
    Ok(match r.u8()? {
        0 => Notification::FocusChanged(NodeId(r.u64()?)),
        1 => Notification::NameChanged(NodeId(r.u64()?)),
        2 => Notification::ValueChanged(NodeId(r.u64()?)),
        3 => Notification::StatesChanged { node: NodeId(r.u64()?), old: States(r.u64()?), new: States(r.u64()?) },
        4 => Notification::ChildrenChanged(NodeId(r.u64()?)),
        5 => Notification::TextChanged { node: NodeId(r.u64()?), start: r.u32()?, removed: r.u32()?, inserted: r.u32()? },
        6 => Notification::CaretMoved(NodeId(r.u64()?)),
        7 => Notification::LiveRegionChanged(NodeId(r.u64()?)),
        8 => Notification::Announce { text: r.str()?, live: Live::from_u8(r.u8()?) },
        t => return Err(WireError(format!("unknown notification tag {t}"))),
    })
}

pub fn encode_update(u: &TreeUpdate, out: &mut Vec<u8>) {
    let mut w = Writer { buf: std::mem::take(out) };
    w.buf.clear();
    w.u32(MAGIC);
    w.u16(VERSION);
    w.u8(u.full as u8);
    w.u64(u.root.0);
    w.u64(u.focus.0);
    w.u32(u.nodes.len() as u32);
    for n in &u.nodes {
        put_node(&mut w, n)
    }
    u.removed.put(&mut w);
    w.u32(u.notifications.len() as u32);
    for n in &u.notifications {
        put_notification(&mut w, n)
    }
    *out = w.buf;
}

pub fn decode_update(bytes: &[u8]) -> Result<TreeUpdate, WireError> {
    let mut r = Reader::new(bytes);
    if r.u32()? != MAGIC {
        return Err(WireError("bad magic".into()));
    }
    let v = r.u16()?;
    if v != VERSION {
        return Err(WireError(format!("version {v}, expected {VERSION}")));
    }
    let full = r.u8()? != 0;
    let root = NodeId(r.u64()?);
    let focus = NodeId(r.u64()?);
    let n = r.count(16)?;
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        nodes.push(get_node(&mut r)?)
    }
    let removed = Vec::<NodeId>::get(&mut r)?;
    let n = r.count(1)?;
    let mut notifications = Vec::with_capacity(n);
    for _ in 0..n {
        notifications.push(get_notification(&mut r)?)
    }
    if !r.at_end() {
        return Err(WireError(format!("{} trailing bytes", r.buf.len() - r.pos)));
    }
    Ok(TreeUpdate { full, root, focus, nodes, removed, notifications })
}

pub fn encode_action(a: &ActionRequest, out: &mut Vec<u8>) {
    let mut w = Writer { buf: std::mem::take(out) };
    w.buf.clear();
    w.u64(a.req);
    w.u64(a.node.0);
    w.u32(a.action.0);
    match &a.arg {
        ActionArg::None => w.u8(0),
        ActionArg::Number(v) => {
            w.u8(1);
            w.f64(*v)
        }
        ActionArg::Text(s) => {
            w.u8(2);
            w.str(s)
        }
        ActionArg::Range(a, b) => {
            w.u8(3);
            w.u32(*a);
            w.u32(*b)
        }
        ActionArg::Replace(a, b, s) => {
            w.u8(4);
            w.u32(*a);
            w.u32(*b);
            w.str(s)
        }
        ActionArg::Scroll(x, y) => {
            w.u8(5);
            w.f32(*x);
            w.f32(*y)
        }
        ActionArg::Custom(s) => {
            w.u8(6);
            w.str(s)
        }
    }
    *out = w.buf;
}

pub fn decode_action(bytes: &[u8]) -> Result<ActionRequest, WireError> {
    let mut r = Reader::new(bytes);
    let req = r.u64()?;
    let node = NodeId(r.u64()?);
    let action = Actions(r.u32()?);
    let arg = match r.u8()? {
        0 => ActionArg::None,
        1 => ActionArg::Number(r.f64()?),
        2 => ActionArg::Text(r.str()?),
        3 => ActionArg::Range(r.u32()?, r.u32()?),
        4 => ActionArg::Replace(r.u32()?, r.u32()?, r.str()?),
        5 => ActionArg::Scroll(r.f32()?, r.f32()?),
        6 => ActionArg::Custom(r.str()?),
        t => return Err(WireError(format!("unknown action arg tag {t}"))),
    };
    Ok(ActionRequest { req, node, action, arg })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> TreeUpdate {
        let mut btn = Node { id: NodeId::widget(0x1a2), role: Role::Button, name: "Save".into(), ..Default::default() };
        btn.states = States::FOCUSABLE | States::DEFAULT;
        btn.actions = Actions::PRESS | Actions::FOCUS;
        btn.bounds = Rect { x: 10.0, y: 20.0, w: 80.0, h: 24.0 };
        let edit = Node {
            id: NodeId::part(0x1a3, 2),
            role: Role::TextField,
            value: "héllo".into(),
            text: Some(TextInfo {
                content: "héllo".into(),
                selection: (1, 3),
                lines: vec![LineBox { start: 0, rect: Rect { x: 0.0, y: 0.0, w: 50.0, h: 12.0 }, baseline: 10.0 }],
                glyph_x: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
            }),
            range: Some(RangeValue { min: 0.0, max: 1.0, now: 0.5, step: 0.1 }),
            relations: Relations { labelled_by: vec![NodeId(7)], ..Default::default() },
            custom_actions: vec!["archive".into()],
            table: Some(TableInfo { rows: 3, cols: 2, row: 1, col: 0, row_span: 1, col_span: 1 }),
            ..Default::default()
        };
        let root = Node { id: NodeId(1), role: Role::Window, name: "App".into(), children: vec![btn.id, edit.id], ..Default::default() };
        TreeUpdate {
            full: true,
            root: root.id,
            focus: btn.id,
            nodes: vec![root, btn, edit],
            removed: vec![NodeId(99)],
            notifications: vec![
                Notification::FocusChanged(NodeId(5)),
                Notification::StatesChanged { node: NodeId(5), old: States::FOCUSABLE, new: States::FOCUSED },
                Notification::TextChanged { node: NodeId(6), start: 1, removed: 2, inserted: 3 },
                Notification::Announce { text: "Saved".into(), live: Live::Polite },
            ],
        }
    }

    #[test]
    fn update_round_trip() {
        let u = sample();
        let mut bytes = Vec::new();
        encode_update(&u, &mut bytes);
        assert_eq!(decode_update(&bytes).unwrap(), u);
        // truncations fail cleanly, never panic
        for cut in 0..bytes.len() {
            assert!(decode_update(&bytes[..cut]).is_err());
        }
    }

    #[test]
    fn action_round_trip() {
        for arg in [
            ActionArg::None,
            ActionArg::Number(2.5),
            ActionArg::Text("x".into()),
            ActionArg::Range(1, 2),
            ActionArg::Replace(1, 2, "y".into()),
            ActionArg::Scroll(1.0, -2.0),
            ActionArg::Custom("archive".into()),
        ] {
            let a = ActionRequest { req: 9, node: NodeId(3), action: Actions::SET_VALUE, arg };
            let mut bytes = Vec::new();
            encode_action(&a, &mut bytes);
            assert_eq!(decode_action(&bytes).unwrap(), a);
        }
    }

    #[test]
    fn default_fields_are_left_out() {
        let n = Node { id: NodeId(1), bounds: Rect { x: 1.0, y: 2.0, w: 3.0, h: 4.0 }, ..Default::default() };
        let mut w = Writer::default();
        put_node(&mut w, &n);
        assert_eq!(w.buf.len(), 8 + 8 + 16);
    }
}
