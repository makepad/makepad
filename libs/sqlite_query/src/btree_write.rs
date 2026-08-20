//! B-tree modification: insert, delete, split, page allocation and the
//! freelist.
//!
//! Pages are always rebuilt whole. A modified page is decoded into its cells,
//! changed, and re-serialized compactly (cell content packed against the end of
//! the page, no free blocks, zero fragmented bytes). That is a legal page by the
//! file format and removes every free-block bookkeeping bug at the cost of one
//! memcpy per modified page — the same trade SQLite makes each time it
//! defragments.
//!
//! Splits are the classic ones: a full page is divided in two and a separator
//! is pushed into the parent; when the root splits, its content moves to a new
//! page so the root keeps its page number (`sqlite_master` records it).
//! SQLite's `PRAGMA integrity_check` does not require a minimum fill, so this
//! produces files the C library and the CLI accept without complaint.

use crate::btree::{
    index_cell, local_payload_size, table_interior_cell, table_leaf_cell, BtreePage, PageType,
};
use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::value::{
    be_u32, compare_records, parse_record, read_varint, write_varint, Collation, TextMode, Value,
};
use std::cmp::Ordering;

/// Header byte offsets used by the freelist.
const HDR_FREELIST_TRUNK: usize = 32;
const HDR_FREELIST_COUNT: usize = 36;

pub struct BtreeWriter<'p> {
    pub pager: &'p mut Pager,
    /// The index b-tree currently being modified. Unlinking an emptied index
    /// page drops a cell from its parent, and in an index b-tree that cell is
    /// a real entry, not just a separator: it has to go back into the same
    /// tree, so the unlink needs to know which tree that is.
    index: Option<(u32, Vec<Collation>)>,
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

/// A cell's raw bytes plus the key needed to order it.
#[derive(Clone)]
struct Cell {
    bytes: Vec<u8>,
    /// Table b-trees order by rowid.
    rowid: i64,
}

/// Length in bytes of cell `i` on `page`.
fn cell_len(page: &BtreePage, usable: usize, i: usize) -> Result<usize> {
    let off = page.cell_offset(i)?;
    let data = &page.data;
    let rest = data
        .get(off..usable)
        .ok_or_else(|| Error::corrupt("cell starts past the usable area"))?;
    Ok(match page.page_type {
        PageType::TableLeaf => {
            let (size, n1) = read_varint(rest)?;
            let (_, n2) = read_varint(&rest[n1..])?;
            let (_, local) = local_payload_size(size as usize, PageType::TableLeaf, usable);
            n1 + n2 + local
        }
        PageType::TableInterior => {
            let (_, n) = read_varint(&rest[4..])?;
            4 + n
        }
        PageType::IndexLeaf => {
            let (size, n) = read_varint(rest)?;
            let (_, local) = local_payload_size(size as usize, PageType::IndexLeaf, usable);
            n + local
        }
        PageType::IndexInterior => {
            let (size, n) = read_varint(&rest[4..])?;
            let (_, local) = local_payload_size(size as usize, PageType::IndexInterior, usable);
            4 + n + local
        }
    })
}

/// Decode every cell of a page into owned bytes.
fn read_cells(page: &BtreePage, usable: usize) -> Result<Vec<Cell>> {
    let mut out = Vec::with_capacity(page.n_cells);
    for i in 0..page.n_cells {
        let off = page.cell_offset(i)?;
        let len = cell_len(page, usable, i)?;
        let bytes = page
            .data
            .get(off..off + len)
            .ok_or_else(|| Error::corrupt("cell runs past the usable area"))?
            .to_vec();
        let rowid = match page.page_type {
            PageType::TableLeaf => table_leaf_cell(page, i)?.0,
            PageType::TableInterior => table_interior_cell(page, i)?.1,
            _ => 0,
        };
        out.push(Cell { bytes, rowid });
    }
    Ok(out)
}

/// The declared payload size and the locally stored key bytes (including the
/// overflow pointer, when there is one) of an index cell. Index leaf and index
/// interior pages use the same local-size formula, so a cell moved between them
/// keeps its bytes and its overflow chain.
fn index_key_parts(cell: &[u8], page_type: PageType, usable: usize) -> Result<(u64, &[u8])> {
    let at = if page_type == PageType::IndexInterior {
        4
    } else {
        0
    };
    let rest = cell
        .get(at..)
        .ok_or_else(|| Error::corrupt("index cell shorter than its header"))?;
    let (size, n) = read_varint(rest)?;
    let (_, local) = local_payload_size(size as usize, page_type, usable);
    let key = rest
        .get(n..n + local)
        .ok_or_else(|| Error::corrupt("index cell shorter than its payload"))?;
    Ok((size, key))
}

/// Re-encode an index cell with a child pointer in front (leaf -> interior).
fn index_cell_with_child(
    cell: &[u8],
    page_type: PageType,
    usable: usize,
    child: u32,
) -> Result<Vec<u8>> {
    let (size, key) = index_key_parts(cell, page_type, usable)?;
    let mut out = Vec::with_capacity(key.len() + 13);
    out.extend_from_slice(&child.to_be_bytes());
    write_varint(&mut out, size);
    out.extend_from_slice(key);
    Ok(out)
}

/// Build a table leaf cell, allocating overflow pages when the payload does not
/// fit on one page.
fn build_table_leaf_cell(
    w: &mut BtreeWriter,
    rowid: i64,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let usable = w.pager.usable_size();
    let mut cell = Vec::with_capacity(payload.len() + 18);
    write_varint(&mut cell, payload.len() as u64);
    write_varint(&mut cell, rowid as u64);
    append_payload(w, &mut cell, payload, PageType::TableLeaf, usable)?;
    Ok(cell)
}

fn build_index_cell(w: &mut BtreeWriter, key: &[u8], interior_child: Option<u32>) -> Result<Vec<u8>> {
    let usable = w.pager.usable_size();
    let page_type = if interior_child.is_some() {
        PageType::IndexInterior
    } else {
        PageType::IndexLeaf
    };
    let mut cell = Vec::with_capacity(key.len() + 8);
    if let Some(child) = interior_child {
        cell.extend_from_slice(&child.to_be_bytes());
    }
    write_varint(&mut cell, key.len() as u64);
    append_payload(w, &mut cell, key, page_type, usable)?;
    Ok(cell)
}

/// Append the local part of a payload, spilling the rest onto overflow pages.
fn append_payload(
    w: &mut BtreeWriter,
    cell: &mut Vec<u8>,
    payload: &[u8],
    page_type: PageType,
    usable: usize,
) -> Result<()> {
    let (overflows, local_size) = local_payload_size(payload.len(), page_type, usable);
    if !overflows {
        cell.extend_from_slice(payload);
        return Ok(());
    }
    let local = local_size - 4;
    cell.extend_from_slice(&payload[..local]);
    // Write the chain back to front so each page knows its successor.
    let content = usable - 4;
    let rest = &payload[local..];
    let chunks: Vec<&[u8]> = rest.chunks(content).collect();
    let mut next: u32 = 0;
    let mut first = 0u32;
    for chunk in chunks.iter().rev() {
        let pgno = w.allocate_page()?;
        let mut page = vec![0u8; w.pager.page_size()];
        page[0..4].copy_from_slice(&next.to_be_bytes());
        page[4..4 + chunk.len()].copy_from_slice(chunk);
        w.pager.write_page(pgno, &page)?;
        next = pgno;
        first = pgno;
    }
    cell.extend_from_slice(&first.to_be_bytes());
    Ok(())
}

/// Free the overflow chain a cell points at, if any.
fn free_cell_overflow(w: &mut BtreeWriter, page_type: PageType, cell: &[u8]) -> Result<()> {
    let usable = w.pager.usable_size();
    let (size, mut at) = match page_type {
        PageType::TableLeaf => {
            let (size, n1) = read_varint(cell)?;
            let (_, n2) = read_varint(&cell[n1..])?;
            (size as usize, n1 + n2)
        }
        PageType::IndexLeaf => {
            let (size, n) = read_varint(cell)?;
            (size as usize, n)
        }
        PageType::IndexInterior => {
            let (size, n) = read_varint(&cell[4..])?;
            (size as usize, 4 + n)
        }
        PageType::TableInterior => return Ok(()),
    };
    let (overflows, local_size) = local_payload_size(size, page_type, usable);
    if !overflows {
        return Ok(());
    }
    at += local_size - 4;
    let mut pgno = be_u32(cell, at)?;
    let mut guard = size / (usable - 4).max(1) + 2;
    while pgno != 0 && guard > 0 {
        guard -= 1;
        let page = w.pager.page(pgno)?;
        let next = be_u32(&page, 0)?;
        w.free_page(pgno)?;
        pgno = next;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Page serialization
// ---------------------------------------------------------------------------

/// Serialize a page from its cells. Returns None when the cells do not fit.
fn serialize_page(
    pgno: u32,
    page_type: PageType,
    cells: &[Cell],
    right_child: Option<u32>,
    page_size: usize,
    usable: usize,
    header_bytes: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let hdr = if pgno == 1 { 100 } else { 0 };
    let mut page = vec![0u8; page_size];
    if let Some(h) = header_bytes {
        page[..100].copy_from_slice(&h[..100]);
    }
    let header_size = page_type.header_size();
    let ptr_end = hdr + header_size + cells.len() * 2;
    let total: usize = cells.iter().map(|c| c.bytes.len()).sum();
    if ptr_end + total > usable {
        return None;
    }
    let mut content = usable;
    for (i, cell) in cells.iter().enumerate() {
        content -= cell.bytes.len();
        page[content..content + cell.bytes.len()].copy_from_slice(&cell.bytes);
        let at = hdr + header_size + i * 2;
        page[at..at + 2].copy_from_slice(&(content as u16).to_be_bytes());
    }
    page[hdr] = match page_type {
        PageType::IndexInterior => 2,
        PageType::TableInterior => 5,
        PageType::IndexLeaf => 10,
        PageType::TableLeaf => 13,
    };
    // First freeblock: none.
    page[hdr + 1..hdr + 3].copy_from_slice(&0u16.to_be_bytes());
    page[hdr + 3..hdr + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
    let start = if content == 65536 { 0u16 } else { content as u16 };
    page[hdr + 5..hdr + 7].copy_from_slice(&start.to_be_bytes());
    page[hdr + 7] = 0; // fragmented free bytes
    if let Some(child) = right_child {
        page[hdr + 8..hdr + 12].copy_from_slice(&child.to_be_bytes());
    }
    Some(page)
}

/// Bytes a set of cells needs on a page of this type.
fn cells_fit(pgno: u32, page_type: PageType, cells: &[Cell], usable: usize) -> bool {
    let hdr = if pgno == 1 { 100 } else { 0 };
    let total: usize = cells.iter().map(|c| c.bytes.len()).sum();
    hdr + page_type.header_size() + cells.len() * 2 + total <= usable
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

impl<'p> BtreeWriter<'p> {
    pub fn new(pager: &'p mut Pager) -> BtreeWriter<'p> {
        BtreeWriter { pager, index: None }
    }

    fn header(&mut self) -> Result<Vec<u8>> {
        Ok(self.pager.page(1)?[..100].to_vec())
    }

    fn set_header_field(&mut self, at: usize, value: u32) -> Result<()> {
        let mut page = self.pager.page(1)?.to_vec();
        page[at..at + 4].copy_from_slice(&value.to_be_bytes());
        self.pager.write_page(1, &page)?;
        Ok(())
    }

    fn header_field(&mut self, at: usize) -> Result<u32> {
        let page = self.pager.page(1)?;
        be_u32(&page, at)
    }

    /// Take a page from the freelist, or grow the file.
    pub fn allocate_page(&mut self) -> Result<u32> {
        let trunk = self.header_field(HDR_FREELIST_TRUNK)?;
        if trunk != 0 {
            let page = self.pager.page(trunk)?.to_vec();
            let next = be_u32(&page, 0)?;
            let count = be_u32(&page, 4)?;
            let capacity = (self.pager.usable_size() / 4).saturating_sub(2) as u32;
            if count > capacity {
                return Err(Error::corrupt("freelist trunk claims too many leaves"));
            }
            let free_count = self.header_field(HDR_FREELIST_COUNT)?;
            if count > 0 {
                // Hand out the last leaf.
                let idx = (count - 1) as usize;
                let leaf = be_u32(&page, 8 + idx * 4)?;
                let mut updated = page.clone();
                updated[4..8].copy_from_slice(&(count - 1).to_be_bytes());
                self.pager.write_page(trunk, &updated)?;
                self.set_header_field(HDR_FREELIST_COUNT, free_count.saturating_sub(1))?;
                let empty = vec![0u8; self.pager.page_size()];
                self.pager.write_page(leaf, &empty)?;
                return Ok(leaf);
            }
            // The trunk itself becomes the allocation.
            self.set_header_field(HDR_FREELIST_TRUNK, next)?;
            self.set_header_field(HDR_FREELIST_COUNT, free_count.saturating_sub(1))?;
            let empty = vec![0u8; self.pager.page_size()];
            self.pager.write_page(trunk, &empty)?;
            return Ok(trunk);
        }
        self.pager.append_page()
    }

    /// Put a page on the freelist.
    pub fn free_page(&mut self, pgno: u32) -> Result<()> {
        if pgno <= 1 {
            return Err(Error::corrupt("cannot free page 1"));
        }
        let trunk = self.header_field(HDR_FREELIST_TRUNK)?;
        let free_count = self.header_field(HDR_FREELIST_COUNT)?;
        let capacity = (self.pager.usable_size() / 4).saturating_sub(2) as u32;
        if trunk != 0 {
            let page = self.pager.page(trunk)?.to_vec();
            let count = be_u32(&page, 4)?;
            if count < capacity {
                let mut updated = page.clone();
                let at = 8 + count as usize * 4;
                updated[at..at + 4].copy_from_slice(&pgno.to_be_bytes());
                updated[4..8].copy_from_slice(&(count + 1).to_be_bytes());
                self.pager.write_page(trunk, &updated)?;
                self.set_header_field(HDR_FREELIST_COUNT, free_count + 1)?;
                // Zero the freed page so stale bytes never look like data.
                let empty = vec![0u8; self.pager.page_size()];
                self.pager.write_page(pgno, &empty)?;
                return Ok(());
            }
        }
        // Start a new trunk.
        let mut page = vec![0u8; self.pager.page_size()];
        page[0..4].copy_from_slice(&trunk.to_be_bytes());
        page[4..8].copy_from_slice(&0u32.to_be_bytes());
        self.pager.write_page(pgno, &page)?;
        self.set_header_field(HDR_FREELIST_TRUNK, pgno)?;
        self.set_header_field(HDR_FREELIST_COUNT, free_count + 1)?;
        Ok(())
    }

    /// Create an empty b-tree and return its root page.
    pub fn create_btree(&mut self, index: bool) -> Result<u32> {
        let pgno = self.allocate_page()?;
        let page_type = if index {
            PageType::IndexLeaf
        } else {
            PageType::TableLeaf
        };
        let page = serialize_page(
            pgno,
            page_type,
            &[],
            None,
            self.pager.page_size(),
            self.pager.usable_size(),
            None,
        )
        .ok_or_else(|| Error::corrupt("empty page does not fit"))?;
        self.pager.write_page(pgno, &page)?;
        Ok(pgno)
    }

    /// Free every page of a b-tree, optionally keeping the root as an empty
    /// leaf (`DELETE FROM` keeps it, `DROP TABLE` does not).
    pub fn clear_btree(&mut self, root: u32, keep_root: bool) -> Result<()> {
        let mut stack = vec![root];
        let mut to_free = Vec::new();
        while let Some(pgno) = stack.pop() {
            let page = BtreePage::load(self.pager, pgno)?;
            let usable = self.pager.usable_size();
            let cells = read_cells(&page, usable)?;
            for (i, cell) in cells.iter().enumerate() {
                match page.page_type {
                    PageType::TableLeaf | PageType::IndexLeaf | PageType::IndexInterior => {
                        free_cell_overflow(self, page.page_type, &cell.bytes)?;
                    }
                    PageType::TableInterior => {}
                }
                if page.page_type.is_interior() {
                    let child = match page.page_type {
                        PageType::TableInterior => table_interior_cell(&page, i)?.0,
                        _ => index_cell(&page, i)?
                            .0
                            .ok_or_else(|| Error::corrupt("index interior cell without a child"))?,
                    };
                    stack.push(child);
                }
            }
            if let Some(right) = page.right_child {
                stack.push(right);
            }
            if pgno != root {
                to_free.push(pgno);
            }
        }
        let root_is_index = BtreePage::load(self.pager, root)?.page_type.is_table() == false;
        for pgno in to_free {
            self.free_page(pgno)?;
        }
        if keep_root {
            let header = if root == 1 { Some(self.header()?) } else { None };
            let page = serialize_page(
                root,
                if root_is_index {
                    PageType::IndexLeaf
                } else {
                    PageType::TableLeaf
                },
                &[],
                None,
                self.pager.page_size(),
                self.pager.usable_size(),
                header.as_deref(),
            )
            .ok_or_else(|| Error::corrupt("empty page does not fit"))?;
            self.pager.write_page(root, &page)?;
        } else {
            self.free_page(root)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Table b-tree
    // -----------------------------------------------------------------------

    /// Insert or replace one row.
    pub fn insert_table(&mut self, root: u32, rowid: i64, payload: &[u8]) -> Result<()> {
        let path = self.descend_table(root, rowid)?;
        let (leaf_no, _) = *path.last().expect("a leaf on the path");
        let leaf = BtreePage::load(self.pager, leaf_no)?;
        let usable = self.pager.usable_size();
        let mut cells = read_cells(&leaf, usable)?;
        let pos = cells.partition_point(|c| c.rowid < rowid);
        if pos < cells.len() && cells[pos].rowid == rowid {
            // Replace: the old payload's overflow pages go back on the freelist.
            let old = cells[pos].bytes.clone();
            free_cell_overflow(self, PageType::TableLeaf, &old)?;
            cells.remove(pos);
        }
        let bytes = build_table_leaf_cell(self, rowid, payload)?;
        cells.insert(pos, Cell { bytes, rowid });
        self.store_cells(&path, PageType::TableLeaf, cells, leaf.right_child)
    }

    /// Delete one row; returns whether it existed.
    pub fn delete_table(&mut self, root: u32, rowid: i64) -> Result<bool> {
        let path = self.descend_table(root, rowid)?;
        let (leaf_no, _) = *path.last().expect("a leaf on the path");
        let leaf = BtreePage::load(self.pager, leaf_no)?;
        let usable = self.pager.usable_size();
        let mut cells = read_cells(&leaf, usable)?;
        let Some(pos) = cells.iter().position(|c| c.rowid == rowid) else {
            return Ok(false);
        };
        let old = cells[pos].bytes.clone();
        free_cell_overflow(self, PageType::TableLeaf, &old)?;
        cells.remove(pos);
        self.store_cells(&path, PageType::TableLeaf, cells, leaf.right_child)?;
        Ok(true)
    }

    /// Path from the root to the leaf that owns `rowid`: (page, index of the
    /// child pointer taken in that page).
    fn descend_table(&mut self, root: u32, rowid: i64) -> Result<Vec<(u32, usize)>> {
        let mut path = Vec::new();
        let mut pgno = root;
        for _ in 0..64 {
            let page = BtreePage::load(self.pager, pgno)?;
            match page.page_type {
                PageType::TableLeaf => {
                    path.push((pgno, usize::MAX));
                    return Ok(path);
                }
                PageType::TableInterior => {
                    let mut child = None;
                    let mut idx = page.n_cells;
                    for i in 0..page.n_cells {
                        let (c, key) = table_interior_cell(&page, i)?;
                        if rowid <= key {
                            child = Some(c);
                            idx = i;
                            break;
                        }
                    }
                    let next = match child {
                        Some(c) => c,
                        None => page
                            .right_child
                            .ok_or_else(|| Error::corrupt("interior page without a right child"))?,
                    };
                    path.push((pgno, idx));
                    pgno = next;
                }
                _ => return Err(Error::corrupt("table write reached an index page")),
            }
        }
        Err(Error::corrupt("b-tree deeper than 64 levels"))
    }

    // -----------------------------------------------------------------------
    // Index b-tree
    // -----------------------------------------------------------------------

    /// Insert one index entry (key record including the trailing rowid).
    pub fn insert_index(&mut self, root: u32, key: &[u8], colls: &[Collation]) -> Result<()> {
        self.index = Some((root, colls.to_vec()));
        let target = parse_record(key, self.pager.text_encoding(), TextMode::Strict)?;
        let path = self.descend_index(root, &target, colls)?;
        let (leaf_no, _) = *path.last().expect("a leaf on the path");
        let leaf = BtreePage::load(self.pager, leaf_no)?;
        let usable = self.pager.usable_size();
        let mut cells = read_cells(&leaf, usable)?;
        // Find the insertion point by decoding each key.
        let mut pos = cells.len();
        for i in 0..cells.len() {
            let (_, payload) = index_cell(&leaf, i)?;
            let vals = payload.values(self.pager, TextMode::Strict)?;
            if compare_records(&vals, &target, colls) != Ordering::Less {
                pos = i;
                break;
            }
        }
        let bytes = build_index_cell(self, key, None)?;
        cells.insert(pos, Cell { bytes, rowid: 0 });
        self.store_cells(&path, PageType::IndexLeaf, cells, leaf.right_child)
    }

    /// Delete one index entry; returns whether it existed.
    pub fn delete_index(&mut self, root: u32, key: &[u8], colls: &[Collation]) -> Result<bool> {
        self.index = Some((root, colls.to_vec()));
        let target = parse_record(key, self.pager.text_encoding(), TextMode::Strict)?;
        // The entry may sit on an interior page; walk every page on the path.
        let path = self.descend_index(root, &target, colls)?;
        let Some((depth, i)) = self.find_index_entry(&path, &target, colls)? else {
            return Ok(false);
        };
        let (pgno, _) = path[depth];
        let page = BtreePage::load(self.pager, pgno)?;
        let usable = self.pager.usable_size();
        if page.page_type == PageType::IndexInterior {
            // Removing a separator would orphan a subtree; this engine only
            // deletes from leaves, so pull the successor up first.
            return self.delete_index_interior(root, &path[..=depth], i, colls);
        }
        let mut cells = read_cells(&page, usable)?;
        let old = cells[i].bytes.clone();
        free_cell_overflow(self, PageType::IndexLeaf, &old)?;
        cells.remove(i);
        self.store_cells(&path[..=depth], PageType::IndexLeaf, cells, page.right_child)?;
        Ok(true)
    }

    /// Where `target` sits on a descent path: the deepest page holding it and
    /// the cell index within that page. An index b-tree stores real entries on
    /// interior pages too, so the search runs from the leaf back up.
    fn find_index_entry(
        &mut self,
        path: &[(u32, usize)],
        target: &[Value],
        colls: &[Collation],
    ) -> Result<Option<(usize, usize)>> {
        for depth in (0..path.len()).rev() {
            let (pgno, _) = path[depth];
            let page = BtreePage::load(self.pager, pgno)?;
            for i in 0..page.n_cells {
                let (_, payload) = index_cell(&page, i)?;
                let vals = payload.values(self.pager, TextMode::Strict)?;
                if compare_records(&vals, target, colls) == Ordering::Equal
                    && vals.len() == target.len()
                {
                    return Ok(Some((depth, i)));
                }
            }
        }
        Ok(None)
    }

    /// Replace an interior separator with the smallest key of its right
    /// subtree, then delete that key from the leaf it came from.
    fn delete_index_interior(
        &mut self,
        root: u32,
        path: &[(u32, usize)],
        cell_index: usize,
        colls: &[Collation],
    ) -> Result<bool> {
        let (pgno, _) = *path.last().expect("a page");
        let page = BtreePage::load(self.pager, pgno)?;
        let target = {
            let (_, payload) = index_cell(&page, cell_index)?;
            payload.values(self.pager, TextMode::Strict)?
        };
        // Walk to the leftmost leaf of the subtree to the right of the cell.
        // That subtree hangs off the *next* slot: the descent recorded the
        // slot of the separator itself, and a path that names the wrong slot
        // makes a later unlink cut the wrong cell out of this page.
        let (right_child, right_slot) = if cell_index + 1 < page.n_cells {
            (
                index_cell(&page, cell_index + 1)?
                    .0
                    .ok_or_else(|| Error::corrupt("interior cell without a child"))?,
                cell_index + 1,
            )
        } else {
            (
                page.right_child
                    .ok_or_else(|| Error::corrupt("interior page without a right child"))?,
                usize::MAX,
            )
        };
        let mut leaf_path: Vec<(u32, usize)> = path.to_vec();
        if let Some(last) = leaf_path.last_mut() {
            last.1 = right_slot;
        }
        let mut pgno_walk = right_child;
        loop {
            let p = BtreePage::load(self.pager, pgno_walk)?;
            if p.page_type == PageType::IndexLeaf {
                leaf_path.push((pgno_walk, usize::MAX));
                break;
            }
            leaf_path.push((pgno_walk, 0));
            pgno_walk = index_cell(&p, 0)?
                .0
                .ok_or_else(|| Error::corrupt("interior cell without a child"))?;
        }
        let (leaf_no, _) = *leaf_path.last().expect("a leaf");
        let leaf = BtreePage::load(self.pager, leaf_no)?;
        if leaf.n_cells == 0 {
            return Err(Error::corrupt("empty leaf under an interior separator"));
        }
        let usable = self.pager.usable_size();
        let mut leaf_cells = read_cells(&leaf, usable)?;
        let successor = leaf_cells.remove(0);
        // The whole record, for the rare case where it has to be re-inserted
        // rather than moved cell-for-cell (which keeps its overflow chain).
        let successor_key = {
            let (_, payload) = index_cell(&leaf, 0)?;
            payload.read(self.pager)?
        };
        // Take the successor out of its leaf first: `leaf_path` is only valid
        // until something restructures the tree, and storing the separator can
        // split this page and move the leaf's parent out from under it.
        self.store_cells(&leaf_path, PageType::IndexLeaf, leaf_cells, leaf.right_child)?;
        // Then put the successor where the deleted key was. The write above
        // may have moved that key to another page, so find it again instead of
        // trusting the path it was found on.
        let path = self.descend_index(root, &target, colls)?;
        let Some((depth, i)) = self.find_index_entry(&path, &target, colls)? else {
            return Err(Error::corrupt(
                "the index entry being deleted vanished from its own tree",
            ));
        };
        let (pgno, _) = path[depth];
        let page = BtreePage::load(self.pager, pgno)?;
        let mut cells = read_cells(&page, usable)?;
        let old = cells[i].bytes.clone();
        free_cell_overflow(self, page.page_type, &old)?;
        if page.page_type == PageType::IndexInterior {
            // The successor came from a leaf: re-encode it with the child
            // pointer the separator it replaces was carrying.
            let child = be_u32(&old, 0)?;
            cells[i] = Cell {
                bytes: index_cell_with_child(&successor.bytes, PageType::IndexLeaf, usable, child)?,
                rowid: 0,
            };
            self.store_cells(&path[..=depth], PageType::IndexInterior, cells, page.right_child)?;
        } else {
            // The restructuring pushed the key down onto a leaf: delete it
            // there, and the successor simply goes back into the tree.
            cells.remove(i);
            self.store_cells(&path[..=depth], PageType::IndexLeaf, cells, page.right_child)?;
            free_cell_overflow(self, PageType::IndexLeaf, &successor.bytes)?;
            self.insert_index(root, &successor_key, colls)?;
        }
        Ok(true)
    }

    fn descend_index(
        &mut self,
        root: u32,
        target: &[Value],
        colls: &[Collation],
    ) -> Result<Vec<(u32, usize)>> {
        let mut path = Vec::new();
        let mut pgno = root;
        for _ in 0..64 {
            let page = BtreePage::load(self.pager, pgno)?;
            match page.page_type {
                PageType::IndexLeaf => {
                    path.push((pgno, usize::MAX));
                    return Ok(path);
                }
                PageType::IndexInterior => {
                    let mut idx = page.n_cells;
                    let mut child = None;
                    for i in 0..page.n_cells {
                        let (c, payload) = index_cell(&page, i)?;
                        let vals = payload.values(self.pager, TextMode::Strict)?;
                        if compare_records(target, &vals, colls) != Ordering::Greater {
                            child = c;
                            idx = i;
                            break;
                        }
                    }
                    let next = match child {
                        Some(c) => c,
                        None => page
                            .right_child
                            .ok_or_else(|| Error::corrupt("interior page without a right child"))?,
                    };
                    path.push((pgno, idx));
                    pgno = next;
                }
                _ => return Err(Error::corrupt("index write reached a table page")),
            }
        }
        Err(Error::corrupt("b-tree deeper than 64 levels"))
    }

    // -----------------------------------------------------------------------
    // Storing a modified page, with splits propagating upward
    // -----------------------------------------------------------------------

    /// Write `cells` back to the page at the end of `path`, splitting it (and
    /// its ancestors) when they no longer fit, or unlinking it when it empties.
    fn store_cells(
        &mut self,
        path: &[(u32, usize)],
        page_type: PageType,
        cells: Vec<Cell>,
        right_child: Option<u32>,
    ) -> Result<()> {
        let (pgno, _) = *path.last().expect("a page on the path");
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        if cells.is_empty() {
            // A b-tree page other than the root must hold at least one cell,
            // and an interior page with no cells is just a detour to its right
            // child. Both are cleaned up here rather than left on disk.
            if page_type.is_interior() {
                if let Some(child) = right_child {
                    return self.collapse_into(path, child);
                }
            }
            if path.len() > 1 {
                return self.unlink_from_parent(path);
            }
            // An empty root leaf is a legal empty table.
        }
        if cells_fit(pgno, page_type, &cells, usable) {
            let header = if pgno == 1 { Some(self.header()?) } else { None };
            let page = serialize_page(
                pgno,
                page_type,
                &cells,
                right_child,
                page_size,
                usable,
                header.as_deref(),
            )
            .ok_or_else(|| Error::corrupt("page does not fit after an update"))?;
            self.pager.write_page(pgno, &page)?;
            return Ok(());
        }
        self.split(path, page_type, cells, right_child)
    }

    /// Replace the page at the end of `path` with its only child: either by
    /// pulling the child's content up into the page (so a root keeps its page
    /// number) or by repointing the parent straight at the child.
    fn collapse_into(&mut self, path: &[(u32, usize)], child: u32) -> Result<()> {
        let (pgno, _) = *path.last().expect("a page on the path");
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        let child_page = BtreePage::load(self.pager, child)?;
        let child_cells = read_cells(&child_page, usable)?;
        if cells_fit(pgno, child_page.page_type, &child_cells, usable) {
            let header = if pgno == 1 { Some(self.header()?) } else { None };
            let page = serialize_page(
                pgno,
                child_page.page_type,
                &child_cells,
                child_page.right_child,
                page_size,
                usable,
                header.as_deref(),
            )
            .ok_or_else(|| Error::corrupt("collapsed page does not fit"))?;
            self.pager.write_page(pgno, &page)?;
            self.free_page(child)?;
            return Ok(());
        }
        if path.len() == 1 {
            // The root cannot move and the child does not fit into it (page 1
            // is 100 bytes smaller); leave the root as a one-hop interior page,
            // which cursors and integrity_check both accept.
            let header = if pgno == 1 { Some(self.header()?) } else { None };
            let page = serialize_page(
                pgno,
                if child_page.page_type.is_table() {
                    PageType::TableInterior
                } else {
                    PageType::IndexInterior
                },
                &[],
                Some(child),
                page_size,
                usable,
                header.as_deref(),
            )
            .ok_or_else(|| Error::corrupt("pass-through root does not fit"))?;
            self.pager.write_page(pgno, &page)?;
            return Ok(());
        }
        self.repoint_parent(path, child)?;
        self.free_page(pgno)
    }

    /// Make the parent point at `new_child` wherever it pointed at the page at
    /// the end of `path`.
    fn repoint_parent(&mut self, path: &[(u32, usize)], new_child: u32) -> Result<()> {
        let (parent_no, ci) = path[path.len() - 2];
        let parent = BtreePage::load(self.pager, parent_no)?;
        let usable = self.pager.usable_size();
        let mut cells = read_cells(&parent, usable)?;
        let mut right = parent.right_child;
        if ci == usize::MAX || ci >= cells.len() {
            right = Some(new_child);
        } else {
            let mut bytes = cells[ci].bytes.clone();
            bytes[0..4].copy_from_slice(&new_child.to_be_bytes());
            cells[ci] = Cell {
                bytes,
                rowid: cells[ci].rowid,
            };
        }
        self.store_cells(&path[..path.len() - 1], parent.page_type, cells, right)
    }

    /// Remove the pointer to the (now empty) page at the end of `path` from its
    /// parent and free it.
    fn unlink_from_parent(&mut self, path: &[(u32, usize)]) -> Result<()> {
        let (pgno, _) = *path.last().expect("a page on the path");
        let (parent_no, ci) = path[path.len() - 2];
        let parent = BtreePage::load(self.pager, parent_no)?;
        let usable = self.pager.usable_size();
        let mut cells = read_cells(&parent, usable)?;
        let mut right = parent.right_child;
        // On an index b-tree the cell that goes with the child pointer is a
        // real entry of the index, not just a boundary the way a table
        // interior cell's rowid is: dropping it here would silently lose a row
        // from the index, so it is re-homed once the parent is written.
        let is_index = parent.page_type == PageType::IndexInterior;
        let mut dropped: Option<usize> = None;
        if ci == usize::MAX || ci >= cells.len() {
            // The page was the rightmost child: the last separator's child
            // takes its place, and that separator goes with it.
            match cells.len().checked_sub(1) {
                Some(last) => {
                    right = Some(be_u32(&cells[last].bytes, 0)?);
                    dropped = Some(last);
                }
                None => right = None,
            }
        } else {
            dropped = Some(ci);
        }
        let orphan = match (is_index, dropped) {
            (true, Some(at)) => {
                let (_, payload) = index_cell(&parent, at)?;
                let key = payload.read(self.pager)?;
                free_cell_overflow(self, PageType::IndexInterior, &cells[at].bytes)?;
                Some(key)
            }
            _ => None,
        };
        if let Some(at) = dropped {
            cells.remove(at);
        }
        self.free_page(pgno)?;
        self.store_cells(&path[..path.len() - 1], parent.page_type, cells, right)?;
        if let Some(key) = orphan {
            let Some((root, colls)) = self.index.clone() else {
                return Err(Error::corrupt(
                    "an index page was unlinked outside an index operation",
                ));
            };
            self.insert_index(root, &key, &colls)?;
        }
        Ok(())
    }

    /// Split one page into two and push a separator into its parent.
    fn split(
        &mut self,
        path: &[(u32, usize)],
        page_type: PageType,
        cells: Vec<Cell>,
        right_child: Option<u32>,
    ) -> Result<()> {
        let (pgno, _) = *path.last().expect("a page on the path");
        let page_size = self.pager.page_size();
        let usable = self.pager.usable_size();
        if cells.len() < 2 {
            return Err(Error::corrupt("a single cell does not fit on a page"));
        }

        // Halve by byte weight so both sides fit.
        let total: usize = cells.iter().map(|c| c.bytes.len() + 2).sum();
        let mut acc = 0usize;
        let mut split_at = 0usize;
        for (i, c) in cells.iter().enumerate() {
            acc += c.bytes.len() + 2;
            if acc * 2 >= total {
                split_at = i + 1;
                break;
            }
        }
        split_at = split_at.clamp(1, cells.len() - 1);

        let is_index = matches!(page_type, PageType::IndexLeaf | PageType::IndexInterior);
        let mut left: Vec<Cell> = cells[..split_at].to_vec();
        let right: Vec<Cell> = cells[split_at..].to_vec();
        // For an index split the median moves up instead of staying put.
        let median = if is_index { left.pop() } else { None };
        if left.is_empty() {
            return Err(Error::corrupt("index split left no cells behind"));
        }

        // The new page always takes the right half, so page numbers of existing
        // pages (and therefore sqlite_master) never change.
        let new_pgno = self.allocate_page()?;
        let (left_right_child, right_right_child) = match page_type {
            PageType::TableInterior => {
                // The last left cell's child becomes the left page's rightmost.
                let last = left.pop().ok_or_else(|| Error::corrupt("empty split"))?;
                let child = be_u32(&last.bytes, 0)?;
                let sep_key = last.rowid;
                let _ = sep_key;
                (Some(child), right_child)
            }
            PageType::IndexInterior => {
                let med = median.as_ref().ok_or_else(|| Error::corrupt("no median"))?;
                (Some(be_u32(&med.bytes, 0)?), right_child)
            }
            _ => (None, None),
        };
        let left_last_key = match page_type {
            PageType::TableInterior => {
                // After moving the last cell up, the separator is its rowid.
                cells[split_at - 1].rowid
            }
            PageType::TableLeaf => left.last().map(|c| c.rowid).unwrap_or(0),
            _ => 0,
        };

        let header = if pgno == 1 { Some(self.header()?) } else { None };
        let left_page = serialize_page(
            pgno,
            page_type,
            &left,
            left_right_child,
            page_size,
            usable,
            header.as_deref(),
        )
        .ok_or_else(|| Error::corrupt("left half of a split does not fit"))?;
        let right_page = serialize_page(
            new_pgno,
            page_type,
            &right,
            right_right_child,
            page_size,
            usable,
            None,
        )
        .ok_or_else(|| Error::corrupt("right half of a split does not fit"))?;

        if path.len() == 1 {
            // Splitting the root: its content moves to a fresh page so the root
            // page number stays valid, and the root becomes an interior page.
            let moved = self.allocate_page()?;
            let moved_page = serialize_page(
                moved,
                page_type,
                &left,
                left_right_child,
                page_size,
                usable,
                None,
            )
            .ok_or_else(|| Error::corrupt("moved root half does not fit"))?;
            self.pager.write_page(moved, &moved_page)?;
            self.pager.write_page(new_pgno, &right_page)?;

            let root_type = if is_index {
                PageType::IndexInterior
            } else {
                PageType::TableInterior
            };
            let sep_cell = if is_index {
                let med = median.ok_or_else(|| Error::corrupt("no median for an index split"))?;
                Cell {
                    bytes: index_cell_with_child(&med.bytes, page_type, usable, moved)?,
                    rowid: 0,
                }
            } else {
                let mut cell = Vec::with_capacity(12);
                cell.extend_from_slice(&moved.to_be_bytes());
                write_varint(&mut cell, left_last_key as u64);
                Cell {
                    bytes: cell,
                    rowid: left_last_key,
                }
            };
            let header = if pgno == 1 { Some(self.header()?) } else { None };
            let root_page = serialize_page(
                pgno,
                root_type,
                &[sep_cell],
                Some(new_pgno),
                page_size,
                usable,
                header.as_deref(),
            )
            .ok_or_else(|| Error::corrupt("new root does not fit"))?;
            self.pager.write_page(pgno, &root_page)?;
            return Ok(());
        }

        self.pager.write_page(pgno, &left_page)?;
        self.pager.write_page(new_pgno, &right_page)?;

        // Insert the separator into the parent, which may split in turn.
        let parent_path = &path[..path.len() - 1];
        let (parent_no, child_index) = path[path.len() - 2];
        let parent = BtreePage::load(self.pager, parent_no)?;
        let mut parent_cells = read_cells(&parent, usable)?;
        let parent_type = parent.page_type;
        let mut parent_right = parent.right_child;

        let sep_cell = if is_index {
            let med = median.ok_or_else(|| Error::corrupt("no median for an index split"))?;
            Cell {
                bytes: index_cell_with_child(&med.bytes, page_type, usable, pgno)?,
                rowid: 0,
            }
        } else {
            let mut cell = Vec::with_capacity(12);
            cell.extend_from_slice(&pgno.to_be_bytes());
            write_varint(&mut cell, left_last_key as u64);
            Cell {
                bytes: cell,
                rowid: left_last_key,
            }
        };

        // The old page's pointer in the parent now points at the right half.
        let at = child_index.min(parent_cells.len());
        if child_index == usize::MAX || child_index >= parent_cells.len() {
            parent_right = Some(new_pgno);
            parent_cells.push(sep_cell);
        } else {
            let mut updated = parent_cells[at].bytes.clone();
            updated[0..4].copy_from_slice(&new_pgno.to_be_bytes());
            parent_cells[at] = Cell {
                bytes: updated,
                rowid: parent_cells[at].rowid,
            };
            parent_cells.insert(at, sep_cell);
        }
        self.store_cells(parent_path, parent_type, parent_cells, parent_right)
    }
}
