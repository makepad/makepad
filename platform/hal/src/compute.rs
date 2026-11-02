//! Plain compute ABI and checked decoding shared by every native backend.
use crate::{buffers::{CmdWriter, PipelineBlob, Wire}, types::*};

fn word(b: &[u8], i: usize) -> u32 { u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) }
fn range(b: &[u8], i: usize) -> Range { Range { off: word(b, i), len: word(b, i + 4) } }
fn id(b: &[u8], i: usize) -> Id { Id::new(word(b, i), word(b, i + 4)) }
fn words(out: &mut Vec<u8>, values: &[u32]) { for v in values { out.extend_from_slice(&v.to_le_bytes()); } }

impl Wire for StorageBinding {
    const SIZE: usize = 12;
    fn write(&self, out: &mut Vec<u8>) { words(out, &[self.index, self.offset, self.writable]); }
    fn read(b: &[u8]) -> Self { Self { index: word(b, 0), offset: word(b, 4), writable: word(b, 8) } }
}
impl Wire for StorageImageBinding {
    const SIZE: usize = 8;
    fn write(&self, out: &mut Vec<u8>) { words(out, &[self.index, self.format]); }
    fn read(b: &[u8]) -> Self { Self { index: word(b, 0), format: word(b, 4) } }
}
impl Wire for ComputeDesc {
    const SIZE: usize = 48;
    fn write(&self, out: &mut Vec<u8>) {
        words(out, &[self.buffers.off, self.buffers.len, self.textures.off, self.textures.len,
            self.push_size, self.param_binding, self.param_binding_size, self.group_count_offset,
            self.workgroup[0], self.workgroup[1], self.workgroup[2], self.precise_math]);
    }
    fn read(b: &[u8]) -> Self {
        Self { buffers: range(b, 0), textures: range(b, 8), push_size: word(b, 16), param_binding: word(b, 20),
            param_binding_size: word(b, 24), group_count_offset: word(b, 28),
            workgroup: [word(b, 32), word(b, 36), word(b, 40)], precise_math: word(b, 44) }
    }
}
impl Wire for Dispatch {
    const SIZE: usize = 60;
    fn write(&self, out: &mut Vec<u8>) {
        words(out, &[self.pipeline.index, self.pipeline.gen, self.groups[0], self.groups[1], self.groups[2],
            self.indirect_buffer.index, self.indirect_buffer.gen]);
        out.extend_from_slice(&self.indirect_offset.to_le_bytes());
        words(out, &[self.params.off, self.params.len, self.buffers.off, self.buffers.len, self.textures.off, self.textures.len]);
    }
    fn read(b: &[u8]) -> Self {
        Self { pipeline: id(b, 0), groups: [word(b, 8), word(b, 12), word(b, 16)], indirect_buffer: id(b, 20),
            indirect_offset: u64::from_le_bytes(b[28..36].try_into().unwrap()), params: range(b, 36), buffers: range(b, 44), textures: range(b, 52) }
    }
}

pub fn bytes(arena: &[u8], r: Range) -> Option<&[u8]> {
    let start = r.off as usize;
    arena.get(start..start.checked_add(r.len as usize)?)
}
pub fn records<T: Wire>(arena: &[u8], r: Range) -> Option<Vec<T>> {
    let b = bytes(arena, r)?;
    if b.len() % T::SIZE != 0 { return None; }
    Some(b.chunks_exact(T::SIZE).map(T::read).collect())
}
pub fn ids(arena: &[u8], r: Range) -> Option<Vec<Id>> {
    let b = bytes(arena, r)?;
    if b.len() % 8 != 0 { return None; }
    Some(b.chunks_exact(8).map(|b| id(b, 0)).collect())
}

#[derive(Clone, Debug)]
pub struct KernelLayout {
    pub desc: ComputeDesc,
    pub buffers: Vec<StorageBinding>,
    pub textures: Vec<StorageImageBinding>,
}
impl KernelLayout {
    pub fn parse(blob: &PipelineBlob<'_>) -> Result<Self, String> {
        let b = bytes(blob.raw, blob.desc.compute).filter(|b| b.len() == ComputeDesc::SIZE).ok_or("invalid compute descriptor")?;
        let desc = ComputeDesc::read(b);
        if desc.push_size > 128 || desc.param_binding_size < desc.push_size || desc.param_binding_size > 128
            || desc.workgroup.contains(&0) || (desc.group_count_offset != u32::MAX && desc.group_count_offset.checked_add(12).is_none_or(|end| end > desc.push_size)) {
            return Err("invalid compute parameter/workgroup layout".into());
        }
        let buffers: Vec<StorageBinding> = records(blob.raw, desc.buffers).ok_or("invalid compute buffers")?;
        let textures: Vec<StorageImageBinding> = records(blob.raw, desc.textures).ok_or("invalid storage textures")?;
        if buffers.iter().any(|b| b.writable > 1 || b.offset % 8 != 0 || b.offset.checked_add(8).is_none_or(|end| end > desc.push_size)) {
            return Err("invalid compute buffer binding".into());
        }
        Ok(Self { desc, buffers, textures })
    }
}

impl KernelLayout {
    pub fn validate_device(&self, info: &HalInfo) -> Result<(), String> {
        if info.flags & info_flags::COMPUTE == 0 { return Err("backend does not support compute".into()); }
        let mut invocations = 1u64;
        for (size, limit) in self.desc.workgroup.iter().zip(info.max_workgroup_size) {
            if *size > limit { return Err("workgroup exceeds device dimension".into()); }
            invocations = invocations.checked_mul(*size as u64).ok_or("workgroup overflow")?;
        }
        if invocations > info.max_workgroup_invocations as u64 { return Err("workgroup exceeds device invocation limit".into()); }
        for t in &self.textures {
            if t.format >= 64 || info.storage_formats & (1u64 << t.format) == 0 { return Err("unsupported storage texture format".into()); }
        }
        Ok(())
    }
    pub(crate) fn validate_dispatch(&self, s: &crate::session::Session, d: &Dispatch, arena: &[u8]) -> Result<(), String> {
        if bytes(arena, d.params).is_none_or(|b| b.len() != self.desc.push_size as usize) { return Err("invalid compute parameters".into()); }
        let buffers = ids(arena, d.buffers).ok_or("invalid buffer list")?;
        let textures = ids(arena, d.textures).ok_or("invalid texture list")?;
        if buffers.len() != self.buffers.len() || textures.len() != self.textures.len() { return Err("compute binding count mismatch".into()); }
        for id in buffers {
            if !s.buffers.get(s.id, id).is_ok_and(|b| b.val.kind & buffer_kind::KIND_MASK == buffer_kind::STORAGE) { return Err("invalid storage buffer".into()); }
        }
        for (id, binding) in textures.into_iter().zip(&self.textures) {
            if !s.textures.get(s.id, id).is_ok_and(|t| t.val.format == binding.format && t.val.usage & usage::STORAGE != 0 && t.val.depth == 1) { return Err("invalid storage texture".into()); }
        }
        validate_indirect(s, d.indirect_buffer, d.indirect_offset, 12)
    }
}
pub(crate) fn validate_indirect(s: &crate::session::Session, id: Id, offset: u64, len: u64) -> Result<(), String> {
    if id.is_none() { return Ok(()); }
    if !s.buffers.get(s.id, id).is_ok_and(|b| b.val.kind & buffer_kind::INDIRECT != 0 && offset % 4 == 0 && offset.checked_add(len).is_some_and(|n| n <= b.val.size)) { return Err("invalid indirect buffer/range".into()); }
    Ok(())
}
pub(crate) fn validate_draw(s: &crate::session::Session, d: &Draw, arena: &[u8], storage: &[StorageBinding]) -> Result<(), String> {
    if d.storage_buffers.len as usize != storage.len() { return Err("draw storage count mismatch".into()); }
    for i in 0..storage.len() {
        let id = crate::buffers::id_at(arena, d.storage_buffers, i);
        if !s.buffers.get(s.id, id).is_ok_and(|b| b.val.kind & buffer_kind::KIND_MASK == buffer_kind::STORAGE) { return Err("invalid draw storage buffer".into()); }
    }
    validate_indirect(s, d.indirect_buffer, d.indirect_offset, 20)
}

fn append<T: Wire>(out: &mut Vec<u8>, records: &[T]) -> Range {
    let off = out.len() as u32;
    for record in records { record.write(out); }
    Range { off, len: out.len() as u32 - off }
}
impl CmdWriter {
    pub fn compute_pipeline_blob(mut pipeline: PipelineDesc, code: &[u8], mut desc: ComputeDesc,
        buffers: &[StorageBinding], textures: &[StorageImageBinding]) -> Vec<u8> {
        let mut out = Self::pipeline_blob(pipeline, code, &[], &[]);
        pipeline = PipelineDesc::read(&out);
        desc.buffers = append(&mut out, buffers);
        desc.textures = append(&mut out, textures);
        pipeline.compute = append(&mut out, &[desc]);
        let mut header = Vec::new(); pipeline.write(&mut header);
        out[..header.len()].copy_from_slice(&header);
        out
    }
    pub fn draw_pipeline_blob(desc: PipelineDesc, code: &[u8], bindings: &[u32], vertex: &[u32], storage: &[StorageBinding]) -> Vec<u8> {
        let mut out = Self::pipeline_blob(desc, code, bindings, vertex);
        let mut desc = PipelineDesc::read(&out);
        desc.storage_buffers = append(&mut out, storage);
        let mut header = Vec::new(); desc.write(&mut header);
        out[..header.len()].copy_from_slice(&header);
        out
    }
    pub fn compute_pass(&mut self, name: StrId, owner: StrId, dispatches: &[Dispatch]) {
        let dispatches = self.put_items(dispatches);
        self.pass(Pass { kind: pass_kind::COMPUTE, dispatches, name, owner, ..Default::default() });
    }
}

/// Unsupported backends report once per process, then skip safely.
pub(crate) fn unsupported() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("[hal] compute and draw storage buffers are unsupported on this backend; work skipped"));
}
