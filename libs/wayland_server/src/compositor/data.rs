//! wl_data_device_manager: the clipboard selection between Wayland clients. The selection source
//! stays with its client; a receiver's fd is handed straight to it. (Drag and drop is not done.)
use super::*;

impl Compositor {
    pub(crate) fn data_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &mut Vec<Value>) -> Result<(), Fatal> {
        match o.iface.name {
            "wl_data_device_manager" => match op {
                wl_data_device_manager::REQ_CREATE_DATA_SOURCE => { self.objs.insert((c, a[0].as_uint()), Obj::DataSource { mimes: Vec::new() }); }
                wl_data_device_manager::REQ_GET_DATA_DEVICE => {
                    let id = a[0].as_uint();
                    self.objs.insert((c, id), Obj::DataDevice);
                    self.input.data_devices.entry(c).or_default().push(id);
                }
                _ => {}
            },
            "wl_data_source" => if op == wl_data_source::REQ_OFFER {
                if let Some(Obj::DataSource { mimes }) = self.objs.get_mut(&(c, o.id)) { mimes.push(a[0].as_str().to_string()); }
            },
            "wl_data_device" => match op {
                wl_data_device::REQ_SET_SELECTION => {
                    let src = a[0].as_uint();
                    let new = (src != 0).then_some((c, src));
                    if self.selection != new {
                        if let Some((oc, os)) = self.selection { let _ = core.send(oc, os, wl_data_source::EVT_CANCELLED, vec![]); }
                        self.selection = new;
                        if let Some(f) = self.input.kb_focus { self.send_selection(core, f.0); }
                    }
                }
                _ => {}
            },
            "wl_data_offer" => if op == wl_data_offer::REQ_RECEIVE {
                let Some(Obj::DataOffer { source }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
                let (sc, sid) = *source;
                let mime = a[0].as_str().to_string();
                let Some(Value::Fd(fd)) = a.pop() else { return Ok(()) };
                // A stale offer (selection moved on) closes the fd unanswered.
                if self.selection == Some((sc, sid)) {
                    let _ = core.send(sc, sid, wl_data_source::EVT_SEND, vec![Value::Str(Some(mime)), Value::Fd(fd)]);
                }
            },
            _ => {}
        }
        Ok(())
    }

    /// Tells the data devices of `client` what the selection is now.
    pub(crate) fn send_selection(&mut self, core: &mut Core, client: ClientId) {
        let devices = self.input.data_devices.get(&client).cloned().unwrap_or_default();
        for dev in devices {
            let sel = self.selection.and_then(|(sc, sid)| match self.objs.get(&(sc, sid)) {
                Some(Obj::DataSource { mimes }) => Some(((sc, sid), mimes.clone())),
                _ => None,
            });
            match sel {
                Some((src, mimes)) => {
                    let Some(offer) = core.new_object(client, &wl_data_offer::INTERFACE, 3) else { continue };
                    self.objs.insert((client, offer), Obj::DataOffer { source: src });
                    let _ = core.send(client, dev, wl_data_device::EVT_DATA_OFFER, vec![Value::NewId(offer)]);
                    for m in mimes { let _ = core.send(client, offer, wl_data_offer::EVT_OFFER, vec![Value::Str(Some(m))]); }
                    let _ = core.send(client, dev, wl_data_device::EVT_SELECTION, vec![Value::Object(offer)]);
                }
                None => { let _ = core.send(client, dev, wl_data_device::EVT_SELECTION, vec![Value::Object(0)]); }
            }
        }
    }

    pub(crate) fn data_source_gone(&mut self, core: &mut Core, c: ClientId, id: u32) {
        self.objs.remove(&(c, id));
        if self.selection == Some((c, id)) {
            self.selection = None;
            if let Some(f) = self.input.kb_focus { self.send_selection(core, f.0); }
        }
    }
}
