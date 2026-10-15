//! Front-end images (plans/memory-use.md, "Image design"): a file's parsed state as sections
//! of plain arrays, written into the run's image store when the file is final and read back by
//! mapping from then on (that run and later ones), so the arrays cost clean pages instead of
//! heap and a warm run neither lexes nor parses.
//!
//! Layout of one image (all little endian, the store keeps payloads 8-byte aligned):
//!   magic u32, version u32, section count u32, 0 u32
//!   per section: offset u64 (from the image start), length u64 (elements), element size u32,
//!     section id u32
//!   sections, each 8-byte aligned
//! An element type is written as its in-memory bytes (`Pod`): the same Relax build (part of
//! the store's key) reads it with the same layout. A reader checks every section's id, element
//! size, bounds and alignment before handing out a slice.

/// Types whose values are plain bytes: no pointers, no Drop, every bit pattern written by
/// this build reads back as the same value. Implemented only for such types.
pub unsafe trait Pod: Copy + 'static {}

unsafe impl Pod for u8 {}
unsafe impl Pod for u16 {}
unsafe impl Pod for u32 {}
unsafe impl Pod for u64 {}
unsafe impl Pod for (u32, u32, u32) {}
unsafe impl Pod for crate::lexer::Tok {}
// tree nodes: Copy, no pointers (ast.rs keeps every node type position independent)
unsafe impl Pod for crate::ast::Arm {}
unsafe impl Pod for crate::ast::AsmOp {}
unsafe impl Pod for crate::ast::Attr {}
unsafe impl Pod for crate::ast::Block {}
unsafe impl Pod for crate::ast::Bound {}
unsafe impl Pod for crate::ast::ClosureParam {}
unsafe impl Pod for crate::ast::Expr {}
unsafe impl Pod for crate::ast::ExprId {}
unsafe impl Pod for crate::ast::FieldExpr {}
unsafe impl Pod for crate::ast::FieldPat {}
unsafe impl Pod for crate::ast::FnPtr {}
unsafe impl Pod for crate::ast::GenericArg {}
unsafe impl Pod for crate::ast::GenericArgs {}
unsafe impl Pod for crate::ast::GenericParam {}
unsafe impl Pod for crate::ast::Ident {}
unsafe impl Pod for crate::ast::ItemId {}
unsafe impl Pod for crate::ast::MacArgs {}
unsafe impl Pod for crate::ast::Pat {}
unsafe impl Pod for crate::ast::PatId {}
unsafe impl Pod for crate::ast::PathSeg {}
unsafe impl Pod for crate::ast::QSelf {}
unsafe impl Pod for crate::ast::Stmt {}
unsafe impl Pod for crate::ast::Ty {}
unsafe impl Pod for crate::ast::TyId {}

unsafe impl Pod for crate::ast::Item {}
unsafe impl Pod for crate::ast::FnSig {}
unsafe impl Pod for crate::ast::Param {}
unsafe impl Pod for crate::ast::FieldDef {}
unsafe impl Pod for crate::ast::Variant {}
unsafe impl Pod for crate::ast::UseTree {}
unsafe impl Pod for crate::ast::WherePred {}
unsafe impl Pod for crate::ast::MethodCallRow {}
unsafe impl Pod for crate::ast::ForRow {}
unsafe impl Pod for crate::ast::ClosureRow {}
unsafe impl Pod for crate::ast::StructRow {}
unsafe impl Pod for crate::ast::MacCall {}
unsafe impl Pod for crate::ast::WhileRow {}
unsafe impl Pod for crate::ast::LetStmt {}
unsafe impl Pod for crate::ast::AttrsStmt {}
unsafe impl Pod for crate::ast::UseItem {}
unsafe impl Pod for crate::ast::StructItem {}
unsafe impl Pod for crate::ast::UnionItem {}
unsafe impl Pod for crate::ast::EnumItem {}
unsafe impl Pod for crate::ast::TypeAliasItem {}
unsafe impl Pod for crate::ast::TraitItem {}
unsafe impl Pod for crate::ast::TraitAliasItem {}
unsafe impl Pod for crate::ast::ImplItem {}
unsafe impl Pod for crate::ast::ForeignModItem {}
unsafe impl Pod for crate::ast::Path {}

pub const MAGIC: u32 = u32::from_le_bytes(*b"RFI1");
/// bump when a section's meaning changes (layouts are covered by the build id)
pub const VERSION: u32 = 10;

const HEAD: usize = 16;
const ENTRY: usize = 24;

/// One image section can combine generated metadata with existing immutable source bytes.
#[derive(Clone)]
pub enum Chunk<'a> {
    Borrowed(&'a [u8]),
    /// Immutable bytes pinned by their image owner for the rest of the process.
    Mapped(&'static [u8]),
    Owned(Vec<u8>),
    Shared(std::sync::Arc<[u8]>),
    Image(std::sync::Arc<Writer<'a>>),
}
impl Chunk<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Borrowed(b) => b.len(), Self::Mapped(b) => b.len(), Self::Owned(b) => b.len(),
            Self::Shared(b) => b.len(), Self::Image(image) => image.len(),
        }
    }
    fn write_to(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        match self {
            Self::Borrowed(b) => out.write_all(b), Self::Mapped(b) => out.write_all(b), Self::Owned(b) => out.write_all(b),
            Self::Shared(b) => out.write_all(b), Self::Image(image) => image.write_to(out),
        }
    }
}

/// Builds one image: `add` sections in a fixed order, then `finish`.
#[derive(Clone, Default)]
pub struct Writer<'a> {
    secs: Vec<(u32, u32, Vec<Chunk<'a>>, u64)>,
}

impl<'a> Writer<'a> {
    pub fn add<T: Pod>(&mut self, id: u32, v: &'a [T]) {
        let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) };
        self.secs.push((id, std::mem::size_of::<T>() as u32, vec![Chunk::Borrowed(bytes)], v.len() as u64));
    }

    /// Keep an immutable mapped array by reference, including across snapshot freezing.
    pub fn add_mapped<T: Pod>(&mut self, id: u32, v: &'static [T]) {
        let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) };
        self.secs.push((id, std::mem::size_of::<T>() as u32, vec![Chunk::Mapped(bytes)], v.len() as u64));
    }

    /// One array with a mapped prefix and newly generated tail, in the same section.
    pub fn add_mapped_prefix<T: Pod>(&mut self, id: u32, mapped: &'static [T], tail: &[T]) {
        let mapped_bytes = unsafe { std::slice::from_raw_parts(mapped.as_ptr() as *const u8, std::mem::size_of_val(mapped)) };
        let tail_bytes = unsafe { std::slice::from_raw_parts(tail.as_ptr() as *const u8, std::mem::size_of_val(tail)) };
        self.secs.push((id, std::mem::size_of::<T>() as u32,
            vec![Chunk::Mapped(mapped_bytes), Chunk::Owned(tail_bytes.to_vec())], (mapped.len() + tail.len()) as u64));
    }

    /// Small generated sections whose temporary backing array does not outlive the writer.
    pub fn add_owned<T: Pod>(&mut self, id: u32, v: &[T]) {
        let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) };
        self.secs.push((id, std::mem::size_of::<T>() as u32, vec![Chunk::Owned(bytes.to_vec())], v.len() as u64));
    }

    /// A section of encoded bytes (read back with `Reader::bytes`).
    pub fn add_bytes(&mut self, id: u32, b: Vec<u8>) {
        let n = b.len() as u64;
        self.secs.push((id, 1, vec![Chunk::Owned(b)], n));
    }

    pub fn add_chunks(&mut self, id: u32, chunks: Vec<Chunk<'a>>) {
        let n = chunks.iter().map(|b| b.len() as u64).sum();
        self.secs.push((id, 1, chunks, n));
    }

    /// Nest an image without assembling or copying its bytes.
    pub fn add_image(&mut self, id: u32, image: std::sync::Arc<Writer<'a>>) {
        let n = image.len() as u64;
        self.secs.push((id, 1, vec![Chunk::Image(image)], n));
    }

    /// Freeze borrowed arrays while keeping generated sections and nested images intact.
    pub fn into_owned(self) -> Writer<'static> {
        Writer { secs: self.secs.into_iter().map(|(id, es, chunks, n)| {
            let chunks = chunks.into_iter().map(|chunk| match chunk {
                Chunk::Borrowed(b) => Chunk::Owned(b.to_vec()),
                Chunk::Mapped(b) => Chunk::Mapped(b),
                Chunk::Owned(b) => Chunk::Owned(b),
                Chunk::Shared(b) => Chunk::Shared(b),
                Chunk::Image(image) => {
                    // Shared builders are legal too; only that case needs an owned copy.
                    let image = std::sync::Arc::try_unwrap(image).unwrap_or_else(|image| (*image).clone());
                    Chunk::Image(std::sync::Arc::new(image.into_owned()))
                }
            }).collect();
            (id, es, chunks, n)
        }).collect() }
    }

    pub fn len(&self) -> usize {
        self.secs.iter().fold(align8(HEAD + ENTRY * self.secs.len()), |off, (_, _, b, _)| align8(off + b.iter().map(|c| c.len()).sum::<usize>()))
    }

    /// Write borrowed array sections directly; no assembled image or section copies.
    pub fn write_to(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        out.write_all(&MAGIC.to_le_bytes())?;
        out.write_all(&VERSION.to_le_bytes())?;
        out.write_all(&(self.secs.len() as u32).to_le_bytes())?;
        out.write_all(&0u32.to_le_bytes())?;
        let mut off = align8(HEAD + ENTRY * self.secs.len());
        for (id, es, b, n) in &self.secs {
            out.write_all(&(off as u64).to_le_bytes())?;
            out.write_all(&n.to_le_bytes())?;
            out.write_all(&es.to_le_bytes())?;
            out.write_all(&id.to_le_bytes())?;
            off = align8(off + b.iter().map(|c| c.len()).sum::<usize>());
        }
        for (_, _, b, _) in &self.secs {
            for chunk in b { chunk.write_to(out)?; }
            let len: usize = b.iter().map(|c| c.len()).sum();
            out.write_all(&[0; 7][..align8(len) - len])?;
        }
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        self.write_to(&mut out).unwrap();
        out
    }
}

fn align8(x: usize) -> usize {
    (x + 7) & !7
}

/// Reads an image that stays mapped for the rest of the process. The section table is read
/// in place: each access checks its own entry (no per-image allocation or table pass).
pub struct Reader {
    img: &'static [u8],
    n: usize,
}

impl Reader {
    pub fn new(img: &'static [u8]) -> Option<Reader> {
        if img.len() < HEAD || img.as_ptr() as usize % 8 != 0 {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
        if u32_at(0) != MAGIC || u32_at(4) != VERSION {
            return None;
        }
        let n = u32_at(8) as usize;
        if HEAD + n * ENTRY > img.len() {
            return None;
        }
        Some(Reader { img, n })
    }

    /// Section `k` (in write order) as a slice of `T`, if its id and element size match.
    pub fn slice<T: Pod>(&self, k: usize, id: u32) -> Option<&'static [T]> {
        if k >= self.n {
            return None;
        }
        let img = self.img;
        let e = HEAD + k * ENTRY;
        let u32_at = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(img[o..o + 8].try_into().unwrap());
        let (o, n, es, sid) = (u64_at(e), u64_at(e + 8), u32_at(e + 16), u32_at(e + 20));
        let end = n.checked_mul(es as u64).and_then(|b| b.checked_add(o))?;
        if o % 8 != 0 || end > img.len() as u64 || sid != id || es as usize != std::mem::size_of::<T>()
            || (img.as_ptr() as usize + o as usize) % std::mem::align_of::<T>() != 0 {
            return None;
        }
        let p = unsafe { img.as_ptr().add(o as usize) } as *const T;
        Some(unsafe { std::slice::from_raw_parts(p, n as usize) })
    }

    pub fn bytes(&self, k: usize, id: u32) -> Option<&'static [u8]> {
        self.slice::<u8>(k, id)
    }

    pub fn len(&self) -> usize {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_image_matches_format_bytes() {
        let values = [0x11223344u32, 7];
        let mut w = Writer::default();
        w.add(5, b"abc");
        w.add(9, &values);
        w.add_bytes(12, Vec::new());
        let mut expected = Vec::new();
        for v in [MAGIC, VERSION, 3, 0] { expected.extend_from_slice(&v.to_le_bytes()); }
        for (off, len, size, id) in [(88u64, 3u64, 1u32, 5u32), (96, 2, 4, 9), (104, 0, 1, 12)] {
            expected.extend_from_slice(&off.to_le_bytes());
            expected.extend_from_slice(&len.to_le_bytes());
            expected.extend_from_slice(&size.to_le_bytes());
            expected.extend_from_slice(&id.to_le_bytes());
        }
        expected.extend_from_slice(b"abc\0\0\0\0\0");
        for value in values { expected.extend_from_slice(&value.to_ne_bytes()); }
        let mut actual = Vec::new();
        w.write_to(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(w.len(), actual.len());
    }

    #[test]
    fn round_trip() {
        let mut w = Writer::default();
        w.add::<u8>(1, b"hello");
        w.add::<u32>(2, &[1, 2, 3]);
        w.add::<(u32, u32, u32)>(3, &[(4, 5, 6)]);
        w.add_bytes(4, vec![9; 3]);
        let img = w.finish();
        // the store hands out 8-byte aligned payloads: leak an aligned copy
        let words: &'static mut [u64] = Box::leak(vec![0u64; img.len() / 8].into_boxed_slice());
        let b: &'static mut [u8] = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, img.len()) };
        b.copy_from_slice(&img);
        let r = Reader::new(b).unwrap();
        assert_eq!(r.slice::<u8>(0, 1).unwrap(), b"hello");
        assert_eq!(r.slice::<u32>(1, 2).unwrap(), &[1, 2, 3]);
        assert_eq!(r.slice::<(u32, u32, u32)>(2, 3).unwrap(), &[(4, 5, 6)]);
        assert_eq!(r.bytes(3, 4).unwrap(), &[9, 9, 9]);
        assert!(r.slice::<u32>(0, 1).is_none());
        assert!(r.slice::<u8>(1, 9).is_none());
    }
}
