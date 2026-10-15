//! wl_data_device and primary-selection transfers. Each fd remains registered until EOF;
//! WouldBlock is not completion, and a slow clipboard owner never blocks the shell.
use super::*;
use crate::clipboard::ClipboardData;
use std::fs::File;

pub(super) enum Purpose {
    Reply(u32, u64),
    Paste(Key),
    Preview(Key, wayland_client::backend::ObjectId),
}
struct Offer {
    proxy: wl_data_offer::WlDataOffer,
    mimes: Vec<String>,
}
struct PrimaryOffer {
    proxy: zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
    mimes: Vec<String>,
}
struct Group {
    purpose: Purpose,
    remaining: usize,
    content: ClipboardData,
    items: Vec<(u32, String)>,
}
struct Incoming {
    key: Key,
    offer: wl_data_offer::WlDataOffer,
    serial: u32,
    pos: [f64; 2],
    items: Vec<(u32, String)>,
    ready: bool,
    dropped: bool,
    answer: u32,
    replies: usize,
}
#[derive(Clone)]
pub(super) struct Source {
    items: Arc<Vec<(u32, String)>>,
    drag: Option<Key>,
}

#[derive(Default)]
pub(super) struct Selection {
    pub manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pub device: Option<wl_data_device::WlDataDevice>,
    pub primary_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    pub primary_device: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    offers: HashMap<wayland_client::backend::ObjectId, Offer>,
    primary_offers: HashMap<wayland_client::backend::ObjectId, PrimaryOffer>,
    clipboard: Option<wl_data_offer::WlDataOffer>,
    primary: Option<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1>,
    source: Option<wl_data_source::WlDataSource>,
    primary_source: Option<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1>,
    pending_text: Option<String>,
    pending_primary: Option<String>,
    reads: Vec<io::ReadPipe<(u64, String)>>,
    writes: Vec<io::WritePipe>,
    groups: HashMap<u64, Group>,
    next: u64,
    incoming: Option<Incoming>,
    drag_icon: Option<shadow::DragIcon>,
    drag_source: Option<wl_data_source::WlDataSource>,
}
impl Selection {
    pub fn fds(&self, fds: &mut Vec<io::PollFd>) {
        fds.extend(self.reads.iter().map(|r| io::PollFd {
            fd: r.file.as_raw_fd(),
            events: io::READ,
            revents: 0,
        }));
        fds.extend(self.writes.iter().map(|w| io::PollFd {
            fd: w.file.as_raw_fd(),
            events: io::WRITE,
            revents: 0,
        }));
    }
    pub fn close_session(&mut self, sid: u32) {
        self.groups.retain(|_, g| match g.purpose {
            Purpose::Reply(s, _) => s != sid,
            Purpose::Paste(k) | Purpose::Preview(k, _) => k.0 != sid,
        });
        self.reads.retain(|r| self.groups.contains_key(&r.target.0));
        if self.incoming.as_ref().is_some_and(|i| i.key.0 == sid) {
            if let Some(d) = self.incoming.take() {
                d.offer.destroy();
            }
        }
        if self
            .drag_source
            .as_ref()
            .and_then(|p| p.data::<Source>())
            .and_then(|s| s.drag)
            .is_some_and(|k| k.0 == sid)
        {
            if let Some(source) = self.drag_source.take() {
                source.destroy();
            }
            self.drag_icon = None;
        }
    }
}
impl State {
    pub(super) fn set_clipboard(&mut self, text: String, primary: bool) {
        if primary {
            self.selection.pending_primary = Some(text);
        } else {
            self.selection.pending_text = Some(text);
        }
        self.apply_selection();
    }
    pub(super) fn apply_selection(&mut self) {
        if self.input.serial == 0 {
            return;
        }
        if let (Some(manager), Some(device)) = (
            self.selection.manager.as_ref(),
            self.selection.device.as_ref(),
        ) {
            if let Some(text) = self.selection.pending_text.take() {
                let source = manager.create_data_source(
                    &self.qh,
                    Source {
                        items: Arc::new(vec![(raw::drag_item::STRING, text)]),
                        drag: None,
                    },
                );
                for mime in common::TEXT_MIMES {
                    source.offer(mime.into());
                }
                device.set_selection(Some(&source), self.input.serial);
                if let Some(old) = self.selection.source.replace(source) {
                    old.destroy();
                }
            }
        }
        if let (Some(manager), Some(device)) = (
            self.selection.primary_manager.as_ref(),
            self.selection.primary_device.as_ref(),
        ) {
            if let Some(text) = self.selection.pending_primary.take() {
                let source = manager.create_source(
                    &self.qh,
                    Source {
                        items: Arc::new(vec![(raw::drag_item::STRING, text)]),
                        drag: None,
                    },
                );
                for mime in common::TEXT_MIMES {
                    source.offer(mime.into());
                }
                device.set_selection(Some(&source), self.input.serial);
                if let Some(old) = self.selection.primary_source.replace(source) {
                    old.destroy();
                }
            }
        }
    }
    pub(super) fn read_clipboard(&mut self, purpose: Purpose, primary: bool) {
        if primary {
            let offer = self
                .selection
                .primary
                .as_ref()
                .and_then(|p| self.selection.primary_offers.get(&p.id()))
                .map(|o| (o.proxy.clone(), o.mimes.clone()));
            if let Some((offer, mimes)) = offer {
                self.read_offer(purpose, &mimes, |mime, fd| offer.receive(mime, fd));
            } else {
                self.empty_read(purpose);
            }
        } else {
            let offer = self
                .selection
                .clipboard
                .as_ref()
                .and_then(|p| self.selection.offers.get(&p.id()))
                .map(|o| (o.proxy.clone(), o.mimes.clone()));
            if let Some((offer, mimes)) = offer {
                self.read_offer(purpose, &mimes, |mime, fd| offer.receive(mime, fd));
            } else {
                self.empty_read(purpose);
            }
        }
    }
    fn read_offer(
        &mut self,
        purpose: Purpose,
        mimes: &[String],
        receive: impl Fn(String, std::os::fd::BorrowedFd<'_>),
    ) {
        let mut wanted = Vec::new();
        if let Some(mime) = common::TEXT_MIMES
            .iter()
            .find(|m| mimes.iter().any(|offered| offered == **m))
        {
            wanted.push((*mime).to_string());
        }
        if matches!(purpose, Purpose::Reply(..)) && mimes.iter().any(|m| m == common::PNG_MIME) {
            wanted.push(common::PNG_MIME.to_string());
        }
        if mimes.iter().any(|m| m == common::URI_MIME) {
            wanted.push(common::URI_MIME.to_string());
        }
        if wanted.is_empty() {
            self.empty_read(purpose);
            return;
        }
        self.selection.next += 1;
        let group = self.selection.next;
        let mut count = 0;
        for mime in wanted {
            match io::pipe() {
                Ok((read, write)) => {
                    receive(mime.clone(), write.as_fd());
                    drop(write);
                    self.selection.reads.push(io::ReadPipe {
                        file: File::from(read),
                        bytes: Vec::new(),
                        target: (group, mime),
                    });
                    count += 1;
                }
                Err(e) => eprintln!("[hal-wayland] clipboard pipe: {e}"),
            }
        }
        if count == 0 {
            self.empty_read(purpose);
        } else {
            self.selection.groups.insert(
                group,
                Group {
                    purpose,
                    remaining: count,
                    content: ClipboardData::default(),
                    items: Vec::new(),
                },
            );
        }
    }
    fn empty_read(&mut self, purpose: Purpose) {
        self.read_done(purpose, ClipboardData::default(), Vec::new());
    }
    fn read_done(&mut self, purpose: Purpose, content: ClipboardData, items: Vec<(u32, String)>) {
        match purpose {
            Purpose::Reply(sid, req) => {
                let content = content.pack();
                self.shared.push((sid, crate::Id::NONE), |q, _| {
                    q.payload_size(content.len() as u32);
                    q.clipboard(req, true, &content);
                });
            }
            Purpose::Paste(key) => {
                for (_, text) in items
                    .iter()
                    .filter(|(kind, _)| *kind == raw::drag_item::STRING)
                {
                    self.shared
                        .push(key, |q, id| q.text(id, raw::text::PASTE, text.as_bytes()));
                }
            }
            Purpose::Preview(key, offer) => {
                if let Some(d) = self
                    .selection
                    .incoming
                    .as_mut()
                    .filter(|d| d.key == key && d.offer.id() == offer)
                {
                    d.items = items;
                    d.ready = true;
                    let bytes = common::packed(&d.items);
                    d.replies += 1;
                    self.shared.push(d.key, |q, id| {
                        q.drag_over(id, d.pos[0], d.pos[1], 0, d.items.len() as u32, &bytes)
                    });
                }
                self.finish_drop();
            }
        }
    }
    pub(super) fn transfers(&mut self) {
        let mut completed = Vec::new();
        let mut pending = Vec::new();
        for mut pipe in std::mem::take(&mut self.selection.reads) {
            match pipe.read() {
                Ok(false) => pending.push(pipe),
                Ok(true) => completed.push(pipe),
                Err(e) => {
                    eprintln!("[hal-wayland] selection read: {e}");
                    completed.push(pipe);
                }
            }
        }
        self.selection.reads = pending;
        for pipe in completed {
            let (id, mime) = pipe.target;
            if let Some(group) = self.selection.groups.get_mut(&id) {
                if matches!(group.purpose, Purpose::Reply(..)) {
                    common::decode_clipboard(&mut group.content, &mime, pipe.bytes);
                } else {
                    group.items.extend(common::decode_items(&mime, &pipe.bytes));
                }
                group.remaining -= 1;
                if group.remaining == 0 {
                    let group = self.selection.groups.remove(&id).unwrap();
                    self.read_done(group.purpose, group.content, group.items);
                }
            }
        }
        self.selection.writes.retain_mut(|pipe| match pipe.write() {
            Ok(done) => !done,
            Err(e) => {
                eprintln!("[hal-wayland] selection write: {e}");
                false
            }
        });
    }
    fn send_selection(&mut self, fd: std::os::fd::OwnedFd, mime: &str, source: &Source) {
        if let Err(e) = io::nonblocking(fd.as_raw_fd()) {
            eprintln!("[hal-wayland] nonblocking selection: {e}");
            return;
        }
        let bytes = if mime == common::URI_MIME {
            common::encode_uris(&source.items)
        } else {
            source
                .items
                .iter()
                .filter(|(k, _)| *k == raw::drag_item::STRING)
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        };
        self.selection.writes.push(io::WritePipe {
            file: File::from(fd),
            bytes: Arc::new(bytes),
            offset: 0,
        });
    }
    pub(super) fn start_drag(&mut self, key: Key, items: Vec<(u32, String)>) {
        let recipient = self.windows.get(&key).map_or(key, |w| w.recipient);
        if let (Some(manager), Some(device), Some(w)) = (
            self.selection.manager.as_ref(),
            self.selection.device.as_ref(),
            self.windows.get(&key),
        ) {
            if let Some((_, serial)) = self.input.drag_start.filter(|(origin, _)| *origin == key) {
                let has_files = items.iter().any(|(k, _)| *k == raw::drag_item::FILE);
                let has_text = items.iter().any(|(k, _)| *k == raw::drag_item::STRING);
                let source = manager.create_data_source(
                    &self.qh,
                    Source {
                        items: Arc::new(items),
                        drag: Some(recipient),
                    },
                );
                if has_files {
                    source.offer(common::URI_MIME.into());
                }
                if has_text {
                    for mime in common::TEXT_MIMES {
                        source.offer(mime.into());
                    }
                }
                if source.version() >= 3 {
                    source.set_actions(
                        wl_data_device_manager::DndAction::Copy
                            | wl_data_device_manager::DndAction::Move,
                    );
                }
                let icon = if let (Some(comp), Some(shm)) =
                    (self.compositor.as_ref(), self.shm.as_ref())
                {
                    shadow::DragIcon::new(comp, shm, &self.qh)
                } else {
                    None
                };
                device.start_drag(
                    Some(&source),
                    &w.surface,
                    icon.as_ref().map(|i| &i.surface),
                    serial,
                );
                self.selection.drag_icon = icon;
                self.selection.drag_source = Some(source);
                return;
            }
        }
        self.shared
            .push(recipient, |q, id| q.drag_drop(id, 0.0, 0.0, false, 0, &[]));
    }
    pub(super) fn drag_reply(&mut self, key: Key, answer: u32) {
        let recipient = self.windows.get(&key).map_or(key, |w| w.recipient);
        if let Some(d) = self
            .selection
            .incoming
            .as_mut()
            .filter(|d| d.key == recipient)
        {
            d.answer = answer;
            d.replies = d.replies.saturating_sub(1);
            let mime = self.selection.offers.get(&d.offer.id()).and_then(|o| {
                if o.mimes.iter().any(|m| m == common::URI_MIME) {
                    Some(common::URI_MIME.to_string())
                } else {
                    common::TEXT_MIMES
                        .iter()
                        .find(|m| o.mimes.iter().any(|s| s == **m))
                        .map(|s| s.to_string())
                }
            });
            d.offer
                .accept(d.serial, if answer == 0 { None } else { mime });
            if d.offer.version() >= 3 {
                let action = match answer {
                    raw::drag_response::MOVE => wl_data_device_manager::DndAction::Move,
                    0 => wl_data_device_manager::DndAction::empty(),
                    _ => wl_data_device_manager::DndAction::Copy,
                };
                d.offer.set_actions(action, action);
            }
        }
        self.finish_drop();
    }
    fn finish_drop(&mut self) {
        if self
            .selection
            .incoming
            .as_ref()
            .is_some_and(|d| d.dropped && d.ready && d.replies == 0)
        {
            let d = self.selection.incoming.take().unwrap();
            let bytes = common::packed(&d.items);
            let accepted = d.answer != 0;
            self.shared.push(d.key, |q, id| {
                q.drag_drop(
                    id,
                    d.pos[0],
                    d.pos[1],
                    accepted,
                    d.items.len() as u32,
                    &bytes,
                )
            });
            if accepted && d.offer.version() >= 3 {
                d.offer.finish();
            }
            d.offer.destroy();
            self.selection.offers.remove(&d.offer.id());
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        s: &mut Self,
        _: &wl_data_device::WlDataDevice,
        e: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_data_device::Event::DataOffer { id } => {
                s.selection.offers.entry(id.id()).or_insert(Offer {
                    proxy: id,
                    mimes: Vec::new(),
                });
            }
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = s.selection.clipboard.take() {
                    if id.as_ref() != Some(&old) {
                        old.destroy();
                        s.selection.offers.remove(&old.id());
                    }
                }
                s.selection.clipboard = id;
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id,
            } => {
                if let (Some(key), Some(offer)) = (s.surface_key(&surface), id) {
                    let recipient = s.windows[&key].recipient;
                    let mimes = s
                        .selection
                        .offers
                        .get(&offer.id())
                        .map_or(Vec::new(), |o| o.mimes.clone());
                    s.selection.incoming = Some(Incoming {
                        key: recipient,
                        offer: offer.clone(),
                        serial,
                        pos: [x, y],
                        items: Vec::new(),
                        ready: false,
                        dropped: false,
                        answer: 0,
                        replies: 0,
                    });
                    s.read_offer(
                        Purpose::Preview(recipient, offer.id()),
                        &mimes,
                        |mime, fd| offer.receive(mime, fd),
                    );
                }
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some(d) = s.selection.incoming.as_mut() {
                    d.pos = [x, y];
                    if d.ready {
                        d.replies += 1;
                        let bytes = common::packed(&d.items);
                        s.shared.push(d.key, |q, id| {
                            q.drag_over(id, x, y, 1, d.items.len() as u32, &bytes)
                        });
                    }
                }
            }
            wl_data_device::Event::Drop => {
                if let Some(d) = s.selection.incoming.as_mut() {
                    d.dropped = true;
                }
                s.finish_drop();
            }
            wl_data_device::Event::Leave => {
                if s.selection.incoming.as_ref().is_some_and(|d| !d.dropped) {
                    let d = s.selection.incoming.take().unwrap();
                    s.shared.push(d.key, |q, id| {
                        q.drag_drop(id, d.pos[0], d.pos[1], false, 0, &[])
                    });
                    d.offer.destroy();
                    s.selection.offers.remove(&d.offer.id());
                }
            }
            _ => {}
        }
    }
    fn event_created_child(
        opcode: u16,
        q: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            wl_data_device::EVT_DATA_OFFER_OPCODE => {
                q.make_data::<wl_data_offer::WlDataOffer, ()>(())
            }
            _ => unreachable!("wl_data_device child opcode {opcode}"),
        }
    }
}
impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(
        s: &mut Self,
        p: &wl_data_offer::WlDataOffer,
        e: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = e {
            s.selection
                .offers
                .entry(p.id())
                .or_insert_with(|| Offer {
                    proxy: p.clone(),
                    mimes: Vec::new(),
                })
                .mimes
                .push(mime_type);
        }
    }
}
impl Dispatch<wl_data_source::WlDataSource, Source> for State {
    fn event(
        s: &mut Self,
        p: &wl_data_source::WlDataSource,
        e: wl_data_source::Event,
        data: &Source,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_data_source::Event::Send { mime_type, fd } => s.send_selection(fd, &mime_type, data),
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                let success = matches!(e, wl_data_source::Event::DndFinished);
                if let Some(key) = data.drag {
                    let bytes = common::packed(&data.items);
                    s.shared.push(key, |q, id| {
                        q.drag_drop(id, 0.0, 0.0, success, data.items.len() as u32, &bytes)
                    });
                    s.selection.drag_icon = None;
                    s.selection.drag_source = None;
                }
                if s.selection.source.as_ref() == Some(p) {
                    s.selection.source = None;
                }
                p.destroy();
            }
            _ => {}
        }
    }
}
impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        e: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                s.selection
                    .primary_offers
                    .entry(offer.id())
                    .or_insert(PrimaryOffer {
                        proxy: offer,
                        mimes: Vec::new(),
                    });
            }
            zwp_primary_selection_device_v1::Event::Selection { id } => {
                if let Some(old) = s.selection.primary.take() {
                    if id.as_ref() != Some(&old) {
                        old.destroy();
                        s.selection.primary_offers.remove(&old.id());
                    }
                }
                s.selection.primary = id;
            }
            _ => {}
        }
    }
    fn event_created_child(
        opcode: u16,
        q: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => {
                q.make_data::<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()>(())
            }
            _ => unreachable!("primary selection child opcode {opcode}"),
        }
    }
}
impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for State {
    fn event(
        s: &mut Self,
        p: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        e: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = e {
            s.selection
                .primary_offers
                .entry(p.id())
                .or_insert_with(|| PrimaryOffer {
                    proxy: p.clone(),
                    mimes: Vec::new(),
                })
                .mimes
                .push(mime_type);
        }
    }
}
impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, Source> for State {
    fn event(
        s: &mut Self,
        p: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        e: zwp_primary_selection_source_v1::Event,
        data: &Source,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                s.send_selection(fd, &mime_type, data)
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                if s.selection.primary_source.as_ref() == Some(p) {
                    s.selection.primary_source = None;
                }
                p.destroy();
            }
            _ => {}
        }
    }
}
delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(State: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
