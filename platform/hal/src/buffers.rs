//! The two buffers: [`CmdWriter`] (Cx side: builds the CmdBuf, reusable every frame without
//! reallocation) and [`EventQueue`] / [`EventReader`] (HAL side queue, Cx side reader).
//! Typed arrays that ride in the byte arena (`DragItem`, `MenuItem`, `TouchPoint`, pipeline
//! blobs) are encoded field by field through [`Wire`], so no padding byte is ever read.

use crate::codec::ArenaSink;
use crate::events::HalEvent;
use crate::types::*;
use std::collections::VecDeque;

// ------------------------------------------------------------------ wire encoding

/// Little-endian field encoding of the small records that travel in the arena.
pub trait Wire: Sized + 'static {
    const SIZE: usize;
    fn write(&self, out: &mut Vec<u8>);
    fn read(b: &[u8]) -> Self;
}

fn w32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn w64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn r32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn r64(b: &[u8], at: usize) -> u64 {
    r32(b, at) as u64 | (r32(b, at + 4) as u64) << 32
}

impl Wire for DragItem {
    const SIZE: usize = 8;
    fn write(&self, out: &mut Vec<u8>) {
        w32(out, self.kind);
        w32(out, self.str);
    }
    fn read(b: &[u8]) -> Self {
        DragItem { kind: r32(b, 0), str: r32(b, 4) }
    }
}

impl Wire for MenuItem {
    const SIZE: usize = 24;
    fn write(&self, out: &mut Vec<u8>) {
        w32(out, self.parent);
        w32(out, self.kind);
        w64(out, self.cmd);
        w32(out, self.key);
        w32(out, self.str);
    }
    fn read(b: &[u8]) -> Self {
        MenuItem { parent: r32(b, 0), kind: r32(b, 4), cmd: r64(b, 8), key: r32(b, 16), str: r32(b, 20) }
    }
}

impl Wire for TouchPoint {
    const SIZE: usize = 20;
    fn write(&self, out: &mut Vec<u8>) {
        w32(out, self.id);
        w32(out, self.phase);
        w32(out, self.x.to_bits());
        w32(out, self.y.to_bits());
        w32(out, self.force.to_bits());
    }
    fn read(b: &[u8]) -> Self {
        TouchPoint { id: r32(b, 0), phase: r32(b, 4), x: f32::from_bits(r32(b, 8)), y: f32::from_bits(r32(b, 12)), force: f32::from_bits(r32(b, 16)) }
    }
}

impl Wire for [u32; 2] {
    const SIZE: usize = 8;
    fn write(&self, out: &mut Vec<u8>) {
        w32(out, self[0]);
        w32(out, self[1]);
    }
    fn read(b: &[u8]) -> Self {
        [r32(b, 0), r32(b, 4)]
    }
}

impl Wire for u32 {
    const SIZE: usize = 4;
    fn write(&self, out: &mut Vec<u8>) {
        w32(out, *self);
    }
    fn read(b: &[u8]) -> Self {
        r32(b, 0)
    }
}

impl Wire for PipelineDesc {
    const SIZE: usize = 72;
    fn write(&self, out: &mut Vec<u8>) {
        for r in [self.code, self.bindings, self.vertex_layout] {
            w32(out, r.off);
            w32(out, r.len);
        }
        for f in self.color_formats {
            w32(out, f);
        }
        for v in [self.blend, self.depth, self.shader, self.name, self.source, self.instance_stride, self.varyings, self.outputs] {
            w32(out, v);
        }
    }
    fn read(b: &[u8]) -> Self {
        let rg = |at| Range { off: r32(b, at), len: r32(b, at + 4) };
        PipelineDesc {
            code: rg(0),
            bindings: rg(8),
            vertex_layout: rg(16),
            color_formats: [r32(b, 24), r32(b, 28), r32(b, 32), r32(b, 36)],
            blend: r32(b, 40),
            depth: r32(b, 44),
            shader: r32(b, 48),
            name: r32(b, 52),
            source: r32(b, 56),
            instance_stride: r32(b, 60),
            varyings: r32(b, 64),
            outputs: r32(b, 68),
        }
    }
}

/// The items a range of the arena holds (`count` records); a truncated range yields fewer.
pub fn read_items<T: Wire>(bytes: &[u8], count: u32) -> impl Iterator<Item = T> + '_ {
    bytes.chunks_exact(T::SIZE).take(count as usize).map(T::read)
}

/// The strings a range holds (`count` of u32 length + utf8 bytes); a damaged tail ends the list.
pub fn read_strs(bytes: &[u8], count: u32) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for _ in 0..count {
        let Some(head) = bytes.get(at..at + 4) else { break };
        let len = r32(head, 0) as usize;
        let Some(s) = bytes.get(at + 4..at + 4 + len) else { break };
        out.push(String::from_utf8_lossy(s).into_owned());
        at += 4 + len;
    }
    out
}

/// The pairs an element range of the arena names (see [`CmdWriter::put_pairs`]); a range that
/// leaves the arena yields none.
pub fn read_pairs(arena: &[u8], r: Range) -> impl Iterator<Item = [u32; 2]> + '_ {
    let (a, b) = (r.off as usize * 8, (r.off as usize).saturating_add(r.len as usize).saturating_mul(8));
    arena.get(a..b).unwrap_or(&[]).chunks_exact(8).map(<[u32; 2]>::read)
}

/// The ids an element range of the arena names (see [`CmdWriter::put_ids`]); a range that leaves
/// the arena yields none.
pub fn read_ids(arena: &[u8], r: Range) -> impl Iterator<Item = Id> + '_ {
    read_pairs(arena, r).map(|[index, gen]| Id { index, gen })
}

/// Element `i` of an id range (see [`CmdWriter::put_ids`]); [`Id::NONE`] past its end.
pub fn id_at(arena: &[u8], r: Range, i: usize) -> Id {
    if i >= r.len as usize {
        return Id::NONE;
    }
    let at = (r.off as usize).saturating_add(i).saturating_mul(8);
    match arena.get(at..at + 8) {
        Some(b) => {
            let [index, gen] = <[u32; 2]>::read(b);
            Id { index, gen }
        }
        None => Id::NONE,
    }
}

/// The payload of an event that carries dragged items: the `DragItem`s, then each item's string
/// record, `str` being the record's offset in the payload.
pub fn pack_drag_items(items: &[(u32, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = items.len() * DragItem::SIZE;
    for (kind, s) in items {
        DragItem { kind: *kind, str: at as u32 }.write(&mut out);
        at += 4 + s.len();
    }
    for (_, s) in items {
        w32(&mut out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }
    out
}

/// (kind, string) of the `count` items of an event payload; a damaged payload ends the list.
pub fn read_drag_items(payload: &[u8], count: u32) -> Vec<(u32, String)> {
    read_items::<DragItem>(payload, count)
        .map_while(|i| {
            let at = i.str as usize;
            let len = r32(payload.get(at..at + 4)?, 0) as usize;
            let s = payload.get(at + 4..at + 4 + len)?;
            Some((i.kind, String::from_utf8_lossy(s).into_owned()))
        })
        .collect()
}

/// Strings as a payload (u32 length + bytes each); read back with [`read_strs`].
pub fn pack_strs(strs: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in strs {
        w32(&mut out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    }
    out
}

/// A `Forward` payload: the event field by field (tag, window, time, floats, words), then its
/// own payload bytes; `data` is rewritten by the HAL to name them in the child's arena.
pub fn pack_forward(e: &RawEvent, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FORWARD_HEAD + payload.len());
    w32(&mut out, e.tag as u32);
    w32(&mut out, e.window.index);
    w32(&mut out, e.window.gen);
    w64(&mut out, e.time_ns);
    for f in e.a {
        w64(&mut out, f.to_bits());
    }
    for b in e.b {
        w64(&mut out, b);
    }
    out.extend_from_slice(payload);
    out
}

const FORWARD_HEAD: usize = 4 + 8 + 8 + 32 + 24;

/// The event and payload of a `Forward` command; `None` when it is shorter than its header.
/// The returned event has `window` and `data` unset (the HAL fills them for the child).
pub fn unpack_forward(b: &[u8]) -> Option<(RawEvent, &[u8])> {
    if b.len() < FORWARD_HEAD {
        return None;
    }
    let mut a = [0.0f64; 4];
    for (i, f) in a.iter_mut().enumerate() {
        *f = f64::from_bits(r64(b, 20 + 8 * i));
    }
    let words = [r64(b, 52), r64(b, 60), r64(b, 68)];
    let e = RawEvent { tag: r32(b, 0) as u16, window: Id::NONE, time_ns: r64(b, 12), a, b: words, data: Range::default() };
    Some((e, &b[FORWARD_HEAD..]))
}

/// One record of a `DomainListed` payload.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainInfo {
    pub domain: u32,
    pub parent: u32,
    pub state: u32,
    pub phase: u32,
    pub pct: f32,
    pub app: String,
}

/// The records of a `DomainListed` event: five words and the app string each.
pub fn pack_domains(list: &[DomainInfo]) -> Vec<u8> {
    let mut out = Vec::new();
    for d in list {
        for v in [d.domain, d.parent, d.state, d.phase, d.pct.to_bits(), d.app.len() as u32] {
            w32(&mut out, v);
        }
        out.extend_from_slice(d.app.as_bytes());
    }
    out
}

/// The `count` records of a `DomainListed` payload; a damaged tail ends the list.
pub fn read_domains(b: &[u8], count: u32) -> Vec<DomainInfo> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for _ in 0..count {
        let Some(head) = b.get(at..at + 24) else { break };
        let len = r32(head, 20) as usize;
        let Some(app) = b.get(at + 24..at + 24 + len) else { break };
        out.push(DomainInfo {
            domain: r32(head, 0),
            parent: r32(head, 4),
            state: r32(head, 8),
            phase: r32(head, 12),
            pct: f32::from_bits(r32(head, 16)),
            app: String::from_utf8_lossy(app).into_owned(),
        });
        at += 24 + len;
    }
    out
}

/// A parsed `PipelineCreate` blob.
pub struct PipelineBlob<'a> {
    pub desc: PipelineDesc,
    pub code: &'a [u8],
    pub bindings: Vec<u32>,
    pub vertex_layout: Vec<u32>,
}

impl<'a> PipelineBlob<'a> {
    /// `None` when the blob is shorter than its header or a range leaves it.
    pub fn parse(blob: &'a [u8]) -> Option<PipelineBlob<'a>> {
        if blob.len() < PipelineDesc::SIZE {
            return None;
        }
        let desc = PipelineDesc::read(blob);
        let part = |r: Range| {
            let a = r.off as usize;
            blob.get(a..a.checked_add(r.len as usize)?)
        };
        let words = |r: Range| part(r).map(|b| b.chunks_exact(4).map(|c| r32(c, 0)).collect::<Vec<u32>>());
        Some(PipelineBlob { desc, code: part(desc.code)?, bindings: words(desc.bindings)?, vertex_layout: words(desc.vertex_layout)? })
    }
}

// ------------------------------------------------------------------ CmdWriter

/// Cx's builder of one loop turn's CmdBuf. Every vector keeps its capacity across [`clear`], so a
/// steady-state frame allocates nothing.
///
/// [`clear`]: CmdWriter::clear
#[derive(Default)]
pub struct CmdWriter {
    session: u32,
    cmds: Vec<Cmd>,
    arena: Vec<u8>,
    frames: Vec<FramePacket>,
    passes: Vec<Pass>,
    draws: Vec<Draw>,
    open_frame: Option<OpenFrame>,
}

struct OpenFrame {
    frame: u64,
    meta: FrameMeta,
    pass0: u32,
    draw0: u32,
    pass_draw0: Option<u32>,
}

impl CmdWriter {
    pub fn new(session: u32) -> CmdWriter {
        CmdWriter { session, ..Default::default() }
    }

    pub fn session(&self) -> u32 {
        self.session
    }

    pub fn push(&mut self, cmd: Cmd) {
        self.cmds.push(cmd);
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    pub fn cmds(&self) -> &[Cmd] {
        &self.cmds
    }

    pub fn arena(&self) -> &[u8] {
        &self.arena
    }

    /// Copies a typed record array into the arena.
    pub fn put_items<T: Wire>(&mut self, items: &[T]) -> Range {
        let off = self.arena.len();
        for i in items {
            i.write(&mut self.arena);
        }
        self.range_from(off)
    }

    /// Copies (first, count) pairs into the arena at an 8-aligned offset and returns their
    /// ELEMENT range (offset and length in pairs), as `Draw::inst_ranges` names them.
    pub fn put_pairs(&mut self, pairs: &[[u32; 2]]) -> Range {
        while self.arena.len() % 8 != 0 {
            self.arena.push(0);
        }
        let off = (self.arena.len() / 8) as u32;
        for p in pairs {
            p.write(&mut self.arena);
        }
        Range { off, len: pairs.len() as u32 }
    }

    /// Copies ids into the arena as (index, gen) pairs and returns their ELEMENT range, as
    /// `Draw::tex` / `Draw::pick_tex` name them.
    pub fn put_ids(&mut self, ids: &[Id]) -> Range {
        while self.arena.len() % 8 != 0 {
            self.arena.push(0);
        }
        let off = (self.arena.len() / 8) as u32;
        for id in ids {
            [id.index, id.gen].write(&mut self.arena);
        }
        Range { off, len: ids.len() as u32 }
    }

    /// Copies strings (u32 length + bytes each) into the arena; read back with [`read_strs`].
    pub fn put_strs(&mut self, strs: &[&str]) -> Range {
        let off = self.arena.len();
        for s in strs {
            w32(&mut self.arena, s.len() as u32);
            self.arena.extend_from_slice(s.as_bytes());
        }
        self.range_from(off)
    }

    /// Builds the `PipelineCreate` blob (header, code, bindings, vertex layout) into the arena.
    /// The ranges in `desc` are filled in; the returned range is what `pipeline_create` takes as
    /// `desc` (use [`CmdWriter::pipeline_create`] with the blob's bytes instead of calling this
    /// directly).
    pub fn pipeline_blob(desc: PipelineDesc, code: &[u8], bindings: &[u32], vertex_layout: &[u32]) -> Vec<u8> {
        let mut d = desc;
        let mut at = PipelineDesc::SIZE as u32;
        d.code = Range { off: at, len: code.len() as u32 };
        at += code.len() as u32;
        d.bindings = Range { off: at, len: bindings.len() as u32 * 4 };
        at += bindings.len() as u32 * 4;
        d.vertex_layout = Range { off: at, len: vertex_layout.len() as u32 * 4 };
        let mut out = Vec::with_capacity(at as usize + vertex_layout.len() * 4);
        d.write(&mut out);
        out.extend_from_slice(code);
        bindings.iter().for_each(|v| w32(&mut out, *v));
        vertex_layout.iter().for_each(|v| w32(&mut out, *v));
        out
    }

    fn range_from(&self, off: usize) -> Range {
        Range { off: off as u32, len: (self.arena.len() - off) as u32 }
    }

    // ---- frames

    /// Starts a FramePacket; passes and draws follow, then [`CmdWriter::frame_end`].
    pub fn frame_begin(&mut self, frame: u64, meta: FrameMeta) {
        assert!(self.open_frame.is_none(), "CmdWriter: frame_begin inside a frame");
        self.open_frame = Some(OpenFrame { frame, meta, pass0: self.passes.len() as u32, draw0: self.draws.len() as u32, pass_draw0: None });
    }

    /// Stamps the open frame's `build_end_ns` (the builder knows it only after the walk).
    pub fn frame_set_build_end(&mut self, ns: u64) {
        if let Some(f) = self.open_frame.as_mut() {
            f.meta.build_end_ns = ns;
        }
    }

    fn close_pass(&mut self) {
        let f = self.open_frame.as_mut().expect("CmdWriter: no open frame");
        if let Some(d0) = f.pass_draw0.take() {
            let last = self.passes.last_mut().expect("open pass");
            last.draws = Range { off: d0, len: self.draws.len() as u32 - d0 };
        }
    }

    /// Starts a pass (its `draws` range is filled in by the writer); the draws that follow belong to it.
    pub fn pass(&mut self, pass: Pass) {
        self.close_pass();
        let d0 = self.draws.len() as u32;
        self.open_frame.as_mut().expect("CmdWriter: pass outside a frame").pass_draw0 = Some(d0);
        self.passes.push(pass);
    }

    pub fn draw(&mut self, draw: Draw) {
        assert!(self.open_frame.as_ref().is_some_and(|f| f.pass_draw0.is_some()), "CmdWriter: draw outside a pass");
        self.draws.push(draw);
    }

    /// Closes the packet and writes the `Frame` command that renders it to `target`
    /// (`target::WINDOW` / `target::TEXTURE`).
    pub fn frame_end(&mut self, target_id: Id, target_kind: u32) {
        self.close_pass();
        let f = self.open_frame.take().expect("CmdWriter: frame_end without frame_begin");
        let packet = self.frames.len() as u32;
        self.frames.push(FramePacket {
            frame: f.frame,
            passes: Range { off: f.pass0, len: self.passes.len() as u32 - f.pass0 },
            draws: Range { off: f.draw0, len: self.draws.len() as u32 - f.draw0 },
            meta: f.meta,
        });
        self.push(Cmd::frame(target_id, target_kind, packet));
    }

    /// The CmdBuf over this writer's memory: valid until the writer is next changed; pass it to
    /// `hal_submit` and [`clear`](CmdWriter::clear).
    pub fn buf(&self) -> CmdBuf {
        assert!(self.open_frame.is_none(), "CmdWriter: buf inside a frame");
        CmdBuf {
            session: self.session,
            cmds: Slice::of(&self.cmds),
            arena: Slice::from_bytes(&self.arena),
            frames: Slice::of(&self.frames),
            passes: Slice::of(&self.passes),
            draws: Slice::of(&self.draws),
        }
    }

    /// Empties the writer for the next turn, keeping every allocation.
    pub fn clear(&mut self) {
        self.cmds.clear();
        self.arena.clear();
        self.frames.clear();
        self.passes.clear();
        self.draws.clear();
        self.open_frame = None;
    }

    /// Capacities (cmds, arena bytes, frames, passes, draws): tests use them to show a steady
    /// state does not grow.
    pub fn capacities(&self) -> [usize; 5] {
        [self.cmds.capacity(), self.arena.capacity(), self.frames.capacity(), self.passes.capacity(), self.draws.capacity()]
    }
}

impl ArenaSink for CmdWriter {
    fn put_bytes(&mut self, bytes: &[u8]) -> Range {
        let off = self.arena.len();
        self.arena.extend_from_slice(bytes);
        self.range_from(off)
    }
}

/// The slices of a CmdBuf read as typed arrays, valid for the `hal_submit` call it came from.
pub struct CmdView<'a> {
    pub session: u32,
    pub cmds: &'a [Cmd],
    pub arena: &'a [u8],
    pub frames: &'a [FramePacket],
    pub passes: &'a [Pass],
    pub draws: &'a [Draw],
}

impl CmdBuf {
    /// Reads the borrowed slices; `None` when one is misaligned or not a whole number of
    /// elements (the batch is then rejected as a whole).
    ///
    /// # Safety
    /// The slices must still be alive: call only inside the `hal_submit` call that received `self`.
    pub unsafe fn view(&self) -> Option<CmdView<'_>> {
        Some(CmdView {
            session: self.session,
            cmds: self.cmds.as_slice()?,
            arena: self.arena.as_bytes(),
            frames: self.frames.as_slice()?,
            passes: self.passes.as_slice()?,
            draws: self.draws.as_slice()?,
        })
    }
}

// ------------------------------------------------------------------ events

/// The HAL's pending events for one session, plus the arena their payloads live in.
#[derive(Default)]
pub struct EventQueue {
    events: VecDeque<RawEvent>,
    arena: Vec<u8>,
    now_ns: u64,
    /// events delivered with a payload cut because the caller's arena was too small
    pub truncated: u64,
}

impl EventQueue {
    pub fn new() -> EventQueue {
        EventQueue::default()
    }

    /// The time stamped on events pushed from now on (events built with a time keep it).
    pub fn set_time(&mut self, ns: u64) {
        self.now_ns = ns;
    }

    pub fn push(&mut self, mut e: RawEvent) {
        if e.time_ns == 0 {
            e.time_ns = self.now_ns;
        }
        if e.tag == crate::events::event_tag::ChildFrame {
            // Append the replacement after intervening surface announcements, so a resize's
            // new texture is known before the parent hears that it presented.
            if let Some(HalEvent::ChildFrame { child, window, .. }) = e.decode(&[]) {
                if let Some(at) = self.events.iter().position(|old| {
                    old.tag == e.tag && matches!(old.decode(&[]), Some(HalEvent::ChildFrame { child: c, window: w, .. }) if c == child && w == window)
                }) {
                    self.events.remove(at);
                }
            }
        }
        self.events.push_back(e);
    }

    /// Pushes `e` with `payload` copied into this queue's arena (an event moved from another
    /// queue, whose payload range pointed into that queue's arena).
    pub fn push_with_payload(&mut self, mut e: RawEvent, payload: &[u8]) {
        let off = self.arena.len();
        self.arena.extend_from_slice(payload);
        e.data.off = off as u32;
        e.data.len = payload.len() as u32;
        self.push(e);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Deliver native notifications through the world's ownership routing, retaining
    /// their original stamps and payloads (including text and composition input).
    pub fn drain_with(&mut self, mut deliver: impl FnMut(RawEvent, &[u8])) {
        for event in self.events.drain(..) {
            let start = event.data.off as usize;
            let payload = start.checked_add(event.data.len as usize)
                .and_then(|end| self.arena.get(start..end)).unwrap_or(&[]);
            deliver(event, payload);
        }
        self.arena.clear();
    }

    /// Moves every event of `other` to the end of this queue (payloads follow into this arena).
    /// Events keep the time they were stamped with.
    pub fn append(&mut self, other: &mut EventQueue) {
        let base = self.arena.len() as u32;
        self.arena.extend_from_slice(&other.arena);
        for mut e in other.events.drain(..) {
            if e.data.len > 0 {
                e.data.off += base;
            }
            self.events.push_back(e);
        }
        other.arena.clear();
    }

    /// Moves queued events into the caller's buffer while they fit (events and arena); returns
    /// how many were delivered. An event whose payload alone exceeds an empty arena is delivered
    /// with the payload cut (counted in `truncated`), never stuck.
    pub fn drain_into(&mut self, out: &mut EventBuf) -> u32 {
        let mut n = 0u32;
        let mut used = 0usize;
        while n < out.cap {
            let Some(e) = self.events.front() else { break };
            let a = e.data.off as usize;
            let payload = a.checked_add(e.data.len as usize).and_then(|end| self.arena.get(a..end)).unwrap_or(&[]);
            let mut len = payload.len();
            if used + len > out.arena_cap as usize {
                if n > 0 {
                    break;
                }
                len = out.arena_cap as usize;
                self.truncated += 1;
            }
            let mut e = *e;
            // SAFETY: the caller's EventBuf names `cap` events and `arena_cap` bytes it owns
            // for this call (hal_pump contract); n < cap and used + len <= arena_cap
            unsafe {
                if len > 0 {
                    std::ptr::copy_nonoverlapping(payload.as_ptr(), out.arena.add(used), len);
                }
                e.data = Range { off: used as u32, len: len as u32 };
                out.events.add(n as usize).write(e);
            }
            used += len;
            n += 1;
            self.events.pop_front();
        }
        out.arena_len = used as u32;
        if self.events.is_empty() {
            self.arena.clear();
        }
        n
    }
}

impl ArenaSink for EventQueue {
    fn put_bytes(&mut self, bytes: &[u8]) -> Range {
        let off = self.arena.len();
        self.arena.extend_from_slice(bytes);
        Range { off: off as u32, len: bytes.len() as u32 }
    }
}

/// Memory for `hal_pump` to write events into, kept by Cx between turns.
pub struct EventStore {
    events: Vec<RawEvent>,
    arena: Vec<u8>,
    session: u32,
}

impl EventStore {
    pub fn new(session: u32, cap: usize, arena_cap: usize) -> EventStore {
        EventStore { events: vec![RawEvent::default(); cap], arena: vec![0; arena_cap], session }
    }

    /// The EventBuf over this store (borrowed: valid while the store is not moved or changed).
    pub fn buf(&mut self) -> EventBuf {
        EventBuf {
            session: self.session,
            events: self.events.as_mut_ptr(),
            cap: self.events.len() as u32,
            arena: self.arena.as_mut_ptr(),
            arena_cap: self.arena.len() as u32,
            arena_len: 0,
        }
    }

    /// The first `n` events `hal_pump` delivered, to decode.
    pub fn reader(&self, n: u32, arena_len: u32) -> EventReader<'_> {
        EventReader { events: &self.events[..(n as usize).min(self.events.len())], arena: &self.arena[..(arena_len as usize).min(self.arena.len())], at: 0 }
    }
}

/// Iterates the delivered events as [`HalEvent`]s; events with an unknown tag are skipped and counted.
pub struct EventReader<'a> {
    events: &'a [RawEvent],
    arena: &'a [u8],
    at: usize,
}

impl<'a> EventReader<'a> {
    pub fn new(events: &'a [RawEvent], arena: &'a [u8]) -> EventReader<'a> {
        EventReader { events, arena, at: 0 }
    }
}

impl<'a> Iterator for EventReader<'a> {
    /// (time_ns, event)
    type Item = (u64, HalEvent<'a>);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some(e) = self.events.get(self.at) {
            self.at += 1;
            if let Some(d) = e.decode(self.arena) {
                return Some((e.time_ns, d));
            }
        }
        None
    }
}
