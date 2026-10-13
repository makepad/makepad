//! ICCCM selections and XDND. Reads are correlated by their own requestor window, so a paste,
//! drag and API clipboard read may overlap. INCR transfers finish only on the zero chunk.
use super::super::input;
use super::{send_message, sys as x, Key, Shared, PROPERTY_CHANGE};
use crate::clipboard::ClipboardData;
use crate::types::input as raw;
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_int, c_long, c_ulong};
use std::ptr;
use std::sync::Arc;

pub(super) struct Property {
    pub ty: x::Atom,
    pub format: i32,
    pub bytes: Vec<u8>,
    pub words: Vec<c_ulong>,
}
pub(super) fn property(
    display: *mut x::Display,
    window: x::Window,
    atom: x::Atom,
    delete: bool,
) -> Option<Property> {
    unsafe {
        let mut out = Property {
            ty: 0,
            format: 0,
            bytes: Vec::new(),
            words: Vec::new(),
        };
        let mut offset = 0;
        loop {
            let mut ty = 0;
            let mut format = 0;
            let mut count = 0;
            let mut remaining = 0;
            let mut data = ptr::null_mut();
            let chunk = x::XMaxRequestSize(display).max(1);
            if x::XGetWindowProperty(
                display,
                window,
                atom,
                offset,
                chunk,
                0,
                0,
                &mut ty,
                &mut format,
                &mut count,
                &mut remaining,
                &mut data,
            ) != 0
            {
                return None;
            }
            if ty == 0 {
                if !data.is_null() {
                    x::XFree(data.cast());
                }
                return None;
            }
            out.ty = ty;
            out.format = format;
            if !data.is_null() {
                if format == 32 {
                    out.words.extend_from_slice(std::slice::from_raw_parts(
                        data.cast::<c_ulong>(),
                        count as usize,
                    ));
                } else if matches!(format, 8 | 16) {
                    out.bytes.extend_from_slice(std::slice::from_raw_parts(
                        data,
                        count as usize * format as usize / 8,
                    ));
                }
                x::XFree(data.cast());
            }
            if remaining == 0 {
                break;
            }
            let consumed = (count * format as c_ulong + 31) / 32;
            if consumed == 0 {
                return None;
            }
            offset += consumed as c_long;
        }
        if delete {
            x::XDeleteProperty(display, window, atom);
        }
        Some(out)
    }
}

pub(super) enum Purpose {
    Reply(u32, u64),
    Paste(Key),
    Preview(Key, x::Window, u64),
}
struct Read {
    purpose: Purpose,
    selection: x::Atom,
    current: x::Atom,
    remaining: VecDeque<(x::Atom, String)>,
    mime: String,
    incremental: bool,
    targets: Vec<x::Atom>,
    bytes: Vec<u8>,
    content: ClipboardData,
    items: Vec<(u32, String)>,
}
struct Write {
    target: x::Atom,
    data: Arc<Vec<u8>>,
    offset: usize,
}
struct DragIn {
    transfer: u64,
    source: x::Window,
    key: Key,
    pos: [f64; 2],
    items: Vec<(u32, String)>,
    ready: bool,
    dropped: bool,
    answer: u32,
    replies: usize,
}
struct DragOut {
    icon: x::Window,
    display: *mut x::Display,
    source: x::Window,
    key: Key,
    target: x::Window,
    accepted: bool,
    released: bool,
    position: Option<[i32; 2]>,
    items: Vec<(u32, String)>,
}
pub(super) struct Selections {
    display: *mut x::Display,
    root: x::Window,
    owner: x::Window,
    atoms: HashMap<String, x::Atom>,
    clipboard: Arc<Vec<u8>>,
    primary: Arc<Vec<u8>>,
    reads: HashMap<x::Window, Read>,
    writes: HashMap<(x::Window, x::Atom), Write>,
    incoming: HashMap<x::Window, DragIn>,
    outgoing: Option<DragOut>,
    next_drag: u64,
}
impl Selections {
    pub fn new(display: *mut x::Display, root: x::Window) -> Self {
        let owner = unsafe { x::XCreateSimpleWindow(display, root, 0, 0, 1, 1, 0, 0, 0) };
        Self {
            display,
            root,
            owner,
            atoms: HashMap::new(),
            clipboard: Arc::new(Vec::new()),
            primary: Arc::new(Vec::new()),
            reads: HashMap::new(),
            writes: HashMap::new(),
            incoming: HashMap::new(),
            outgoing: None,
            next_drag: 0,
        }
    }
    fn atom(&mut self, name: &str) -> x::Atom {
        *self
            .atoms
            .entry(name.to_string())
            .or_insert_with(|| unsafe {
                x::XInternAtom(self.display, input::cstring(name).as_ptr(), 0)
            })
    }
    pub fn set(&mut self, text: String, primary: bool) {
        let atom = self.atom(if primary { "PRIMARY" } else { "CLIPBOARD" });
        if primary {
            self.primary = Arc::new(text.into_bytes());
        } else {
            self.clipboard = Arc::new(text.into_bytes());
        }
        unsafe {
            x::XSetSelectionOwner(self.display, atom, self.owner, 0);
        }
    }
    pub fn read(&mut self, purpose: Purpose, primary: bool) {
        let selection = self.atom(if primary { "PRIMARY" } else { "CLIPBOARD" });
        self.begin(purpose, selection, None);
    }
    fn begin(&mut self, purpose: Purpose, selection: x::Atom, types: Option<Vec<x::Atom>>) {
        let native =
            unsafe { x::XCreateSimpleWindow(self.display, self.root, 0, 0, 1, 1, 0, 0, 0) };
        unsafe {
            x::XSelectInput(self.display, native, PROPERTY_CHANGE);
        }
        let targets = self.atom("TARGETS");
        let remaining = types.map(|t| self.formats(&t, matches!(purpose, Purpose::Reply(..))));
        let mut read = Read {
            purpose,
            selection,
            current: targets,
            remaining: remaining.clone().unwrap_or_default(),
            mime: String::new(),
            incremental: false,
            targets: Vec::new(),
            bytes: Vec::new(),
            content: ClipboardData::default(),
            items: Vec::new(),
        };
        if remaining.is_some() {
            if let Some((target, mime)) = read.remaining.pop_front() {
                read.current = target;
                read.mime = mime;
            }
        }
        let prop = self.atom("_MAKEPAD_HAL_SELECTION");
        unsafe {
            x::XConvertSelection(self.display, selection, read.current, prop, native, 0);
        }
        self.reads.insert(native, read);
    }
    fn formats(&mut self, offered: &[x::Atom], clipboard: bool) -> VecDeque<(x::Atom, String)> {
        let mut formats = VecDeque::new();
        if let Some((atom, name)) = input::TEXT_MIMES
            .iter()
            .map(|m| (self.atom(m), *m))
            .find(|(atom, _)| offered.contains(atom))
        {
            formats.push_back((atom, name.into()));
        }
        if clipboard {
            let png = self.atom(input::PNG_MIME);
            if offered.contains(&png) {
                formats.push_back((png, input::PNG_MIME.into()));
            }
        }
        let uri = self.atom(input::URI_MIME);
        if offered.contains(&uri) {
            formats.push_back((uri, input::URI_MIME.into()));
        }
        formats
    }
    fn complete(&mut self, window: x::Window, shared: &Shared) {
        if let Some(read) = self.reads.remove(&window) {
            unsafe {
                x::XDestroyWindow(self.display, window);
            }
            match read.purpose {
                Purpose::Reply(sid, req) => {
                    let content = read.content.pack();
                    shared.push((sid, crate::Id::NONE), |q, _| {
                        q.payload_size(content.len() as u32);
                        q.clipboard(req, true, &content);
                    });
                }
                Purpose::Paste(key) => {
                    // The paste chord inserts text; files remain available through ClipRead.
                    for (_, text) in read
                        .items
                        .iter()
                        .filter(|(kind, _)| *kind == raw::drag_item::STRING)
                    {
                        shared.push(key, |q, id| q.text(id, raw::text::PASTE, text.as_bytes()));
                    }
                }
                Purpose::Preview(key, target, transfer) => {
                    let payload = input::packed(&read.items);
                    if let Some(d) = self
                        .incoming
                        .get_mut(&target)
                        .filter(|d| d.key == key && d.transfer == transfer)
                    {
                        d.items = read.items;
                        d.ready = true;
                        d.replies += 1;
                        shared.push(d.key, |q, id| {
                            q.drag_over(id, d.pos[0], d.pos[1], 0, d.items.len() as u32, &payload)
                        });
                    }
                    self.finish_drop(target, shared);
                }
            }
        }
    }
    fn finish_drop(&mut self, target: x::Window, shared: &Shared) {
        if !self
            .incoming
            .get(&target)
            .is_some_and(|d| d.ready && d.dropped && d.replies == 0)
        {
            return;
        }
        let d = self.incoming.remove(&target).unwrap();
        let accepted = d.answer != 0;
        let bytes = input::packed(&d.items);
        shared.push(d.key, |q, id| {
            q.drag_drop(
                id,
                d.pos[0],
                d.pos[1],
                accepted,
                d.items.len() as u32,
                &bytes,
            )
        });
        let atom = self.atom("XdndFinished");
        let action = self.atom(match d.answer {
            raw::drag_response::MOVE => "XdndActionMove",
            raw::drag_response::LINK => "XdndActionLink",
            _ => "XdndActionCopy",
        });
        send_message(
            self.display,
            d.source,
            d.source,
            atom,
            0,
            [
                target as c_long,
                accepted as c_long,
                if accepted { action as c_long } else { 0 },
                0,
                0,
            ],
        );
    }
    fn next(&mut self, window: x::Window, shared: &Shared) {
        let prop = self.atom("_MAKEPAD_HAL_SELECTION");
        let Some(read) = self.reads.get_mut(&window) else {
            return;
        };
        if !read.mime.is_empty() && !read.bytes.is_empty() {
            if matches!(read.purpose, Purpose::Reply(..)) {
                input::decode_clipboard(
                    &mut read.content,
                    &read.mime,
                    std::mem::take(&mut read.bytes),
                );
            } else {
                read.items
                    .extend(input::decode_items(&read.mime, &read.bytes));
            }
        }
        read.bytes.clear();
        read.incremental = false;
        if let Some((target, mime)) = read.remaining.pop_front() {
            read.current = target;
            read.mime = mime;
            unsafe {
                x::XConvertSelection(self.display, read.selection, target, prop, window, 0);
            }
        } else {
            self.complete(window, shared);
        }
    }
    fn received(&mut self, window: x::Window, prop: x::Atom, shared: &Shared) {
        if prop == 0 {
            self.next(window, shared);
            return;
        }
        let Some(value) = property(self.display, window, prop, true) else {
            self.next(window, shared);
            return;
        };
        let targets = self.atom("TARGETS");
        let incr = self.atom("INCR");
        let is_targets = self
            .reads
            .get(&window)
            .is_some_and(|r| r.current == targets);
        if value.ty == incr {
            if let Some(r) = self.reads.get_mut(&window) {
                r.incremental = true;
            }
            return;
        }
        if is_targets {
            let Some(r) = self.reads.get_mut(&window) else {
                return;
            };
            let done = !r.incremental || value.words.is_empty();
            r.targets.extend(value.words);
            if !done {
                return;
            }
            let targets = std::mem::take(&mut r.targets);
            let clipboard = matches!(r.purpose, Purpose::Reply(..));
            let formats = self.formats(&targets, clipboard);
            if let Some(r) = self.reads.get_mut(&window) {
                r.remaining = formats;
            }
            self.next(window, shared);
        } else if let Some(r) = self.reads.get_mut(&window) {
            let done = !r.incremental || value.bytes.is_empty();
            r.bytes.extend(value.bytes);
            if done {
                self.next(window, shared);
            }
        }
    }
    fn request(&mut self, e: x::XSelectionRequestEvent) {
        let targets = self.atom("TARGETS");
        let utf8 = self.atom("UTF8_STRING");
        let uri = self.atom(input::URI_MIME);
        let primary = self.atom("PRIMARY");
        let xdnd = self.atom("XdndSelection");
        let prop = if e.property == 0 {
            e.target
        } else {
            e.property
        };
        let mut result = prop;
        if e.target == targets {
            let mut offered: Vec<_> = input::TEXT_MIMES.iter().map(|m| self.atom(m)).collect();
            offered.push(targets);
            if e.selection == xdnd
                && self
                    .outgoing
                    .as_ref()
                    .is_some_and(|d| d.items.iter().any(|(k, _)| *k == raw::drag_item::FILE))
            {
                offered.push(uri);
            }
            let atom = self.atom("ATOM");
            unsafe {
                x::XChangeProperty(
                    self.display,
                    e.requestor,
                    prop,
                    atom,
                    32,
                    0,
                    offered.as_ptr().cast(),
                    offered.len() as c_int,
                );
            }
        } else {
            let text = input::TEXT_MIMES.iter().any(|m| self.atom(m) == e.target);
            let bytes = if e.selection == xdnd {
                self.outgoing.as_ref().and_then(|d| {
                    if e.target == uri {
                        Some(Arc::new(input::encode_uris(&d.items)))
                    } else if text {
                        Some(Arc::new(
                            d.items
                                .iter()
                                .map(|(_, s)| s.as_str())
                                .collect::<Vec<_>>()
                                .join("\n")
                                .into_bytes(),
                        ))
                    } else {
                        None
                    }
                })
            } else if text {
                Some(if e.selection == primary {
                    self.primary.clone()
                } else {
                    self.clipboard.clone()
                })
            } else {
                None
            };
            if let Some(bytes) = bytes {
                let chunk = self.chunk();
                if bytes.len() > chunk {
                    let incr = self.atom("INCR");
                    let size = [bytes.len() as c_ulong];
                    unsafe {
                        x::XSelectInput(self.display, e.requestor, PROPERTY_CHANGE);
                        x::XChangeProperty(
                            self.display,
                            e.requestor,
                            prop,
                            incr,
                            32,
                            0,
                            size.as_ptr().cast(),
                            1,
                        );
                    }
                    self.writes.insert(
                        (e.requestor, prop),
                        Write {
                            target: if text { utf8 } else { uri },
                            data: bytes,
                            offset: 0,
                        },
                    );
                } else {
                    unsafe {
                        x::XChangeProperty(
                            self.display,
                            e.requestor,
                            prop,
                            e.target,
                            8,
                            0,
                            bytes.as_ptr(),
                            bytes.len() as c_int,
                        );
                    }
                }
            } else {
                result = 0;
            }
        }
        unsafe {
            let mut reply: x::XEvent = std::mem::zeroed();
            reply.xselection = x::XSelectionEvent {
                type_: x::SelectionNotify as c_int,
                serial: 0,
                send_event: 1,
                display: self.display,
                requestor: e.requestor,
                selection: e.selection,
                target: e.target,
                property: result,
                time: e.time,
            };
            x::XSendEvent(self.display, e.requestor, 0, 0, &mut reply);
        }
    }
    fn chunk(&self) -> usize {
        (unsafe { x::XMaxRequestSize(self.display) } as usize * 4)
            .saturating_sub(128)
            .max(1)
            .min(c_int::MAX as usize)
    }
    pub fn event(&mut self, ev: &mut x::XEvent, shared: &Shared) -> bool {
        unsafe {
            match ev.type_ as u32 {
                x::SelectionRequest => {
                    self.request(ev.xselectionrequest);
                    true
                }
                x::SelectionNotify if self.reads.contains_key(&ev.xselection.requestor) => {
                    self.received(ev.xselection.requestor, ev.xselection.property, shared);
                    true
                }
                28 => {
                    let e = ev.xproperty;
                    if self.reads.get(&e.window).is_some_and(|r| r.incremental) && e.state == 0 {
                        self.received(e.window, e.atom, shared);
                        return true;
                    }
                    let chunk = self.chunk();
                    if e.state == 1 {
                        if let Some(w) = self.writes.get_mut(&(e.window, e.atom)) {
                            let end = (w.offset + chunk).min(w.data.len());
                            let bytes = &w.data[w.offset..end];
                            x::XChangeProperty(
                                self.display,
                                e.window,
                                e.atom,
                                w.target,
                                8,
                                0,
                                bytes.as_ptr(),
                                bytes.len() as c_int,
                            );
                            if bytes.is_empty() {
                                self.writes.remove(&(e.window, e.atom));
                            } else {
                                w.offset = end;
                            }
                            return true;
                        }
                    }
                    false
                }
                x::ClientMessage => {
                    let e = ev.xclient;
                    if e.message_type == self.atom("XdndStatus") {
                        if let Some(d) = self.outgoing.as_mut().filter(|d| {
                            d.target == e.data.l[0] as x::Window && d.source == e.window
                        }) {
                            d.accepted = e.data.l[1] & 1 != 0;
                        }
                        return true;
                    }
                    if e.message_type == self.atom("XdndFinished") {
                        if !self.outgoing.as_ref().is_some_and(|d| {
                            d.target == e.data.l[0] as x::Window
                                && d.source == e.window
                                && d.released
                        }) {
                            return true;
                        }
                        if let Some(d) = self.outgoing.take() {
                            let bytes = input::packed(&d.items);
                            shared.push(d.key, |q, id| {
                                q.drag_drop(
                                    id,
                                    0.0,
                                    0.0,
                                    e.data.l[1] & 1 != 0,
                                    d.items.len() as u32,
                                    &bytes,
                                )
                            });
                        }
                        return true;
                    }
                    false
                }
                x::DestroyNotify => {
                    self.writes
                        .retain(|(window, _), _| *window != ev.xdestroywindow.window);
                    false
                }
                _ => false,
            }
        }
    }
    pub fn drag_message(&mut self, e: x::XClientMessageEvent, key: Key, dpi: f32, shared: &Shared) {
        unsafe {
            if e.message_type == self.atom("XdndEnter") {
                let source = e.data.l[0] as x::Window;
                let types = if e.data.l[1] & 1 != 0 {
                    let atom = self.atom("XdndTypeList");
                    property(self.display, source, atom, false).map_or(Vec::new(), |p| p.words)
                } else {
                    e.data.l[2..5]
                        .iter()
                        .map(|v| *v as x::Atom)
                        .filter(|v| *v != 0)
                        .collect()
                };
                self.next_drag = self.next_drag.wrapping_add(1);
                let transfer = self.next_drag;
                self.incoming.insert(
                    e.window,
                    DragIn {
                        transfer,
                        source,
                        key,
                        pos: [0.0; 2],
                        items: Vec::new(),
                        ready: false,
                        dropped: false,
                        answer: 0,
                        replies: 0,
                    },
                );
                let selection = self.atom("XdndSelection");
                self.begin(
                    Purpose::Preview(key, e.window, transfer),
                    selection,
                    Some(types),
                );
            } else if e.message_type == self.atom("XdndPosition") {
                let mut px = 0;
                let mut py = 0;
                let mut child = 0;
                let point = e.data.l[2] as u32;
                x::XTranslateCoordinates(
                    self.display,
                    self.root,
                    e.window,
                    (point >> 16) as i16 as c_int,
                    point as i16 as c_int,
                    &mut px,
                    &mut py,
                    &mut child,
                );
                if let Some(d) = self.incoming.get_mut(&e.window) {
                    d.pos = [px as f64 / dpi as f64, py as f64 / dpi as f64];
                    d.replies += 1;
                    shared.push(d.key, |q, id| {
                        q.drag_over(
                            id,
                            d.pos[0],
                            d.pos[1],
                            1,
                            d.items.len() as u32,
                            &input::packed(&d.items),
                        )
                    });
                }
            } else if e.message_type == self.atom("XdndDrop") {
                if let Some(d) = self.incoming.get_mut(&e.window) {
                    d.dropped = true;
                }
                self.finish_drop(e.window, shared);
            } else if e.message_type == self.atom("XdndLeave") {
                if self.incoming.get(&e.window).is_some_and(|d| d.dropped) {
                    return;
                }
                if let Some(d) = self.incoming.remove(&e.window) {
                    shared.push(d.key, |q, id| {
                        q.drag_drop(id, d.pos[0], d.pos[1], false, 0, &[])
                    });
                }
            }
        }
    }
    pub fn drag_reply(&mut self, native: x::Window, answer: u32, shared: &Shared) {
        let Some(d) = self.incoming.get_mut(&native) else {
            return;
        };
        d.answer = answer;
        d.replies = d.replies.saturating_sub(1);
        let source = d.source;
        let atom = self.atom("XdndStatus");
        let action = self.atom(match answer {
            raw::drag_response::MOVE => "XdndActionMove",
            raw::drag_response::LINK => "XdndActionLink",
            _ => "XdndActionCopy",
        });
        send_message(
            self.display,
            source,
            source,
            atom,
            0,
            [
                native as c_long,
                2 | ((answer != 0) as c_long),
                0,
                0,
                if answer != 0 { action as c_long } else { 0 },
            ],
        );
        self.finish_drop(native, shared);
    }
    pub fn start_drag(
        &mut self,
        source: x::Window,
        key: Key,
        items: Vec<(u32, String)>,
        shared: &Shared,
    ) {
        if let Some(old) = self.outgoing.take() {
            shared.push(old.key, |q, id| q.drag_drop(id, 0.0, 0.0, false, 0, &[]));
        }
        let atom = self.atom("XdndSelection");
        unsafe {
            x::XSetSelectionOwner(self.display, atom, source, 0);
        }
        let icon = self.drag_chip(&items);
        self.outgoing = Some(DragOut {
            icon,
            display: self.display,
            source,
            key,
            target: 0,
            accepted: false,
            released: false,
            position: None,
            items,
        });
    }
    fn drag_chip(&self, items: &[(u32, String)]) -> x::Window {
        unsafe {
            let label = items
                .first()
                .map(|(_, s)| {
                    std::path::Path::new(s)
                        .file_name()
                        .map_or_else(|| s.clone(), |s| s.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "Drag".into());
            let label = input::cstring(&label);
            let mut attributes: x::XSetWindowAttributes = std::mem::zeroed();
            attributes.override_redirect = 1;
            attributes.background_pixel = 0xe5eaf0;
            let mut screen: x::XWindowAttributes = std::mem::zeroed();
            x::XGetWindowAttributes(self.display, self.root, &mut screen);
            let width = (label.as_bytes().len().saturating_mul(8).saturating_add(24))
                .min(screen.width.max(1) as usize) as u32;
            let icon = x::XCreateWindow(
                self.display,
                self.root,
                0,
                0,
                width,
                30,
                1,
                0,
                x::InputOutput,
                std::ptr::null_mut(),
                (x::CWOverrideRedirect | 2) as c_ulong,
                &mut attributes,
            );
            if icon != 0 {
                let gc = XCreateGC(self.display, icon, 0, std::ptr::null_mut());
                x::XMapRaised(self.display, icon);
                if !gc.is_null() {
                    XSetForeground(self.display, gc, 0x26354a);
                    XDrawString(
                        self.display,
                        icon,
                        gc,
                        10,
                        20,
                        label.as_ptr(),
                        label.as_bytes().len().min(c_int::MAX as usize) as c_int,
                    );
                    XFreeGC(self.display, gc);
                }
            }
            icon
        }
    }
    pub fn tick(&mut self, shared: &Shared) {
        let Some(mut d) = self.outgoing.take() else {
            return;
        };
        if d.released {
            self.outgoing = Some(d);
            return;
        }
        unsafe {
            let mut root = 0;
            let mut child = 0;
            let mut rx = 0;
            let mut ry = 0;
            let mut px = 0;
            let mut py = 0;
            let mut mask = 0;
            x::XQueryPointer(
                self.display,
                self.root,
                &mut root,
                &mut child,
                &mut rx,
                &mut ry,
                &mut px,
                &mut py,
                &mut mask,
            );
            let moved = d.position != Some([rx, ry]);
            d.position = Some([rx, ry]);
            if d.icon != 0 && moved {
                x::XMoveWindow(self.display, d.icon, rx + 12, ry + 16);
            }
            let aware = self.atom("XdndAware");
            let mut candidate = child;
            // Follow the pointer's child chain to the application window under a WM frame.
            let mut target = 0;
            while candidate != 0 {
                if property(self.display, candidate, aware, false).is_some() {
                    target = candidate;
                }
                let parent = candidate;
                x::XQueryPointer(
                    self.display,
                    parent,
                    &mut root,
                    &mut candidate,
                    &mut rx,
                    &mut ry,
                    &mut px,
                    &mut py,
                    &mut mask,
                );
                if candidate == parent {
                    break;
                }
            }
            let changed_target = target != d.target;
            if changed_target {
                if d.target != 0 {
                    let atom = self.atom("XdndLeave");
                    send_message(
                        self.display,
                        d.target,
                        d.target,
                        atom,
                        0,
                        [d.source as c_long, 0, 0, 0, 0],
                    );
                }
                d.target = target;
                d.accepted = false;
                if target != 0 {
                    let atom = self.atom("XdndEnter");
                    let text = self.atom("UTF8_STRING");
                    let uri = if d.items.iter().any(|(k, _)| *k == raw::drag_item::FILE) {
                        self.atom(input::URI_MIME)
                    } else {
                        0
                    };
                    send_message(
                        self.display,
                        target,
                        target,
                        atom,
                        0,
                        [
                            d.source as c_long,
                            5 << 24,
                            text as c_long,
                            uri as c_long,
                            0,
                        ],
                    );
                }
            }
            if mask & 256 != 0 {
                if target != 0 && (moved || changed_target) {
                    let atom = self.atom("XdndPosition");
                    let action = self.atom("XdndActionCopy");
                    send_message(
                        self.display,
                        target,
                        target,
                        atom,
                        0,
                        [
                            d.source as c_long,
                            0,
                            (((rx as u32 & 0xffff) << 16) | (ry as u32 & 0xffff)) as c_long,
                            0,
                            action as c_long,
                        ],
                    );
                }
                self.outgoing = Some(d);
            } else if target != 0 && d.accepted {
                let atom = self.atom("XdndDrop");
                send_message(
                    self.display,
                    target,
                    target,
                    atom,
                    0,
                    [d.source as c_long, 0, 0, 0, 0],
                );
                d.released = true;
                self.outgoing = Some(d);
            } else {
                shared.push(d.key, |q, id| q.drag_drop(id, 0.0, 0.0, false, 0, &[]));
            }
        }
    }
    pub fn close_session(&mut self, sid: u32) {
        let keys: Vec<_> = self
            .reads
            .iter()
            .filter(|(_, r)| match r.purpose {
                Purpose::Reply(s, _) => s == sid,
                Purpose::Paste(k) | Purpose::Preview(k, ..) => k.0 == sid,
            })
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            self.reads.remove(&key);
            unsafe {
                x::XDestroyWindow(self.display, key);
            }
        }
        self.incoming.retain(|_, d| d.key.0 != sid);
        if self.outgoing.as_ref().is_some_and(|d| d.key.0 == sid) {
            self.outgoing = None;
        }
    }
    pub fn destroy(&mut self) {
        self.outgoing = None;
        unsafe {
            for (window, _) in self.reads.drain() {
                x::XDestroyWindow(self.display, window);
            }
            x::XDestroyWindow(self.display, self.owner);
        }
    }
}

impl Drop for DragOut {
    fn drop(&mut self) {
        if self.icon != 0 {
            unsafe {
                x::XDestroyWindow(self.display, self.icon);
            }
        }
    }
}
#[link(name = "X11")]
extern "C" {
    fn XCreateGC(
        display: *mut x::Display,
        window: x::Window,
        mask: c_ulong,
        values: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn XSetForeground(display: *mut x::Display, gc: *mut std::ffi::c_void, color: c_ulong)
        -> c_int;
    fn XDrawString(
        display: *mut x::Display,
        window: x::Window,
        gc: *mut std::ffi::c_void,
        x: c_int,
        y: c_int,
        text: *const std::ffi::c_char,
        length: c_int,
    ) -> c_int;
    fn XFreeGC(display: *mut x::Display, gc: *mut std::ffi::c_void) -> c_int;
}
