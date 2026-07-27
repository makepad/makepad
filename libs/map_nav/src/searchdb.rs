//! Disk-backed search database for continent-scale "all searchable strings"
//! indexes (~200M docs: every named feature plus addresses). The RAM index
//! in `search.rs` stays for region-scale data; this engine answers queries
//! with a bounded number of `pread`s against one immutable file.
//!
//! Layout (all little-endian, offsets u64):
//!
//! ```text
//! [header 128B]
//! [dict_index]   dict_count × 16B: token blob off:48/len:16, meta off u64
//! [dict_blob]    concatenated token strings, sorted
//! [token_meta]   per token: total u32, head u16, cells u32,
//!                head doc ids u32×, (cell u32, count u32, off u64)×cells
//! [postings]     per token per cell: doc ids u32×, rank-sorted
//! [docs]         doc_count × 32B fixed records
//! [strings]      name/secondary pool (secondary strings deduped)
//! ```
//!
//! Retrieval per query token = global rank-head (famous entities surface at
//! any distance) + expanding cell rings around `near` (local intent), then
//! the shared `score_search_hit` tiered scorer ranks candidates.

use crate::geo::{fixed_to_lon_lat, haversine_m, lon_lat_to_norm, LonLat};
use crate::search::{normalize_tokens, score_search_hit, Category, SearchResult};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

const SEARCHDB_MAGIC: u32 = 0x4d53_4442; // "BDSM"^W "MSDB"
const SEARCHDB_VERSION: u32 = 1;
const HEADER_BYTES: u64 = 128;
const DICT_ENTRY_BYTES: u64 = 16;
const DOC_RECORD_BYTES: u64 = 32;

/// World-fixed grid over normalized mercator: 512×512 ≈ 50km cells at
/// mid-European latitudes.
const GRID_DIM: u32 = 512;
/// Per-token global head: the highest static-rank docs regardless of cell.
const HEAD_MAX: usize = 128;
/// Candidate budget per token during retrieval.
const TOKEN_CANDIDATE_BUDGET: usize = 2500;
/// Maximum ring radius in cells (~1200km) for local retrieval.
const MAX_RING: i64 = 24;
/// Prefix expansion cap (last query token).
const PREFIX_TOKEN_CAP: usize = 48;
/// In-memory tuple buffer before a sorted run spills to disk (12B each).
const SPILL_TUPLES: usize = 8_000_000;

fn cell_of_norm(x: f64, y: f64) -> u32 {
    let cx = ((x * GRID_DIM as f64) as i64).clamp(0, GRID_DIM as i64 - 1) as u32;
    let cy = ((y * GRID_DIM as f64) as i64).clamp(0, GRID_DIM as i64 - 1) as u32;
    cy * GRID_DIM + cx
}

// --- Builder ---

#[derive(Clone, Copy)]
struct Tuple {
    token: u32,
    cell: u32,
    rank: u8,
    doc: u32,
}

pub struct SearchDbBuilder {
    out_path: PathBuf,
    spill_dir: PathBuf,
    docs_tmp: BufWriter<File>,
    strings_tmp: BufWriter<File>,
    strings_len: u64,
    secondary_pool: HashMap<String, (u64, u16)>,
    token_ids: HashMap<String, u32>,
    tuples: Vec<Tuple>,
    runs: Vec<PathBuf>,
    doc_count: u32,
}

pub struct SearchDbStats {
    pub docs: u64,
    pub tokens: u64,
    pub file_bytes: u64,
}

impl SearchDbBuilder {
    pub fn create(out_path: &Path) -> Result<Self, String> {
        let spill_dir = out_path.with_extension("spill");
        let _ = fs::remove_dir_all(&spill_dir);
        fs::create_dir_all(&spill_dir)
            .map_err(|err| format!("create {}: {err}", spill_dir.display()))?;
        let docs_tmp = BufWriter::new(
            File::create(spill_dir.join("docs.tmp"))
                .map_err(|err| format!("create docs.tmp: {err}"))?,
        );
        let strings_tmp = BufWriter::new(
            File::create(spill_dir.join("strings.tmp"))
                .map_err(|err| format!("create strings.tmp: {err}"))?,
        );
        Ok(Self {
            out_path: out_path.to_path_buf(),
            spill_dir,
            docs_tmp,
            strings_tmp,
            strings_len: 0,
            secondary_pool: HashMap::new(),
            token_ids: HashMap::new(),
            tuples: Vec::with_capacity(SPILL_TUPLES),
            runs: Vec::new(),
            doc_count: 0,
        })
    }

    fn write_string(&mut self, s: &str) -> Result<(u64, u16), String> {
        let len = s.len().min(u16::MAX as usize);
        let off = self.strings_len;
        self.strings_tmp
            .write_all(&s.as_bytes()[..len])
            .map_err(|err| format!("write string: {err}"))?;
        self.strings_len += len as u64;
        Ok((off, len as u16))
    }

    pub fn add_doc(
        &mut self,
        name: &str,
        secondary: &str,
        pos: LonLat,
        category: Category,
        rank: u8,
    ) -> Result<(), String> {
        if name.trim().is_empty() || self.doc_count == u32::MAX {
            return Ok(());
        }
        let doc_id = self.doc_count;
        let (nx, ny) = lon_lat_to_norm(pos);
        let cell = cell_of_norm(nx, ny);
        let x = crate::geo::norm_to_fixed(nx);
        let y = crate::geo::norm_to_fixed(ny);
        let (name_off, name_len) = self.write_string(name.trim())?;
        let (sec_off, sec_len) = if secondary.trim().is_empty() {
            (0, 0)
        } else if let Some(entry) = self.secondary_pool.get(secondary.trim()) {
            *entry
        } else {
            let entry = self.write_string(secondary.trim())?;
            // Bound the dedup map; the most common secondaries (city names)
            // enter early.
            if self.secondary_pool.len() < 4_000_000 {
                self.secondary_pool
                    .insert(secondary.trim().to_string(), entry);
            }
            entry
        };

        let mut record = [0u8; DOC_RECORD_BYTES as usize];
        record[0..4].copy_from_slice(&x.to_le_bytes());
        record[4..8].copy_from_slice(&y.to_le_bytes());
        record[8..10].copy_from_slice(&(category as u16).to_le_bytes());
        record[10] = rank;
        record[11] = 0;
        record[12..20]
            .copy_from_slice(&((name_off & 0xffff_ffff_ffff) | ((name_len as u64) << 48)).to_le_bytes());
        record[20..28]
            .copy_from_slice(&((sec_off & 0xffff_ffff_ffff) | ((sec_len as u64) << 48)).to_le_bytes());
        self.docs_tmp
            .write_all(&record)
            .map_err(|err| format!("write doc: {err}"))?;

        for token in normalize_tokens(name) {
            let next_id = self.token_ids.len() as u32;
            let token_id = *self.token_ids.entry(token).or_insert(next_id);
            self.tuples.push(Tuple {
                token: token_id,
                cell,
                rank,
                doc: doc_id,
            });
            if self.tuples.len() >= SPILL_TUPLES {
                self.flush_run()?;
            }
        }
        self.doc_count += 1;
        Ok(())
    }

    pub fn doc_count(&self) -> u32 {
        self.doc_count
    }

    fn flush_run(&mut self) -> Result<(), String> {
        if self.tuples.is_empty() {
            return Ok(());
        }
        self.tuples.sort_unstable_by(|a, b| {
            (a.token, a.cell, std::cmp::Reverse(a.rank), a.doc)
                .cmp(&(b.token, b.cell, std::cmp::Reverse(b.rank), b.doc))
        });
        let path = self.spill_dir.join(format!("run_{:05}.bin", self.runs.len()));
        let mut writer = BufWriter::new(
            File::create(&path).map_err(|err| format!("create {}: {err}", path.display()))?,
        );
        for t in &self.tuples {
            let mut rec = [0u8; 12];
            rec[0..4].copy_from_slice(&t.token.to_le_bytes());
            rec[4..8].copy_from_slice(&t.cell.to_le_bytes());
            rec[8] = t.rank;
            // rec[9] pad
            rec[10..12].copy_from_slice(&0u16.to_le_bytes());
            // doc id straddles: use 4 bytes at 8..12? rank already at 8.
            writer
                .write_all(&rec[0..8])
                .and_then(|_| writer.write_all(&[t.rank, 0]))
                .and_then(|_| writer.write_all(&t.doc.to_le_bytes()))
                .map_err(|err| format!("write run: {err}"))?;
        }
        writer.flush().map_err(|err| format!("flush run: {err}"))?;
        self.runs.push(path);
        self.tuples.clear();
        Ok(())
    }

    pub fn finish(mut self) -> Result<SearchDbStats, String> {
        self.flush_run()?;
        self.docs_tmp.flush().map_err(|err| err.to_string())?;
        self.strings_tmp.flush().map_err(|err| err.to_string())?;

        // Sorted dictionary: token string order; map token_id -> dict slot.
        let mut sorted: Vec<(String, u32)> = self
            .token_ids
            .drain()
            .collect();
        sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let token_count = sorted.len();
        let mut slot_of_id = vec![0u32; token_count];
        for (slot, (_, id)) in sorted.iter().enumerate() {
            slot_of_id[*id as usize] = slot as u32;
        }

        // K-way merge of runs grouped by token id; write postings + meta.
        let meta_path = self.spill_dir.join("meta.tmp");
        let postings_path = self.spill_dir.join("postings.tmp");
        let mut meta_out = BufWriter::new(
            File::create(&meta_path).map_err(|err| format!("create meta.tmp: {err}"))?,
        );
        let mut postings_out = BufWriter::new(
            File::create(&postings_path).map_err(|err| format!("create postings.tmp: {err}"))?,
        );
        let mut meta_off_of_id: Vec<u64> = vec![u64::MAX; token_count];
        let mut meta_len: u64 = 0;
        let mut postings_len: u64 = 0;

        struct RunReader {
            reader: BufReader<File>,
            current: Option<(u32, u32, u8, u32)>, // token, cell, rank, doc
        }
        impl RunReader {
            fn advance(&mut self) -> Result<(), String> {
                let mut rec = [0u8; 14];
                match self.reader.read_exact(&mut rec) {
                    Ok(()) => {
                        self.current = Some((
                            u32::from_le_bytes(rec[0..4].try_into().unwrap()),
                            u32::from_le_bytes(rec[4..8].try_into().unwrap()),
                            rec[8],
                            u32::from_le_bytes(rec[10..14].try_into().unwrap()),
                        ));
                        Ok(())
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                        self.current = None;
                        Ok(())
                    }
                    Err(err) => Err(format!("read run: {err}")),
                }
            }
        }

        let mut readers = Vec::new();
        for path in &self.runs {
            let mut reader = RunReader {
                reader: BufReader::with_capacity(
                    1 << 20,
                    File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?,
                ),
                current: None,
            };
            reader.advance()?;
            readers.push(reader);
        }

        // Heap over (token, cell, Reverse(rank), doc, reader index)
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        struct HeapKey(u32, u32, std::cmp::Reverse<u8>, u32, usize);
        let mut heap: BinaryHeap<std::cmp::Reverse<HeapKey>> = BinaryHeap::new();
        for (i, r) in readers.iter().enumerate() {
            if let Some((t, c, rk, d)) = r.current {
                heap.push(std::cmp::Reverse(HeapKey(t, c, std::cmp::Reverse(rk), d, i)));
            }
        }

        let mut current_token: Option<u32> = None;
        // (cell, rank, doc) for the token being assembled, in merge order
        // (cell asc, rank desc).
        let mut token_tuples: Vec<(u32, u8, u32)> = Vec::new();

        let flush_token = |token_id: u32,
                               tuples: &mut Vec<(u32, u8, u32)>,
                               meta_out: &mut BufWriter<File>,
                               postings_out: &mut BufWriter<File>,
                               meta_len: &mut u64,
                               postings_len: &mut u64,
                               meta_off_of_id: &mut Vec<u64>|
         -> Result<(), String> {
            if tuples.is_empty() {
                return Ok(());
            }
            // Dedup identical (doc) within a cell (same doc repeated token).
            tuples.dedup_by_key(|t| (t.0, t.2));
            // Global head by rank.
            let mut head: Vec<(u8, u32)> = tuples.iter().map(|t| (t.1, t.2)).collect();
            head.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            head.truncate(HEAD_MAX);
            head.dedup_by_key(|h| h.1);
            // Cells.
            let mut cells: Vec<(u32, u32, u64)> = Vec::new(); // cell, count, off
            let mut i = 0;
            while i < tuples.len() {
                let cell = tuples[i].0;
                let start = i;
                while i < tuples.len() && tuples[i].0 == cell {
                    i += 1;
                }
                let off = *postings_len;
                for t in &tuples[start..i] {
                    postings_out
                        .write_all(&t.2.to_le_bytes())
                        .map_err(|err| format!("write postings: {err}"))?;
                }
                *postings_len += ((i - start) * 4) as u64;
                cells.push((cell, (i - start) as u32, off));
            }
            let meta_off = *meta_len;
            let mut meta = Vec::with_capacity(10 + head.len() * 4 + cells.len() * 16);
            meta.extend_from_slice(&(tuples.len() as u32).to_le_bytes());
            meta.extend_from_slice(&(head.len() as u16).to_le_bytes());
            meta.extend_from_slice(&(cells.len() as u32).to_le_bytes());
            for (_, doc) in &head {
                meta.extend_from_slice(&doc.to_le_bytes());
            }
            for (cell, count, off) in &cells {
                meta.extend_from_slice(&cell.to_le_bytes());
                meta.extend_from_slice(&count.to_le_bytes());
                meta.extend_from_slice(&off.to_le_bytes());
            }
            meta_out
                .write_all(&meta)
                .map_err(|err| format!("write meta: {err}"))?;
            *meta_len += meta.len() as u64;
            meta_off_of_id[token_id as usize] = meta_off;
            tuples.clear();
            Ok(())
        };

        while let Some(std::cmp::Reverse(HeapKey(token, cell, std::cmp::Reverse(rank), doc, idx))) =
            heap.pop()
        {
            if current_token != Some(token) {
                if let Some(prev) = current_token {
                    flush_token(
                        prev,
                        &mut token_tuples,
                        &mut meta_out,
                        &mut postings_out,
                        &mut meta_len,
                        &mut postings_len,
                        &mut meta_off_of_id,
                    )?;
                }
                current_token = Some(token);
            }
            token_tuples.push((cell, rank, doc));
            readers[idx].advance()?;
            if let Some((t, c, rk, d)) = readers[idx].current {
                heap.push(std::cmp::Reverse(HeapKey(t, c, std::cmp::Reverse(rk), d, idx)));
            }
        }
        if let Some(prev) = current_token {
            flush_token(
                prev,
                &mut token_tuples,
                &mut meta_out,
                &mut postings_out,
                &mut meta_len,
                &mut postings_len,
                &mut meta_off_of_id,
            )?;
        }
        meta_out.flush().map_err(|err| err.to_string())?;
        postings_out.flush().map_err(|err| err.to_string())?;

        // Dict blob + index.
        let dict_index_len = token_count as u64 * DICT_ENTRY_BYTES;
        let mut dict_blob: Vec<u8> = Vec::new();
        let mut dict_index: Vec<u8> = Vec::with_capacity(dict_index_len as usize);
        for (token, id) in &sorted {
            let blob_off = dict_blob.len() as u64;
            let len = token.len().min(u16::MAX as usize) as u64;
            dict_blob.extend_from_slice(&token.as_bytes()[..len as usize]);
            dict_index
                .extend_from_slice(&((blob_off & 0xffff_ffff_ffff) | (len << 48)).to_le_bytes());
            dict_index.extend_from_slice(&meta_off_of_id[*id as usize].to_le_bytes());
        }

        // Assemble the final file.
        let docs_len = self.doc_count as u64 * DOC_RECORD_BYTES;
        let dict_index_off = HEADER_BYTES;
        let dict_blob_off = dict_index_off + dict_index.len() as u64;
        let meta_off = dict_blob_off + dict_blob.len() as u64;
        let postings_off = meta_off + meta_len;
        let docs_off = postings_off + postings_len;
        let strings_off = docs_off + docs_len;

        let mut header = [0u8; HEADER_BYTES as usize];
        header[0..4].copy_from_slice(&SEARCHDB_MAGIC.to_le_bytes());
        header[4..8].copy_from_slice(&SEARCHDB_VERSION.to_le_bytes());
        header[8..12].copy_from_slice(&self.doc_count.to_le_bytes());
        header[12..16].copy_from_slice(&(token_count as u32).to_le_bytes());
        header[16..24].copy_from_slice(&dict_index_off.to_le_bytes());
        header[24..32].copy_from_slice(&dict_blob_off.to_le_bytes());
        header[32..40].copy_from_slice(&meta_off.to_le_bytes());
        header[40..48].copy_from_slice(&postings_off.to_le_bytes());
        header[48..56].copy_from_slice(&docs_off.to_le_bytes());
        header[56..64].copy_from_slice(&strings_off.to_le_bytes());
        header[64..72].copy_from_slice(&self.strings_len.to_le_bytes());

        let mut out = BufWriter::with_capacity(
            1 << 22,
            File::create(&self.out_path)
                .map_err(|err| format!("create {}: {err}", self.out_path.display()))?,
        );
        out.write_all(&header).map_err(|err| err.to_string())?;
        out.write_all(&dict_index).map_err(|err| err.to_string())?;
        out.write_all(&dict_blob).map_err(|err| err.to_string())?;
        for tmp in ["meta.tmp", "postings.tmp", "docs.tmp", "strings.tmp"] {
            let mut reader = BufReader::with_capacity(
                1 << 22,
                File::open(self.spill_dir.join(tmp))
                    .map_err(|err| format!("open {tmp}: {err}"))?,
            );
            std::io::copy(&mut reader, &mut out).map_err(|err| format!("copy {tmp}: {err}"))?;
        }
        out.flush().map_err(|err| err.to_string())?;
        let file_bytes = strings_off + self.strings_len;
        let _ = fs::remove_dir_all(&self.spill_dir);
        Ok(SearchDbStats {
            docs: self.doc_count as u64,
            tokens: token_count as u64,
            file_bytes,
        })
    }
}

// --- Reader ---

pub struct SearchDb {
    file: File,
    doc_count: u32,
    token_count: u32,
    dict_index_off: u64,
    dict_blob_off: u64,
    meta_off: u64,
    postings_off: u64,
    docs_off: u64,
    strings_off: u64,
}

impl SearchDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
        let mut header = [0u8; HEADER_BYTES as usize];
        file.read_exact_at(&mut header, 0)
            .map_err(|err| format!("read header: {err}"))?;
        if u32::from_le_bytes(header[0..4].try_into().unwrap()) != SEARCHDB_MAGIC {
            return Err("bad searchdb magic".into());
        }
        let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
        if version != SEARCHDB_VERSION {
            return Err(format!("unsupported searchdb version {version}"));
        }
        Ok(Self {
            file,
            doc_count: u32::from_le_bytes(header[8..12].try_into().unwrap()),
            token_count: u32::from_le_bytes(header[12..16].try_into().unwrap()),
            dict_index_off: u64::from_le_bytes(header[16..24].try_into().unwrap()),
            dict_blob_off: u64::from_le_bytes(header[24..32].try_into().unwrap()),
            meta_off: u64::from_le_bytes(header[32..40].try_into().unwrap()),
            postings_off: u64::from_le_bytes(header[40..48].try_into().unwrap()),
            docs_off: u64::from_le_bytes(header[48..56].try_into().unwrap()),
            strings_off: u64::from_le_bytes(header[56..64].try_into().unwrap()),
        })
    }

    pub fn doc_count(&self) -> u32 {
        self.doc_count
    }

    fn dict_entry(&self, slot: u32) -> Result<(u64, u16, u64), String> {
        let mut buf = [0u8; 16];
        self.file
            .read_exact_at(&mut buf, self.dict_index_off + slot as u64 * DICT_ENTRY_BYTES)
            .map_err(|err| format!("dict entry: {err}"))?;
        let packed = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let meta = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        Ok((packed & 0xffff_ffff_ffff, (packed >> 48) as u16, meta))
    }

    fn dict_token(&self, slot: u32) -> Result<String, String> {
        let (off, len, _) = self.dict_entry(slot)?;
        let mut buf = vec![0u8; len as usize];
        self.file
            .read_exact_at(&mut buf, self.dict_blob_off + off)
            .map_err(|err| format!("dict token: {err}"))?;
        String::from_utf8(buf).map_err(|_| "dict token utf8".into())
    }

    /// First dictionary slot whose token is >= `needle`.
    fn lower_bound(&self, needle: &str) -> Result<u32, String> {
        let (mut lo, mut hi) = (0u32, self.token_count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.dict_token(mid)?.as_str() < needle {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// Gather candidate doc ids for one token slot: global head + rings of
    /// cells around `near`.
    fn gather_token(
        &self,
        slot: u32,
        near_cell: Option<(i64, i64)>,
        out: &mut HashSet<u32>,
        budget: usize,
    ) -> Result<(), String> {
        let (_, _, meta_off) = self.dict_entry(slot)?;
        if meta_off == u64::MAX {
            return Ok(());
        }
        let mut fixed = [0u8; 10];
        self.file
            .read_exact_at(&mut fixed, self.meta_off + meta_off)
            .map_err(|err| format!("token meta: {err}"))?;
        let _total = u32::from_le_bytes(fixed[0..4].try_into().unwrap());
        let head_count = u16::from_le_bytes(fixed[4..6].try_into().unwrap()) as usize;
        let cell_count = u32::from_le_bytes(fixed[6..10].try_into().unwrap()) as usize;

        let mut head = vec![0u8; head_count * 4];
        self.file
            .read_exact_at(&mut head, self.meta_off + meta_off + 10)
            .map_err(|err| format!("token head: {err}"))?;
        for chunk in head.chunks_exact(4) {
            out.insert(u32::from_le_bytes(chunk.try_into().unwrap()));
        }

        let Some((ncx, ncy)) = near_cell else {
            return Ok(());
        };
        // Cell table (bounded read).
        let table_bytes = (cell_count * 16).min(8 << 20);
        let mut table = vec![0u8; table_bytes];
        self.file
            .read_exact_at(
                &mut table,
                self.meta_off + meta_off + 10 + head_count as u64 * 4,
            )
            .map_err(|err| format!("token cells: {err}"))?;
        let entries: Vec<(u32, u32, u64)> = table
            .chunks_exact(16)
            .map(|c| {
                (
                    u32::from_le_bytes(c[0..4].try_into().unwrap()),
                    u32::from_le_bytes(c[4..8].try_into().unwrap()),
                    u64::from_le_bytes(c[8..16].try_into().unwrap()),
                )
            })
            .collect();

        let mut ring = 0i64;
        while ring <= MAX_RING && out.len() < budget {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    let cx = ncx + dx;
                    let cy = ncy + dy;
                    if cx < 0 || cy < 0 || cx >= GRID_DIM as i64 || cy >= GRID_DIM as i64 {
                        continue;
                    }
                    let cell = (cy as u32) * GRID_DIM + cx as u32;
                    let Ok(found) = entries.binary_search_by_key(&cell, |e| e.0) else {
                        continue;
                    };
                    let (_, count, off) = entries[found];
                    let take = (count as usize).min(budget.saturating_sub(out.len()).max(64));
                    let mut ids = vec![0u8; take * 4];
                    self.file
                        .read_exact_at(&mut ids, self.postings_off + off)
                        .map_err(|err| format!("cell postings: {err}"))?;
                    for chunk in ids.chunks_exact(4) {
                        out.insert(u32::from_le_bytes(chunk.try_into().unwrap()));
                    }
                }
            }
            ring += 1;
        }
        Ok(())
    }

    fn read_string(&self, packed: u64) -> Result<String, String> {
        let off = packed & 0xffff_ffff_ffff;
        let len = (packed >> 48) as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let mut buf = vec![0u8; len];
        self.file
            .read_exact_at(&mut buf, self.strings_off + off)
            .map_err(|err| format!("string: {err}"))?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    pub fn query(
        &self,
        text: &str,
        near: Option<LonLat>,
        limit: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let tokens = normalize_tokens(text);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let near_cell = near.map(|n| {
            let (nx, ny) = lon_lat_to_norm(n);
            (
                ((nx * GRID_DIM as f64) as i64).clamp(0, GRID_DIM as i64 - 1),
                ((ny * GRID_DIM as f64) as i64).clamp(0, GRID_DIM as i64 - 1),
            )
        });

        let last = tokens.len() - 1;
        let is_digit = |t: &str| t.chars().all(|c| c.is_ascii_digit());
        // Pure-number tokens (house numbers) are verified against the doc
        // name at scoring time instead of joining the posting intersection:
        // number posting lists are enormous and cell-capped, so intersecting
        // two capped lists loses exactly the address being searched for.
        let digit_filters: Vec<(String, bool)> = if tokens.iter().all(|t| is_digit(t)) {
            Vec::new()
        } else {
            tokens
                .iter()
                .enumerate()
                .filter(|(_, t)| is_digit(t))
                .map(|(i, t)| (t.clone(), i == last))
                .collect()
        };
        let lookup_tokens: Vec<String> = if digit_filters.is_empty() {
            tokens.clone()
        } else {
            tokens.iter().filter(|t| !is_digit(t)).cloned().collect()
        };
        let budget = if digit_filters.is_empty() {
            TOKEN_CANDIDATE_BUDGET
        } else {
            TOKEN_CANDIDATE_BUDGET * 4
        };
        let lookup_last = lookup_tokens.len() - 1;
        let mut candidate_sets: Vec<HashSet<u32>> = Vec::new();
        for (i, token) in lookup_tokens.iter().enumerate() {
            let mut set = HashSet::new();
            let start = self.lower_bound(token)?;
            let mut slot = start;
            let mut expanded = 0usize;
            while slot < self.token_count && expanded < PREFIX_TOKEN_CAP {
                let t = self.dict_token(slot)?;
                let matches = if i == lookup_last {
                    t.starts_with(token.as_str())
                } else {
                    &t == token
                };
                if !matches {
                    break;
                }
                self.gather_token(slot, near_cell, &mut set, budget)?;
                expanded += 1;
                slot += 1;
                if i != lookup_last {
                    break;
                }
            }
            if set.is_empty() {
                return Ok(Vec::new());
            }
            candidate_sets.push(set);
        }

        // Intersect (smallest first).
        candidate_sets.sort_by_key(|s| s.len());
        let (first, rest) = candidate_sets.split_first().unwrap();
        let mut candidates: Vec<u32> = first
            .iter()
            .copied()
            .filter(|id| rest.iter().all(|s| s.contains(id)))
            .collect();
        candidates.truncate(8000);

        let normalized_query = tokens.join(" ");
        let query_has_number = tokens.iter().any(|t| t.chars().all(|c| c.is_ascii_digit()));
        let mut results: Vec<SearchResult> = Vec::with_capacity(candidates.len());
        let mut fallback: Vec<SearchResult> = Vec::new();
        for doc_id in candidates {
            if doc_id >= self.doc_count {
                continue;
            }
            let mut rec = [0u8; DOC_RECORD_BYTES as usize];
            self.file
                .read_exact_at(&mut rec, self.docs_off + doc_id as u64 * DOC_RECORD_BYTES)
                .map_err(|err| format!("doc: {err}"))?;
            let x = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let y = u32::from_le_bytes(rec[4..8].try_into().unwrap());
            let category = Category::from_u16(u16::from_le_bytes(rec[8..10].try_into().unwrap()));
            let rank = rec[10];
            let name = self.read_string(u64::from_le_bytes(rec[12..20].try_into().unwrap()))?;
            let pos = fixed_to_lon_lat(x, y);
            let name_norm = normalize_tokens(&name).join(" ");
            let digits_match = digit_filters.iter().all(|(digit, is_prefix)| {
                name_norm.split(' ').any(|w| {
                    if *is_prefix {
                        w.starts_with(digit.as_str())
                    } else {
                        w == digit
                    }
                })
            });
            let distance_m = near.map(|n| haversine_m(n, pos));
            let score = score_search_hit(
                category,
                rank,
                &name_norm,
                &normalized_query,
                false,
                query_has_number,
                distance_m,
            );
            let result = SearchResult {
                doc_id,
                name,
                secondary: String::new(), // filled below for the top hits only
                category,
                pos,
                distance_m,
                score,
            };
            if digits_match {
                results.push(result);
            } else {
                fallback.push(result);
            }
        }
        // If the exact house number doesn\'t exist, fall back to the
        // number-less matches (the street itself) instead of zero results.
        if results.is_empty() {
            results = fallback;
        }
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        // Secondary strings only for the returned page.
        for result in &mut results {
            let mut rec = [0u8; DOC_RECORD_BYTES as usize];
            self.file
                .read_exact_at(
                    &mut rec,
                    self.docs_off + result.doc_id as u64 * DOC_RECORD_BYTES,
                )
                .map_err(|err| format!("doc: {err}"))?;
            result.secondary =
                self.read_string(u64::from_le_bytes(rec[20..28].try_into().unwrap()))?;
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_db(dir: &Path) -> SearchDb {
        let path = dir.join("test.searchdb");
        let mut builder = SearchDbBuilder::create(&path).unwrap();
        builder
            .add_doc(
                "Brussel",
                "",
                LonLat::new(4.3517, 50.8466),
                Category::City,
                255,
            )
            .unwrap();
        builder
            .add_doc(
                "Brusselsestraat",
                "Amsterdam",
                LonLat::new(4.9, 52.37),
                Category::Street,
                90,
            )
            .unwrap();
        builder
            .add_doc(
                "Brusselsestraat 12",
                "Amsterdam",
                LonLat::new(4.9001, 52.3701),
                Category::Address,
                20,
            )
            .unwrap();
        builder
            .add_doc(
                "Albert Heijn",
                "Vijzelstraat",
                LonLat::new(4.891, 52.362),
                Category::Supermarket,
                85,
            )
            .unwrap();
        builder.finish().unwrap();
        SearchDb::open(&path).unwrap()
    }

    #[test]
    fn city_beats_local_street() {
        let dir = std::env::temp_dir().join(format!("searchdb_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = build_test_db(&dir);
        // From Amsterdam, "brussel" must mean the city 170km away, not the
        // street around the corner.
        let results = db
            .query("brussel", Some(LonLat::new(4.89, 52.37)), 5)
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "Brussel");
        assert_eq!(results[0].category, Category::City);
        assert!(results.iter().any(|r| r.name == "Brusselsestraat"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn address_intent_with_number() {
        let dir = std::env::temp_dir().join(format!("searchdb_test_addr_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = build_test_db(&dir);
        let results = db
            .query("brusselsestraat 12", Some(LonLat::new(4.89, 52.37)), 5)
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "Brusselsestraat 12");
        assert_eq!(results[0].category, Category::Address);
        assert_eq!(results[0].secondary, "Amsterdam");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefix_and_no_near() {
        let dir = std::env::temp_dir().join(format!("searchdb_test_pfx_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let db = build_test_db(&dir);
        let results = db.query("albert he", None, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Albert Heijn");
        let _ = fs::remove_dir_all(&dir);
    }
}
