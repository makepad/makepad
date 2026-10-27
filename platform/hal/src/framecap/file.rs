//! The `.mkcap` file: one [`Capture`], little-endian, `MKFC`, a version, then the fields in the
//! order of the structs (lengths before every list and byte string). A file written by a newer
//! version is refused with its version named; the format has no optional parts.

use super::*;

const MAGIC: &[u8; 4] = b"MKFC";
const VERSION: u32 = 1;

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
    }
    fn opt(&mut self, v: &Option<Vec<u8>>) {
        match v {
            Some(b) => {
                self.u8(1);
                self.bytes(b);
            }
            None => self.u8(0),
        }
    }
    fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    fn id(&mut self, v: Id) {
        self.u32(v.index);
        self.u32(v.gen);
    }
    fn range(&mut self, r: Range) {
        self.u32(r.off);
        self.u32(r.len);
    }
    fn f4(&mut self, v: [f32; 4]) {
        v.into_iter().for_each(|x| self.f32(x));
    }
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

type Res<T> = Result<T, String>;

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Res<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or_else(|| format!("the capture file ends early (at byte {})", self.at))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Res<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Res<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Res<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Res<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Res<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn len(&mut self) -> Res<usize> {
        let n = self.u64()?;
        usize::try_from(n).ok().filter(|n| *n <= self.b.len() - self.at.min(self.b.len())).ok_or_else(|| format!("a length {n} past the end of the file"))
    }
    fn bytes(&mut self) -> Res<Vec<u8>> {
        let n = self.len()?;
        Ok(self.take(n)?.to_vec())
    }
    fn opt(&mut self) -> Res<Option<Vec<u8>>> {
        Ok(if self.u8()? != 0 { Some(self.bytes()?) } else { None })
    }
    fn str(&mut self) -> Res<String> {
        String::from_utf8(self.bytes()?).map_err(|_| "a string is not utf8".to_string())
    }
    fn id(&mut self) -> Res<Id> {
        Ok(Id { index: self.u32()?, gen: self.u32()? })
    }
    fn range(&mut self) -> Res<Range> {
        Ok(Range { off: self.u32()?, len: self.u32()? })
    }
    fn f4(&mut self) -> Res<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    /// A list length, bounded by what the rest of the file could hold.
    fn count(&mut self) -> Res<usize> {
        self.len()
    }
}

fn desc_w(w: &mut W, d: &PipelineDesc) {
    w.range(d.code);
    w.range(d.bindings);
    w.range(d.vertex_layout);
    d.color_formats.into_iter().for_each(|f| w.u32(f));
    for v in [d.blend, d.depth, d.shader, d.name, d.source, d.instance_stride, d.varyings, d.outputs] {
        w.u32(v);
    }
}

fn desc_r(r: &mut R) -> Res<PipelineDesc> {
    Ok(PipelineDesc {
        code: r.range()?,
        bindings: r.range()?,
        vertex_layout: r.range()?,
        color_formats: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
        blend: r.u32()?,
        depth: r.u32()?,
        shader: r.u32()?,
        name: r.u32()?,
        source: r.u32()?,
        instance_stride: r.u32()?,
        varyings: r.u32()?,
        outputs: r.u32()?,
    })
}

/// The file bytes of a capture.
pub fn encode(c: &Capture) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(c.bytes() as usize + 4096));
    w.0.extend_from_slice(MAGIC);
    w.u32(VERSION);
    w.str(&c.app);
    for v in [c.backend, c.dialect, c.session] {
        w.u32(v);
    }
    w.u64(c.frame);
    w.id(c.target);
    w.u32(c.kind);
    w.u32(c.size[0]);
    w.u32(c.size[1]);
    w.f32(c.dpi);
    w.u64(c.at_unix_ms);
    w.u64(c.meta.build_start_ns);
    w.u64(c.meta.build_end_ns);
    w.u64(c.meta.input_ns);
    for [a, b] in c.meta.phases {
        w.u64(a);
        w.u64(b);
    }
    w.u64(c.strings.len() as u64);
    for (id, s) in &c.strings {
        w.u32(*id);
        w.str(s);
    }
    w.u64(c.textures.len() as u64);
    for t in &c.textures {
        w.id(t.id);
        for v in [t.format, t.w, t.h, t.depth, t.mips] {
            w.u32(v);
        }
        w.u16(t.usage);
        for v in [t.owner, t.site, t.label] {
            w.u32(v);
        }
        w.u8(t.lent as u8);
        w.opt(&t.contents);
        w.str(&t.note);
    }
    w.u64(c.buffers.len() as u64);
    for b in &c.buffers {
        w.id(b.id);
        w.u16(b.kind);
        w.u64(b.size);
        for v in [b.owner, b.site, b.label] {
            w.u32(v);
        }
        w.opt(&b.contents);
    }
    w.u64(c.pipelines.len() as u64);
    for p in &c.pipelines {
        w.id(p.id);
        w.u32(p.dialect);
        desc_w(&mut w, &p.desc);
        w.u32(p.owner);
        w.u32(p.site);
        w.bytes(&p.blob);
    }
    w.u64(c.passes.len() as u64);
    for p in &c.passes {
        p.color.into_iter().for_each(|i| w.id(i));
        w.id(p.depth);
        p.load.into_iter().for_each(|l| w.u8(l));
        p.clear.into_iter().for_each(|c| w.f4(c));
        w.f32(p.clear_depth);
        p.layer.into_iter().for_each(|l| w.u32(l));
        w.f4(p.viewport);
        w.f32(p.dpi);
        w.bytes(&p.uniforms);
        w.range(p.draws);
        w.u32(p.name);
        w.u32(p.owner);
    }
    w.u64(c.draws.len() as u64);
    for d in &c.draws {
        w.u32(d.pass);
        w.id(d.pipeline);
        w.id(d.vb);
        w.id(d.ib);
        w.u32(d.index[0]);
        w.u32(d.index[1]);
        w.u32(d.inst_kind);
        w.id(d.inst_id);
        w.u32(d.inst_off);
        w.u32(d.inst_count);
        w.bytes(&d.inst_data);
        w.u64(d.inst_ranges.len() as u64);
        for [a, b] in &d.inst_ranges {
            w.u32(*a);
            w.u32(*b);
        }
        d.uniforms.iter().for_each(|u| w.bytes(u));
        d.ubufs.into_iter().for_each(|i| w.id(i));
        w.u64(d.tex.len() as u64);
        d.tex.iter().for_each(|i| w.id(*i));
        w.f4(d.scissor);
        w.f32(d.zbias);
        for v in [d.flags, d.owner, d.list, d.item, d.list_name] {
            w.u32(v);
        }
        w.f4(d.bounds);
        w.str(&d.widget);
    }
    w.u64(c.notes.len() as u64);
    c.notes.iter().for_each(|n| w.str(n));
    w.u64(c.widgets.len() as u64);
    for x in &c.widgets {
        w.str(&x.path);
        w.str(&x.kind);
        w.f4(x.rect);
        w.str(&x.text);
    }
    w.0
}

/// A capture from its file bytes.
pub fn decode(b: &[u8]) -> Result<Capture, String> {
    if b.len() < 8 || &b[..4] != MAGIC {
        return Err("not a frame capture (.mkcap) file".into());
    }
    let mut r = R { b, at: 4 };
    let version = r.u32()?;
    if version != VERSION {
        return Err(format!("capture file version {version}; this build reads version {VERSION}"));
    }
    let mut c = Capture { app: r.str()?, backend: r.u32()?, dialect: r.u32()?, session: r.u32()?, frame: r.u64()?, target: r.id()?, kind: r.u32()?, ..Default::default() };
    c.size = [r.u32()?, r.u32()?];
    c.dpi = r.f32()?;
    c.at_unix_ms = r.u64()?;
    c.meta.build_start_ns = r.u64()?;
    c.meta.build_end_ns = r.u64()?;
    c.meta.input_ns = r.u64()?;
    for p in &mut c.meta.phases {
        *p = [r.u64()?, r.u64()?];
    }
    for _ in 0..r.count()? {
        c.strings.push((r.u32()?, r.str()?));
    }
    for _ in 0..r.count()? {
        let id = r.id()?;
        let (format, w, h, depth, mips) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let usage = r.u16()?;
        let (owner, site, label) = (r.u32()?, r.u32()?, r.u32()?);
        let lent = r.u8()? != 0;
        let contents = r.opt()?;
        let note = r.str()?;
        c.textures.push(CapTexture { id, format, w, h, depth, mips, usage, owner, site, label, lent, contents, note });
    }
    for _ in 0..r.count()? {
        let id = r.id()?;
        let kind = r.u16()?;
        let size = r.u64()?;
        let (owner, site, label) = (r.u32()?, r.u32()?, r.u32()?);
        c.buffers.push(CapBuffer { id, kind, size, owner, site, label, contents: r.opt()? });
    }
    for _ in 0..r.count()? {
        let id = r.id()?;
        let dialect = r.u32()?;
        let desc = desc_r(&mut r)?;
        let (owner, site) = (r.u32()?, r.u32()?);
        c.pipelines.push(CapPipeline { id, dialect, desc, owner, site, blob: r.bytes()? });
    }
    for _ in 0..r.count()? {
        let color = [r.id()?, r.id()?, r.id()?, r.id()?];
        let depth = r.id()?;
        let load = [r.u8()?, r.u8()?, r.u8()?, r.u8()?, r.u8()?];
        let clear = [r.f4()?, r.f4()?, r.f4()?, r.f4()?];
        let clear_depth = r.f32()?;
        let layer = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
        let viewport = r.f4()?;
        let dpi = r.f32()?;
        let uniforms = r.bytes()?;
        let draws = r.range()?;
        let (name, owner) = (r.u32()?, r.u32()?);
        c.passes.push(CapPass { color, depth, load, clear, clear_depth, layer, viewport, dpi, uniforms, draws, name, owner });
    }
    for _ in 0..r.count()? {
        let pass = r.u32()?;
        let (pipeline, vb, ib) = (r.id()?, r.id()?, r.id()?);
        let index = [r.u32()?, r.u32()?];
        let inst_kind = r.u32()?;
        let inst_id = r.id()?;
        let (inst_off, inst_count) = (r.u32()?, r.u32()?);
        let inst_data = r.bytes()?;
        let mut inst_ranges = Vec::new();
        for _ in 0..r.count()? {
            inst_ranges.push([r.u32()?, r.u32()?]);
        }
        let uniforms = [r.bytes()?, r.bytes()?, r.bytes()?, r.bytes()?];
        let ubufs = [r.id()?, r.id()?, r.id()?, r.id()?];
        let mut tex = Vec::new();
        for _ in 0..r.count()? {
            tex.push(r.id()?);
        }
        let scissor = r.f4()?;
        let zbias = r.f32()?;
        let (flags, owner, list, item, list_name) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let bounds = r.f4()?;
        let widget = r.str()?;
        c.draws.push(CapDraw { pass, pipeline, vb, ib, index, inst_kind, inst_id, inst_off, inst_count, inst_data, inst_ranges, uniforms, ubufs, tex, scissor, zbias, flags, owner, list, item, list_name, bounds, widget });
    }
    for _ in 0..r.count()? {
        c.notes.push(r.str()?);
    }
    for _ in 0..r.count()? {
        c.widgets.push(CapWidget { path: r.str()?, kind: r.str()?, rect: r.f4()?, text: r.str()? });
    }
    // the draw ranges must stay inside the draw list
    for p in &c.passes {
        if (p.draws.off as usize).checked_add(p.draws.len as usize).is_none_or(|e| e > c.draws.len()) {
            return Err("a pass names draws past the end of the draw list".into());
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_survives_its_file() {
        let mut c = Capture { app: "calc".into(), backend: 1, dialect: 1, session: 2, frame: 77, target: Id::new(1, 1), kind: target::WINDOW, size: [800, 600], dpi: 2.0, ..Default::default() };
        c.strings.push((5, "Button".into()));
        c.textures.push(CapTexture { id: Id::new(3, 1), format: format::RGBA8, w: 2, h: 2, depth: 1, mips: 1, usage: usage::SAMPLED, contents: Some(vec![9; 16]), ..Default::default() });
        c.buffers.push(CapBuffer { id: Id::new(4, 2), kind: buffer_kind::VERTEX, size: 8, contents: Some(vec![1; 8]), ..Default::default() });
        c.pipelines.push(CapPipeline { id: Id::new(1, 1), dialect: 1, blob: vec![1, 2, 3], ..Default::default() });
        c.passes.push(CapPass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], viewport: [0.0, 0.0, 800.0, 600.0], dpi: 2.0, draws: Range { off: 0, len: 1 }, ..Default::default() });
        c.draws.push(CapDraw { pipeline: Id::new(1, 1), inst_data: vec![0; 32], inst_count: 2, tex: vec![Id::new(3, 1)], inst_ranges: vec![[0, 2]], owner: 5, ..Default::default() });
        c.notes.push("n".into());
        c.widgets.push(CapWidget { path: "main.ok".into(), kind: "Button".into(), rect: [1.0, 2.0, 3.0, 4.0], text: "OK".into() });
        c.draws[0].widget = "main.ok (Button)".into();
        let back = decode(&encode(&c)).unwrap();
        assert_eq!(format!("{c:?}"), format!("{back:?}"));
        assert!(decode(&encode(&c)[..40]).is_err());
    }
}
