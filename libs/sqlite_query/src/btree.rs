//! B-tree page parsing and forward cursors.
//!
//! Every offset that comes off the disk is bounds-checked here: a corrupt page
//! produces [`Error::Corrupt`], never a panic. Layout reference:
//! <https://www.sqlite.org/fileformat.html> section 1.6 ("B-tree Pages").

use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::value::{
    be_u16, be_u32, compare_records, parse_record, parse_record_prefix, read_varint, Collation,
    TextMode, Value,
};
use std::cmp::Ordering;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    IndexInterior,
    TableInterior,
    IndexLeaf,
    TableLeaf,
}

impl PageType {
    pub fn from_byte(b: u8) -> Result<PageType> {
        match b {
            2 => Ok(PageType::IndexInterior),
            5 => Ok(PageType::TableInterior),
            10 => Ok(PageType::IndexLeaf),
            13 => Ok(PageType::TableLeaf),
            other => Err(Error::corrupt(format!("b-tree page type byte {other}"))),
        }
    }
    pub fn is_interior(self) -> bool {
        matches!(self, PageType::IndexInterior | PageType::TableInterior)
    }
    pub fn is_table(self) -> bool {
        matches!(self, PageType::TableInterior | PageType::TableLeaf)
    }
    pub fn header_size(self) -> usize {
        if self.is_interior() {
            12
        } else {
            8
        }
    }
}

// ---------------------------------------------------------------------------
// Overflow thresholds
// ---------------------------------------------------------------------------

pub fn max_local(page_type: PageType, usable: usize) -> usize {
    match page_type {
        PageType::IndexInterior | PageType::IndexLeaf => ((usable - 12) * 64 / 255) - 23,
        PageType::TableInterior | PageType::TableLeaf => usable - 35,
    }
}

pub fn min_local(usable: usize) -> usize {
    ((usable - 12) * 32 / 255) - 23
}

/// Returns (overflows, bytes stored on this page including the 4-byte
/// overflow pointer when it overflows).
pub fn local_payload_size(
    payload_size: usize,
    page_type: PageType,
    usable: usize,
) -> (bool, usize) {
    let max = max_local(page_type, usable);
    if payload_size <= max {
        return (false, payload_size);
    }
    let min = min_local(usable);
    let mut space = min + (payload_size - min) % (usable - 4);
    if space > max {
        space = min;
    }
    (true, space + 4)
}

// ---------------------------------------------------------------------------
// A parsed b-tree page
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct BtreePage {
    pub pgno: u32,
    pub data: Arc<[u8]>,
    pub hdr: usize,
    pub page_type: PageType,
    pub n_cells: usize,
    pub right_child: Option<u32>,
    usable: usize,
}

impl BtreePage {
    pub fn load(pager: &mut Pager, pgno: u32) -> Result<BtreePage> {
        let data = pager.page(pgno)?;
        BtreePage::parse(pgno, data, pager.usable_size())
    }

    pub fn parse(pgno: u32, data: Arc<[u8]>, usable: usize) -> Result<BtreePage> {
        let hdr = if pgno == 1 { 100 } else { 0 };
        if data.len() < hdr + 12 {
            return Err(Error::corrupt("page shorter than its b-tree header"));
        }
        let page_type = PageType::from_byte(data[hdr])?;
        let n_cells = be_u16(&data, hdr + 3)? as usize;
        let right_child = if page_type.is_interior() {
            Some(be_u32(&data, hdr + 8)?)
        } else {
            None
        };
        let ptr_end = hdr + page_type.header_size() + n_cells * 2;
        if ptr_end > usable || usable > data.len() {
            return Err(Error::corrupt(format!(
                "page {pgno} claims {n_cells} cells, which do not fit its pointer array"
            )));
        }
        Ok(BtreePage {
            pgno,
            data,
            hdr,
            page_type,
            n_cells,
            right_child,
            usable,
        })
    }

    /// Byte offset of cell `i` within the page, validated against the usable
    /// area and the end of the cell pointer array.
    pub fn cell_offset(&self, i: usize) -> Result<usize> {
        if i >= self.n_cells {
            return Err(Error::corrupt("cell index past the end of the page"));
        }
        let at = self.hdr + self.page_type.header_size() + i * 2;
        let off = be_u16(&self.data, at)? as usize;
        let content_floor = self.hdr + self.page_type.header_size() + self.n_cells * 2;
        if off < content_floor || off >= self.usable {
            return Err(Error::corrupt(format!(
                "cell pointer {off} on page {} is outside the cell content area",
                self.pgno
            )));
        }
        Ok(off)
    }

    fn slice(&self, from: usize) -> Result<&[u8]> {
        self.data
            .get(from..self.usable)
            .ok_or_else(|| Error::corrupt("cell starts past the usable area"))
    }
}

// ---------------------------------------------------------------------------
// Payload handle
// ---------------------------------------------------------------------------

/// A row/key payload that may continue on overflow pages. Reading the bytes is
/// deferred so scans can filter on the locally stored prefix first.
#[derive(Clone)]
pub struct Payload {
    page: Arc<[u8]>,
    start: usize,
    local_len: usize,
    total: usize,
    overflow: Option<u32>,
}

impl Payload {
    pub fn total_size(&self) -> usize {
        self.total
    }
    pub fn is_local(&self) -> bool {
        self.overflow.is_none()
    }
    /// First page of the overflow chain, when the payload does not fit on the
    /// b-tree page.
    pub fn overflow_page(&self) -> Option<u32> {
        self.overflow
    }
    /// The bytes stored on the b-tree page itself (without the 4-byte overflow
    /// pointer when the payload continues elsewhere).
    pub fn local(&self) -> &[u8] {
        let len = if self.overflow.is_some() {
            self.local_len.saturating_sub(4)
        } else {
            self.local_len
        };
        &self.page[self.start..self.start + len]
    }
    /// The whole payload, following the overflow chain when needed.
    pub fn read(&self, pager: &mut Pager) -> Result<Vec<u8>> {
        let local = self.local();
        if self.overflow.is_none() {
            return Ok(local.to_vec());
        }
        // Grow as the chain is read: a corrupt size must never reserve
        // gigabytes up front.
        let content = pager.usable_size() - 4;
        let mut out = Vec::with_capacity(local.len() + content.min(self.total));
        out.extend_from_slice(local);
        let mut next = self.overflow.unwrap_or(0);
        let mut guard = self.total / content.max(1) + 2;
        while out.len() < self.total {
            if next == 0 {
                return Err(Error::corrupt("overflow chain ended early"));
            }
            if guard == 0 {
                return Err(Error::corrupt("overflow chain does not terminate"));
            }
            guard -= 1;
            let page = pager.page(next)?;
            next = be_u32(&page, 0)?;
            let want = (self.total - out.len()).min(content);
            let bytes = page
                .get(4..4 + want)
                .ok_or_else(|| Error::corrupt("overflow page shorter than its content"))?;
            out.extend_from_slice(bytes);
        }
        Ok(out)
    }
    /// Decode the payload into column values.
    pub fn values(&self, pager: &mut Pager, mode: TextMode) -> Result<Vec<Value>> {
        let enc = pager.text_encoding();
        if self.is_local() {
            parse_record(self.local(), enc, mode)
        } else {
            let bytes = self.read(pager)?;
            parse_record(&bytes, enc, mode)
        }
    }
    /// Decode only the first `n` columns, assembling overflow pages only if the
    /// local bytes do not cover them.
    pub fn prefix(&self, pager: &mut Pager, n: usize, mode: TextMode) -> Result<Vec<Value>> {
        let enc = pager.text_encoding();
        if self.is_local() {
            return parse_record_prefix(self.local(), n, enc, mode);
        }
        if let Ok(vals) = parse_record_prefix(self.local(), n, enc, mode) {
            if vals.len() >= n {
                return Ok(vals);
            }
        }
        let bytes = self.read(pager)?;
        parse_record_prefix(&bytes, n, enc, mode)
    }
}

/// SQLite's own ceiling on a single value (SQLITE_MAX_LENGTH); nothing longer
/// can be stored, so a larger claim is corruption rather than a huge row.
const MAX_PAYLOAD: usize = 1_000_000_000;

fn payload_at(page: &BtreePage, off: usize, page_type: PageType, size: u64) -> Result<Payload> {
    let total = usize::try_from(size)
        .map_err(|_| Error::corrupt("payload size exceeds the address space"))?;
    if total > MAX_PAYLOAD {
        return Err(Error::corrupt(format!(
            "cell claims a {total}-byte payload"
        )));
    }
    let (overflows, local_len) = local_payload_size(total, page_type, page.usable);
    let end = off
        .checked_add(local_len)
        .ok_or_else(|| Error::corrupt("cell payload offset overflow"))?;
    if end > page.usable {
        return Err(Error::corrupt(format!(
            "cell payload on page {} runs past the usable area",
            page.pgno
        )));
    }
    let overflow = if overflows {
        if local_len < 4 {
            return Err(Error::corrupt("overflowing cell has no overflow pointer"));
        }
        let ptr = be_u32(&page.data, end - 4)?;
        if ptr == 0 {
            return Err(Error::corrupt("overflow pointer is page 0"));
        }
        Some(ptr)
    } else {
        None
    };
    Ok(Payload {
        page: page.data.clone(),
        start: off,
        local_len,
        total,
        overflow,
    })
}

/// (rowid, payload) of a table leaf cell.
pub fn table_leaf_cell(page: &BtreePage, i: usize) -> Result<(i64, Payload)> {
    let off = page.cell_offset(i)?;
    let buf = page.slice(off)?;
    let (size, n1) = read_varint(buf)?;
    let (rowid, n2) = read_varint(
        buf.get(n1..)
            .ok_or_else(|| Error::corrupt("table cell truncated before its rowid"))?,
    )?;
    let payload = payload_at(page, off + n1 + n2, PageType::TableLeaf, size)?;
    Ok((rowid as i64, payload))
}

/// (left child page, largest rowid in that child) of a table interior cell.
pub fn table_interior_cell(page: &BtreePage, i: usize) -> Result<(u32, i64)> {
    let off = page.cell_offset(i)?;
    let child = be_u32(&page.data, off)?;
    if child == 0 {
        return Err(Error::corrupt("interior cell points at page 0"));
    }
    let (key, _) = read_varint(page.slice(off + 4)?)?;
    Ok((child, key as i64))
}

/// (left child page or None on a leaf, key payload) of an index cell.
pub fn index_cell(page: &BtreePage, i: usize) -> Result<(Option<u32>, Payload)> {
    let off = page.cell_offset(i)?;
    let (child, at) = if page.page_type == PageType::IndexInterior {
        let c = be_u32(&page.data, off)?;
        if c == 0 {
            return Err(Error::corrupt("index interior cell points at page 0"));
        }
        (Some(c), off + 4)
    } else {
        (None, off)
    };
    let (size, n) = read_varint(page.slice(at)?)?;
    let payload = payload_at(page, at + n, page.page_type, size)?;
    Ok((child, payload))
}

// ---------------------------------------------------------------------------
// Cursors
// ---------------------------------------------------------------------------

struct Frame {
    page: BtreePage,
    /// Next cell to visit on this page.
    idx: usize,
    /// Whether the child to the left of `idx` has already been walked.
    descended: bool,
    /// Whether the rightmost child has already been walked.
    right_done: bool,
}

/// Guard against cyclic page links in a corrupt file.
const MAX_DEPTH: usize = 64;

/// Forward cursor over a table b-tree, in rowid order.
pub struct TableCursor {
    root: u32,
    stack: Vec<Frame>,
    started: bool,
}

/// One row from a table b-tree.
pub struct TableRow {
    pub rowid: i64,
    pub payload: Payload,
}

impl TableCursor {
    pub fn new(root: u32) -> TableCursor {
        TableCursor {
            root,
            stack: Vec::new(),
            started: false,
        }
    }

    pub fn root(&self) -> u32 {
        self.root
    }

    fn push(&mut self, pager: &mut Pager, pgno: u32) -> Result<()> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(Error::corrupt("b-tree deeper than 64 levels"));
        }
        let page = BtreePage::load(pager, pgno)?;
        if !page.page_type.is_table() {
            return Err(Error::corrupt(
                "table b-tree walk reached an index page",
            ));
        }
        self.stack.push(Frame {
            page,
            idx: 0,
            descended: false,
            right_done: false,
        });
        Ok(())
    }

    /// Position before the first row.
    pub fn rewind(&mut self, pager: &mut Pager) -> Result<()> {
        self.stack.clear();
        self.started = true;
        self.push(pager, self.root)?;
        Ok(())
    }

    /// Position so the next [`TableCursor::next`] returns the first row with
    /// `rowid >= target`.
    pub fn seek_ge(&mut self, pager: &mut Pager, target: i64) -> Result<()> {
        self.stack.clear();
        self.started = true;
        let mut pgno = self.root;
        loop {
            self.push(pager, pgno)?;
            let frame = self.stack.last_mut().expect("just pushed");
            let page = frame.page.clone();
            match page.page_type {
                PageType::TableLeaf => {
                    // First cell with rowid >= target.
                    let mut lo = 0usize;
                    let mut hi = page.n_cells;
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        let (rowid, _) = table_leaf_cell(&page, mid)?;
                        if rowid < target {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    frame.idx = lo;
                    return Ok(());
                }
                PageType::TableInterior => {
                    // First cell whose key (max rowid of its child) >= target.
                    let mut lo = 0usize;
                    let mut hi = page.n_cells;
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        let (_, key) = table_interior_cell(&page, mid)?;
                        if key < target {
                            lo = mid + 1;
                        } else {
                            hi = mid;
                        }
                    }
                    if lo < page.n_cells {
                        let (child, _) = table_interior_cell(&page, lo)?;
                        frame.idx = lo;
                        frame.descended = true;
                        pgno = child;
                    } else {
                        frame.idx = page.n_cells;
                        frame.right_done = true;
                        pgno = page
                            .right_child
                            .ok_or_else(|| Error::corrupt("interior page without a right child"))?;
                    }
                }
                _ => return Err(Error::corrupt("table cursor reached an index page")),
            }
        }
    }

    /// Look up one row by rowid.
    pub fn seek_exact(&mut self, pager: &mut Pager, rowid: i64) -> Result<Option<TableRow>> {
        self.seek_ge(pager, rowid)?;
        match self.next(pager)? {
            Some(row) if row.rowid == rowid => Ok(Some(row)),
            _ => Ok(None),
        }
    }

    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<TableRow>> {
        if !self.started {
            self.rewind(pager)?;
        }
        loop {
            let Some(frame) = self.stack.last_mut() else {
                return Ok(None);
            };
            let page = frame.page.clone();
            match page.page_type {
                PageType::TableLeaf => {
                    if frame.idx < page.n_cells {
                        let i = frame.idx;
                        frame.idx += 1;
                        let (rowid, payload) = table_leaf_cell(&page, i)?;
                        return Ok(Some(TableRow { rowid, payload }));
                    }
                    self.stack.pop();
                }
                PageType::TableInterior => {
                    if frame.idx < page.n_cells {
                        if !frame.descended {
                            frame.descended = true;
                            let (child, _) = table_interior_cell(&page, frame.idx)?;
                            self.push(pager, child)?;
                        } else {
                            frame.descended = false;
                            frame.idx += 1;
                        }
                    } else if !frame.right_done {
                        frame.right_done = true;
                        let right = page
                            .right_child
                            .ok_or_else(|| Error::corrupt("interior page without a right child"))?;
                        self.push(pager, right)?;
                    } else {
                        self.stack.pop();
                    }
                }
                _ => return Err(Error::corrupt("table cursor reached an index page")),
            }
        }
    }
}

/// Forward cursor over an index b-tree, in key order.
pub struct IndexCursor {
    root: u32,
    stack: Vec<Frame>,
    started: bool,
}

impl IndexCursor {
    pub fn new(root: u32) -> IndexCursor {
        IndexCursor {
            root,
            stack: Vec::new(),
            started: false,
        }
    }

    fn push(&mut self, pager: &mut Pager, pgno: u32) -> Result<()> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(Error::corrupt("b-tree deeper than 64 levels"));
        }
        let page = BtreePage::load(pager, pgno)?;
        if page.page_type.is_table() {
            return Err(Error::corrupt("index b-tree walk reached a table page"));
        }
        self.stack.push(Frame {
            page,
            idx: 0,
            descended: false,
            right_done: false,
        });
        Ok(())
    }

    pub fn rewind(&mut self, pager: &mut Pager) -> Result<()> {
        self.stack.clear();
        self.started = true;
        self.push(pager, self.root)?;
        Ok(())
    }

    fn key_of(
        &self,
        pager: &mut Pager,
        payload: &Payload,
        ncols: usize,
        mode: TextMode,
    ) -> Result<Vec<Value>> {
        payload.prefix(pager, ncols, mode)
    }

    /// Position so the following [`IndexCursor::next`] calls return every entry
    /// whose key is `>= target`, comparing only the columns `target` provides.
    pub fn seek_ge(
        &mut self,
        pager: &mut Pager,
        target: &[Value],
        colls: &[Collation],
    ) -> Result<()> {
        self.stack.clear();
        self.started = true;
        let n = target.len();
        let mut pgno = self.root;
        loop {
            self.push(pager, pgno)?;
            let page = self.stack.last().expect("just pushed").page.clone();
            // First cell whose key >= target.
            let mut lo = 0usize;
            let mut hi = page.n_cells;
            while lo < hi {
                let mid = (lo + hi) / 2;
                let (_, payload) = index_cell(&page, mid)?;
                let key = self.key_of(pager, &payload, n, TextMode::Strict)?;
                if compare_records(&key, target, colls) == Ordering::Less {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let frame = self.stack.last_mut().expect("just pushed");
            frame.idx = lo;
            if page.page_type == PageType::IndexLeaf {
                return Ok(());
            }
            if lo < page.n_cells {
                let (child, _) = index_cell(&page, lo)?;
                frame.descended = true;
                pgno = child.ok_or_else(|| Error::corrupt("interior index cell without a child"))?;
            } else {
                frame.right_done = true;
                pgno = page
                    .right_child
                    .ok_or_else(|| Error::corrupt("interior page without a right child"))?;
            }
        }
    }

    /// Next index entry (key columns followed by the table rowid for ordinary
    /// indexes on rowid tables).
    pub fn next(&mut self, pager: &mut Pager) -> Result<Option<Payload>> {
        if !self.started {
            self.rewind(pager)?;
        }
        loop {
            let Some(frame) = self.stack.last_mut() else {
                return Ok(None);
            };
            let page = frame.page.clone();
            match page.page_type {
                PageType::IndexLeaf => {
                    if frame.idx < page.n_cells {
                        let i = frame.idx;
                        frame.idx += 1;
                        let (_, payload) = index_cell(&page, i)?;
                        return Ok(Some(payload));
                    }
                    self.stack.pop();
                }
                PageType::IndexInterior => {
                    if frame.idx < page.n_cells {
                        if !frame.descended {
                            frame.descended = true;
                            let (child, _) = index_cell(&page, frame.idx)?;
                            let child = child
                                .ok_or_else(|| Error::corrupt("interior cell without a child"))?;
                            self.push(pager, child)?;
                        } else {
                            frame.descended = false;
                            let i = frame.idx;
                            frame.idx += 1;
                            let (_, payload) = index_cell(&page, i)?;
                            return Ok(Some(payload));
                        }
                    } else if !frame.right_done {
                        frame.right_done = true;
                        let right = page
                            .right_child
                            .ok_or_else(|| Error::corrupt("interior page without a right child"))?;
                        self.push(pager, right)?;
                    } else {
                        self.stack.pop();
                    }
                }
                _ => return Err(Error::corrupt("index cursor reached a table page")),
            }
        }
    }
}
