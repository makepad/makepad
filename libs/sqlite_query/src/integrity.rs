//! Structural verification of a database file, in the spirit of
//! `PRAGMA integrity_check`.
//!
//! Walks every b-tree named by `sqlite_master`, then the freelist, and reports
//! what it finds instead of trusting it: page types, cell bounds, key order,
//! overflow chains, index-to-table agreement and page accounting. The write
//! path (P2) is graded against this after every crash-injection run, and the
//! reader is graded against it on files SQLite wrote.

use crate::btree::{
    index_cell, table_interior_cell, table_leaf_cell, BtreePage, PageType,
};
use crate::error::Result;
use crate::pager::Pager;
use crate::schema::Schema;
use crate::value::{be_u32, compare_records, Collation, TextMode, Value};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub problems: Vec<String>,
    pub pages_seen: u32,
    pub rows: u64,
    pub index_entries: u64,
    pub freelist_pages: u32,
}

impl IntegrityReport {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

struct Checker<'a> {
    pager: &'a mut Pager,
    /// page number -> what claimed it
    owners: HashMap<u32, String>,
    report: IntegrityReport,
}

impl<'a> Checker<'a> {
    fn claim(&mut self, pgno: u32, owner: &str) -> bool {
        if pgno == 0 || pgno > self.pager.page_count() {
            self.report
                .problems
                .push(format!("{owner}: page {pgno} is out of range"));
            return false;
        }
        if let Some(prev) = self.owners.get(&pgno) {
            self.report.problems.push(format!(
                "page {pgno} is used twice: {prev} and {owner}"
            ));
            return false;
        }
        self.owners.insert(pgno, owner.to_string());
        self.report.pages_seen += 1;
        true
    }

    /// Claim every page of an overflow chain.
    fn claim_overflow(&mut self, first: Option<u32>, total: usize, owner: &str) {
        let Some(mut pgno) = first else { return };
        let content = self.pager.usable_size().saturating_sub(4).max(1);
        let mut remaining = total;
        let mut guard = total / content + 2;
        while pgno != 0 && remaining > 0 && guard > 0 {
            guard -= 1;
            if !self.claim(pgno, owner) {
                return;
            }
            let Ok(page) = self.pager.page(pgno) else {
                self.report
                    .problems
                    .push(format!("{owner}: page {pgno} unreadable"));
                return;
            };
            let next = match be_u32(&page, 0) {
                Ok(n) => n,
                Err(_) => return,
            };
            remaining = remaining.saturating_sub(content);
            pgno = next;
        }
    }

    /// Walk a table b-tree, checking rowid order and cell integrity.
    /// Returns the rowids found, in order.
    fn check_table(&mut self, name: &str, root: u32, rowids: &mut Vec<i64>) -> Result<()> {
        let mut stack = vec![(root, i64::MIN, i64::MAX)];
        while let Some((pgno, lo, hi)) = stack.pop() {
            if !self.claim(pgno, &format!("table {name}")) {
                continue;
            }
            let page = match BtreePage::load(self.pager, pgno) {
                Ok(p) => p,
                Err(e) => {
                    self.report
                        .problems
                        .push(format!("table {name} page {pgno}: {e}"));
                    continue;
                }
            };
            if !page.page_type.is_table() {
                self.report
                    .problems
                    .push(format!("table {name} page {pgno} is an index page"));
                continue;
            }
            match page.page_type {
                PageType::TableLeaf => {
                    let mut prev = None;
                    for i in 0..page.n_cells {
                        match table_leaf_cell(&page, i) {
                            Ok((rowid, payload)) => {
                                if rowid < lo || rowid > hi {
                                    self.report.problems.push(format!(
                                        "table {name} page {pgno} cell {i}: rowid {rowid} outside {lo}..{hi}"
                                    ));
                                }
                                if let Some(p) = prev {
                                    if rowid <= p {
                                        self.report.problems.push(format!(
                                            "table {name} page {pgno}: rowid {rowid} follows {p}"
                                        ));
                                    }
                                }
                                prev = Some(rowid);
                                rowids.push(rowid);
                                self.report.rows += 1;
                                if let Err(e) = payload.read(self.pager) {
                                    self.report.problems.push(format!(
                                        "table {name} rowid {rowid}: payload unreadable: {e}"
                                    ));
                                }
                                let first = payload.overflow_page();
                                self.claim_overflow(
                                    first,
                                    payload.total_size(),
                                    &format!("table {name} rowid {rowid} overflow"),
                                );
                            }
                            Err(e) => self
                                .report
                                .problems
                                .push(format!("table {name} page {pgno} cell {i}: {e}")),
                        }
                    }
                }
                PageType::TableInterior => {
                    let mut prev_key = lo;
                    let mut children = Vec::new();
                    for i in 0..page.n_cells {
                        match table_interior_cell(&page, i) {
                            Ok((child, key)) => {
                                children.push((child, prev_key, key));
                                prev_key = key.saturating_add(1);
                            }
                            Err(e) => self
                                .report
                                .problems
                                .push(format!("table {name} page {pgno} cell {i}: {e}")),
                        }
                    }
                    if let Some(right) = page.right_child {
                        children.push((right, prev_key, hi));
                    } else {
                        self.report
                            .problems
                            .push(format!("table {name} page {pgno} has no right child"));
                    }
                    // Push in reverse so the walk stays left to right.
                    for c in children.into_iter().rev() {
                        stack.push(c);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Walk an index b-tree, checking key order; returns the keys in order.
    fn check_index(
        &mut self,
        name: &str,
        root: u32,
        colls: &[Collation],
        keys: &mut Vec<Vec<Value>>,
    ) -> Result<()> {
        let mut stack = vec![root];
        let mut in_order: Vec<Vec<Value>> = Vec::new();
        while let Some(pgno) = stack.pop() {
            if !self.claim(pgno, &format!("index {name}")) {
                continue;
            }
            let page = match BtreePage::load(self.pager, pgno) {
                Ok(p) => p,
                Err(e) => {
                    self.report
                        .problems
                        .push(format!("index {name} page {pgno}: {e}"));
                    continue;
                }
            };
            if page.page_type.is_table() {
                self.report
                    .problems
                    .push(format!("index {name} page {pgno} is a table page"));
                continue;
            }
            for i in 0..page.n_cells {
                match index_cell(&page, i) {
                    Ok((child, payload)) => {
                        let first = payload.overflow_page();
                        let total = payload.total_size();
                        match payload.values(self.pager, TextMode::Lossy) {
                            Ok(vals) => {
                                in_order.push(vals);
                                self.report.index_entries += 1;
                                self.claim_overflow(
                                    first,
                                    total,
                                    &format!("index {name} overflow"),
                                );
                            }
                            Err(e) => self
                                .report
                                .problems
                                .push(format!("index {name} page {pgno} cell {i}: {e}")),
                        }
                        if let Some(c) = child {
                            stack.push(c);
                        }
                    }
                    Err(e) => self
                        .report
                        .problems
                        .push(format!("index {name} page {pgno} cell {i}: {e}")),
                }
            }
            if let Some(right) = page.right_child {
                stack.push(right);
            } else if page.page_type == PageType::IndexInterior {
                self.report
                    .problems
                    .push(format!("index {name} page {pgno} has no right child"));
            }
        }
        // Page-order traversal above does not produce key order, so sort a copy
        // and compare against a proper in-order walk done by the cursor.
        let mut cursor = crate::btree::IndexCursor::new(root);
        let mut ordered: Vec<Vec<Value>> = Vec::new();
        if cursor.rewind(self.pager).is_ok() {
            while let Ok(Some(entry)) = cursor.next(self.pager) {
                if let Ok(vals) = entry.values(self.pager, TextMode::Lossy) {
                    ordered.push(vals);
                }
            }
        }
        for w in ordered.windows(2) {
            if compare_records(&w[0], &w[1], colls) == Ordering::Greater {
                self.report.problems.push(format!(
                    "index {name} is out of order: {:?} before {:?}",
                    w[0], w[1]
                ));
                break;
            }
        }
        if ordered.len() != in_order.len() {
            self.report.problems.push(format!(
                "index {name}: cursor walk saw {} entries, page walk saw {}",
                ordered.len(),
                in_order.len()
            ));
        }
        *keys = ordered;
        Ok(())
    }

    fn check_freelist(&mut self) -> Result<()> {
        let mut trunk = self.pager.header().freelist_trunk_page;
        let declared = self.pager.header().freelist_pages;
        let mut count = 0u32;
        let mut guard = self.pager.page_count() + 1;
        while trunk != 0 {
            if guard == 0 {
                self.report
                    .problems
                    .push("freelist does not terminate".into());
                break;
            }
            guard -= 1;
            if !self.claim(trunk, "freelist trunk") {
                break;
            }
            count += 1;
            let page = self.pager.page(trunk)?;
            let next = be_u32(&page, 0)?;
            let n = be_u32(&page, 4)?;
            if n as usize > (self.pager.usable_size() / 4).saturating_sub(2) {
                self.report
                    .problems
                    .push(format!("freelist trunk {trunk} claims {n} leaves"));
                break;
            }
            for i in 0..n {
                let leaf = be_u32(&page, 8 + i as usize * 4)?;
                if self.claim(leaf, "freelist leaf") {
                    count += 1;
                }
            }
            trunk = next;
        }
        self.report.freelist_pages = count;
        if count != declared {
            self.report.problems.push(format!(
                "header says {declared} free pages, the freelist has {count}"
            ));
        }
        Ok(())
    }
}

/// Check the whole database. `strict_pages` also verifies that every page in
/// the file is accounted for by a b-tree, the freelist or a pointer map.
pub fn check(pager: &mut Pager, schema: &Schema, strict_pages: bool) -> Result<IntegrityReport> {
    let mut checker = Checker {
        pager,
        owners: HashMap::new(),
        report: IntegrityReport::default(),
    };
    // Page 1 is the schema table's root and holds the file header.
    let mut rowids = Vec::new();
    checker.check_table("sqlite_master", 1, &mut rowids)?;

    for table in &schema.tables {
        if table.root_page <= 1 || table.unsupported.is_some() {
            continue;
        }
        let mut rowids = Vec::new();
        checker.check_table(&table.name, table.root_page, &mut rowids)?;
        rowids.sort_unstable();
        for index in &table.indexes {
            if index.root_page == 0 {
                continue;
            }
            let colls: Vec<Collation> = index.columns.iter().map(|c| c.collation).collect();
            let mut keys = Vec::new();
            checker.check_index(&index.name, index.root_page, &colls, &mut keys)?;
            if index.partial {
                continue;
            }
            if keys.len() != rowids.len() {
                checker.report.problems.push(format!(
                    "index {} has {} entries for {} rows in {}",
                    index.name,
                    keys.len(),
                    rowids.len(),
                    table.name
                ));
            }
            // Every index entry must point at a row that exists.
            for key in &keys {
                let Some(rowid) = key.last().and_then(Value::as_integer) else {
                    checker
                        .report
                        .problems
                        .push(format!("index {} entry without a rowid", index.name));
                    continue;
                };
                if rowids.binary_search(&rowid).is_err() {
                    checker.report.problems.push(format!(
                        "index {} points at missing rowid {rowid} of {}",
                        index.name, table.name
                    ));
                }
            }
        }
    }

    checker.check_freelist()?;

    if strict_pages {
        let total = checker.pager.page_count();
        let page_size = checker.pager.page_size() as u64;
        // Auto-vacuum pointer-map pages sit at fixed intervals.
        let ptrmap_stride = page_size / 5 + 1;
        let auto_vacuum = checker.pager.header().auto_vacuum();
        // The lock byte page (the page holding file offset 1 GiB) is never used.
        let lock_page = (1u64 << 30) / page_size + 1;
        for pgno in 1..=total {
            if checker.owners.contains_key(&pgno) {
                continue;
            }
            if pgno as u64 == lock_page {
                continue;
            }
            if auto_vacuum {
                let p = pgno as u64;
                if p == 2 || (p >= 2 && (p - 2) % ptrmap_stride == 0) {
                    continue;
                }
            }
            checker
                .report
                .problems
                .push(format!("page {pgno} is not referenced by anything"));
        }
    }
    Ok(checker.report)
}
