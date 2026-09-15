use crate::error::GitError;
use crate::oid::ObjectId;
use crate::tree::{Tree, TreeEntry};
use crate::bounded_read::{self, Bytes, Scratch, ScratchLease};
use crate::memory::{MemoryAccount, Reservation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeChangeKind { Added, Deleted, Modified, ModeOnly, TypeChanged }

/// A changed leaf (regular file, symlink or gitlink), with repository-relative
/// UTF-8 path. File/directory replacements expand to deletion/addition leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChangeRecord {
    pub path: String,
    pub kind: TreeChangeKind,
    pub old: Option<(ObjectId, u32)>,
    pub new: Option<(ObjectId, u32)>,
    /// Always false for this file-expanded API; no directory summaries are
    /// substituted for file records, including when traversal is exhausted.
    pub is_tree_recursive_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeDiffExhaustion { Records, ResultBytes, ScratchBytes, PathLength, Depth, MemoryAccount }

#[derive(Debug, Clone)]
pub struct TreeDiffLimits {
    pub max_records: usize,
    pub max_result_bytes: usize,
    pub max_scratch_bytes: usize,
    /// Maximum UTF-8 bytes in a repository-relative file path.
    pub max_path_bytes: usize,
    /// Root tree is depth zero. Values above 256 are clamped for stack safety.
    pub max_depth: usize,
}

impl Default for TreeDiffLimits {
    fn default() -> Self {
        Self { max_records: 20_000, max_result_bytes: 16 * 1024 * 1024,
            max_scratch_bytes: 32 * 1024 * 1024, max_path_bytes: 4096, max_depth: 128 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeDiffCounters {
    pub trees_decoded: u64,
    /// Entries parsed in decoded trees, including unchanged siblings.
    pub entries_visited: u64,
    /// Decoded tree payload bytes, excluding pack delta bases.
    pub object_bytes: u64,
    /// Compressed loose/pack bytes read, including delta bases; excludes index IO.
    pub bytes_read: u64,
    pub scratch_peak_bytes: usize,
    /// Retained record capacity plus path capacities.
    pub result_bytes: usize,
}

/// Incomplete manifests must not be presented as the whole comparison.
/// Results are sorted by UTF-8 path bytes even when incomplete. Record/result
/// refusal continues counting within traversal limits; it need not be a prefix.
#[derive(Debug)]
pub struct BoundedTreeDiff {
    pub records: Vec<TreeChangeRecord>,
    pub complete: bool,
    /// Exact number of omitted leaf records iff `omitted_records_exact`.
    /// Otherwise a lower bound, excluding the unopened subtree comparisons.
    pub omitted_records: u64,
    pub omitted_records_exact: bool,
    pub omitted_subtrees: u64,
    /// First refusal in deterministic traversal order.
    pub reason: Option<TreeDiffExhaustion>,
    pub counters: TreeDiffCounters,
    /// Keep this with records if transferring their ownership. The result is
    /// deliberately not Clone: copying records requires fresh admission.
    pub reservation: Reservation,
}

impl BoundedTreeDiff {
    pub(crate) fn empty(account: &MemoryAccount) -> Self {
        Self { records: Vec::new(), complete: true, omitted_records: 0,
            omitted_records_exact: true, omitted_subtrees: 0, reason: None,
            counters: TreeDiffCounters::default(), reservation: account.try_reserve(0).unwrap() }
    }

    pub(crate) fn exhaust(&mut self, reason: TreeDiffExhaustion, subtree: bool) {
        self.complete = false;
        self.reason.get_or_insert(reason);
        if subtree {
            self.omitted_subtrees = self.omitted_subtrees.saturating_add(1);
            self.omitted_records_exact = false;
        } else { self.omitted_records = self.omitted_records.saturating_add(1); }
    }
}

#[derive(Clone, Copy)]
struct RawTreeEntry { start: usize, end: usize, mode: u32, oid: ObjectId }

fn raw_entry(data: &[u8], pos: &mut usize) -> Result<RawTreeEntry, GitError> {
    let invalid = || GitError::InvalidObject("invalid tree entry".into());
    let space = *pos + data[*pos..].iter().position(|b| *b == b' ').ok_or_else(invalid)?;
    let mode = u32::from_str_radix(std::str::from_utf8(&data[*pos..space]).map_err(|_| invalid())?, 8).map_err(|_| invalid())?;
    let start = space + 1;
    let end = start + data[start..].iter().position(|b| *b == 0).ok_or_else(invalid)?;
    let name = std::str::from_utf8(&data[start..end]).map_err(|_| invalid())?;
    if name.is_empty() || name.contains('/') || name == "." || name == ".." { return Err(invalid()); }
    let oid = ObjectId::from_slice(data.get(end + 1..end + 21).ok_or_else(invalid)?)?;
    *pos = end + 21;
    Ok(RawTreeEntry { start, end, mode, oid })
}

struct DecodedTree { raw: Bytes, entries: Vec<RawTreeEntry>, _lease: ScratchLease }

impl DecodedTree {
    fn name(&self, entry: RawTreeEntry) -> &str {
        // Validated by raw_entry before allocating the entry table.
        std::str::from_utf8(&self.raw.data[entry.start..entry.end]).unwrap()
    }
}

struct TreeDiffWalk<'a> {
    common_dir: &'a std::path::Path,
    limits: &'a TreeDiffLimits,
    scratch: Scratch,
    cancel: &'a dyn Fn() -> bool,
    result: BoundedTreeDiff,
}

impl TreeDiffWalk<'_> {
    fn decode(&mut self, oid: Option<ObjectId>) -> Result<Option<DecodedTree>, GitError> {
        let Some(oid) = oid else { return Ok(None); };
        let raw = bounded_read::read(self.common_dir, &oid, crate::ObjectKind::Tree, &self.scratch,
            &mut self.result.counters.bytes_read, self.cancel)?;
        let mut pos = 0;
        let mut count = 0usize;
        while pos < raw.data.len() { raw_entry(&raw.data, &mut pos)?; count += 1; }
        let lease = self.scratch.reserve(count.checked_mul(std::mem::size_of::<RawTreeEntry>())
            .ok_or(GitError::TreeDiffLimit(TreeDiffExhaustion::ScratchBytes))?)?;
        let mut entries = Vec::with_capacity(count);
        pos = 0;
        while pos < raw.data.len() { entries.push(raw_entry(&raw.data, &mut pos)?); }
        // Git tree order treats directories as name + '/'; match by bare name
        // here, then sort the final file manifest by its full path.
        entries.sort_unstable_by(|a, b| raw.data[a.start..a.end].cmp(&raw.data[b.start..b.end]));
        if entries.windows(2).any(|p| raw.data[p[0].start..p[0].end] == raw.data[p[1].start..p[1].end]) {
            return Err(GitError::InvalidObject("duplicate tree name".into()));
        }
        self.result.counters.trees_decoded += 1;
        self.result.counters.entries_visited += count as u64;
        self.result.counters.object_bytes += raw.data.len() as u64;
        Ok(Some(DecodedTree { raw, entries, _lease: lease }))
    }

    fn record(&mut self, prefix: &str, name: &str, old: Option<(ObjectId, u32)>, new: Option<(ObjectId, u32)>) {
        let path_len = prefix.len().saturating_add(name.len());
        let result = &mut self.result;
        if path_len > self.limits.max_path_bytes { result.exhaust(TreeDiffExhaustion::PathLength, false); return; }
        if result.records.len() >= self.limits.max_records { result.exhaust(TreeDiffExhaustion::Records, false); return; }
        let old_slots = result.records.capacity() * std::mem::size_of::<TreeChangeRecord>();
        let capacity = if result.records.len() == result.records.capacity() {
            result.records.capacity().saturating_mul(2).max(1).min(self.limits.max_records)
        } else { result.records.capacity() };
        let slots = capacity.saturating_mul(std::mem::size_of::<TreeChangeRecord>());
        let bytes = result.reservation.bytes().saturating_sub(old_slots).saturating_add(slots).saturating_add(path_len);
        if bytes > self.limits.max_result_bytes { result.exhaust(TreeDiffExhaustion::ResultBytes, false); return; }
        // Account both old/new backing arrays during replacement, before any
        // allocation. Path ownership moves; its reservation stays continuous.
        let additional = if slots != old_slots { slots.saturating_add(path_len) } else { path_len };
        let Some(mut lease) = self.scratch.account.try_reserve(additional) else { result.exhaust(TreeDiffExhaustion::MemoryAccount, false); return; };
        if slots != old_slots {
            let mut records = Vec::with_capacity(capacity);
            records.append(&mut result.records);
            result.records = records;
            assert!(result.reservation.shrink_to(result.reservation.bytes() - old_slots));
        }
        assert!(result.reservation.merge(&mut lease));
        let mut path = String::with_capacity(path_len);
        path.push_str(prefix);
        path.push_str(name);
        let kind = match (old, new) {
            (None, _) => TreeChangeKind::Added,
            (_, None) => TreeChangeKind::Deleted,
            (Some((a, am)), Some((b, bm))) => {
                if am & 0o170000 != bm & 0o170000 { TreeChangeKind::TypeChanged }
                else if a == b { TreeChangeKind::ModeOnly }
                else { TreeChangeKind::Modified }
            }
        };
        result.records.push(TreeChangeRecord { path, kind, old, new, is_tree_recursive_root: false });
    }

    fn visit(&mut self, old: Option<ObjectId>, new: Option<ObjectId>, prefix: &str, depth: usize) -> Result<(), GitError> {
        if (self.cancel)() { return Err(GitError::Cancelled); }
        if old == new { return Ok(()); }
        if depth > self.limits.max_depth.min(256) {
            self.result.exhaust(TreeDiffExhaustion::Depth, true); return Ok(());
        }
        let decoded = (|| {
            let frame = self.scratch.reserve(2048)?;
            let old = self.decode(old)?;
            let new = self.decode(new)?;
            Ok((frame, old, new))
        })();
        let (_frame, old, new) = match decoded {
            Ok(trees) => trees,
            Err(GitError::TreeDiffLimit(reason)) => { self.result.exhaust(reason, true); return Ok(()); }
            Err(error) => return Err(error),
        };
        let old_entries = old.as_ref().map_or(&[][..], |t| t.entries.as_slice());
        let new_entries = new.as_ref().map_or(&[][..], |t| t.entries.as_slice());
        let (mut i, mut j) = (0, 0);
        while i < old_entries.len() || j < new_entries.len() {
            let a = old_entries.get(i).copied();
            let b = new_entries.get(j).copied();
            let an = a.map(|e| old.as_ref().unwrap().name(e));
            let bn = b.map(|e| new.as_ref().unwrap().name(e));
            let (a, b, name) = match (an, bn) {
                (Some(an), Some(bn)) if an == bn => { i += 1; j += 1; (a, b, an) }
                (Some(an), Some(bn)) if an < bn => { i += 1; (a, None, an) }
                (Some(an), None) => { i += 1; (a, None, an) }
                (_, Some(bn)) => { j += 1; (None, b, bn) }
                _ => unreachable!(),
            };
            if a.map(|e| (e.oid, e.mode)) == b.map(|e| (e.oid, e.mode)) { continue; }
            let at = a.filter(|e| e.mode == 0o040000);
            let bt = b.filter(|e| e.mode == 0o040000);
            let af = a.filter(|e| e.mode != 0o040000).map(|e| (e.oid, e.mode));
            let bf = b.filter(|e| e.mode != 0o040000).map(|e| (e.oid, e.mode));
            if af.is_some() || bf.is_some() { self.record(prefix, name, af, bf); }
            if at.is_some() || bt.is_some() {
                if at.map(|e| e.oid) == bt.map(|e| e.oid) { continue; }
                if depth >= self.limits.max_depth.min(256) { self.result.exhaust(TreeDiffExhaustion::Depth, true); continue; }
                let length = prefix.len().saturating_add(name.len()).saturating_add(1);
                if length > self.limits.max_path_bytes { self.result.exhaust(TreeDiffExhaustion::PathLength, true); continue; }
                let _lease = match self.scratch.reserve(length) {
                    Ok(lease) => lease,
                    Err(GitError::TreeDiffLimit(reason)) => { self.result.exhaust(reason, true); continue; }
                    Err(error) => return Err(error),
                };
                let mut path = String::with_capacity(length);
                path.push_str(prefix); path.push_str(name); path.push('/');
                self.visit(at.map(|e| e.oid), bt.map(|e| e.oid), &path, depth + 1)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn bounded_tree_diff(common_dir: &std::path::Path, old: Option<ObjectId>, new: Option<ObjectId>,
    limits: &TreeDiffLimits, account: &MemoryAccount, cancel: &dyn Fn() -> bool, blob_reads: &mut u64) -> Result<BoundedTreeDiff, GitError>
{
    let mut walk = TreeDiffWalk { common_dir, limits, scratch: Scratch::new(account, limits.max_scratch_bytes),
        cancel, result: BoundedTreeDiff::empty(account) };
    let visited = walk.visit(old, new, "", 0);
    *blob_reads = blob_reads.saturating_add(walk.scratch.blob_reads.get());
    visited?;
    if cancel() { return Err(GitError::Cancelled); }
    walk.result.records.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    walk.result.counters.scratch_peak_bytes = walk.scratch.peak();
    walk.result.counters.result_bytes = walk.result.reservation.bytes();
    Ok(walk.result)
}

/// A single diff operation on lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    Equal {
        old_index: usize,
        new_index: usize,
        len: usize,
    },
    Insert {
        new_index: usize,
        len: usize,
    },
    Delete {
        old_index: usize,
        len: usize,
    },
}

/// A diff between two files (blobs).
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_oid: Option<ObjectId>,
    pub new_oid: Option<ObjectId>,
    pub ops: Vec<DiffOp>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

/// A change detected in a tree diff.
#[derive(Debug, Clone)]
pub enum TreeChange {
    Added {
        path: String,
        oid: ObjectId,
        mode: u32,
    },
    Deleted {
        path: String,
        oid: ObjectId,
        mode: u32,
    },
    Modified {
        path: String,
        old_oid: ObjectId,
        new_oid: ObjectId,
        old_mode: u32,
        new_mode: u32,
    },
}

/// Myers diff algorithm on line-split text.
///
/// Returns a list of DiffOps describing how to transform `old` into `new`.
pub fn diff_lines(old_text: &str, new_text: &str) -> Vec<DiffOp> {
    let old_lines: Vec<&str> = split_lines(old_text);
    let new_lines: Vec<&str> = split_lines(new_text);
    let n = old_lines.len();
    let m = new_lines.len();

    if n == 0 && m == 0 {
        return vec![];
    }
    if n == 0 {
        return vec![DiffOp::Insert {
            new_index: 0,
            len: m,
        }];
    }
    if m == 0 {
        return vec![DiffOp::Delete {
            old_index: 0,
            len: n,
        }];
    }

    // Myers algorithm - find shortest edit script
    let max_d = n + m;
    // v[k] = furthest reaching x on diagonal k
    // We offset k by max_d so indices are non-negative
    let v_size = 2 * max_d + 1;
    let mut v = vec![0i64; v_size];
    let mut trace: Vec<Vec<i64>> = Vec::new();

    let offset = max_d as i64;

    'outer: for d in 0..=(max_d as i64) {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let ki = (k + offset) as usize;
            let x: i64;
            if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
                x = v[ki + 1]; // move down
            } else {
                x = v[ki - 1] + 1; // move right
            }
            let mut x = x;
            let mut y = x - k;

            // Follow diagonal (matching lines)
            while (x as usize) < n
                && (y as usize) < m
                && old_lines[x as usize] == new_lines[y as usize]
            {
                x += 1;
                y += 1;
            }

            v[ki] = x;

            if (x as usize) >= n && (y as usize) >= m {
                break 'outer;
            }
            k += 2;
        }
    }

    // Backtrack to find the actual edit script
    let edits = backtrack(&trace, n, m, offset);
    compress_edits(&edits, n, m)
}

/// Represent individual edit steps
#[derive(Debug, Clone, Copy, PartialEq)]
enum Edit {
    Keep,   // diagonal move
    Insert, // down move
    Delete, // right move
}

fn backtrack(trace: &[Vec<i64>], n: usize, m: usize, offset: i64) -> Vec<Edit> {
    let mut x = n as i64;
    let mut y = m as i64;
    let mut edits = Vec::new();

    for d in (0..trace.len()).rev() {
        let v = &trace[d];
        let d = d as i64;
        let k = x - y;
        let ki = (k + offset) as usize;

        let prev_k;
        if k == -d || (k != d && v[ki - 1] < v[ki + 1]) {
            prev_k = k + 1; // came from above (insert)
        } else {
            prev_k = k - 1; // came from left (delete)
        }

        let prev_ki = (prev_k + offset) as usize;
        let prev_x = v[prev_ki];
        let prev_y = prev_x - prev_k;

        // Diagonal moves (matches)
        while x > prev_x && y > prev_y {
            edits.push(Edit::Keep);
            x -= 1;
            y -= 1;
        }

        if d > 0 {
            if prev_k == k + 1 {
                edits.push(Edit::Insert);
                y -= 1;
            } else {
                edits.push(Edit::Delete);
                x -= 1;
            }
        }
    }

    edits.reverse();
    edits
}

fn compress_edits(edits: &[Edit], _n: usize, _m: usize) -> Vec<DiffOp> {
    let mut ops = Vec::new();
    let mut old_idx = 0usize;
    let mut new_idx = 0usize;
    let mut i = 0;

    while i < edits.len() {
        match edits[i] {
            Edit::Keep => {
                let start_old = old_idx;
                let start_new = new_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Keep {
                    len += 1;
                    old_idx += 1;
                    new_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Equal {
                    old_index: start_old,
                    new_index: start_new,
                    len,
                });
            }
            Edit::Delete => {
                let start = old_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Delete {
                    len += 1;
                    old_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Delete {
                    old_index: start,
                    len,
                });
            }
            Edit::Insert => {
                let start = new_idx;
                let mut len = 0;
                while i < edits.len() && edits[i] == Edit::Insert {
                    len += 1;
                    new_idx += 1;
                    i += 1;
                }
                ops.push(DiffOp::Insert {
                    new_index: start,
                    len,
                });
            }
        }
    }
    ops
}

// ---------------------------------------------------------------- bounded

/// How a line ends in the source text: nothing (the last line of a text
/// without a final newline), `\n`, or `\r\n`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    None,
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::None => "",
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
    pub fn len(self) -> usize {
        self.as_str().len()
    }
}

/// One line of an endpoint: the byte range of its content (without the
/// terminator) and how it ended. Byte offsets index the exact UTF-8 text
/// the index was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRecord {
    pub start: usize,
    pub end: usize,
    pub ending: LineEnding,
}

impl LineRecord {
    /// The content range, terminator excluded.
    pub fn content(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
    /// The full range, terminator included.
    pub fn full(&self) -> std::ops::Range<usize> {
        self.start..self.end + self.ending.len()
    }
}

/// The exact line structure of one endpoint. Lines split where
/// `str::lines()` splits them (at `\n`, a preceding `\r` belonging to the
/// terminator), so line contents compare exactly as `diff_lines` compares
/// them, but byte offsets, terminators and the final-newline state are
/// retained so the text can be reconstructed byte for byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineIndex {
    pub lines: Vec<LineRecord>,
    /// The text length in bytes.
    pub bytes: usize,
    /// The text ends with a line terminator.
    pub final_newline: bool,
}

impl LineIndex {
    pub fn of(text: &str) -> LineIndex {
        let bytes = text.as_bytes();
        let mut lines = Vec::new();
        let mut start = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'\n' {
                let (end, ending) = if i > start && bytes[i - 1] == b'\r' { (i - 1, LineEnding::CrLf) } else { (i, LineEnding::Lf) };
                lines.push(LineRecord { start, end, ending });
                start = i + 1;
            }
            i += 1;
        }
        let final_newline = !bytes.is_empty() && bytes[bytes.len() - 1] == b'\n';
        if start < bytes.len() {
            lines.push(LineRecord { start, end: bytes.len(), ending: LineEnding::None });
        }
        LineIndex { lines, bytes: bytes.len(), final_newline }
    }
    pub fn len(&self) -> usize {
        self.lines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
    /// The content of line `i` of `text` (the text this index was built from).
    pub fn content<'a>(&self, text: &'a str, i: usize) -> &'a str {
        let r = &self.lines[i];
        &text[r.start..r.end]
    }
    /// The line holding byte `offset`, or the last line for an offset at the
    /// end of the text; `None` for an empty text.
    pub fn line_of(&self, offset: usize) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        match self.lines.binary_search_by(|r| r.start.cmp(&offset)) {
            Ok(i) => Some(i),
            Err(i) => Some(i.saturating_sub(1)),
        }
    }
    /// Rebuild the text from its lines: exact when `text` is the text the
    /// index was built from.
    pub fn reconstruct(&self, text: &str) -> String {
        let mut out = String::with_capacity(self.bytes);
        for r in &self.lines {
            out.push_str(&text[r.start..r.end]);
            out.push_str(r.ending.as_str());
        }
        out
    }
}

/// Explicit bounds on a line diff: per endpoint at most `max_bytes` and
/// `max_lines`, at most `max_trace_bytes` of search trace and
/// `max_frontier_ops` frontier operations (diagonal steps and snake
/// advances) for the whole search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub max_trace_bytes: usize,
    pub max_frontier_ops: u64,
}

impl Default for DiffLimits {
    fn default() -> Self {
        DiffLimits { max_bytes: 4 * 1024 * 1024, max_lines: 20_000, max_trace_bytes: 8 * 1024 * 1024, max_frontier_ops: 2_000_000 }
    }
}

/// Why a bounded diff stopped short of a complete line correspondence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exhaustion {
    /// An endpoint exceeds the byte or line limit.
    TooLarge { old_bytes: usize, new_bytes: usize, old_lines: usize, new_lines: usize },
    /// The search trace would exceed its storage limit (bytes it reached).
    TraceStorage { bytes: usize },
    /// The frontier operation limit was reached.
    FrontierOps { ops: u64 },
    /// The caller cancelled between two fronts.
    Cancelled,
}

impl Exhaustion {
    /// The label shown for the hunk without a correspondence.
    pub const LABEL: &'static str = "Line correspondence unavailable";
}

/// The one replacement hunk of an exhausted diff: old lines
/// `old_index..old_index + old_len` and new lines `new_index..new_index +
/// new_len` between the verified equal prefix and suffix, with no line
/// identities claimed inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableHunk {
    pub old_index: usize,
    pub old_len: usize,
    pub new_index: usize,
    pub new_len: usize,
    pub reason: Exhaustion,
}

/// A bounded line diff: `diff_lines`' `Equal`/`Insert`/`Delete` run
/// contract over exact UTF-8 text, the line structure of both endpoints,
/// and, when the search was exhausted, the verified equal prefix and suffix
/// with one explicit replacement hunk between them (`unavailable`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedDiff {
    pub ops: Vec<DiffOp>,
    pub old: LineIndex,
    pub new: LineIndex,
    /// Set when the search stopped short: the ops then hold the equal
    /// prefix, one `Delete` and one `Insert` covering this hunk, and the
    /// equal suffix.
    pub unavailable: Option<UnavailableHunk>,
    /// Frontier operations spent.
    pub frontier_ops: u64,
    /// Trace bytes used at most.
    pub trace_bytes: usize,
}

impl BoundedDiff {
    /// Every line correspondence was established.
    pub fn is_complete(&self) -> bool {
        self.unavailable.is_none()
    }
    /// The equal runs, in order.
    pub fn equal_runs(&self) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
        self.ops.iter().filter_map(|op| match op {
            DiffOp::Equal { old_index, new_index, len } => Some((*old_index, *new_index, *len)),
            _ => None,
        })
    }
    /// Every (old line, new line) pair of the equal runs, in order.
    pub fn equal_pairs(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.equal_runs().flat_map(|(o, n, len)| (0..len).map(move |k| (o + k, n + k)))
    }
    /// The new line an old line maps to, when it is an equal line.
    pub fn map_old_to_new(&self, old_line: usize) -> Option<usize> {
        let runs: Vec<(usize, usize, usize)> = self.equal_runs().collect();
        let i = runs.partition_point(|(o, _, _)| *o <= old_line);
        let (o, n, len) = *runs.get(i.checked_sub(1)?)?;
        (old_line < o + len).then_some(n + (old_line - o))
    }
    /// The old line a new line maps to, when it is an equal line.
    pub fn map_new_to_old(&self, new_line: usize) -> Option<usize> {
        let runs: Vec<(usize, usize, usize)> = self.equal_runs().collect();
        let i = runs.partition_point(|(_, n, _)| *n <= new_line);
        let (o, n, len) = *runs.get(i.checked_sub(1)?)?;
        (new_line < n + len).then_some(o + (new_line - n))
    }
    /// The same diff seen from the other side (new → old): the stored
    /// mapping inverted, so a traversal back uses exactly the same line
    /// identities instead of a fresh search with its own tie-breaking.
    pub fn inverse(&self) -> BoundedDiff {
        let ops = self
            .ops
            .iter()
            .map(|op| match *op {
                DiffOp::Equal { old_index, new_index, len } => DiffOp::Equal { old_index: new_index, new_index: old_index, len },
                DiffOp::Insert { new_index, len } => DiffOp::Delete { old_index: new_index, len },
                DiffOp::Delete { old_index, len } => DiffOp::Insert { new_index: old_index, len },
            })
            .collect();
        // a replacement hunk is Delete then Insert; inverted it stays in that order
        let ops = reorder_replacements(ops);
        BoundedDiff {
            ops,
            old: self.new.clone(),
            new: self.old.clone(),
            unavailable: self.unavailable.as_ref().map(|h| UnavailableHunk { old_index: h.new_index, old_len: h.new_len, new_index: h.old_index, new_len: h.old_len, reason: h.reason.clone() }),
            frontier_ops: self.frontier_ops,
            trace_bytes: self.trace_bytes,
        }
    }
}

/// Keep every replacement hunk as `Delete` followed by `Insert`, the order
/// `diff_lines` emits.
fn reorder_replacements(mut ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut i = 0;
    while i + 1 < ops.len() {
        if matches!(ops[i], DiffOp::Insert { .. }) && matches!(ops[i + 1], DiffOp::Delete { .. }) {
            ops.swap(i, i + 1);
        }
        i += 1;
    }
    ops
}

/// `diff_lines_bounded` without cancellation.
pub fn diff_lines_with_limits(old_text: &str, new_text: &str, limits: &DiffLimits) -> BoundedDiff {
    diff_lines_bounded(old_text, new_text, limits, &|| false)
}

/// Myers on exact UTF-8 text with explicit bounds and cancellation.
///
/// The verified equal prefix and suffix are always established. Within the
/// bounds the result is the full `Equal`/`Insert`/`Delete` run sequence of
/// `diff_lines`; when an endpoint exceeds its size limit, the trace or
/// frontier budget runs out, or `cancel` returns true between two fronts,
/// the remaining middle is returned as one `Delete` + `Insert` pair and
/// named in `unavailable` (no line identities are invented inside it).
pub fn diff_lines_bounded(old_text: &str, new_text: &str, limits: &DiffLimits, cancel: &dyn Fn() -> bool) -> BoundedDiff {
    let old = LineIndex::of(old_text);
    let new = LineIndex::of(new_text);
    let n = old.len();
    let m = new.len();
    let old_lines: Vec<&str> = (0..n).map(|i| old.content(old_text, i)).collect();
    let new_lines: Vec<&str> = (0..m).map(|i| new.content(new_text, i)).collect();
    // the verified equal prefix and suffix
    let mut p = 0usize;
    while p < n && p < m && old_lines[p] == new_lines[p] {
        p += 1;
    }
    let mut s = 0usize;
    while s < n - p && s < m - p && old_lines[n - 1 - s] == new_lines[m - 1 - s] {
        s += 1;
    }
    let (on, nm) = (n - p - s, m - p - s);
    let mut ops: Vec<DiffOp> = Vec::new();
    if p > 0 {
        ops.push(DiffOp::Equal { old_index: 0, new_index: 0, len: p });
    }
    let mut frontier_ops = 0u64;
    let mut trace_bytes = 0usize;
    let too_large = old_text.len() > limits.max_bytes || new_text.len() > limits.max_bytes || n > limits.max_lines || m > limits.max_lines;
    let middle: Result<Vec<DiffOp>, Exhaustion> = if on == 0 && nm == 0 {
        Ok(Vec::new())
    } else if on == 0 {
        Ok(vec![DiffOp::Insert { new_index: p, len: nm }])
    } else if nm == 0 {
        Ok(vec![DiffOp::Delete { old_index: p, len: on }])
    } else if too_large {
        Err(Exhaustion::TooLarge { old_bytes: old_text.len(), new_bytes: new_text.len(), old_lines: n, new_lines: m })
    } else {
        myers_bounded(&old_lines[p..n - s], &new_lines[p..m - s], limits, cancel, &mut frontier_ops, &mut trace_bytes).map(|edits| {
            compress_edits(&edits, on, nm)
                .into_iter()
                .map(|op| match op {
                    DiffOp::Equal { old_index, new_index, len } => DiffOp::Equal { old_index: old_index + p, new_index: new_index + p, len },
                    DiffOp::Insert { new_index, len } => DiffOp::Insert { new_index: new_index + p, len },
                    DiffOp::Delete { old_index, len } => DiffOp::Delete { old_index: old_index + p, len },
                })
                .collect()
        })
    };
    let unavailable = match middle {
        Ok(mid) => {
            ops.extend(mid);
            None
        }
        Err(reason) => {
            ops.push(DiffOp::Delete { old_index: p, len: on });
            ops.push(DiffOp::Insert { new_index: p, len: nm });
            Some(UnavailableHunk { old_index: p, old_len: on, new_index: p, new_len: nm, reason })
        }
    };
    if s > 0 {
        ops.push(DiffOp::Equal { old_index: n - s, new_index: m - s, len: s });
    }
    BoundedDiff { ops: merge_runs(ops), old, new, unavailable, frontier_ops, trace_bytes }
}

/// Merge adjacent runs of the same kind.
fn merge_runs(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut out: Vec<DiffOp> = Vec::with_capacity(ops.len());
    for op in ops {
        match (out.last_mut(), op) {
            (Some(DiffOp::Equal { old_index, new_index, len }), DiffOp::Equal { old_index: o, new_index: nw, len: l }) if *old_index + *len == o && *new_index + *len == nw => *len += l,
            (Some(DiffOp::Insert { new_index, len }), DiffOp::Insert { new_index: nw, len: l }) if *new_index + *len == nw => *len += l,
            (Some(DiffOp::Delete { old_index, len }), DiffOp::Delete { old_index: o, len: l }) if *old_index + *len == o => *len += l,
            (_, op) => out.push(op),
        }
    }
    out
}

/// The bounded search on a middle segment with no common prefix or suffix
/// (both non-empty). The trace keeps only the `2d + 1` diagonals a front
/// can reach, as `i32`, so its storage is `4·(D + 1)²` bytes for `D` edits.
fn myers_bounded(old: &[&str], new: &[&str], limits: &DiffLimits, cancel: &dyn Fn() -> bool, frontier_ops: &mut u64, trace_bytes: &mut usize) -> Result<Vec<Edit>, Exhaustion> {
    let n = old.len();
    let m = new.len();
    let max_d = n + m;
    let offset = max_d as i64;
    let mut v = vec![0i32; 2 * max_d + 1];
    let mut trace: Vec<i32> = Vec::new();
    let mut reached: Option<usize> = None;
    'outer: for d in 0..=max_d {
        if cancel() {
            return Err(Exhaustion::Cancelled);
        }
        let slice_len = 2 * d + 1;
        if *trace_bytes + slice_len * 4 > limits.max_trace_bytes {
            return Err(Exhaustion::TraceStorage { bytes: *trace_bytes + slice_len * 4 });
        }
        for k in -(d as i64)..=(d as i64) {
            trace.push(v[(k + offset) as usize]);
        }
        *trace_bytes += slice_len * 4;
        let mut k = -(d as i64);
        while k <= d as i64 {
            let ki = (k + offset) as usize;
            let mut x: i64 = if k == -(d as i64) || (k != d as i64 && v[ki - 1] < v[ki + 1]) { v[ki + 1] as i64 } else { v[ki - 1] as i64 + 1 };
            let mut y = x - k;
            let mut ops = 1u64;
            while (x as usize) < n && (y as usize) < m && x >= 0 && y >= 0 && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
                ops += 1;
            }
            *frontier_ops += ops;
            v[ki] = x as i32;
            if x >= n as i64 && y >= m as i64 {
                reached = Some(d);
                break 'outer;
            }
            if *frontier_ops > limits.max_frontier_ops {
                return Err(Exhaustion::FrontierOps { ops: *frontier_ops });
            }
            k += 2;
        }
    }
    let Some(dmax) = reached else { return Err(Exhaustion::FrontierOps { ops: *frontier_ops }) };
    // backtrack over the compact trace: round d's slice starts at d² and
    // holds k = -d..=d
    let mut x = n as i64;
    let mut y = m as i64;
    let mut edits: Vec<Edit> = Vec::new();
    for d in (0..=dmax).rev() {
        let base = d * d;
        let at = |k: i64| -> i64 { trace[base + (k + d as i64) as usize] as i64 };
        let k = x - y;
        if d == 0 {
            while x > 0 && y > 0 {
                edits.push(Edit::Keep);
                x -= 1;
                y -= 1;
            }
            break;
        }
        let prev_k = if k == -(d as i64) || (k != d as i64 && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            edits.push(Edit::Keep);
            x -= 1;
            y -= 1;
        }
        if prev_k == k + 1 {
            edits.push(Edit::Insert);
            y -= 1;
        } else {
            edits.push(Edit::Delete);
            x -= 1;
        }
    }
    edits.reverse();
    Ok(edits)
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![];
    }
    text.lines().collect()
}

/// Diff two blob byte arrays as text, returning a FileDiff.
pub fn diff_blobs(
    old_data: &[u8],
    new_data: &[u8],
    old_path: Option<String>,
    new_path: Option<String>,
    old_oid: Option<ObjectId>,
    new_oid: Option<ObjectId>,
) -> FileDiff {
    let old_text = String::from_utf8_lossy(old_data);
    let new_text = String::from_utf8_lossy(new_data);

    let ops = diff_lines(&old_text, &new_text);

    FileDiff {
        old_path,
        new_path,
        old_oid,
        new_oid,
        ops,
        old_lines: split_lines(&old_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        new_lines: split_lines(&new_text)
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Compare two trees recursively and return a list of changes.
/// `read_tree_fn` is called to read sub-trees as needed.
pub fn diff_trees(
    old_tree: &Tree,
    new_tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
) -> Result<Vec<TreeChange>, GitError> {
    let mut changes = Vec::new();

    // Build maps of name -> entry for both trees
    let old_map: std::collections::HashMap<&str, &TreeEntry> = old_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();
    let new_map: std::collections::HashMap<&str, &TreeEntry> = new_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    // Entries in old but not in new (deleted)
    for (name, old_entry) in &old_map {
        if !new_map.contains_key(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };
            if old_entry.is_tree() {
                // Recursively list all files under deleted directory
                let sub_tree = read_tree(&old_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Deleted { path: p, oid, mode });
                })?;
            } else {
                changes.push(TreeChange::Deleted {
                    path,
                    oid: old_entry.oid,
                    mode: old_entry.mode,
                });
            }
        }
    }

    // Entries in new but not in old (added)
    for (name, new_entry) in &new_map {
        if !old_map.contains_key(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };
            if new_entry.is_tree() {
                let sub_tree = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Added { path: p, oid, mode });
                })?;
            } else {
                changes.push(TreeChange::Added {
                    path,
                    oid: new_entry.oid,
                    mode: new_entry.mode,
                });
            }
        }
    }

    // Entries in both (check for modifications)
    for (name, old_entry) in &old_map {
        if let Some(new_entry) = new_map.get(name) {
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}{}", prefix, name)
            };

            if old_entry.oid == new_entry.oid && old_entry.mode == new_entry.mode {
                continue; // identical
            }

            if old_entry.is_tree() && new_entry.is_tree() {
                // Both are trees — recurse
                let old_sub = read_tree(&old_entry.oid)?;
                let new_sub = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                let sub_changes = diff_trees(&old_sub, &new_sub, &sub_prefix, read_tree)?;
                changes.extend(sub_changes);
            } else if old_entry.is_tree() && !new_entry.is_tree() {
                // Tree replaced by file: delete all tree contents, add file
                let sub_tree = read_tree(&old_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Deleted { path: p, oid, mode });
                })?;
                changes.push(TreeChange::Added {
                    path,
                    oid: new_entry.oid,
                    mode: new_entry.mode,
                });
            } else if !old_entry.is_tree() && new_entry.is_tree() {
                // File replaced by tree: delete file, add all tree contents
                changes.push(TreeChange::Deleted {
                    path: path.clone(),
                    oid: old_entry.oid,
                    mode: old_entry.mode,
                });
                let sub_tree = read_tree(&new_entry.oid)?;
                let sub_prefix = format!("{}/", path);
                collect_tree_entries(&sub_tree, &sub_prefix, read_tree, &mut |p, oid, mode| {
                    changes.push(TreeChange::Added { path: p, oid, mode });
                })?;
            } else {
                // Both are files, content or mode changed
                changes.push(TreeChange::Modified {
                    path,
                    old_oid: old_entry.oid,
                    new_oid: new_entry.oid,
                    old_mode: old_entry.mode,
                    new_mode: new_entry.mode,
                });
            }
        }
    }

    changes.sort_by(|a, b| {
        let pa = match a {
            TreeChange::Added { path, .. } => path,
            TreeChange::Deleted { path, .. } => path,
            TreeChange::Modified { path, .. } => path,
        };
        let pb = match b {
            TreeChange::Added { path, .. } => path,
            TreeChange::Deleted { path, .. } => path,
            TreeChange::Modified { path, .. } => path,
        };
        pa.cmp(pb)
    });

    Ok(changes)
}

/// Recursively collect all file entries in a tree.
fn collect_tree_entries(
    tree: &Tree,
    prefix: &str,
    read_tree: &mut dyn FnMut(&ObjectId) -> Result<Tree, GitError>,
    emit: &mut dyn FnMut(String, ObjectId, u32),
) -> Result<(), GitError> {
    for entry in &tree.entries {
        let path = format!("{}{}", prefix, entry.name);
        if entry.is_tree() {
            let sub = read_tree(&entry.oid)?;
            let sub_prefix = format!("{}/", path);
            collect_tree_entries(&sub, &sub_prefix, read_tree, emit)?;
        } else {
            emit(path, entry.oid, entry.mode);
        }
    }
    Ok(())
}

/// Generate a unified diff string from a FileDiff.
pub fn format_unified_diff(diff: &FileDiff, context_lines: usize) -> String {
    let mut output = String::new();

    let old_path = diff.old_path.as_deref().unwrap_or("/dev/null");
    let new_path = diff.new_path.as_deref().unwrap_or("/dev/null");
    output.push_str(&format!("--- a/{}\n", old_path));
    output.push_str(&format!("+++ b/{}\n", new_path));

    // Convert ops into hunks with context
    let hunks = build_hunks(&diff.ops, &diff.old_lines, &diff.new_lines, context_lines);
    for hunk in &hunks {
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start + 1,
            hunk.old_count,
            hunk.new_start + 1,
            hunk.new_count
        ));
        for line in &hunk.lines {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    lines: Vec<String>,
}

fn build_hunks(
    ops: &[DiffOp],
    old_lines: &[String],
    new_lines: &[String],
    context: usize,
) -> Vec<Hunk> {
    // First, expand ops into individual line edits
    let mut line_edits: Vec<(char, &str)> = Vec::new(); // (' ', '+', '-')
    let mut old_idx = 0;
    let mut new_idx = 0;

    for op in ops {
        match op {
            DiffOp::Equal { len, .. } => {
                for i in 0..*len {
                    line_edits.push((' ', &old_lines[old_idx + i]));
                }
                old_idx += len;
                new_idx += len;
            }
            DiffOp::Delete { len, .. } => {
                for i in 0..*len {
                    line_edits.push(('-', &old_lines[old_idx + i]));
                }
                old_idx += len;
            }
            DiffOp::Insert { len, .. } => {
                for i in 0..*len {
                    line_edits.push(('+', &new_lines[new_idx + i]));
                }
                new_idx += len;
            }
        }
    }

    if line_edits.is_empty() {
        return vec![];
    }

    // Find ranges of changes and group with context
    let mut change_positions: Vec<usize> = Vec::new();
    for (i, (kind, _)) in line_edits.iter().enumerate() {
        if *kind != ' ' {
            change_positions.push(i);
        }
    }

    if change_positions.is_empty() {
        return vec![];
    }

    // Group changes that overlap when context is added
    let mut groups: Vec<(usize, usize)> = Vec::new(); // (start, end) in line_edits
    let mut group_start = change_positions[0].saturating_sub(context);
    let mut group_end = (change_positions[0] + context + 1).min(line_edits.len());

    for &pos in &change_positions[1..] {
        let new_start = pos.saturating_sub(context);
        let new_end = (pos + context + 1).min(line_edits.len());
        if new_start <= group_end {
            group_end = new_end;
        } else {
            groups.push((group_start, group_end));
            group_start = new_start;
            group_end = new_end;
        }
    }
    groups.push((group_start, group_end));

    // Build hunks from groups
    let mut hunks = Vec::new();
    for (start, end) in groups {
        let mut hunk_lines = Vec::new();
        let mut old_count = 0;
        let mut new_count = 0;

        // Calculate old_start and new_start by counting through line_edits
        let mut oi = 0;
        let mut ni = 0;
        for i in 0..start {
            match line_edits[i].0 {
                ' ' => {
                    oi += 1;
                    ni += 1;
                }
                '-' => {
                    oi += 1;
                }
                '+' => {
                    ni += 1;
                }
                _ => {}
            }
        }
        let old_start = oi;
        let new_start = ni;

        for i in start..end {
            let (kind, text) = &line_edits[i];
            hunk_lines.push(format!("{}{}", kind, text));
            match kind {
                ' ' => {
                    old_count += 1;
                    new_count += 1;
                }
                '-' => {
                    old_count += 1;
                }
                '+' => {
                    new_count += 1;
                }
                _ => {}
            }
        }

        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            lines: hunk_lines,
        });
    }

    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_repo() -> (crate::test_support::TempDir, crate::Repository) {
        let dir = crate::test_support::tempdir().unwrap();
        let git = dir.path().join(".git");
        std::fs::create_dir_all(git.join("objects")).unwrap();
        let repo = crate::Repository::open_git_dir(git, dir.path().to_path_buf()).unwrap();
        (dir, repo)
    }

    fn fixture_tree(repo: &crate::Repository, entries: &[(&str, u32, ObjectId)]) -> ObjectId {
        repo.write_tree(&Tree { entries: entries.iter().map(|(name, mode, oid)| TreeEntry {
            name: (*name).into(), mode: *mode, oid: *oid,
        }).collect() }).unwrap()
    }

    #[test]
    fn bounded_tree_two_thousand_files_are_admitted_without_blob_reads() {
        let (_dir, mut repo) = tree_repo();
        let blob = repo.write_blob(b"never read this blob").unwrap();
        // Calibrate the reader counter, then make any subsequent blob read fail.
        assert_eq!(repo.read_blob(&blob).unwrap(), b"never read this blob");
        let before = repo.read_stats().blob_reads;
        let (dir, file) = blob.loose_path_components();
        std::fs::remove_file(repo.common_dir.join("objects").join(dir).join(file)).unwrap();
        let tree = repo.write_tree(&Tree { entries: (0..2000).map(|i| TreeEntry {
            name: format!("file-{i:04}"), mode: 0o100644, oid: blob,
        }).collect() }).unwrap();
        let account = MemoryAccount::new(64 * 1024 * 1024);
        let limits = TreeDiffLimits::default();
        for (old, new, kind) in [(None, Some(tree), TreeChangeKind::Added), (Some(tree), None, TreeChangeKind::Deleted)] {
            let result = repo.diff_trees_bounded(old, new, &limits, &account, &|| false).unwrap();
            assert!(result.complete);
            assert_eq!(result.records.len(), 2000);
            assert!(result.records.iter().all(|r| r.kind == kind && !r.is_tree_recursive_root));
            assert!(result.records.windows(2).all(|p| p[0].path < p[1].path));
            assert_eq!(result.omitted_records, 0);
            assert_eq!(result.counters.trees_decoded, 1);
            assert_eq!(result.counters.entries_visited, 2000);
            assert!(result.counters.object_bytes > 0);
            assert!(result.counters.bytes_read > 0);
            assert!(result.counters.scratch_peak_bytes <= limits.max_scratch_bytes);
            assert_eq!(account.reserved(), result.counters.result_bytes);
            assert_eq!(result.counters.result_bytes, result.records.capacity() * std::mem::size_of::<TreeChangeRecord>()
                + result.records.iter().map(|r| r.path.capacity()).sum::<usize>());
            drop(result);
            assert_eq!(account.reserved(), 0);
        }
        assert_eq!(repo.read_stats().blob_reads, before);
    }

    #[test]
    fn bounded_tree_record_result_path_limits_count_omitted_leaves() {
        let (_dir, mut repo) = tree_repo();
        let tree = fixture_tree(&repo, &[("a", 0o100644, ObjectId::ZERO), ("bb", 0o100644, ObjectId::ZERO), ("ccc", 0o100644, ObjectId::ZERO)]);
        let account = MemoryAccount::new(1024 * 1024);
        for (limits, reason, retained, omitted) in [
            (TreeDiffLimits { max_records: 1, ..Default::default() }, TreeDiffExhaustion::Records, 1, 2),
            (TreeDiffLimits { max_result_bytes: std::mem::size_of::<TreeChangeRecord>() + 1, ..Default::default() }, TreeDiffExhaustion::ResultBytes, 1, 2),
            (TreeDiffLimits { max_path_bytes: 1, ..Default::default() }, TreeDiffExhaustion::PathLength, 1, 2),
            (TreeDiffLimits { max_records: 0, ..Default::default() }, TreeDiffExhaustion::Records, 0, 3),
        ] {
            let result = repo.diff_trees_bounded(None, Some(tree), &limits, &account, &|| false).unwrap();
            assert!(!result.complete);
            assert_eq!(result.reason, Some(reason));
            assert_eq!(result.records.len(), retained);
            assert_eq!(result.omitted_records, omitted);
            assert!(result.omitted_records_exact);
            assert_eq!(result.omitted_subtrees, 0);
            assert!(result.counters.result_bytes <= limits.max_result_bytes);
            drop(result);
            assert_eq!(account.reserved(), 0);
        }
    }

    #[test]
    fn bounded_tree_scratch_depth_and_path_refusals_do_not_invent_counts() {
        let (_dir, mut repo) = tree_repo();
        let child = fixture_tree(&repo, &[("leaf", 0o100644, ObjectId::ZERO)]);
        let tree = fixture_tree(&repo, &[("dir", 0o040000, child)]);
        let account = MemoryAccount::new(1024 * 1024);
        for (limits, reason, decoded) in [
            (TreeDiffLimits { max_scratch_bytes: 0, ..Default::default() }, TreeDiffExhaustion::ScratchBytes, 0),
            (TreeDiffLimits { max_depth: 0, ..Default::default() }, TreeDiffExhaustion::Depth, 1),
            (TreeDiffLimits { max_path_bytes: 3, ..Default::default() }, TreeDiffExhaustion::PathLength, 1),
        ] {
            let result = repo.diff_trees_bounded(None, Some(tree), &limits, &account, &|| false).unwrap();
            assert!(!result.complete);
            assert_eq!(result.reason, Some(reason));
            assert_eq!(result.omitted_records, 0);
            assert!(!result.omitted_records_exact);
            assert_eq!(result.omitted_subtrees, 1);
            assert_eq!(result.counters.trees_decoded, decoded);
            assert!(result.records.is_empty());
            assert!(result.counters.scratch_peak_bytes <= limits.max_scratch_bytes);
        }
        let full = repo.diff_trees_bounded(None, Some(tree), &TreeDiffLimits { max_depth: 1, max_path_bytes: 8, ..Default::default() }, &account, &|| false).unwrap();
        assert!(full.complete);
        assert_eq!(full.records[0].path, "dir/leaf");
    }

    #[test]
    fn bounded_tree_shared_pressure_refuses_and_releases_scratch() {
        let (_dir, mut repo) = tree_repo();
        let tree = fixture_tree(&repo, &[("file", 0o100644, ObjectId::ZERO)]);
        let account = MemoryAccount::new(1024 * 1024);
        let pin = account.try_reserve(account.capacity()).unwrap();
        let result = repo.diff_trees_bounded(None, Some(tree), &TreeDiffLimits::default(), &account, &|| false).unwrap();
        assert_eq!(result.reason, Some(TreeDiffExhaustion::MemoryAccount));
        assert_eq!(result.omitted_subtrees, 1);
        assert_eq!(account.reserved(), account.capacity());
        drop(result);
        drop(pin);
        assert_eq!(account.reserved(), 0);
    }

    #[test]
    fn bounded_reader_blob_counter_observes_mistyped_tree_objects() {
        let (_dir, mut repo) = tree_repo();
        let blob = repo.write_blob(b"not a tree").unwrap();
        let account = MemoryAccount::new(1024 * 1024);
        assert!(matches!(repo.diff_trees_bounded(None, Some(blob), &TreeDiffLimits::default(), &account, &|| false), Err(GitError::InvalidObject(_))));
        assert_eq!(repo.read_stats().blob_reads, 1);
        assert_eq!(account.reserved(), 0);
        crate::test_support::pack_all_loose(&repo.git_dir).unwrap();
        assert!(matches!(repo.diff_trees_bounded(None, Some(blob), &TreeDiffLimits::default(), &account, &|| false), Err(GitError::InvalidObject(_))));
        assert_eq!(repo.read_stats().blob_reads, 2);
        assert_eq!(account.reserved(), 0);
    }

    #[test]
    fn bounded_tree_cancellation_discards_an_already_retained_record() {
        let (_dir, mut repo) = tree_repo();
        let child = fixture_tree(&repo, &[("file", 0o100644, ObjectId::ZERO)]);
        let root = fixture_tree(&repo, &[("a", 0o100644, ObjectId::ZERO), ("z", 0o040000, child)]);
        let account = MemoryAccount::new(1024 * 1024);
        let calls = std::cell::Cell::new(0);
        let cancel = || { let n = calls.get() + 1; calls.set(n); n == 3 };
        let result = repo.diff_trees_bounded(None, Some(root), &TreeDiffLimits::default(), &account, &cancel);
        // Calls 1/2 enter and read root; call 3 visits z after storing a.
        assert!(matches!(result, Err(GitError::Cancelled)));
        assert_eq!(calls.get(), 3);
        assert_eq!(account.reserved(), 0);
        assert!(account.peak() > 0);
        assert!(matches!(repo.diff_trees_bounded(None, None, &TreeDiffLimits::default(), &account, &|| true), Err(GitError::Cancelled)));
    }

    #[test]
    fn bounded_tree_equal_subtrees_skip_missing_objects_and_order_paths() {
        let (_dir, mut repo) = tree_repo();
        let child = fixture_tree(&repo, &[("z", 0o100644, ObjectId::ZERO)]);
        let old = fixture_tree(&repo, &[("missing", 0o040000, ObjectId::ZERO)]);
        let new = fixture_tree(&repo, &[("missing", 0o040000, ObjectId::ZERO), ("foo", 0o040000, child), ("foo.txt", 0o100644, ObjectId::ZERO)]);
        let account = MemoryAccount::new(1024 * 1024);
        let result = repo.diff_trees_bounded(Some(old), Some(new), &TreeDiffLimits::default(), &account, &|| false).unwrap();
        assert!(result.complete);
        assert_eq!(result.records.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["foo.txt", "foo/z"]);
        assert_eq!(result.counters.trees_decoded, 3);
        let empty = repo.diff_trees_bounded(Some(ObjectId::ZERO), Some(ObjectId::ZERO), &TreeDiffLimits::default(), &MemoryAccount::new(0), &|| false).unwrap();
        assert!(empty.complete);
        assert_eq!(empty.counters, TreeDiffCounters::default());
    }

    #[test]
    fn bounded_tree_modes_types_and_directory_replacements() {
        let (_dir, mut repo) = tree_repo();
        let blob = repo.write_blob(b"content").unwrap();
        let other = repo.write_blob(b"other").unwrap();
        let child = fixture_tree(&repo, &[("leaf", 0o100644, blob)]);
        let old = fixture_tree(&repo, &[("mode", 0o100644, blob), ("type", 0o100644, blob), ("dir", 0o040000, child), ("modified", 0o100644, blob), ("link", 0o160000, blob)]);
        let new = fixture_tree(&repo, &[("mode", 0o100755, blob), ("type", 0o120000, blob), ("dir", 0o100644, blob), ("modified", 0o100644, other), ("link", 0o160000, other)]);
        let account = MemoryAccount::new(1024 * 1024);
        for (old, new) in [(old, new), (new, old)] {
            let result = repo.diff_trees_bounded(Some(old), Some(new), &TreeDiffLimits::default(), &account, &|| false).unwrap();
            assert!(result.complete);
            assert_eq!(result.records.len(), 6);
            for (name, kind) in [("mode", TreeChangeKind::ModeOnly), ("type", TreeChangeKind::TypeChanged), ("modified", TreeChangeKind::Modified), ("link", TreeChangeKind::Modified)] {
                assert_eq!(result.records.iter().find(|r| r.path == name).unwrap().kind, kind);
            }
            let legacy = repo.diff_trees(&old, &new).unwrap();
            assert_eq!(legacy.len(), result.records.len());
        }
        assert_eq!(repo.read_stats().blob_reads, 0);
    }

    #[test]
    fn bounded_tree_loose_inflation_is_admitted_before_decoding() {
        let (_dir, mut repo) = tree_repo();
        let tree = repo.write_tree(&Tree { entries: vec![TreeEntry {
            name: "x".repeat(256 * 1024), mode: 0o100644, oid: ObjectId::ZERO,
        }] }).unwrap();
        let account = MemoryAccount::new(1024 * 1024);
        let limits = TreeDiffLimits { max_scratch_bytes: 64 * 1024, ..Default::default() };
        let result = repo.diff_trees_bounded(None, Some(tree), &limits, &account, &|| false).unwrap();
        assert_eq!(result.reason, Some(TreeDiffExhaustion::ScratchBytes));
        assert_eq!(result.counters.trees_decoded, 0);
        assert_eq!(result.omitted_subtrees, 1);
        assert!(account.peak() <= limits.max_scratch_bytes);
        assert_eq!(account.reserved(), 0);
    }

    #[test]
    fn test_diff_identical() {
        let ops = diff_lines("hello\nworld\n", "hello\nworld\n");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Equal { len: 2, .. }));
    }

    #[test]
    fn test_diff_insert() {
        let ops = diff_lines("a\nc\n", "a\nb\nc\n");
        // Should have: Equal(a), Insert(b), Equal(c)
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Insert { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_delete() {
        let ops = diff_lines("a\nb\nc\n", "a\nc\n");
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Delete { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_replace() {
        let ops = diff_lines("a\nb\nc\n", "a\nX\nc\n");
        // Should be: Equal(a), Delete(b), Insert(X), Equal(c)
        assert_eq!(ops.len(), 4);
        assert!(matches!(ops[0], DiffOp::Equal { len: 1, .. }));
        assert!(matches!(ops[1], DiffOp::Delete { len: 1, .. }));
        assert!(matches!(ops[2], DiffOp::Insert { len: 1, .. }));
        assert!(matches!(ops[3], DiffOp::Equal { len: 1, .. }));
    }

    #[test]
    fn test_diff_empty_old() {
        let ops = diff_lines("", "a\nb\n");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Insert { len: 2, .. }));
    }

    #[test]
    fn test_diff_empty_new() {
        let ops = diff_lines("a\nb\n", "");
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], DiffOp::Delete { len: 2, .. }));
    }

    #[test]
    fn test_diff_both_empty() {
        let ops = diff_lines("", "");
        assert!(ops.is_empty());
    }

    // ------------------------------------------------------------ bounded

    /// Rebuild both endpoints from the ops alone (equal + deleted lines make
    /// the old text, equal + inserted the new), byte for byte.
    fn reconstruct_both(d: &BoundedDiff, old: &str, new: &str) -> (String, String) {
        let (mut o, mut n) = (String::new(), String::new());
        let line = |idx: &LineIndex, text: &str, i: usize| -> String {
            let r = idx.lines[i];
            format!("{}{}", &text[r.start..r.end], r.ending.as_str())
        };
        for op in &d.ops {
            match *op {
                DiffOp::Equal { old_index, new_index, len } => {
                    for k in 0..len {
                        assert_eq!(d.old.content(old, old_index + k), d.new.content(new, new_index + k), "equal lines are byte-equal");
                        o.push_str(&line(&d.old, old, old_index + k));
                        n.push_str(&line(&d.new, new, new_index + k));
                    }
                }
                DiffOp::Delete { old_index, len } => (0..len).for_each(|k| o.push_str(&line(&d.old, old, old_index + k))),
                DiffOp::Insert { new_index, len } => (0..len).for_each(|k| n.push_str(&line(&d.new, new, new_index + k))),
            }
        }
        (o, n)
    }

    /// The run contract: runs cover both endpoints in order, equal runs map
    /// monotonically, the text reconstructs exactly and the inverse maps
    /// every equal line back to itself.
    fn assert_contract(old: &str, new: &str, d: &BoundedDiff) {
        let (mut oi, mut ni) = (0usize, 0usize);
        for op in &d.ops {
            match *op {
                DiffOp::Equal { old_index, new_index, len } => {
                    assert_eq!((old_index, new_index), (oi, ni), "runs are contiguous: {:?}", d.ops);
                    assert!(len > 0);
                    oi += len;
                    ni += len;
                }
                DiffOp::Delete { old_index, len } => {
                    assert_eq!(old_index, oi);
                    assert!(len > 0);
                    oi += len;
                }
                DiffOp::Insert { new_index, len } => {
                    assert_eq!(new_index, ni);
                    assert!(len > 0);
                    ni += len;
                }
            }
        }
        assert_eq!((oi, ni), (d.old.len(), d.new.len()), "runs cover both endpoints");
        let mut last: Option<(usize, usize)> = None;
        for (o, n) in d.equal_pairs() {
            if let Some((lo, ln)) = last {
                assert!(o > lo && n > ln, "equal mapping is strictly monotonic");
            }
            assert_eq!(d.map_old_to_new(o), Some(n));
            assert_eq!(d.map_new_to_old(n), Some(o));
            last = Some((o, n));
        }
        let (ro, rn) = reconstruct_both(d, old, new);
        assert_eq!(ro, old, "old endpoint reconstructs exactly");
        assert_eq!(rn, new, "new endpoint reconstructs exactly");
        assert_eq!(d.old.reconstruct(old), old);
        assert_eq!(d.new.reconstruct(new), new);
        // inverse consistency: A -> B then B -> A through the inverse is the identity
        let inv = d.inverse();
        assert_contract_inverse(new, old, &inv);
        for (o, n) in d.equal_pairs() {
            assert_eq!(inv.map_old_to_new(n), Some(o));
            assert_eq!(inv.map_new_to_old(o), Some(n));
            assert_eq!(inv.map_old_to_new(d.map_old_to_new(o).unwrap()), Some(o));
        }
        assert_eq!(inv.inverse(), *d, "the inverse of the inverse is the diff itself");
        assert_eq!(inv.is_complete(), d.is_complete());
    }

    fn assert_contract_inverse(old: &str, new: &str, d: &BoundedDiff) {
        let (ro, rn) = reconstruct_both(d, old, new);
        assert_eq!(ro, old);
        assert_eq!(rn, new);
    }

    fn corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            ("", ""),
            ("", "a\nb\n"),
            ("a\nb\n", ""),
            ("a\n", "a\n"),
            ("a\nb\nc\n", "a\nX\nc\n"),
            ("a\nc\n", "a\nb\nc\n"),
            ("a\nb\nc\n", "a\nc\n"),
            // repeated lines
            ("a\na\na\n", "a\na\na\na\n"),
            ("x\n\n\nx\n\ny\n", "x\n\nx\n\n\ny\n"),
            ("a\nb\na\nb\na\nb\n", "b\na\nb\na\nb\na\n"),
            // UTF-8 and tabs
            ("fn é() {\n\tlet ü = \"ç\";\n}\n", "fn é() {\n\tlet ü = \"ç\";\n\tlet 日本 = 1;\n}\n"),
            ("α\nβ\nγ\n", "α\nγ\nδ\n"),
            ("\ta\n\t\tb\n", "\ta\n    b\n"),
            // CRLF, mixed, final newline
            ("a\r\nb\r\n", "a\r\nb\r\nc\r\n"),
            ("a\r\nb\n", "a\nb\r\n"),
            ("a\nb", "a\nb\n"),
            ("a\nb\n", "a\nb"),
            ("\n", ""),
            ("\n\n", "\n"),
            // a lone carriage return is content, not a terminator
            ("a\rb\n", "a\rb\nc\n"),
            ("one\ntwo\nthree\nfour\nfive\n", "zero\none\nthree\nfour\nfive\nsix\n"),
        ]
    }

    #[test]
    fn bounded_runs_reconstruct_map_monotonically_and_invert() {
        for (old, new) in corpus() {
            let d = diff_lines_with_limits(old, new, &DiffLimits::default());
            assert!(d.is_complete(), "{old:?} -> {new:?}");
            assert_contract(old, new, &d);
            // the same runs as the unbounded diff, whose splitter is str::lines()
            let plain = diff_lines(old, new);
            let plain_pairs: Vec<(usize, usize)> = {
                let mut v = Vec::new();
                for op in &plain {
                    if let DiffOp::Equal { old_index, new_index, len } = *op {
                        v.extend((0..len).map(|k| (old_index + k, new_index + k)));
                    }
                }
                v
            };
            assert_eq!(d.equal_pairs().collect::<Vec<_>>().len(), plain_pairs.len(), "{old:?} -> {new:?}: the same number of equal lines as diff_lines");
            assert_eq!(d.old.len(), old.lines().count());
            assert_eq!(d.new.len(), new.lines().count());
        }
    }

    #[test]
    fn bounded_runs_are_the_expected_kinds() {
        let d = diff_lines_with_limits("a\nb\nc\n", "a\nX\nc\n", &DiffLimits::default());
        assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: 1 }, DiffOp::Delete { old_index: 1, len: 1 }, DiffOp::Insert { new_index: 1, len: 1 }, DiffOp::Equal { old_index: 2, new_index: 2, len: 1 }]);
        let d = diff_lines_with_limits("a\nc\n", "a\nb\nc\n", &DiffLimits::default());
        assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: 1 }, DiffOp::Insert { new_index: 1, len: 1 }, DiffOp::Equal { old_index: 1, new_index: 2, len: 1 }]);
        let d = diff_lines_with_limits("a\nb\nc\n", "a\nc\n", &DiffLimits::default());
        assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: 1 }, DiffOp::Delete { old_index: 1, len: 1 }, DiffOp::Equal { old_index: 2, new_index: 1, len: 1 }]);
        // repeated lines: three equal, one inserted, monotone
        let d = diff_lines_with_limits("a\na\na\n", "a\na\na\na\n", &DiffLimits::default());
        assert_eq!(d.equal_pairs().count(), 3);
        assert!(d.ops.iter().any(|op| matches!(op, DiffOp::Insert { len: 1, .. })));
        // the inverse of an insert is a delete at the same place
        let inv = d.inverse();
        assert!(inv.ops.iter().any(|op| matches!(op, DiffOp::Delete { len: 1, .. })));
        assert_eq!(inv.old, d.new);
    }

    #[test]
    fn line_index_retains_terminators_and_final_newline_separately() {
        let crlf = "a\r\nb\r\n";
        let lf = "a\nb";
        let d = diff_lines_with_limits(crlf, lf, &DiffLimits::default());
        assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: 2 }], "content compares without terminators");
        assert_eq!(d.old.lines.iter().map(|l| l.ending).collect::<Vec<_>>(), vec![LineEnding::CrLf, LineEnding::CrLf]);
        assert_eq!(d.new.lines.iter().map(|l| l.ending).collect::<Vec<_>>(), vec![LineEnding::Lf, LineEnding::None]);
        assert!(d.old.final_newline && !d.new.final_newline);
        assert_eq!(d.old.lines[1].full(), 3..6);
        assert_eq!(d.old.lines[1].content(), 3..4);
        assert_eq!(d.old.reconstruct(crlf), crlf);
        assert_eq!(d.new.reconstruct(lf), lf);
        assert_eq!(d.old.line_of(4), Some(1));
        assert_eq!(d.old.line_of(0), Some(0));
        assert_eq!(LineIndex::of("").line_of(0), None);
        // empty files
        let e = diff_lines_with_limits("", "", &DiffLimits::default());
        assert!(e.ops.is_empty() && e.old.is_empty() && e.new.is_empty() && e.is_complete());
        let e = diff_lines_with_limits("", "a\n", &DiffLimits::default());
        assert_eq!(e.ops, vec![DiffOp::Insert { new_index: 0, len: 1 }]);
        let e = diff_lines_with_limits("\n", "", &DiffLimits::default());
        assert_eq!(e.ops, vec![DiffOp::Delete { old_index: 0, len: 1 }]);
        assert_eq!(LineIndex::of("\n").len(), 1, "one empty line, as str::lines()");
        assert_eq!(LineIndex::of("a\nb").len(), 2);
        assert_eq!(LineIndex::of("a\nb\n").len(), 2);
    }

    /// A large replacement between a shared prefix and suffix under tiny
    /// limits: the prefix and suffix are established, the middle is one
    /// explicit unavailable hunk, and both endpoints still reconstruct.
    fn big_pair() -> (String, String) {
        let prefix = "// prefix\nuse std::fmt;\n";
        let suffix = "\nfn tail() {}\n";
        let mut a = String::from(prefix);
        let mut b = String::from(prefix);
        for i in 0..400 {
            a.push_str(&format!("let a{i} = {};\n", i * 7 % 13));
            b.push_str(&format!("let b{i} = {};\n", i * 5 % 11));
        }
        a.push_str(suffix);
        b.push_str(suffix);
        (a, b)
    }

    #[test]
    fn bounds_hit_returns_prefix_suffix_and_one_unavailable_hunk() {
        let (a, b) = big_pair();
        let full = diff_lines_with_limits(&a, &b, &DiffLimits::default());
        assert!(full.is_complete());
        assert_contract(&a, &b, &full);
        for limits in [
            DiffLimits { max_frontier_ops: 50, ..DiffLimits::default() },
            DiffLimits { max_trace_bytes: 64, ..DiffLimits::default() },
            DiffLimits { max_lines: 10, ..DiffLimits::default() },
            DiffLimits { max_bytes: 100, ..DiffLimits::default() },
        ] {
            let d = diff_lines_with_limits(&a, &b, &limits);
            let hunk = d.unavailable.clone().expect("exhausted");
            assert_eq!(hunk.old_index, 2, "the two prefix lines are verified equal: {limits:?}");
            assert_eq!(hunk.new_index, 2);
            assert_eq!(hunk.old_len, 400);
            assert_eq!(hunk.new_len, 400);
            match limits {
                l if l.max_frontier_ops == 50 => assert!(matches!(hunk.reason, Exhaustion::FrontierOps { ops } if ops >= 50)),
                l if l.max_trace_bytes == 64 => assert!(matches!(hunk.reason, Exhaustion::TraceStorage { .. })),
                _ => assert!(matches!(hunk.reason, Exhaustion::TooLarge { .. })),
            }
            assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: 2 }, DiffOp::Delete { old_index: 2, len: 400 }, DiffOp::Insert { new_index: 2, len: 400 }, DiffOp::Equal { old_index: 402, new_index: 402, len: 2 }]);
            assert_contract(&a, &b, &d);
            assert_eq!(d.map_old_to_new(100), None, "no identity inside the unavailable hunk");
            assert_eq!(d.map_old_to_new(403), Some(403), "the suffix maps");
            assert_eq!(Exhaustion::LABEL, "Line correspondence unavailable");
        }
        // the frontier budget is measured: a completed search reports what it spent
        assert!(full.frontier_ops > 0 && full.trace_bytes > 0);
        assert!(full.trace_bytes <= DiffLimits::default().max_trace_bytes);
        // identical over-limit inputs are still exactly equal (no search needed)
        let d = diff_lines_with_limits(&a, &a, &DiffLimits { max_lines: 1, ..DiffLimits::default() });
        assert!(d.is_complete());
        assert_eq!(d.ops, vec![DiffOp::Equal { old_index: 0, new_index: 0, len: d.old.len() }]);
    }

    #[test]
    fn cancellation_is_checked_between_fronts() {
        let (a, b) = big_pair();
        let calls = std::cell::Cell::new(0u32);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        };
        let d = diff_lines_bounded(&a, &b, &DiffLimits::default(), &cancel);
        let hunk = d.unavailable.clone().expect("cancelled");
        assert_eq!(hunk.reason, Exhaustion::Cancelled);
        assert_eq!(calls.get(), 3, "checked once per front until it said stop");
        assert_contract(&a, &b, &d);
        // never asked when nothing needs searching
        let calls = std::cell::Cell::new(0u32);
        let d = diff_lines_bounded("a\nb\n", "a\nb\n", &DiffLimits::default(), &|| {
            calls.set(calls.get() + 1);
            true
        });
        assert!(d.is_complete() && calls.get() == 0);
    }
}
