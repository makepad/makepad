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

        let obj = parse_indirect_object_at(self.data, entry.offset)?.1;
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
}
