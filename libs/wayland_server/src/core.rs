//! Clients, object maps, globals and message dispatch. Independent of epoll, so it can be
//! driven directly in tests.
use crate::proto::{wl_callback, wl_display, wl_registry};
use crate::schema::{self, ArgType, Interface};
use crate::transport::{Fill, Transport};
use crate::wire::{self, Value, HEADER, MAX_MESSAGE};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;

pub type ClientId = u32;
pub const DISPLAY: u32 = 1;
const FIRST_SERVER_ID: u32 = 0xff00_0000;
/// Objects one client may hold.
pub const MAX_OBJECTS: usize = 65_536;

pub const ERR_INVALID_OBJECT: u32 = 0;
pub const ERR_INVALID_METHOD: u32 = 1;
pub const ERR_NO_MEMORY: u32 = 2;
pub const ERR_IMPLEMENTATION: u32 = 3;

#[derive(Clone, Copy)]
pub struct ObjRef { pub id: u32, pub iface: &'static Interface, pub version: u32, pub data: u64 }
struct Entry { iface: &'static Interface, version: u32, zombie: bool, data: u64 }

/// A fatal protocol error to report on an object; the client is disconnected after it.
pub struct Fatal { pub object: u32, pub code: u32, pub message: String }
impl Fatal {
    pub fn new(object: u32, code: u32, message: impl Into<String>) -> Self { Self { object, code, message: message.into() } }
}

pub struct Global { pub name: u32, pub iface: &'static Interface, pub version: u32 }

struct Client {
    transport: Transport,
    objects: HashMap<u32, Entry>,
    next_server_id: u32,
    free_server_ids: Vec<u32>,
    registries: Vec<u32>,
    cred: (i32, u32, u32),
    in_pos: usize,
    dying: bool,
}

pub trait Handler {
    /// A client bound a global; `id` is already registered with `iface`/`version`.
    fn bind(&mut self, core: &mut Core, client: ClientId, global: u32, iface: &'static Interface, version: u32, id: u32) -> Result<(), Fatal>;
    /// A request on an object the core does not handle itself. New ids in `args` are registered.
    fn request(&mut self, core: &mut Core, client: ClientId, obj: ObjRef, opcode: u16, args: Vec<Value>) -> Result<(), Fatal>;
    /// An object left the map (destructor request, or the client went away).
    fn destroyed(&mut self, core: &mut Core, client: ClientId, obj: ObjRef);
    /// A client was added (after `add_client`), so the handler can set per-client state.
    fn client_added(&mut self, _core: &mut Core, _client: ClientId) {}
    fn client_gone(&mut self, _core: &mut Core, _client: ClientId) {}
}

#[derive(Debug)]
pub enum SendError { NoClient, NoObject, BadOpcode, Wire(wire::WireError), Overflow }

pub struct Core {
    clients: HashMap<ClientId, Client>,
    next_client: ClientId,
    globals: Vec<Global>,
    next_global: u32,
    serial: u32,
    pub dirty: Vec<ClientId>,
    /// Clients the server itself cut off (protocol error, output overflow), with the reason; the host drains it.
    pub killed: Vec<(ClientId, String)>,
}

impl Core {
    pub fn new() -> Self {
        Self { clients: HashMap::new(), next_client: 1, globals: Vec::new(), next_global: 1, serial: 1, dirty: Vec::new(), killed: Vec::new() }
    }
    pub fn add_client(&mut self, stream: UnixStream) -> std::io::Result<ClientId> {
        let transport = Transport::new(stream)?;
        let cred = transport.peer_cred();
        let id = self.next_client;
        self.next_client += 1;
        let mut objects = HashMap::new();
        objects.insert(DISPLAY, Entry { iface: &wl_display::INTERFACE, version: 1, zombie: false, data: 0 });
        self.clients.insert(id, Client { transport, objects, next_server_id: FIRST_SERVER_ID, free_server_ids: Vec::new(), registries: Vec::new(), cred, in_pos: 0, dying: false });
        Ok(id)
    }
    pub fn client_ids(&self) -> Vec<ClientId> { self.clients.keys().copied().collect() }
    pub fn has_client(&self, c: ClientId) -> bool { self.clients.contains_key(&c) }
    pub fn is_dying(&self, c: ClientId) -> bool { self.clients.get(&c).map_or(true, |c| c.dying) }
    pub fn peer(&self, c: ClientId) -> Option<(i32, u32, u32)> { self.clients.get(&c).map(|c| c.cred) }
    pub fn raw_fd(&self, c: ClientId) -> Option<i32> {
        use std::os::fd::AsRawFd;
        self.clients.get(&c).map(|c| c.transport.stream.as_raw_fd())
    }
    pub fn wants_write(&self, c: ClientId) -> bool { self.clients.get(&c).is_some_and(|c| !c.transport.out_buf.is_empty()) }
    pub fn next_serial(&mut self) -> u32 { self.serial = self.serial.wrapping_add(1); self.serial }

    pub fn obj(&self, c: ClientId, id: u32) -> Option<ObjRef> {
        let e = self.clients.get(&c)?.objects.get(&id)?;
        if e.zombie { return None; }
        Some(ObjRef { id, iface: e.iface, version: e.version, data: e.data })
    }
    pub fn set_data(&mut self, c: ClientId, id: u32, data: u64) {
        if let Some(e) = self.clients.get_mut(&c).and_then(|c| c.objects.get_mut(&id)) { e.data = data; }
    }
    pub fn objects_of(&self, c: ClientId) -> Vec<ObjRef> {
        self.clients.get(&c).map(|c| c.objects.iter().filter(|(_, e)| !e.zombie).map(|(&id, e)| ObjRef { id, iface: e.iface, version: e.version, data: e.data }).collect()).unwrap_or_default()
    }

    pub fn add_global(&mut self, iface: &'static Interface, version: u32) -> u32 {
        let name = self.next_global;
        self.next_global += 1;
        let version = version.min(iface.version);
        self.globals.push(Global { name, iface, version });
        let regs: Vec<(ClientId, u32)> = self.clients.iter().flat_map(|(&c, cl)| cl.registries.iter().map(move |&r| (c, r))).collect();
        for (c, r) in regs {
            let _ = self.send(c, r, wl_registry::EVT_GLOBAL, vec![Value::Uint(name), Value::Str(Some(iface.name.into())), Value::Uint(version)]);
        }
        name
    }
    pub fn remove_global(&mut self, name: u32) {
        self.globals.retain(|g| g.name != name);
        let regs: Vec<(ClientId, u32)> = self.clients.iter().flat_map(|(&c, cl)| cl.registries.iter().map(move |&r| (c, r))).collect();
        for (c, r) in regs { let _ = self.send(c, r, wl_registry::EVT_GLOBAL_REMOVE, vec![Value::Uint(name)]); }
    }

    /// Creates a server-side object (id from the 0xff000000 range).
    pub fn new_object(&mut self, c: ClientId, iface: &'static Interface, version: u32) -> Option<u32> {
        let cl = self.clients.get_mut(&c)?;
        if cl.objects.len() >= MAX_OBJECTS { return None; }
        let id = cl.free_server_ids.pop().unwrap_or_else(|| { let id = cl.next_server_id; cl.next_server_id += 1; id });
        cl.objects.insert(id, Entry { iface, version, zombie: false, data: 0 });
        Some(id)
    }

    /// Sends an event; the signature comes from the object's interface.
    pub fn send(&mut self, c: ClientId, id: u32, opcode: u16, args: Vec<Value>) -> Result<(), SendError> {
        let cl = self.clients.get_mut(&c).ok_or(SendError::NoClient)?;
        let e = cl.objects.get(&id).ok_or(SendError::NoObject)?;
        let msg = e.iface.events.get(opcode as usize).ok_or(SendError::BadOpcode)?;
        // Events newer than the version the client bound are not sent.
        if msg.since > e.version { return Ok(()); }
        let mut fds = Vec::new();
        let bytes = wire::encode(id, opcode, msg.args, args, &mut fds).map_err(SendError::Wire)?;
        if !cl.transport.queue(&bytes, fds) {
            cl.dying = true;
            self.killed.push((c, format!("its output queue overflowed (event {}.{})", e.iface.name, msg.name)));
            return Err(SendError::Overflow);
        }
        if !self.dirty.contains(&c) { self.dirty.push(c); }
        Ok(())
    }

    /// Destroys an object from the server side (e.g. a finished wl_callback): the id is
    /// released to the client with delete_id and tombstoned until the client reuses it.
    pub fn destroy_object(&mut self, h: &mut dyn Handler, c: ClientId, id: u32) {
        let Some(obj) = self.obj(c, id) else { return };
        h.destroyed(self, c, obj);
        let Some(cl) = self.clients.get_mut(&c) else { return };
        if id < FIRST_SERVER_ID {
            if let Some(e) = cl.objects.get_mut(&id) { e.zombie = true; }
            let _ = self.send(c, DISPLAY, wl_display::EVT_DELETE_ID, vec![Value::Uint(id)]);
        } else {
            cl.objects.remove(&id);
            cl.free_server_ids.push(id);
        }
    }

    pub fn post_error(&mut self, c: ClientId, f: Fatal) {
        self.killed.push((c, format!("protocol error {} on object {}: {}", f.code, f.object, f.message)));
        let _ = self.send(c, DISPLAY, wl_display::EVT_ERROR, vec![Value::Object(f.object), Value::Uint(f.code), Value::Str(Some(f.message))]);
        if let Some(cl) = self.clients.get_mut(&c) { cl.dying = true; }
    }
    pub fn kill(&mut self, c: ClientId) { if let Some(cl) = self.clients.get_mut(&c) { cl.dying = true; } }

    /// Reads from the socket and dispatches every complete message.
    pub fn readable(&mut self, h: &mut dyn Handler, c: ClientId) {
        let Some(cl) = self.clients.get_mut(&c) else { return };
        match cl.transport.fill() {
            Err(_) | Ok(Fill::Closed) => { cl.dying = true; return; }
            Ok(_) => {}
        }
        self.dispatch(h, c);
    }

    pub fn dispatch(&mut self, h: &mut dyn Handler, c: ClientId) {
        loop {
            if self.is_dying(c) { break; }
            match self.next_message(c) {
                None => break,
                Some(Err(f)) => { self.post_error(c, f); break; }
                Some(Ok(None)) => continue,
                Some(Ok(Some((obj, opcode, args, destructor)))) => {
                    let r = if obj.id == DISPLAY || obj.iface.name == "wl_registry" {
                        self.builtin(h, c, obj, opcode, args)
                    } else {
                        // A bug in a handler must cost the client its connection, not the server its thread.
                        let call = std::panic::AssertUnwindSafe(|| h.request(self, c, obj, opcode, args));
                        match std::panic::catch_unwind(call) {
                            Ok(r) => r,
                            Err(_) => Err(Fatal::new(obj.id, ERR_IMPLEMENTATION, "internal server error")),
                        }
                    };
                    if let Err(f) = r { self.post_error(c, f); break; }
                    if destructor { self.remove_object(h, c, obj); }
                }
            }
        }
        if let Some(cl) = self.clients.get_mut(&c) {
            let pos = cl.in_pos.min(cl.transport.in_buf.len());
            cl.transport.in_buf.drain(..pos);
            cl.in_pos = 0;
        }
    }

    fn remove_object(&mut self, h: &mut dyn Handler, c: ClientId, obj: ObjRef) {
        h.destroyed(self, c, obj);
        let Some(cl) = self.clients.get_mut(&c) else { return };
        cl.objects.remove(&obj.id);
        if obj.id >= FIRST_SERVER_ID { cl.free_server_ids.push(obj.id); } else {
            let _ = self.send(c, DISPLAY, wl_display::EVT_DELETE_ID, vec![Value::Uint(obj.id)]);
        }
        if let Some(cl) = self.clients.get_mut(&c) { cl.registries.retain(|&r| r != obj.id); }
    }

    /// Parses one message. `None`: need more bytes. `Ok(None)`: message consumed and ignored.
    #[allow(clippy::type_complexity)]
    fn next_message(&mut self, c: ClientId) -> Option<Result<Option<(ObjRef, u16, Vec<Value>, bool)>, Fatal>> {
        let cl = self.clients.get_mut(&c)?;
        let buf = &cl.transport.in_buf[cl.in_pos..];
        let (id, opcode, size) = wire::peek_header(buf)?;
        if size < HEADER || size % 4 != 0 || size > MAX_MESSAGE {
            return Some(Err(Fatal::new(DISPLAY, ERR_INVALID_METHOD, format!("bad message size {size}"))));
        }
        if buf.len() < size { return None; }
        let Some(e) = cl.objects.get(&id) else {
            return Some(Err(Fatal::new(DISPLAY, ERR_INVALID_OBJECT, format!("invalid object {id}"))));
        };
        let (iface, version, zombie, data) = (e.iface, e.version, e.zombie, e.data);
        let Some(msg) = iface.requests.get(opcode as usize) else {
            return Some(Err(Fatal::new(id, ERR_INVALID_METHOD, format!("invalid request {opcode} for {}", iface.name))));
        };
        if msg.since > version {
            return Some(Err(Fatal::new(id, ERR_INVALID_METHOD, format!("{}.{} needs version {}, object is {version}", iface.name, msg.name, msg.since))));
        }
        let body = &buf[HEADER..size];
        let decoded = wire::decode_args(body, msg.args, &mut cl.transport.in_fds);
        cl.in_pos += size;
        let args = match decoded {
            Ok(a) => a,
            Err(e) => return Some(Err(Fatal::new(id, ERR_INVALID_METHOD, format!("{}.{}: {}", iface.name, msg.name, e.text())))),
        };
        if zombie { return Some(Ok(None)); }
        // Validate object references and register new ids.
        for (t, v) in msg.args.iter().zip(args.iter()) {
            match (t, v) {
                (ArgType::Object(want) | ArgType::ObjectOpt(want), Value::Object(oid)) => {
                    if *oid == 0 {
                        if matches!(t, ArgType::Object(_)) {
                            return Some(Err(Fatal::new(id, ERR_INVALID_OBJECT, format!("{}.{}: null object", iface.name, msg.name))));
                        }
                        continue;
                    }
                    match cl.objects.get(oid) {
                        None => return Some(Err(Fatal::new(id, ERR_INVALID_OBJECT, format!("{}.{}: unknown object {oid}", iface.name, msg.name)))),
                        Some(o) if o.zombie => {}
                        Some(o) if !want.is_empty() && o.iface.name != *want => {
                            return Some(Err(Fatal::new(id, ERR_INVALID_OBJECT, format!("{}.{}: object {oid} is {}, expected {want}", iface.name, msg.name, o.iface.name))));
                        }
                        Some(_) => {}
                    }
                }
                (ArgType::NewId(want), Value::NewId(nid)) => {
                    let Some(ni) = schema::find(want) else {
                        return Some(Err(Fatal::new(id, ERR_IMPLEMENTATION, format!("unknown interface {want}"))));
                    };
                    if let Err(f) = Self::claim_id(cl, id, *nid, ni, version) { return Some(Err(f)); }
                }
                _ => {}
            }
        }
        let obj = ObjRef { id, iface, version, data };
        Some(Ok(Some((obj, opcode, args, msg.destructor))))
    }

    fn claim_id(cl: &mut Client, parent: u32, nid: u32, iface: &'static Interface, version: u32) -> Result<(), Fatal> {
        if nid == 0 || nid >= FIRST_SERVER_ID {
            return Err(Fatal::new(parent, ERR_INVALID_OBJECT, format!("invalid new id {nid}")));
        }
        if cl.objects.get(&nid).is_some_and(|e| !e.zombie) {
            return Err(Fatal::new(parent, ERR_INVALID_OBJECT, format!("new id {nid} already in use")));
        }
        if cl.objects.len() >= MAX_OBJECTS {
            return Err(Fatal::new(DISPLAY, ERR_NO_MEMORY, "too many objects"));
        }
        cl.objects.insert(nid, Entry { iface, version, zombie: false, data: 0 });
        Ok(())
    }

    fn builtin(&mut self, h: &mut dyn Handler, c: ClientId, obj: ObjRef, opcode: u16, args: Vec<Value>) -> Result<(), Fatal> {
        if obj.id == DISPLAY {
            match opcode {
                wl_display::REQ_SYNC => {
                    let cb = args[0].as_uint();
                    let serial = self.next_serial();
                    // wl_callback is created by the client (new_id) with the display's version.
                    self.claim(c, cb, &wl_callback::INTERFACE, 1)?;
                    let _ = self.send(c, cb, wl_callback::EVT_DONE, vec![Value::Uint(serial)]);
                    self.destroy_object(h, c, cb);
                }
                wl_display::REQ_GET_REGISTRY => {
                    let reg = args[0].as_uint();
                    self.claim(c, reg, &wl_registry::INTERFACE, 1)?;
                    self.clients.get_mut(&c).unwrap().registries.push(reg);
                    let list: Vec<(u32, &'static str, u32)> = self.globals.iter().map(|g| (g.name, g.iface.name, g.version)).collect();
                    for (name, iface, version) in list {
                        let _ = self.send(c, reg, wl_registry::EVT_GLOBAL, vec![Value::Uint(name), Value::Str(Some(iface.into())), Value::Uint(version)]);
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        // wl_registry.bind
        let Value::Uint(name) = args[0] else { return Ok(()) };
        let Value::NewIdDyn { interface, version, id } = &args[1] else { return Ok(()) };
        let Some(g) = self.globals.iter().find(|g| g.name == name) else {
            return Err(Fatal::new(obj.id, 0, format!("global {name} does not exist")));
        };
        let (giface, gver) = (g.iface, g.version);
        if giface.name != interface {
            return Err(Fatal::new(obj.id, 0, format!("global {name} is {}, not {interface}", giface.name)));
        }
        if *version == 0 || *version > gver {
            return Err(Fatal::new(obj.id, 0, format!("{interface} version {version} not offered (max {gver})")));
        }
        self.claim(c, *id, giface, *version)?;
        h.bind(self, c, name, giface, *version, *id)
    }

    fn claim(&mut self, c: ClientId, id: u32, iface: &'static Interface, version: u32) -> Result<(), Fatal> {
        let cl = self.clients.get_mut(&c).unwrap();
        // For wl_display.sync / get_registry the generated signature has a typed new_id which
        // next_message already registered; only registry.bind (dynamic) registers here.
        if cl.objects.get(&id).is_some_and(|e| std::ptr::eq(e.iface, iface) && !e.zombie) { return Ok(()); }
        Self::claim_id(cl, DISPLAY, id, iface, version)
    }

    /// Flushes pending output. Returns Ok(true) when empty. Errors mark the client dying.
    pub fn flush(&mut self, c: ClientId) -> bool {
        let Some(cl) = self.clients.get_mut(&c) else { return true };
        match cl.transport.flush() {
            Ok(done) => done,
            Err(_) => { cl.dying = true; true }
        }
    }

    /// Removes a client, telling the handler about every object it still held.
    pub fn disconnect(&mut self, h: &mut dyn Handler, c: ClientId) {
        for obj in self.objects_of(c) { h.destroyed(self, c, obj); }
        self.clients.remove(&c);
        self.dirty.retain(|&d| d != c);
        h.client_gone(self, c);
    }
}
