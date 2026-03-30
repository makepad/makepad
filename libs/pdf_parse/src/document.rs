use crate::filter;
use crate::lexer::*;
use crate::object::*;
use crate::page::PdfPage;
use crate::parser::*;
use std::collections::HashMap;

/// A parsed PDF document.
pub struct PdfDocument<'a> {
    data: &'a [u8],
    xref: XRefTable,
    cache: HashMap<u32, PdfObj>,
    object_stream_cache: HashMap<u32, HashMap<u32, PdfObj>>,
    pages: Vec<PageRef>,
}

#[derive(Clone, Debug)]
struct PageRef {
    obj_num: u32,
}

impl<'a> PdfDocument<'a> {
    /// Parse a PDF from a byte slice.
    pub fn parse(data: &'a [u8]) -> PdfResult<Self> {
        if !data.starts_with(b"%PDF-") {
            return Err(PdfError::new("not a PDF file (missing %PDF- header)"));
        }

        let xref = parse_xref(data)?;

        let mut doc = PdfDocument {
            data,
            xref,
            cache: HashMap::new(),
            object_stream_cache: HashMap::new(),
            pages: Vec::new(),
        };

        doc.build_page_list()?;
        Ok(doc)
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Get a page by index (0-based).
    pub fn page(&mut self, index: usize) -> PdfResult<PdfPage> {
        if index >= self.pages.len() {
            return Err(PdfError::new(format!(
                "page index {} out of range ({})",
                index,
                self.pages.len()
            )));
        }
        let obj_num = self.pages[index].obj_num;
        let page_obj = self.resolve_obj_num(obj_num)?;
        PdfPage::from_obj(self, &page_obj)
    }

    /// Resolve an indirect reference to its actual object.
    pub fn resolve(&mut self, obj: &PdfObj) -> PdfResult<PdfObj> {
        match obj {
            PdfObj::Ref(r) => self.resolve_ref(*r),
            other => Ok(other.clone()),
        }
    }

    /// Resolve an ObjRef.
    pub fn resolve_ref(&mut self, r: ObjRef) -> PdfResult<PdfObj> {
        self.resolve_obj_num(r.num)
    }

    /// Resolve by object number.
    pub fn resolve_obj_num(&mut self, num: u32) -> PdfResult<PdfObj> {
        if let Some(cached) = self.cache.get(&num) {
            return Ok(cached.clone());
        }

        let entry = self
            .xref
            .entries
            .get(&num)
            .ok_or_else(|| PdfError::new(format!("object {} not in xref", num)))?
            .clone();

        let obj = match entry.location {
            XRefLocation::Uncompressed { offset } => parse_indirect_object_at(self.data, offset)?.1,
            XRefLocation::Compressed {
                obj_stream_obj_num,
                index,
            } => self.resolve_compressed_object(num, obj_stream_obj_num, index)?,
        };
        self.cache.insert(num, obj.clone());
        Ok(obj)
    }

    /// Resolve an object, and if it's a stream, decompress it.
    pub fn resolve_stream(&mut self, obj: &PdfObj) -> PdfResult<Vec<u8>> {
        let resolved = self.resolve(obj)?;
        match &resolved {
            PdfObj::Stream(s) => filter::decode_stream(&s.data, &s.dict),
            _ => Err(PdfError::new("expected stream object")),
        }
    }

    /// Get the raw data for a stream object (already resolved).
    pub fn decode_stream(&self, stream: &PdfStream) -> PdfResult<Vec<u8>> {
        filter::decode_stream(&stream.data, &stream.dict)
    }

    /// Get the trailer dict.
    pub fn trailer(&self) -> &PdfDict {
        &self.xref.trailer
    }

    fn build_page_list(&mut self) -> PdfResult<()> {
        let root_ref = self
            .xref
            .trailer
            .get_ref("Root")
            .ok_or_else(|| PdfError::new("trailer missing /Root"))?;
        let catalog = self.resolve_ref(root_ref)?;
        let catalog_dict = catalog
            .as_dict()
            .ok_or_else(|| PdfError::new("/Root is not a dict"))?;

        let pages_ref = catalog_dict
            .get("Pages")
            .ok_or_else(|| PdfError::new("catalog missing /Pages"))?
            .clone();

        self.collect_pages_from_ref(&pages_ref)?;
        Ok(())
    }

    /// Collect pages by walking the page tree, tracking object numbers from refs.
    fn collect_pages_from_ref(&mut self, obj: &PdfObj) -> PdfResult<()> {
        let (obj_num, resolved) = match obj {
            PdfObj::Ref(r) => (Some(r.num), self.resolve_ref(*r)?),
            other => (None, other.clone()),
        };

        let dict = resolved
            .as_dict()
            .ok_or_else(|| PdfError::new("page tree node is not a dict"))?;

        let type_name = dict.get_name("Type").unwrap_or("");

        match type_name {
            "Pages" => {
                let kids = dict
                    .get_array("Kids")
                    .ok_or_else(|| PdfError::new("/Pages missing /Kids"))?
                    .to_vec();
                for kid in &kids {
                    self.collect_pages_from_ref(kid)?;
                }
            }
            "Page" | "" => {
                let num = obj_num.unwrap_or_else(|| self.cache.keys().copied().max().unwrap_or(0));
                self.pages.push(PageRef { obj_num: num });
            }
            other => {
                return Err(PdfError::new(format!(
                    "unexpected page tree node type: {}",
                    other
                )));
            }
        }
        Ok(())
    }

    /// Resolve a reference and return as dict.
    pub fn resolve_dict(&mut self, obj: &PdfObj) -> PdfResult<PdfDict> {
        let resolved = self.resolve(obj)?;
        match resolved {
            PdfObj::Dict(d) => Ok(d),
            PdfObj::Stream(s) => Ok(s.dict),
            _ => Err(PdfError::new("expected dict")),
        }
    }

    fn resolve_compressed_object(
        &mut self,
        obj_num: u32,
        obj_stream_obj_num: u32,
        index: usize,
    ) -> PdfResult<PdfObj> {
        self.load_object_stream(obj_stream_obj_num)?;

        let stream_objects = self
            .object_stream_cache
            .get(&obj_stream_obj_num)
            .ok_or_else(|| {
                PdfError::new(format!(
                    "object stream {} was not cached after decoding",
                    obj_stream_obj_num
                ))
            })?;

        stream_objects.get(&obj_num).cloned().ok_or_else(|| {
            PdfError::new(format!(
                "compressed object {} missing from object stream {} at index {}",
                obj_num, obj_stream_obj_num, index
            ))
        })
    }

    fn load_object_stream(&mut self, obj_stream_obj_num: u32) -> PdfResult<()> {
        if self.object_stream_cache.contains_key(&obj_stream_obj_num) {
            return Ok(());
        }

        let stream_obj = self.resolve_obj_num(obj_stream_obj_num)?;
        let stream = stream_obj
            .as_stream()
            .ok_or_else(|| PdfError::new("object stream entry did not resolve to a stream"))?;

        if stream.dict.get_name("Type") != Some("ObjStm") {
            return Err(PdfError::new(format!(
                "xref compressed object points at non-object stream {}",
                obj_stream_obj_num
            )));
        }

        let count = stream
            .dict
            .get_int("N")
            .ok_or_else(|| PdfError::new("object stream missing /N"))? as usize;
        let first = stream
            .dict
            .get_int("First")
            .ok_or_else(|| PdfError::new("object stream missing /First"))?
            as usize;

        let decoded = self.decode_stream(stream)?;
        let mut header_lex = Lexer::new(&decoded, 0);
        let mut object_entries = Vec::with_capacity(count);

        for _ in 0..count {
            let embedded_num = match header_lex.read_object()? {
                PdfObj::Int(n) if n >= 0 => n as u32,
                _ => {
                    return Err(PdfError::new(
                        "object stream header contained invalid embedded object number",
                    ))
                }
            };
            let relative_offset = match header_lex.read_object()? {
                PdfObj::Int(n) if n >= 0 => n as usize,
                _ => {
                    return Err(PdfError::new(
                        "object stream header contained invalid embedded object offset",
                    ))
                }
            };
            object_entries.push((embedded_num, relative_offset));
        }

        let mut objects = HashMap::with_capacity(count);
        for (embedded_num, relative_offset) in object_entries {
            let start = first + relative_offset;
            if start >= decoded.len() {
                return Err(PdfError::new(format!(
                    "object stream {} entry {} points past decoded data",
                    obj_stream_obj_num, embedded_num
                )));
            }
            let mut obj_lex = Lexer::new(&decoded, start);
            let obj = obj_lex.read_object()?;
            objects.insert(embedded_num, obj);
        }

        self.object_stream_cache.insert(obj_stream_obj_num, objects);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{XRefEntry, XRefLocation};

    #[test]
    fn resolves_compressed_objects_from_object_streams() {
        let decoded = b"1 0 2 8 (Hello) 42";
        let object_stream = format!(
            "10 0 obj << /Type /ObjStm /N 2 /First 8 /Length {} >> stream\n{}\
\nendstream\nendobj\n",
            decoded.len(),
            std::str::from_utf8(decoded).unwrap()
        );
        let data = object_stream.into_bytes();

        let mut entries = HashMap::new();
        entries.insert(
            10,
            XRefEntry {
                location: XRefLocation::Uncompressed { offset: 0 },
                gen: 0,
                in_use: true,
            },
        );
        entries.insert(
            1,
            XRefEntry {
                location: XRefLocation::Compressed {
                    obj_stream_obj_num: 10,
                    index: 0,
                },
                gen: 0,
                in_use: true,
            },
        );
        entries.insert(
            2,
            XRefEntry {
                location: XRefLocation::Compressed {
                    obj_stream_obj_num: 10,
                    index: 1,
                },
                gen: 0,
                in_use: true,
            },
        );

        let mut doc = PdfDocument {
            data: &data,
            xref: XRefTable {
                entries,
                trailer: PdfDict::new(),
            },
            cache: HashMap::new(),
            object_stream_cache: HashMap::new(),
            pages: Vec::new(),
        };

        assert_eq!(
            doc.resolve_obj_num(1).unwrap(),
            PdfObj::Str(b"Hello".to_vec())
        );
        assert_eq!(doc.resolve_obj_num(2).unwrap(), PdfObj::Int(42));
    }
}
