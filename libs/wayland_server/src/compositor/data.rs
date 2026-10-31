//! Selections: wl_data_device (the clipboard) and zwp_primary_selection (middle-click paste).
//! A selection is a client's source or text the host set (`set_host_selection`). A client's
//! receiver gets its fd handed straight to the source. When a selection reader is installed
//! (`set_selection_reader`), the text of every client selection is also read once and given to
//! the host as `UiEvent::Selection`, so the host can paste it into its own (non-Wayland) apps.
//! When a client source goes away its text stays the selection, served by the compositor.
//! (Drag and drop is not done: a drag's source is cancelled at once.)
use super::*;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};

/// Which selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Board { Clipboard, Primary }

/// The text types the compositor offers for text it holds, and reads from a client, best first.
pub const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];
/// A client selection's text is read up to this size; a bigger one is not given to the host.
const READ_CAP: usize = 64 << 20;

/// Called (from a reading thread) with a client selection's text and the selection generation it
/// belongs to; the host hands both back through `Compositor::selection_read`.
pub type SelectionReader = Arc<dyn Fn(Board, u64, String) + Send + Sync>;

#[derive(Clone)]
pub(crate) enum Source { Client(ClientId, u32), Host(Arc<str>) }

#[derive(Default)]
pub(crate) struct BoardState {
    pub source: Option<Source>,
    /// Bumped on every change: offers and reads of an older selection are stale.
    pub generation: u64,
    /// Data devices per client.
    pub devices: HashMap<ClientId, Vec<u32>>,
    /// The text read from the current client source.
    pub text: Option<Arc<str>>,
    /// The generation of a client source that went away before its text was read: the text,
    /// when it arrives, becomes the selection (nothing else was selected since).
    pub orphan: Option<u64>,
}

/// The protocol numbers of one board.
struct Proto { offer: &'static Interface, offer_offer: u16, data_offer: u16, selection: u16, send: u16, cancelled: u16 }
fn proto(b: Board) -> Proto {
    match b {
        Board::Clipboard => Proto { offer: &wl_data_offer::INTERFACE, offer_offer: wl_data_offer::EVT_OFFER, data_offer: wl_data_device::EVT_DATA_OFFER,
            selection: wl_data_device::EVT_SELECTION, send: wl_data_source::EVT_SEND, cancelled: wl_data_source::EVT_CANCELLED },
        Board::Primary => Proto { offer: &zwp_primary_selection_offer_v1::INTERFACE, offer_offer: zwp_primary_selection_offer_v1::EVT_OFFER,
            data_offer: zwp_primary_selection_device_v1::EVT_DATA_OFFER, selection: zwp_primary_selection_device_v1::EVT_SELECTION,
            send: zwp_primary_selection_source_v1::EVT_SEND, cancelled: zwp_primary_selection_source_v1::EVT_CANCELLED },
    }
}

fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    if unsafe { crate::sys::pipe2(fds.as_mut_ptr(), crate::sys::O_CLOEXEC) } < 0 { return None; }
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

impl Compositor {
    fn board(&mut self, b: Board) -> &mut BoardState { &mut self.boards[b as usize] }

    /// Installs the reader that receives the text of client selections (see the module docs).
    pub fn set_selection_reader(&mut self, reader: SelectionReader) { self.selection_reader = Some(reader); }

    pub(crate) fn data_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &mut Vec<Value>) -> Result<(), Fatal> {
        use Board::*;
        // (board, what) for the requests that matter; the rest (accept, finish, set_actions, destroy) need nothing
        enum Req { NewSource, NewDevice, Offer, SetSelection, Receive, StartDrag }
        let (board, req) = match (o.iface.name, op) {
            ("wl_data_device_manager", wl_data_device_manager::REQ_CREATE_DATA_SOURCE) => (Clipboard, Req::NewSource),
            ("wl_data_device_manager", wl_data_device_manager::REQ_GET_DATA_DEVICE) => (Clipboard, Req::NewDevice),
            ("wl_data_source", wl_data_source::REQ_OFFER) => (Clipboard, Req::Offer),
            ("wl_data_device", wl_data_device::REQ_SET_SELECTION) => (Clipboard, Req::SetSelection),
            ("wl_data_device", wl_data_device::REQ_START_DRAG) => (Clipboard, Req::StartDrag),
            ("wl_data_offer", wl_data_offer::REQ_RECEIVE) => (Clipboard, Req::Receive),
            ("zwp_primary_selection_device_manager_v1", zwp_primary_selection_device_manager_v1::REQ_CREATE_SOURCE) => (Primary, Req::NewSource),
            ("zwp_primary_selection_device_manager_v1", zwp_primary_selection_device_manager_v1::REQ_GET_DEVICE) => (Primary, Req::NewDevice),
            ("zwp_primary_selection_source_v1", zwp_primary_selection_source_v1::REQ_OFFER) => (Primary, Req::Offer),
            ("zwp_primary_selection_device_v1", zwp_primary_selection_device_v1::REQ_SET_SELECTION) => (Primary, Req::SetSelection),
            ("zwp_primary_selection_offer_v1", zwp_primary_selection_offer_v1::REQ_RECEIVE) => (Primary, Req::Receive),
            _ => return Ok(()),
        };
        match req {
            Req::NewSource => { self.objs.insert((c, a[0].as_uint()), Obj::DataSource { board, mimes: Vec::new() }); }
            Req::NewDevice => {
                let id = a[0].as_uint();
                self.objs.insert((c, id), Obj::DataDevice { board });
                self.board(board).devices.entry(c).or_default().push(id);
                // a device made while its client has the keyboard hears the selection at once
                if self.input.kb_focus.is_some_and(|k| k.0 == c) { self.send_board(core, board, c, Some(id)); }
            }
            Req::Offer => if let Some(Obj::DataSource { mimes, .. }) = self.objs.get_mut(&(c, o.id)) { mimes.push(a[0].as_str().to_string()); },
            Req::SetSelection => {
                let src = a[0].as_uint();
                if src != 0 && !matches!(self.objs.get(&(c, src)), Some(Obj::DataSource { board: b, .. }) if *b == board) { return Ok(()); }
                self.set_client_selection(core, board, c, src);
            }
            Req::StartDrag => {
                let src = a[0].as_uint();
                if src != 0 { let _ = core.send(c, src, wl_data_source::EVT_CANCELLED, vec![]); }
            }
            Req::Receive => {
                let Some(Obj::DataOffer { generation, .. }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
                let generation = *generation;
                let mime = a[0].as_str().to_string();
                let Some(Value::Fd(fd)) = a.pop() else { return Ok(()) };
                let st = self.board(board);
                // An offer of an older selection closes the fd unanswered.
                if st.generation != generation { return Ok(()); }
                match st.source.clone() {
                    Some(Source::Client(sc, sid)) => { let _ = core.send(sc, sid, proto(board).send, vec![Value::Str(Some(mime)), Value::Fd(fd)]); }
                    Some(Source::Host(text)) => if TEXT_MIMES.contains(&mime.as_str()) {
                        // the reader may be slow or gone; the server thread never waits for it
                        let _ = std::thread::Builder::new().name("wayland-selection-write".into()).spawn(move || {
                            let _ = std::fs::File::from(fd).write_all(text.as_bytes());
                        });
                    },
                    None => {}
                }
            }
        }
        Ok(())
    }

    /// The selection is now client `c`'s source `src` (0: no selection).
    fn set_client_selection(&mut self, core: &mut Core, board: Board, c: ClientId, src: u32) {
        let st = self.board(board);
        if matches!(st.source, Some(Source::Client(sc, sid)) if (sc, sid) == (c, src)) { return; }
        let new = (src != 0).then_some(Source::Client(c, src));
        self.replace_selection(core, board, new);
        if src != 0 { self.read_client_selection(core, board, c, src); }
    }

    /// The host's text is now the selection (a host app copied): offered to clients as text.
    pub fn set_host_selection(&mut self, core: &mut Core, board: Board, text: String) {
        let text: Arc<str> = text.into();
        self.replace_selection(core, board, Some(Source::Host(text.clone())));
        self.board(board).text = Some(text);
    }

    fn replace_selection(&mut self, core: &mut Core, board: Board, new: Option<Source>) {
        let st = self.board(board);
        let old = std::mem::replace(&mut st.source, new);
        st.generation += 1;
        st.text = None;
        st.orphan = None;
        if let Some(Source::Client(oc, os)) = old { let _ = core.send(oc, os, proto(board).cancelled, vec![]); }
        if let Some(f) = self.input.kb_focus { self.send_board(core, board, f.0, None); }
    }

    /// Asks the source for its text in the best text type it offers; a thread reads it and hands
    /// it to the selection reader.
    fn read_client_selection(&mut self, core: &mut Core, board: Board, c: ClientId, src: u32) {
        let Some(reader) = self.selection_reader.clone() else { return };
        let Some(Obj::DataSource { mimes, .. }) = self.objs.get(&(c, src)) else { return };
        let Some(mime) = TEXT_MIMES.iter().find(|m| mimes.iter().any(|x| x == *m)) else { return };
        let Some((rd, wr)) = pipe() else { return };
        if core.send(c, src, proto(board).send, vec![Value::Str(Some(mime.to_string())), Value::Fd(wr)]).is_err() { return; }
        let generation = self.board(board).generation;
        let _ = std::thread::Builder::new().name("wayland-selection-read".into()).spawn(move || {
            let mut bytes = Vec::new();
            let read = std::fs::File::from(rd).take(READ_CAP as u64 + 1).read_to_end(&mut bytes);
            if read.is_ok() && bytes.len() <= READ_CAP {
                reader(board, generation, String::from_utf8_lossy(&bytes).into_owned());
            }
        });
    }

    /// The text of the client selection of `generation` was read. Becomes `UiEvent::Selection`
    /// when that selection is still current, or when its source went away before the read
    /// finished and nothing was selected since (then the text is the selection).
    pub fn selection_read(&mut self, core: &mut Core, board: Board, generation: u64, text: String) {
        let st = self.board(board);
        if st.generation == generation && matches!(st.source, Some(Source::Client(..))) {
            st.text = Some(text.as_str().into());
        } else if st.orphan == Some(generation) && st.source.is_none() && !text.is_empty() {
            self.replace_selection(core, board, Some(Source::Host(text.as_str().into())));
            self.board(board).text = Some(text.as_str().into());
        } else {
            return;
        }
        self.events.push(UiEvent::Selection { board, text });
    }

    /// Tells the data devices of `client` (all, or just `only`) what the selection of `board` is now.
    fn send_board(&mut self, core: &mut Core, board: Board, client: ClientId, only: Option<u32>) {
        let st = &self.boards[board as usize];
        let devices: Vec<u32> = st.devices.get(&client).map(|d| d.iter().copied().filter(|d| only.is_none_or(|o| o == *d)).collect()).unwrap_or_default();
        let mimes: Option<Vec<String>> = match &st.source {
            Some(Source::Client(sc, sid)) => match self.objs.get(&(*sc, *sid)) { Some(Obj::DataSource { mimes, .. }) => Some(mimes.clone()), _ => None },
            Some(Source::Host(_)) => Some(TEXT_MIMES.iter().map(|m| m.to_string()).collect()),
            None => None,
        };
        let generation = st.generation;
        let p = proto(board);
        for dev in devices {
            match &mimes {
                Some(mimes) => {
                    let version = core.obj(client, dev).map_or(1, |o| o.version);
                    let Some(offer) = core.new_object(client, p.offer, version) else { continue };
                    self.objs.insert((client, offer), Obj::DataOffer { generation });
                    let _ = core.send(client, dev, p.data_offer, vec![Value::NewId(offer)]);
                    for m in mimes { let _ = core.send(client, offer, p.offer_offer, vec![Value::Str(Some(m.clone()))]); }
                    let _ = core.send(client, dev, p.selection, vec![Value::Object(offer)]);
                }
                None => { let _ = core.send(client, dev, p.selection, vec![Value::Object(0)]); }
            }
        }
    }

    /// Tells `client` (about to get the keyboard) what both selections are.
    pub(crate) fn send_selection(&mut self, core: &mut Core, client: ClientId) {
        self.send_board(core, Board::Clipboard, client, None);
        self.send_board(core, Board::Primary, client, None);
    }

    /// A source, device or offer left the map.
    pub(crate) fn data_object_gone(&mut self, core: &mut Core, c: ClientId, id: u32) {
        match self.objs.remove(&(c, id)) {
            Some(Obj::DataSource { board, .. }) => {
                let st = self.board(board);
                if !matches!(st.source, Some(Source::Client(sc, sid)) if (sc, sid) == (c, id)) { return; }
                // The text read from it stays the selection; a read still running may yet bring it.
                st.source = None;
                let (text, generation) = (st.text.clone(), st.generation);
                self.replace_selection(core, board, text.clone().map(Source::Host));
                let st = self.board(board);
                if text.is_none() { st.orphan = Some(generation); }
                st.text = text;
            }
            Some(Obj::DataDevice { board }) => if let Some(v) = self.board(board).devices.get_mut(&c) { v.retain(|&d| d != id); },
            _ => {}
        }
    }

    pub(crate) fn data_client_gone(&mut self, c: ClientId) {
        for st in &mut self.boards { st.devices.remove(&c); }
    }
}
