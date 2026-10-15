//! Reading the VM's tooling data: where an instruction is written, a fn's text, the docs of an
//! object literal, the loops and calls running now, the registered manifests, the bodies. One
//! op, [`ScriptVmBase::vm_query`], answers all of them and writes what it has to say through a
//! [`ScriptQueryOut`] as bytes on numbered channels (text, or the bytes of a repr(C) record);
//! the `ScriptVm` methods that used to be one op each keep their names and are inline decoders
//! of it, so the text and the records land in the caller's own `String`s and `Vec`s.
use makepad_relax_api::relax_api;

use crate::docs::ScriptFieldDocsOut;
use crate::makepad_live_id::LiveId;
use crate::value::*;
use crate::vm::*;
use crate::vm_api::{ScriptLineCol, ScriptTextOut};

pub const QUERY_FORMAT_ENUM_VARIANT: u32 = 0;
pub const QUERY_FORMAT_OBJECT: u32 = 1;
pub const QUERY_IP_TO_LOC: u32 = 2;
/// `flag`: the source lines from the header line (else the fn from its `fn` keyword).
pub const QUERY_FN_TEXT: u32 = 3;
pub const QUERY_LOOP_VALUES: u32 = 4;
pub const QUERY_CALL_SITES: u32 = 5;
pub const QUERY_THREAD_FOOTPRINT: u32 = 6;
pub const QUERY_BODY_MODULE: u32 = 7;
pub const QUERY_CRATE_MANIFEST: u32 = 8;
pub const QUERY_CRATE_MANIFESTS: u32 = 9;
pub const QUERY_SITE_DOCS: u32 = 10;
pub const QUERY_BODY_ROOT_IDS: u32 = 11;
pub const QUERY_BODY_BY_MODULE_PATH: u32 = 12;
pub const QUERY_BODY_END_SCOPE: u32 = 13;
pub const QUERY_END_SCOPE_OF: u32 = 14;
pub const QUERY_FIELD_LOC: u32 = 15;
pub const QUERY_IP_LINE_TEXT: u32 = 16;
/// `text`: the file; `flag`: a column is given, `index` it; the answer's `index` is the body.
pub const QUERY_BODY_BY_FILE: u32 = 17;
/// `index`: the body; the objects are written as three records (scope, me, end scope: the last
/// is the zero object when the body has not run), none when there is no such body.
pub const QUERY_BODY_SCOPES: u32 = 18;
/// `id`: the global's name; `value` is its value.
pub const QUERY_INJECTED_GLOBAL: u32 = 19;
/// `index`: the body; `value` is its initial scope and `flag` whether that is a GC root.
pub const QUERY_BODY_INITIAL_SCOPE: u32 = 20;

/// The channels a query writes on.
pub const CH_TEXT0: u32 = 0;
pub const CH_TEXT1: u32 = 1;
/// One repr(C) record per write.
pub const CH_RECORD: u32 = 2;
/// A field doc: its [`LiveId`] (8 bytes), then on [`CH_FIELD_TEXT`] its text.
pub const CH_FIELD_ID: u32 = 3;
pub const CH_FIELD_TEXT: u32 = 4;

/// What [`QUERY_LOOP_VALUES`] writes per loop variable.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptLoopValue {
    pub id: LiveId,
    pub value: f64,
}

/// Where a query writes: `write(ctx, channel, bytes, len)`, made on the caller's side.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptQueryOut<'a> {
    ctx: *mut u8,
    write: unsafe extern "C-unwind" fn(*mut u8, u32, *const u8, usize),
    _out: std::marker::PhantomData<&'a mut u8>,
}

impl<'a> ScriptQueryOut<'a> {
    /// # Safety
    /// The callback and context must remain valid for the writer's lifetime.
    #[inline(always)]
    pub unsafe fn from_callback(ctx: *mut u8, write: unsafe extern "C-unwind" fn(*mut u8, u32, *const u8, usize)) -> Self {
        Self { ctx, write, _out: std::marker::PhantomData }
    }

    #[inline(always)]
    pub fn discard() -> ScriptQueryOut<'static> {
        unsafe extern "C-unwind" fn nowhere(_ctx: *mut u8, _channel: u32, _ptr: *const u8, _len: usize) {}
        ScriptQueryOut { ctx: std::ptr::null_mut(), write: nowhere, _out: std::marker::PhantomData }
    }

    #[inline]
    pub fn text(&mut self, channel: u32, text: &str) {
        unsafe { (self.write)(self.ctx, channel, text.as_ptr(), text.len()) }
    }

    #[inline]
    pub fn record<T: Copy>(&mut self, channel: u32, record: &T) {
        unsafe { (self.write)(self.ctx, channel, record as *const T as *const u8, std::mem::size_of::<T>()) }
    }
}

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptQueryReq<'a> {
    pub kind: u32,
    pub ip: ScriptIp,
    pub id: LiveId,
    pub index: usize,
    /// in for [`QUERY_FN_TEXT`]; out for [`QUERY_SITE_DOCS`] (has a doc) and
    /// [`QUERY_THREAD_FOOTPRINT`] (is paused)
    pub flag: bool,
    /// in for the formats; out for the scope queries
    pub value: ScriptValue,
    pub text: &'a str,
    /// out: a location's line and column
    pub at: ScriptLineCol,
    pub out: ScriptQueryOut<'a>,
}

impl<'a> ScriptQueryReq<'a> {
    #[inline(always)]
    pub fn new(kind: u32, out: ScriptQueryOut<'a>) -> Self {
        Self { kind, ip: ScriptIp::default(), id: LiveId(0), index: 0, flag: false, value: NIL, text: "", at: ScriptLineCol::default(), out }
    }
}

/// The caller-side destinations of a query's channels; [`Self::out`] is the writer to hand
/// the request. Whatever a query writes where there is no destination is dropped.
pub struct ScriptQuerySink<'a> {
    text0: *mut ScriptTextOut<'static>,
    text1: *mut ScriptTextOut<'static>,
    fields: *mut ScriptFieldDocsOut<'static>,
    /// where records go: `(context, push(context, bytes, len))`
    pub records: Option<(*mut u8, unsafe extern "C-unwind" fn(*mut u8, *const u8, usize))>,
    field_id: u64,
    _sink: std::marker::PhantomData<&'a mut u8>,
}

impl<'a> ScriptQuerySink<'a> {
    #[inline(always)]
    pub fn new() -> Self {
        Self { text0: std::ptr::null_mut(), text1: std::ptr::null_mut(), fields: std::ptr::null_mut(), records: None, field_id: 0, _sink: std::marker::PhantomData }
    }

    /// Where [`CH_TEXT0`] goes.
    #[inline(always)]
    pub fn text0(&mut self, out: &'a mut ScriptTextOut) {
        self.text0 = out as *mut ScriptTextOut as *mut ScriptTextOut<'static>;
    }

    /// Where [`CH_TEXT1`] goes.
    #[inline(always)]
    pub fn text1(&mut self, out: &'a mut ScriptTextOut) {
        self.text1 = out as *mut ScriptTextOut as *mut ScriptTextOut<'static>;
    }

    /// Where [`CH_FIELD_ID`] / [`CH_FIELD_TEXT`] go.
    #[inline(always)]
    pub fn fields(&mut self, out: &'a mut ScriptFieldDocsOut) {
        self.fields = out as *mut ScriptFieldDocsOut as *mut ScriptFieldDocsOut<'static>;
    }

    /// The writer for this sink (the sink must not move while the query runs).
    #[inline(always)]
    pub fn out(&mut self) -> ScriptQueryOut<'_> {
        unsafe extern "C-unwind" fn write(ctx: *mut u8, channel: u32, ptr: *const u8, len: usize) {
            let sink = &mut *(ctx as *mut ScriptQuerySink<'static>);
            let bytes = std::slice::from_raw_parts(ptr, len);
            match channel {
                CH_TEXT0 => {
                    if let Some(out) = sink.text0.as_mut() {
                        out.push(std::str::from_utf8_unchecked(bytes));
                    }
                }
                CH_TEXT1 => {
                    if let Some(out) = sink.text1.as_mut() {
                        out.push(std::str::from_utf8_unchecked(bytes));
                    }
                }
                CH_RECORD => {
                    if let Some((ctx, push)) = sink.records {
                        push(ctx, ptr, len);
                    }
                }
                CH_FIELD_ID => {
                    let mut id = [0u8; 8];
                    id.copy_from_slice(bytes);
                    sink.field_id = u64::from_le_bytes(id);
                }
                CH_FIELD_TEXT => {
                    if let Some(out) = sink.fields.as_mut() {
                        out.push(LiveId(sink.field_id), std::str::from_utf8_unchecked(bytes));
                    }
                }
                _ => {}
            }
        }
        unsafe { ScriptQueryOut::from_callback(self as *mut Self as *mut u8, write) }
    }
}

/// What a query answered besides its text and records.
pub struct ScriptQueryAnswer {
    pub found: bool,
    pub at: ScriptLineCol,
    pub flag: bool,
    pub value: ScriptValue,
    pub index: usize,
}

/// Runs the op `run` on a request of kind `kind`, `set` filling the request in and its text and
/// records going to `sink`.
#[inline(always)]
fn run_query<'q>(kind: u32, sink: &'q mut ScriptQuerySink, set: impl FnOnce(&mut ScriptQueryReq<'q>), run: impl FnOnce(&mut ScriptQueryReq<'q>) -> bool) -> ScriptQueryAnswer {
    let mut req = ScriptQueryReq::new(kind, sink.out());
    set(&mut req);
    let found = run(&mut req);
    ScriptQueryAnswer { found, at: req.at, flag: req.flag, value: req.value, index: req.index }
}

impl ScriptVmBase {
    /// Runs the thread-and-heap query `kind` ([`Self::vm_query`]) with `set` filling in the
    /// request, its text and records going to `sink`.
    #[inline(always)]
    pub fn query<'q>(&self, kind: u32, sink: &'q mut ScriptQuerySink, set: impl FnOnce(&mut ScriptQueryReq<'q>)) -> ScriptQueryAnswer {
        run_query(kind, sink, set, |req| self.vm_query(req))
    }
}

impl ScriptCode {
    /// Runs the code query `kind` ([`Self::code_query`]) with `set` filling in the request, its
    /// text and records going to `sink`.
    #[inline(always)]
    pub fn query<'q>(&self, kind: u32, sink: &'q mut ScriptQuerySink, set: impl FnOnce(&mut ScriptQueryReq<'q>)) -> ScriptQueryAnswer {
        run_query(kind, sink, set, |req| self.code_query(req))
    }
}

/// A text channel as a [`ScriptTextOut`], for the VM-side code that writes text.
struct Chan {
    out: *mut ScriptQueryOut<'static>,
    channel: u32,
}

unsafe extern "C-unwind" fn chan_write(ctx: *mut u8, ptr: *const u8, len: usize) {
    let chan = &*(ctx as *const Chan);
    let out = &mut *chan.out;
    (out.write)(out.ctx, chan.channel, ptr, len)
}

macro_rules! text_channels {
    ($req:ident, $t0:ident, $t1:ident) => {
        // SAFETY: the channels live as long as the call and the writer outlives it
        let out_ptr = &mut $req.out as *mut ScriptQueryOut as *mut ScriptQueryOut<'static>;
        let mut chan0 = Chan { out: out_ptr, channel: CH_TEXT0 };
        let mut chan1 = Chan { out: out_ptr, channel: CH_TEXT1 };
        #[allow(unused_mut, unused_variables)]
        let mut $t0 = unsafe { ScriptTextOut::from_callback(&mut chan0 as *mut Chan as *mut u8, chan_write) };
        #[allow(unused_mut, unused_variables)]
        let mut $t1 = unsafe { ScriptTextOut::from_callback(&mut chan1 as *mut Chan as *mut u8, chan_write) };
    };
}

#[relax_api]
impl ScriptVmBase {
    /// Answers a question about the running threads and the heap by `req.kind` (QUERY_*): the
    /// formats, the loops and calls running now, the threads' footprints, the injected globals.
    /// The answer's text and records go to `req.out`, its scalars to the request. The result:
    /// whether there was an answer.
    #[relax_api]
    pub fn vm_query(&self, req: &mut ScriptQueryReq) -> bool {
        text_channels!(req, text0, text1);
        match req.kind {
            QUERY_FORMAT_ENUM_VARIANT => {
                text0.push(&crate::suggest::format_enum_variant_error(&self.heap, req.value));
                true
            }
            QUERY_FORMAT_OBJECT => {
                let mut text = String::new();
                let mut recur = Vec::new();
                // Use the heap's debug string but limit depth to keep it concise
                self.heap.to_debug_string(req.value, &mut recur, &mut text, false, 0);
                // Truncate if too long
                // count chars: a byte offset can land inside a multibyte char
                if text.chars().count() > 200 {
                    text = text.chars().take(197).collect();
                    text.push_str("...");
                }
                text0.push(&text);
                true
            }
            QUERY_LOOP_VALUES => {
                let thread = self.threads.cur_ref();
                let Some(&scope) = thread.scopes.last() else { return true };
                for frame in thread.loops.iter() {
                    let Some(values) = &frame.values else { continue };
                    for id in std::iter::once(values.value_id).chain(values.index_id).chain(values.key_id) {
                        if let Some(v) = self.heap.scope_value_opt(scope, id).and_then(|v| v.as_number()) {
                            req.out.record(CH_RECORD, &ScriptLoopValue { id, value: v });
                        }
                    }
                }
                true
            }
            QUERY_CALL_SITES => {
                let thread = self.threads.cur_ref();
                req.out.record(CH_RECORD, &thread.trap.ip);
                let mut count = 1;
                for frame in thread.calls.iter().rev() {
                    if count >= req.index {
                        break;
                    }
                    if let Some(ip) = frame.return_ip {
                        req.out.record(CH_RECORD, &ScriptIp { body: ip.body, index: ip.index.saturating_sub(1) });
                        count += 1;
                    }
                }
                true
            }
            QUERY_THREAD_FOOTPRINT => match self.threads.get(req.index) {
                Some(thread) => {
                    req.out.record(CH_RECORD, &thread.root_footprint());
                    req.flag = thread.is_paused();
                    true
                }
                None => false,
            },
            QUERY_INJECTED_GLOBAL => match self.injected_globals.get(&req.id) {
                Some(value) => {
                    req.value = *value;
                    true
                }
                None => false,
            },
            kind => panic!("vm_query: unknown kind {kind}"),
        }
    }
}

#[relax_api]
impl ScriptCode {
    /// Answers a question about the compiled code by `req.kind` (QUERY_*): where an instruction
    /// is written, a fn's text, a literal's docs, the manifests, the bodies and their scopes.
    /// The answer's text and records go to `req.out`, its scalars to the request. The result:
    /// whether there was an answer.
    #[relax_api]
    pub fn code_query(&self, req: &mut ScriptQueryReq) -> bool {
        text_channels!(req, text0, text1);
        match req.kind {
            QUERY_IP_TO_LOC => self.ip_to_loc_native(req.ip, &mut text0, &mut req.at),
            QUERY_FN_TEXT => {
                if req.flag {
                    self.fn_source_text_native(req.ip, &mut text0, &mut req.at, &mut text1)
                } else {
                    self.fn_text_native(req.ip, &mut text0, &mut req.at, &mut text1)
                }
            }
            QUERY_FIELD_LOC => self.field_loc_native(req.ip, req.id, &mut text0, &mut req.at),
            QUERY_IP_LINE_TEXT => self.ip_line_text_native(req.ip, &mut text0),
            QUERY_BODY_MODULE => {
                let bodies = self.bodies.borrow();
                match bodies.get(req.index).map(|b| &b.source) {
                    Some(ScriptSource::Mod(m)) => {
                        text0.push(&m.cargo_manifest_path);
                        text1.push(&m.module_path);
                        true
                    }
                    _ => false,
                }
            }
            QUERY_CRATE_MANIFEST => match self.crate_manifests.borrow().get(req.text) {
                Some(dir) => {
                    text0.push(dir);
                    true
                }
                None => false,
            },
            QUERY_CRATE_MANIFESTS => {
                for (name, dir) in self.crate_manifests.borrow().iter() {
                    text0.push(name);
                    text0.push("\t");
                    text0.push(dir);
                    text0.push("\n");
                }
                true
            }
            QUERY_SITE_DOCS => {
                for e in self.resolve_body_docs(req.ip.body) {
                    if e.begin_index != req.ip.index {
                        continue;
                    }
                    match e.field {
                        None => {
                            text0.push(&e.text);
                            req.flag = true;
                        }
                        Some(f) => {
                            req.out.record(CH_FIELD_ID, &f.0.to_le_bytes());
                            req.out.text(CH_FIELD_TEXT, &e.text);
                        }
                    }
                }
                true
            }
            QUERY_BODY_ROOT_IDS => {
                for b in self.bodies.borrow().iter() {
                    req.out.record(CH_RECORD, &b.scope.as_object());
                    req.out.record(CH_RECORD, &b.me.as_object());
                    if let Some(end) = &b.end_scope {
                        req.out.record(CH_RECORD, &end.as_object());
                    }
                }
                true
            }
            QUERY_BODY_BY_MODULE_PATH => {
                let bodies = self.bodies.borrow();
                match bodies.iter().position(|body| matches!(&body.source, ScriptSource::Mod(m) if m.module_path == req.text)) {
                    Some(i) => {
                        req.index = i;
                        true
                    }
                    None => false,
                }
            }
            QUERY_BODY_BY_FILE => {
                let bodies = self.bodies.borrow();
                let column = req.flag.then_some(req.index);
                match bodies.iter().position(|body| matches!(&body.source, ScriptSource::Mod(m) if m.file == req.text && column.is_none_or(|column| m.column == column))) {
                    Some(i) => {
                        req.index = i;
                        true
                    }
                    None => false,
                }
            }
            QUERY_BODY_SCOPES => {
                let bodies = self.bodies.borrow();
                match bodies.get(req.index) {
                    Some(b) => {
                        req.out.record(CH_RECORD, &b.scope.as_object());
                        req.out.record(CH_RECORD, &b.me.as_object());
                        req.out.record(CH_RECORD, &b.end_scope.as_ref().map_or(ScriptObject::ZERO, |s| s.as_object()));
                        true
                    }
                    None => false,
                }
            }
            QUERY_BODY_INITIAL_SCOPE => {
                let bodies = self.bodies.borrow();
                match bodies.get(req.index) {
                    Some(b) => {
                        req.value = b.scope.as_object().into();
                        req.flag = !b.scope.is_unrooted();
                        true
                    }
                    None => false,
                }
            }
            QUERY_BODY_END_SCOPE => {
                let bodies = self.bodies.borrow();
                match bodies.get(req.index) {
                    Some(b) => {
                        req.value = b.end_scope.as_ref().unwrap_or(&b.scope).as_object().into();
                        true
                    }
                    None => false,
                }
            }
            QUERY_END_SCOPE_OF => {
                let bodies = self.bodies.borrow();
                let found = bodies.iter().rev().find_map(|body| match &body.source {
                    ScriptSource::Mod(m) if m.file == req.text => Some(body.end_scope.as_ref().map(|s| s.as_object()).unwrap_or_else(|| body.scope.as_object())),
                    _ => None,
                });
                match found {
                    Some(scope) => {
                        req.value = scope.into();
                        true
                    }
                    None => false,
                }
            }
            kind => panic!("code_query: unknown kind {kind}"),
        }
    }
}
