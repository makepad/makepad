//! 7z archives (the High Voltage SID Collection and the game-music sets
//! ship as these): the signature header, an encoded (LZMA-packed) header,
//! the streams (pack info, folders with their coders, substreams) and the
//! files' names; folders packed with LZMA, LZMA2 or stored are unpacked.

use super::lzma;

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.at).ok_or("7z: header truncated")?;
        self.at += 1;
        Ok(v)
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("7z: header truncated")?;
        self.at += n;
        Ok(s)
    }

    /// 7z's variable-length number.
    fn num(&mut self) -> Result<u64, String> {
        let first = self.byte()?;
        let mut mask = 0x80u8;
        let mut value = 0u64;
        for i in 0..8 {
            if first & mask == 0 {
                let high = (first & mask.wrapping_sub(1)) as u64;
                return Ok(value | (high << (8 * i)));
            }
            value |= (self.byte()? as u64) << (8 * i);
            mask >>= 1;
        }
        Ok(value)
    }

    fn bits(&mut self, n: usize) -> Result<Vec<bool>, String> {
        let mut out = Vec::with_capacity(n);
        let mut byte = 0;
        for i in 0..n {
            if i % 8 == 0 {
                byte = self.byte()?;
            }
            out.push(byte & (0x80 >> (i % 8)) != 0);
        }
        Ok(out)
    }

    fn defined(&mut self, n: usize) -> Result<Vec<bool>, String> {
        if self.byte()? != 0 {
            Ok(vec![true; n])
        } else {
            self.bits(n)
        }
    }

    fn skip_digests(&mut self, n: usize) -> Result<(), String> {
        let defined = self.defined(n)?;
        self.bytes(4 * defined.iter().filter(|d| **d).count())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct Coder {
    id: Vec<u8>,
    props: Vec<u8>,
    ins: usize,
    outs: usize,
}

#[derive(Clone, Debug, Default)]
struct Folder {
    coders: Vec<Coder>,
    /// (in index, out index)
    binds: Vec<(usize, usize)>,
    packed: Vec<usize>,
    unpack_sizes: Vec<u64>,
    /// Substreams' sizes (the files in it).
    streams: Vec<u64>,
}

impl Folder {
    fn unpack_size(&self) -> u64 {
        // The output no bind pair consumes.
        (0..self.unpack_sizes.len()).find(|o| !self.binds.iter().any(|b| b.1 == *o)).map_or(0, |o| self.unpack_sizes[o])
    }
}

#[derive(Default)]
struct Streams {
    pack_pos: u64,
    pack_sizes: Vec<u64>,
    folders: Vec<Folder>,
}

fn pack_info(r: &mut Reader, s: &mut Streams) -> Result<(), String> {
    s.pack_pos = r.num()?;
    let n = r.num()? as usize;
    loop {
        match r.byte()? {
            0x00 => break,
            0x09 => s.pack_sizes = (0..n).map(|_| r.num()).collect::<Result<_, _>>()?,
            0x0A => r.skip_digests(n)?,
            other => return Err(format!("7z: pack info property {other}")),
        }
    }
    Ok(())
}

fn folder(r: &mut Reader) -> Result<Folder, String> {
    let mut f = Folder::default();
    let n = r.num()? as usize;
    for _ in 0..n {
        let flag = r.byte()?;
        let id = r.bytes((flag & 15) as usize)?.to_vec();
        let (ins, outs) = if flag & 0x10 != 0 { (r.num()? as usize, r.num()? as usize) } else { (1, 1) };
        let props = if flag & 0x20 != 0 {
            let size = r.num()? as usize;
            r.bytes(size)?.to_vec()
        } else {
            Vec::new()
        };
        f.coders.push(Coder { id, props, ins, outs });
    }
    let outs: usize = f.coders.iter().map(|c| c.outs).sum();
    let ins: usize = f.coders.iter().map(|c| c.ins).sum();
    for _ in 0..outs.saturating_sub(1) {
        f.binds.push((r.num()? as usize, r.num()? as usize));
    }
    let packed = ins - f.binds.len();
    if packed == 1 {
        f.packed = vec![(0..ins).find(|i| !f.binds.iter().any(|b| b.0 == *i)).unwrap_or(0)];
    } else {
        f.packed = (0..packed).map(|_| r.num().map(|v| v as usize)).collect::<Result<_, _>>()?;
    }
    Ok(f)
}

fn unpack_info(r: &mut Reader, s: &mut Streams) -> Result<(), String> {
    if r.byte()? != 0x0B {
        return Err("7z: no folders".into());
    }
    let n = r.num()? as usize;
    if r.byte()? != 0 {
        return Err("7z: external folders".into());
    }
    s.folders = (0..n).map(|_| folder(r)).collect::<Result<_, _>>()?;
    if r.byte()? != 0x0C {
        return Err("7z: no unpack sizes".into());
    }
    for f in s.folders.iter_mut() {
        let outs: usize = f.coders.iter().map(|c| c.outs).sum();
        f.unpack_sizes = (0..outs).map(|_| r.num()).collect::<Result<_, _>>()?;
    }
    loop {
        match r.byte()? {
            0x00 => break,
            0x0A => r.skip_digests(n)?,
            other => return Err(format!("7z: unpack info property {other}")),
        }
    }
    Ok(())
}

fn substreams(r: &mut Reader, s: &mut Streams) -> Result<(), String> {
    let mut counts = vec![1usize; s.folders.len()];
    let mut id = r.byte()?;
    if id == 0x0D {
        counts = (0..s.folders.len()).map(|_| r.num().map(|v| v as usize)).collect::<Result<_, _>>()?;
        id = r.byte()?;
    }
    let mut sizes: Vec<Vec<u64>> = Vec::new();
    for (f, &n) in s.folders.iter().zip(&counts) {
        let total = f.unpack_size();
        let mut v = Vec::with_capacity(n);
        if n > 0 && id == 0x09 {
            let mut sum = 0;
            for _ in 0..n - 1 {
                let size = r.num()?;
                sum += size;
                v.push(size);
            }
            v.push(total.saturating_sub(sum));
        } else if n == 1 {
            v.push(total);
        }
        sizes.push(v);
    }
    if id == 0x09 {
        id = r.byte()?;
    }
    while id != 0x00 {
        match id {
            0x0A => {
                let unknown = counts.iter().sum::<usize>();
                r.skip_digests(unknown)?;
            }
            other => return Err(format!("7z: substreams property {other}")),
        }
        id = r.byte()?;
    }
    for (f, v) in s.folders.iter_mut().zip(sizes) {
        f.streams = v;
    }
    Ok(())
}

fn streams_info(r: &mut Reader) -> Result<Streams, String> {
    let mut s = Streams::default();
    let mut has_sub = false;
    loop {
        match r.byte()? {
            0x00 => break,
            0x06 => pack_info(r, &mut s)?,
            0x07 => unpack_info(r, &mut s)?,
            0x08 => {
                substreams(r, &mut s)?;
                has_sub = true;
            }
            other => return Err(format!("7z: streams property {other}")),
        }
    }
    if !has_sub {
        for f in s.folders.iter_mut() {
            f.streams = vec![f.unpack_size()];
        }
    }
    Ok(s)
}

/// Unpack one folder (its packed streams start at `at` in the archive):
/// the coder whose output nothing else reads, its inputs the packed
/// streams or other coders' outputs (LZMA feeding BCJ2).
fn unpack(archive: &[u8], f: &Folder, at: usize, sizes: &[u64]) -> Result<Vec<u8>, String> {
    let mut starts = Vec::with_capacity(sizes.len());
    let mut pos = at;
    for &s in sizes {
        starts.push(pos);
        pos += s as usize;
    }
    let packed = |k: usize| -> Result<&[u8], String> { archive.get(starts[k]..starts[k] + sizes[k] as usize).ok_or_else(|| "7z: packed stream past the end".to_string()) };
    fn out_stream<'a>(archive: &'a [u8], f: &Folder, o: usize, packed: &dyn Fn(usize) -> Result<&'a [u8], String>, depth: usize) -> Result<Vec<u8>, String> {
        if depth > 8 {
            return Err("7z: coders bound in a loop".into());
        }
        // The coder making output `o`, and its inputs' global indices.
        let (mut first_in, mut first_out) = (0, 0);
        let mut found = None;
        for c in &f.coders {
            if o >= first_out && o < first_out + c.outs {
                found = Some(c);
                break;
            }
            first_in += c.ins;
            first_out += c.outs;
        }
        let c = found.ok_or("7z: no coder for a stream")?;
        let size = f.unpack_sizes[o] as usize;
        let mut inputs: Vec<Vec<u8>> = Vec::with_capacity(c.ins);
        for i in first_in..first_in + c.ins {
            if let Some(k) = f.packed.iter().position(|p| *p == i) {
                inputs.push(packed(k)?.to_vec());
            } else if let Some(&(_, from)) = f.binds.iter().find(|b| b.0 == i) {
                inputs.push(out_stream(archive, f, from, packed, depth + 1)?);
            } else {
                return Err("7z: an unbound coder input".into());
            }
        }
        match c.id.as_slice() {
            [0x00] => Ok(inputs[0][..size.min(inputs[0].len())].to_vec()),
            [0x03, 0x01, 0x01] => lzma::lzma(&c.props, &inputs[0], size),
            [0x21] => lzma::lzma2(&inputs[0], size),
            [0x03, 0x03, 0x01, 0x1B] if inputs.len() == 4 => super::bcj2::decode(&inputs[0], &inputs[1], &inputs[2], &inputs[3], size),
            [0x03, 0x04, 0x01] => super::ppmd::decode(&c.props, &inputs[0], size),
            id => Err(format!("7z: coder {id:02x?} is not supported")),
        }
    }
    let main = (0..f.unpack_sizes.len()).find(|o| !f.binds.iter().any(|b| b.1 == *o)).ok_or("7z: no main stream")?;
    out_stream(archive, f, main, &packed, 0)
}

/// A file in the archive: its name, and where its bytes are.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub folder: usize,
    pub offset: usize,
    pub size: usize,
}

pub struct Archive<'a> {
    bytes: &'a [u8],
    streams: Streams,
    pub entries: Vec<Entry>,
    /// Where each folder's packed streams start.
    folder_at: Vec<usize>,
    folder_pack: Vec<Vec<u64>>,
}

impl<'a> Archive<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Archive<'a>, String> {
        if bytes.len() < 32 || &bytes[..6] != b"7z\xBC\xAF\x27\x1C" {
            return Err("not a 7z archive".into());
        }
        let next_off = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
        let next_size = u64::from_le_bytes(bytes[20..28].try_into().unwrap()) as usize;
        let mut header = bytes.get(32 + next_off..32 + next_off + next_size).ok_or("7z: header past the end")?.to_vec();
        // An encoded header: its own streams, unpacked, are the header.
        while header.first() == Some(&0x17) {
            let mut r = Reader { b: &header, at: 1 };
            let s = streams_info(&mut r)?;
            let f = s.folders.first().ok_or("7z: encoded header without a folder")?;
            let at = 32 + s.pack_pos as usize;
            header = unpack(bytes, f, at, &s.pack_sizes)?;
        }
        let mut r = Reader { b: &header, at: 0 };
        if r.byte()? != 0x01 {
            return Err("7z: no header".into());
        }
        let mut streams = Streams::default();
        let mut names: Vec<String> = Vec::new();
        let mut empty: Vec<bool> = Vec::new();
        loop {
            match r.byte()? {
                0x00 => break,
                0x02 => loop {
                    let t = r.byte()?;
                    if t == 0 {
                        break;
                    }
                    let n = r.num()? as usize;
                    r.bytes(n)?;
                },
                0x03 => {
                    streams_info(&mut r)?;
                }
                0x04 => streams = streams_info(&mut r)?,
                0x05 => {
                    let n = r.num()? as usize;
                    loop {
                        let t = r.byte()?;
                        if t == 0 {
                            break;
                        }
                        let size = r.num()? as usize;
                        let data = r.bytes(size)?;
                        match t {
                            0x0E => empty = Reader { b: data, at: 0 }.bits(n)?,
                            0x11 => {
                                // External flag, then UTF-16LE names.
                                let units: Vec<u16> = data[1..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                                names = units.split(|u| *u == 0).take(n).map(String::from_utf16_lossy).collect();
                            }
                            _ => {}
                        }
                    }
                    if empty.is_empty() {
                        empty = vec![false; n];
                    }
                    names.resize(n, String::new());
                }
                other => return Err(format!("7z: header property {other}")),
            }
        }
        // Files with a stream take the folders' substreams in order.
        let mut entries = Vec::new();
        let mut fi = 0;
        let mut si = 0;
        let mut offset = 0usize;
        for (name, is_empty) in names.into_iter().zip(empty) {
            if is_empty {
                continue;
            }
            while fi < streams.folders.len() && si >= streams.folders[fi].streams.len() {
                fi += 1;
                si = 0;
                offset = 0;
            }
            let Some(f) = streams.folders.get(fi) else { break };
            let size = f.streams[si] as usize;
            entries.push(Entry { name: name.replace('\\', "/"), folder: fi, offset, size });
            offset += size;
            si += 1;
        }
        let mut folder_at = Vec::new();
        let mut folder_pack = Vec::new();
        let mut pack = 0usize;
        let mut at = 32 + streams.pack_pos as usize;
        for f in &streams.folders {
            folder_at.push(at);
            let sizes: Vec<u64> = streams.pack_sizes[pack..pack + f.packed.len()].to_vec();
            at += sizes.iter().sum::<u64>() as usize;
            pack += f.packed.len();
            folder_pack.push(sizes);
        }
        Ok(Archive { bytes, streams, entries, folder_at, folder_pack })
    }

    /// Each folder's coder ids and how many files it holds.
    pub fn describe(&self) -> Vec<(Vec<Vec<u8>>, usize, u64)> {
        self.streams.folders.iter().enumerate().map(|(k, f)| (f.coders.iter().map(|c| c.id.clone()).collect(), self.entries.iter().filter(|e| e.folder == k).count(), f.unpack_size())).collect()
    }

    pub fn folders(&self) -> usize {
        self.streams.folders.len()
    }

    /// A folder's files' bytes, unpacked (a solid archive is one folder).
    pub fn folder(&self, k: usize) -> Result<Vec<u8>, String> {
        unpack(self.bytes, &self.streams.folders[k], self.folder_at[k], &self.folder_pack[k])
    }

    /// Every file with its bytes, folder by folder (each unpacked once).
    pub fn for_each(&self, mut f: impl FnMut(&Entry, &[u8])) -> Result<(), String> {
        for k in 0..self.folders() {
            let data = self.folder(k)?;
            for e in self.entries.iter().filter(|e| e.folder == k) {
                if let Some(bytes) = data.get(e.offset..e.offset + e.size) {
                    f(e, bytes);
                }
            }
        }
        Ok(())
    }
}
