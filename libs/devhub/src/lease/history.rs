//! The master's load history, kept forever: no rotation, no cap. Each series (one node's load
//! samples, [`super::panel::Sample`]) is a folder of files of fixed-size records, one file per
//! level, under `~/.makepad/devhub/samples/<node>/`:
//! - level 0 holds every sample as taken: its time and each field (8 + 4 bytes a field);
//! - each level above holds one record per bucket of its width (2 min at level 1, then four
//!   times the one below: 8 min, 32 min, ... 364 days at level 10), each field's min, max, sum
//!   and count over the bucket (8 + 16 bytes a field).
//!
//! A bucket is written when it closes (the first record of the level below past its end). The
//! open buckets live in memory and are rebuilt from the level below when a series is opened, so a
//! restart loses nothing; a record a crash cut short is dropped on open.
//!
//! A range reduced to `n` columns of `per` ms reads the level of the widest buckets that fit in a
//! column ([`level_for`]): under [`FACTOR`] + 1 records a column from level 1 on (at level 0, one
//! per sample: 13 a column at most at the 10 s caps round), found by a binary search over the
//! fixed records, plus the open buckets of the levels below it. Its cost is O(log size + columns),
//! whatever the history's length.
//!
//! Disk, at the fleet's 10 s caps round and 8 fields: level 0 takes 40 B × 8640 = 346 KB a node
//! a day, the levels above 136 B × (720 + 180 + 45 + ...) = 131 KB: 477 KB a node a day, 5.7 MB
//! a day for 12 nodes (2.1 GB a year).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Every sample, then ten levels of buckets (2 min to 364 days).
pub const LEVELS: usize = 11;
/// Level 1's bucket.
pub const BASE_MS: u64 = 120_000;
/// Each level's bucket is this many of the one below.
pub const FACTOR: u64 = 4;
const MAGIC: [u8; 4] = *b"DHH1";
/// The file header: magic, field count, the level's bucket width.
const HEAD: u64 = 16;
/// Records a read takes at once.
const CHUNK: u64 = 4096;

/// Level `k`'s bucket width (0: level 0, every sample).
pub fn width(k: usize) -> u64 {
    if k == 0 { 0 } else { BASE_MS * FACTOR.pow(k as u32 - 1) }
}

/// The level a column of `per` ms reads: the widest whose bucket fits in it.
pub fn level_for(per: u64) -> usize {
    (1..LEVELS).rev().find(|&k| width(k) <= per).unwrap_or(0)
}

/// A bucket's fields (or one sample's, as a bucket of one): each field's min, max, sum and count
/// of the values it took. A negative value means "not measured" (a box without a GPU) and is not
/// counted.
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub t_ms: u64,
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    pub sum: Vec<f32>,
    pub n: Vec<u32>,
}

impl Bucket {
    pub fn empty(t_ms: u64, fields: usize) -> Bucket {
        Bucket { t_ms, min: vec![0.0; fields], max: vec![0.0; fields], sum: vec![0.0; fields], n: vec![0; fields] }
    }

    fn of_sample(t_ms: u64, vals: &[f32]) -> Bucket {
        let mut b = Bucket::empty(t_ms, vals.len());
        for (i, &v) in vals.iter().enumerate() {
            if v >= 0.0 && v.is_finite() {
                (b.min[i], b.max[i], b.sum[i], b.n[i]) = (v, v, v, 1);
            }
        }
        b
    }

    pub fn merge(&mut self, o: &Bucket) {
        for i in 0..self.n.len().min(o.n.len()) {
            if o.n[i] == 0 {
                continue;
            }
            if self.n[i] == 0 {
                (self.min[i], self.max[i]) = (o.min[i], o.max[i]);
            } else {
                (self.min[i], self.max[i]) = (self.min[i].min(o.min[i]), self.max[i].max(o.max[i]));
            }
            self.sum[i] += o.sum[i];
            self.n[i] += o.n[i];
        }
    }

    /// Field `i`'s mean (None: no value in the bucket).
    pub fn mean(&self, i: usize) -> Option<f32> {
        (self.n[i] > 0).then(|| self.sum[i] / self.n[i] as f32)
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 16 * self.n.len());
        out.extend_from_slice(&self.t_ms.to_le_bytes());
        for i in 0..self.n.len() {
            for v in [self.min[i], self.max[i], self.sum[i]] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&self.n[i].to_le_bytes());
        }
        out
    }
}

fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b[..8].try_into().unwrap())
}

fn le_f32(b: &[u8]) -> f32 {
    f32::from_le_bytes(b[..4].try_into().unwrap())
}

/// A level-0 record: the sample's time and fields as taken.
fn raw_vals(fields: usize, b: &[u8]) -> (u64, Vec<f32>) {
    (le_u64(b), (0..fields).map(|i| le_f32(&b[8 + 4 * i..])).collect())
}

/// Any level's record as a bucket.
fn decode(level: usize, fields: usize, b: &[u8]) -> Bucket {
    if level == 0 {
        let (t, vals) = raw_vals(fields, b);
        return Bucket::of_sample(t, &vals);
    }
    let mut x = Bucket::empty(le_u64(b), fields);
    for i in 0..fields {
        let o = 8 + 16 * i;
        (x.min[i], x.max[i], x.sum[i]) = (le_f32(&b[o..]), le_f32(&b[o + 4..]), le_f32(&b[o + 8..]));
        x.n[i] = u32::from_le_bytes(b[o + 12..o + 16].try_into().unwrap());
    }
    x
}

/// One level's file: a header, then records in time order.
struct Level {
    file: File,
    level: usize,
    /// bytes a record takes
    rec: u64,
    /// whole records on disk
    len: u64,
}

impl Level {
    fn open(path: &Path, fields: usize, level: usize) -> io::Result<Level> {
        let mut file = OpenOptions::new().read(true).append(true).create(true).open(path)?;
        let rec = if level == 0 { 8 + 4 * fields } else { 8 + 16 * fields } as u64;
        let mut head = [0u8; HEAD as usize];
        head[..4].copy_from_slice(&MAGIC);
        head[4..8].copy_from_slice(&(fields as u32).to_le_bytes());
        head[8..16].copy_from_slice(&width(level).to_le_bytes());
        let mut size = file.metadata()?.len();
        if size < HEAD {
            // new (or its header cut short before any record)
            file.set_len(0)?;
            file.write_all(&head)?;
            size = HEAD;
        } else {
            let mut have = [0u8; HEAD as usize];
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut have)?;
            if have != head {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: not a history file of {fields} fields at level {level}", path.display())));
            }
        }
        let len = (size - HEAD) / rec;
        if HEAD + len * rec != size {
            // a record a crash cut short: it goes (its sample is lost; the buckets above are rebuilt without it)
            file.set_len(HEAD + len * rec)?;
        }
        Ok(Level { file, level, rec, len })
    }

    fn t_at(&mut self, i: u64) -> io::Result<u64> {
        let mut b = [0u8; 8];
        self.file.seek(SeekFrom::Start(HEAD + i * self.rec))?;
        self.file.read_exact(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn last_t(&mut self) -> io::Result<Option<u64>> {
        if self.len == 0 { Ok(None) } else { self.t_at(self.len - 1).map(Some) }
    }

    /// The index of the first record at or after `t`.
    fn find(&mut self, t: u64) -> io::Result<u64> {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.t_at(mid)? < t { lo = mid + 1 } else { hi = mid }
        }
        Ok(lo)
    }

    /// Records `from..to` in order, a chunk at a time.
    fn read(&mut self, from: u64, to: u64, mut each: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        let mut i = from;
        while i < to.min(self.len) {
            let take = (to.min(self.len) - i).min(CHUNK);
            let mut buf = vec![0u8; (take * self.rec) as usize];
            self.file.seek(SeekFrom::Start(HEAD + i * self.rec))?;
            self.file.read_exact(&mut buf)?;
            for r in buf.chunks_exact(self.rec as usize) {
                each(r)?;
            }
            i += take;
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        debug_assert_eq!(bytes.len() as u64, self.rec);
        self.file.write_all(bytes)?;
        self.len += 1;
        Ok(())
    }
}

/// One series: its levels and their open buckets.
pub struct Series {
    fields: usize,
    levels: Vec<Level>,
    /// per level (from 1): the bucket still taking records of the level below
    open: Vec<Option<Bucket>>,
    last_t: Option<u64>,
}

impl Series {
    pub fn open(dir: &Path, fields: usize) -> io::Result<Series> {
        std::fs::create_dir_all(dir)?;
        let mut levels = Vec::with_capacity(LEVELS);
        for k in 0..LEVELS {
            levels.push(Level::open(&dir.join(format!("l{k}.bin")), fields, k)?);
        }
        let last_t = levels[0].last_t()?;
        let mut s = Series { fields, levels, open: vec![None; LEVELS], last_t };
        for k in 1..LEVELS {
            s.rebuild(k)?;
        }
        Ok(s)
    }

    /// Level `k`'s open bucket, from the records of the level below past its last closed one
    /// (a few, but all of them the first time); a bucket a crash left unwritten is written now.
    fn rebuild(&mut self, k: usize) -> io::Result<()> {
        let (w, fields) = (width(k), self.fields);
        let (lower, upper) = self.levels.split_at_mut(k);
        let (below, here) = (&mut lower[k - 1], &mut upper[0]);
        let done = here.last_t()?.map_or(0, |t| t + w);
        let from = below.find(done)?;
        let mut open: Option<Bucket> = None;
        let level = below.level;
        let len = below.len;
        below.read(from, len, |r| {
            let rec = decode(level, fields, r);
            let start = rec.t_ms - rec.t_ms % w;
            match open.as_mut() {
                Some(b) if b.t_ms == start => b.merge(&rec),
                _ => {
                    if let Some(b) = open.take() {
                        here.append(&b.encode())?;
                    }
                    let mut b = Bucket::empty(start, fields);
                    b.merge(&rec);
                    open = Some(b);
                }
            }
            Ok(())
        })?;
        self.open[k] = open;
        Ok(())
    }

    pub fn last_t(&self) -> Option<u64> {
        self.last_t
    }

    pub fn first_t(&mut self) -> io::Result<Option<u64>> {
        if self.levels[0].len == 0 { Ok(None) } else { self.levels[0].t_at(0).map(Some) }
    }

    /// Writes a sample; its time must be past the last one's.
    pub fn append(&mut self, t_ms: u64, vals: &[f32]) -> io::Result<()> {
        if vals.len() != self.fields {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} fields, the series has {}", vals.len(), self.fields)));
        }
        if let Some(last) = self.last_t.filter(|l| t_ms <= *l) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("a sample at {t_ms} is not after the last one at {last}: the clock went back")));
        }
        let mut bytes = Vec::with_capacity(self.levels[0].rec as usize);
        bytes.extend_from_slice(&t_ms.to_le_bytes());
        for v in vals {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.levels[0].append(&bytes)?;
        self.last_t = Some(t_ms);
        self.push(1, Bucket::of_sample(t_ms, vals))
    }

    /// A closed record of level `k - 1` goes into level `k`'s open bucket; a record past that
    /// bucket closes it first (written, and pushed on up).
    fn push(&mut self, k: usize, rec: Bucket) -> io::Result<()> {
        if k >= LEVELS {
            return Ok(());
        }
        let start = rec.t_ms - rec.t_ms % width(k);
        if let Some(b) = self.open[k].as_mut().filter(|b| b.t_ms == start) {
            b.merge(&rec);
            return Ok(());
        }
        if let Some(done) = self.open[k].take() {
            self.levels[k].append(&done.encode())?;
            self.push(k + 1, done)?;
        }
        let mut b = Bucket::empty(start, self.fields);
        b.merge(&rec);
        self.open[k] = Some(b);
        Ok(())
    }

    /// `n` columns of `per` ms from `from`, each the fields over the samples in it (an empty
    /// bucket: none), and the level they were read from. A record counts in the column its start
    /// falls in.
    pub fn reduced(&mut self, from: u64, per: u64, n: usize) -> io::Result<(usize, Vec<Bucket>)> {
        let per = per.max(1);
        let to = from.saturating_add(per.saturating_mul(n as u64));
        let k = level_for(per);
        let fields = self.fields;
        let mut cols: Vec<Bucket> = (0..n).map(|i| Bucket::empty(from + i as u64 * per, fields)).collect();
        let lvl = &mut self.levels[k];
        let (a, b) = (lvl.find(from)?, lvl.find(to)?);
        lvl.read(a, b, |r| {
            let rec = decode(k, fields, r);
            if let Some(c) = cols.get_mut(((rec.t_ms - from) / per) as usize) {
                c.merge(&rec);
            }
            Ok(())
        })?;
        // what the levels below it still hold open is newer than any closed record of its own
        for o in self.open[1..=k].iter().flatten() {
            if o.t_ms >= from && o.t_ms < to {
                cols[((o.t_ms - from) / per) as usize].merge(o);
            }
        }
        Ok((k, cols))
    }

    /// Every sample in `[from, to)`, as taken.
    pub fn exact(&mut self, from: u64, to: u64) -> io::Result<Vec<(u64, Vec<f32>)>> {
        let fields = self.fields;
        let lvl = &mut self.levels[0];
        let (a, b) = (lvl.find(from)?, lvl.find(to)?);
        let mut out = Vec::with_capacity((b - a) as usize);
        lvl.read(a, b, |r| {
            out.push(raw_vals(fields, r));
            Ok(())
        })?;
        Ok(out)
    }
}

/// A folder's name for a series (a node's name as it is, but for characters a path would take).
fn dir_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with('.') { format!("_{s}") } else { s }
}

/// The series of one folder, opened as they are written or read.
pub struct History {
    dir: PathBuf,
    fields: usize,
    open: HashMap<String, Series>,
    /// the series whose last write failed (its error is said once, until a write succeeds)
    failing: HashMap<String, String>,
}

impl History {
    pub fn new(dir: PathBuf, fields: usize) -> History {
        History { dir, fields, open: HashMap::new(), failing: HashMap::new() }
    }

    fn series(&mut self, name: &str) -> io::Result<&mut Series> {
        let key = dir_name(name);
        if !self.open.contains_key(&key) {
            let s = Series::open(&self.dir.join(&key), self.fields)?;
            self.open.insert(key.clone(), s);
        }
        Ok(self.open.get_mut(&key).unwrap())
    }

    /// Writes a sample of `name`. What to say, if anything: a write that failed (once, until one
    /// succeeds again), and that it writes again. A series whose file failed is opened afresh at
    /// its next write, which rebuilds its open buckets from its files.
    pub fn append(&mut self, name: &str, t_ms: u64, vals: &[f32]) -> Option<String> {
        match self.series(name).and_then(|s| s.append(t_ms, vals)) {
            Ok(()) => self.failing.remove(name).map(|_| format!("history: {name}'s samples are written again")),
            Err(e) => {
                // (a refused sample leaves the series as it was)
                if e.kind() != io::ErrorKind::InvalidInput {
                    self.open.remove(&dir_name(name));
                }
                let said = self.failing.insert(name.to_string(), e.to_string()).is_some();
                (!said).then(|| format!("history: {name}'s samples are not written ({}): {e}", self.dir.display()))
            }
        }
    }

    /// The series on disk, by name.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.dir).map(|d| d.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    /// The first and last sample's time of `name`.
    pub fn span(&mut self, name: &str) -> Result<Option<(u64, u64)>, String> {
        let s = self.series(name).map_err(|e| e.to_string())?;
        Ok(match (s.first_t().map_err(|e| e.to_string())?, s.last_t()) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        })
    }

    pub fn reduced(&mut self, name: &str, from: u64, per: u64, n: usize) -> Result<(usize, Vec<Bucket>), String> {
        self.series(name).and_then(|s| s.reduced(from, per, n)).map_err(|e| format!("{name}: {e}"))
    }

    pub fn exact(&mut self, name: &str, from: u64, to: u64) -> Result<Vec<(u64, Vec<f32>)>, String> {
        self.series(name).and_then(|s| s.exact(from, to)).map_err(|e| format!("{name}: {e}"))
    }
}

/// A time index of an append-only JSON-lines log: per line it indexes, the line's byte range and
/// its two times (`t0` its start, `t1` its end; 32 bytes), so the lines of any range are found by a
/// binary search, not by reading the log back from its end. The lines appended since the last
/// ask are indexed at the next one; the first ask indexes the whole log. Three logs have one
/// ([`TimeIndex::leases`], [`TimeIndex::buckets`], [`TimeIndex::landings`]).
pub struct TimeIndex {
    path: PathBuf,
    file: Option<File>,
    /// records on disk
    len: u64,
    /// the log's bytes indexed
    scanned: u64,
    /// the longest `t1 - t0` indexed: a range's scan runs this far past its end
    max_dur: u64,
    /// a line's times, None for a line it does not index
    times: fn(&[u8]) -> Option<(u64, u64)>,
    /// how far before a range its search starts: the log is appended in `t1` order but for
    /// writers that raced
    slack_ms: u64,
}

const IDX_MAGIC: [u8; 8] = *b"DHX2\0\0\0\0";
const IDX_REC: u64 = 32;
/// A first build of this many bytes or more is said where the master logs.
const IDX_SAID_BYTES: u64 = 16 << 20;

/// A number after `key` in a line (`"asked_ms":123`), its first.
fn field_u64(line: &[u8], key: &[u8]) -> Option<u64> {
    let at = line.windows(key.len()).position(|w| w == key)? + key.len();
    let digits = line[at..].iter().take_while(|b| b.is_ascii_digit()).count();
    std::str::from_utf8(&line[at..at + digits]).ok()?.parse().ok()
}

impl TimeIndex {
    /// The lease ledger (`leases.jsonl`, appended at each lease's end, a minute out of order at
    /// most): each lease from its ask to its end.
    pub fn leases(path: PathBuf) -> TimeIndex {
        fn times(l: &[u8]) -> Option<(u64, u64)> {
            let ended = field_u64(l, b"\"ended_ms\":")?;
            Some((field_u64(l, b"\"asked_ms\":").unwrap_or(ended), ended))
        }
        TimeIndex::new(path, times, 10 * 60_000)
    }

    /// The bucket ledger (`buckets.jsonl`, a row at each run's end): each run from its start to
    /// its end.
    pub fn buckets(path: PathBuf) -> TimeIndex {
        fn times(l: &[u8]) -> Option<(u64, u64)> {
            let started = field_u64(l, b"\"started_ms\":")?;
            Some((started, field_u64(l, b"\"ended_ms\":").unwrap_or(started)))
        }
        TimeIndex::new(path, times, 10 * 60_000)
    }

    /// The landing train's past in the state history (`history.jsonl`, its `land` section: an
    /// entry as it leaves the train's live view, in hand-in order): each from its hand-in to its
    /// last change (landed, failed). Its other sections' lines are not indexed.
    pub fn landings(path: PathBuf) -> TimeIndex {
        fn times(l: &[u8]) -> Option<(u64, u64)> {
            let key = b"\"section\":\"land\"";
            l.windows(key.len()).position(|w| w == key)?;
            let changed = field_u64(l, b"\"changed_ms\":")?;
            Some((field_u64(l, b"\"asked_ms\":").unwrap_or(changed), changed))
        }
        TimeIndex::new(path, times, 60 * 60_000)
    }

    fn new(path: PathBuf, times: fn(&[u8]) -> Option<(u64, u64)>, slack_ms: u64) -> TimeIndex {
        TimeIndex { path, file: None, len: 0, scanned: 0, max_dur: 0, times, slack_ms }
    }

    fn opened(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            let mut f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&self.path)?;
            let size = f.metadata()?.len();
            let mut head = [0u8; 16];
            let ok = size >= 16 && f.read_exact(&mut head).is_ok() && head[..8] == IDX_MAGIC;
            let (len, scanned) = if ok { ((size - 16) / IDX_REC, le_u64(&head[8..])) } else { (0, 0) };
            // a record cut short, or no index yet (or one of another format): from what is whole
            f.set_len(16 + len * IDX_REC)?;
            if !ok {
                f.seek(SeekFrom::Start(0))?;
                let mut head = [0u8; 16];
                head[..8].copy_from_slice(&IDX_MAGIC);
                f.write_all(&head)?;
            }
            let (mut max_dur, mut last_end) = (0, 0);
            let mut buf = vec![0u8; (CHUNK * IDX_REC) as usize];
            let mut i = 0;
            while i < len {
                let take = (len - i).min(CHUNK);
                f.seek(SeekFrom::Start(16 + i * IDX_REC))?;
                f.read_exact(&mut buf[..(take * IDX_REC) as usize])?;
                for r in buf[..(take * IDX_REC) as usize].chunks_exact(IDX_REC as usize) {
                    max_dur = max_dur.max(le_u64(&r[24..]).saturating_sub(le_u64(&r[16..])));
                    last_end = le_u64(&r[8..]);
                }
                i += take;
            }
            // records are written before the bytes they cover are said: a record past the
            // header's count of bytes moves it on, so its line is not indexed twice
            (self.len, self.scanned, self.max_dur) = (len, scanned.max(last_end), max_dur);
            self.file = Some(f);
        }
        Ok(self.file.as_mut().unwrap())
    }

    fn rec(&mut self, i: u64) -> io::Result<[u64; 4]> {
        let f = self.opened()?;
        let mut b = [0u8; IDX_REC as usize];
        f.seek(SeekFrom::Start(16 + i * IDX_REC))?;
        f.read_exact(&mut b)?;
        Ok([le_u64(&b), le_u64(&b[8..]), le_u64(&b[16..]), le_u64(&b[24..])])
    }

    /// Indexes the log's lines appended since the last call (all of a log that shrank: it was
    /// replaced). A build of [`IDX_SAID_BYTES`] or more says its size and time on stderr.
    fn catch_up(&mut self, log: &Path) -> io::Result<()> {
        self.opened()?;
        let mut l = match File::open(log) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let size = l.metadata()?.len();
        if size < self.scanned {
            self.file.as_mut().unwrap().set_len(16)?;
            (self.len, self.scanned, self.max_dur) = (0, 0, 0);
        }
        if size == self.scanned {
            return Ok(());
        }
        let (t, from, had) = (std::time::Instant::now(), self.scanned, self.len);
        l.seek(SeekFrom::Start(self.scanned))?;
        let mut pos = self.scanned;
        let mut carry: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut out: Vec<u8> = Vec::new();
        loop {
            let n = l.read(&mut buf)?;
            if n == 0 {
                break;
            }
            carry.extend_from_slice(&buf[..n]);
            let mut start = 0;
            while let Some(nl) = carry[start..].iter().position(|b| *b == b'\n') {
                if let Some((t0, t1)) = (self.times)(&carry[start..start + nl]) {
                    let off = pos + start as u64;
                    for v in [off, off + nl as u64, t0.min(t1), t1] {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    self.max_dur = self.max_dur.max(t1.saturating_sub(t0));
                }
                start += nl + 1;
            }
            pos += start as u64;
            carry.drain(..start);
            if out.len() >= 1 << 20 {
                self.flush(&mut out, pos)?;
            }
        }
        self.flush(&mut out, pos)?;
        if pos - from >= IDX_SAID_BYTES {
            eprintln!("devhub history: {} indexed {} MB of {} ({} lines) in {} ms", self.path.display(), (pos - from) >> 20, log.display(), self.len - had, t.elapsed().as_millis());
        }
        Ok(())
    }

    /// Writes the records gathered, then the bytes indexed.
    fn flush(&mut self, out: &mut Vec<u8>, scanned: u64) -> io::Result<()> {
        let f = self.file.as_mut().unwrap();
        f.seek(SeekFrom::Start(16 + self.len * IDX_REC))?;
        f.write_all(out)?;
        self.len += out.len() as u64 / IDX_REC;
        out.clear();
        f.seek(SeekFrom::Start(0))?;
        let mut head = [0u8; 16];
        head[..8].copy_from_slice(&IDX_MAGIC);
        head[8..].copy_from_slice(&scanned.to_le_bytes());
        f.write_all(&head)?;
        self.scanned = scanned;
        Ok(())
    }

    /// The byte ranges of the log's lines whose times meet `[from, to]` (started by its end,
    /// ended at or after its start), in log order.
    pub fn lines(&mut self, log: &Path, from: u64, to: u64) -> io::Result<Vec<(u64, u64)>> {
        self.catch_up(log)?;
        let start = from.saturating_sub(self.slack_ms);
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.rec(mid)?[3] < start { lo = mid + 1 } else { hi = mid }
        }
        let stop = to.saturating_add(self.max_dur).saturating_add(self.slack_ms);
        let mut out = Vec::new();
        let mut i = lo;
        let mut buf = vec![0u8; (CHUNK * IDX_REC) as usize];
        'scan: while i < self.len {
            let take = (self.len - i).min(CHUNK);
            let f = self.file.as_mut().unwrap();
            f.seek(SeekFrom::Start(16 + i * IDX_REC))?;
            f.read_exact(&mut buf[..(take * IDX_REC) as usize])?;
            for r in buf[..(take * IDX_REC) as usize].chunks_exact(IDX_REC as usize) {
                let (t0, t1) = (le_u64(&r[16..]), le_u64(&r[24..]));
                if t1 > stop {
                    break 'scan;
                }
                if t1 >= from && t0 <= to {
                    out.push((le_u64(r), le_u64(&r[8..])));
                }
            }
            i += take;
        }
        Ok(out)
    }
}

/// Reads the lines at `ranges` of `log` (each without its newline).
pub fn read_lines(log: &Path, ranges: &[(u64, u64)]) -> io::Result<Vec<Vec<u8>>> {
    let mut f = File::open(log)?;
    ranges
        .iter()
        .map(|&(a, b)| {
            let mut buf = vec![0u8; (b - a) as usize];
            f.seek(SeekFrom::Start(a))?;
            f.read_exact(&mut buf)?;
            Ok(buf)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("devhub-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A day of samples every 10 s (two fields, the second one unmeasured at times), the store
    /// reopened as a restarted master opens it: every sample reads back exact, a range reduced to
    /// columns matches the same reduction done over the samples by hand (min, max and mean), at
    /// a column of each level, across the open buckets, and after a cut-short write.
    #[test]
    fn samples_read_back_exact_and_reduced_across_a_reopen() {
        let dir = tmp("reopen");
        let t0 = 1_800_000_000_000u64 - 1_800_000_000_000 % (BASE_MS * 4) + 7_000;
        let val = |i: u64| [((i * 37) % 100) as f32, if i % 7 == 0 { -1.0 } else { (i % 13) as f32 * 0.5 }];
        let samples: Vec<(u64, [f32; 2])> = (0..8640u64).map(|i| (t0 + i * 10_000, val(i))).collect();
        let (first, rest) = samples.split_at(5000);
        let mut h = History::new(dir.clone(), 2);
        for (t, v) in first {
            assert_eq!(h.append("node-a", *t, v), None);
        }
        assert!(h.append("node-a", first[4999].0, &val(1)).is_some_and(|e| e.contains("clock went back")), "a sample not after the last one is refused, and said");
        assert_eq!(h.append("node-a", first[4999].0 + 1, &[1.0]).map(|e| e.contains("2 fields, the series has 1")), None, "a refusal is said once until a write succeeds");
        drop(h);
        // the master restarts: its open buckets are rebuilt from the files
        let mut h = History::new(dir.clone(), 2);
        for (t, v) in rest {
            assert_eq!(h.append("node-a", *t, v), None);
        }
        assert_eq!(h.names(), ["node-a"]);
        assert_eq!(h.span("node-a").unwrap(), Some((t0, samples[8639].0)), "first and last sample");

        // exact: as taken, negatives included
        let got = h.exact("node-a", t0 + 100_000, t0 + 200_001).unwrap();
        let want: Vec<(u64, Vec<f32>)> = samples.iter().filter(|(t, _)| *t >= t0 + 100_000 && *t < t0 + 200_001).map(|(t, v)| (*t, v.to_vec())).collect();
        assert_eq!(got, want, "exact samples in a range");

        // reduced: by hand over the samples, each column the samples whose time falls in it,
        // read from the level whose buckets the column's width is a multiple of
        let by_hand = |from: u64, per: u64, n: usize| -> Vec<Bucket> {
            let mut cols: Vec<Bucket> = (0..n).map(|i| Bucket::empty(from + i as u64 * per, 2)).collect();
            for (t, v) in &samples {
                if *t >= from && ((*t - from) / per) < n as u64 {
                    cols[((*t - from) / per) as usize].merge(&Bucket::of_sample(*t, v));
                }
            }
            cols
        };
        let close = |a: &[Bucket], b: &[Bucket], what: &str| {
            assert_eq!(a.len(), b.len(), "{what}: columns");
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                assert_eq!((x.t_ms, &x.n, &x.min, &x.max), (y.t_ms, &y.n, &y.min, &y.max), "{what}: column {i}'s count, min and max");
                for f in 0..2 {
                    let (p, q) = (x.mean(f).unwrap_or(-1.0), y.mean(f).unwrap_or(-1.0));
                    assert!((p - q).abs() < 1e-3 * q.abs().max(1.0), "{what}: column {i} field {f} mean {p} vs {q}");
                }
            }
        };
        for (per, level) in [(30_000, 0), (BASE_MS, 1), (BASE_MS * 4, 2), (BASE_MS * 16, 3), (BASE_MS * 64, 4)] {
            let from = t0 - t0 % per;
            let n = (86_400_000 / per) as usize + 2;
            let (k, cols) = h.reduced("node-a", from, per, n).unwrap();
            assert_eq!(k, level, "a {per} ms column reads level {level}");
            close(&cols, &by_hand(from, per, n), &format!("{per} ms columns"));
        }
        // a column wider than its level's bucket: at most FACTOR buckets each, the same numbers
        let (k, cols) = h.reduced("node-a", t0 - t0 % (BASE_MS * 2), BASE_MS * 2, 800).unwrap();
        assert_eq!(k, 1);
        close(&cols, &by_hand(t0 - t0 % (BASE_MS * 2), BASE_MS * 2, 800), "2-bucket columns");

        // a write cut short by a crash: the record goes, the rest reads the same
        drop(h);
        let raw = dir.join("node-a").join("l0.bin");
        let size = std::fs::metadata(&raw).unwrap().len();
        OpenOptions::new().append(true).open(&raw).unwrap().write_all(&[1, 2, 3]).unwrap();
        let mut h = History::new(dir.clone(), 2);
        assert_eq!(h.exact("node-a", 0, u64::MAX).unwrap().len(), 8640, "every sample, the cut record dropped");
        assert_eq!(std::fs::metadata(&raw).unwrap().len(), size);
        let from = t0 - t0 % BASE_MS;
        close(&h.reduced("node-a", from, BASE_MS, 730).unwrap().1, &by_hand(from, BASE_MS, 730), "after the cut");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The levels a series rebuilds when its upper files are gone (a crash between writes, or a
    /// store from before them) are the ones it wrote live.
    #[test]
    fn levels_rebuilt_from_the_samples_match_the_ones_written_live() {
        let dir = tmp("rebuild");
        let mut h = History::new(dir.clone(), 1);
        for i in 0..3000u64 {
            h.append("box", 1_000_000_000 + i * 10_000, &[(i % 50) as f32]);
        }
        let (_, live) = h.reduced("box", 1_000_000_000 - 1_000_000_000 % (BASE_MS * 16), BASE_MS * 16, 40).unwrap();
        drop(h);
        for k in 1..LEVELS {
            std::fs::remove_file(dir.join("box").join(format!("l{k}.bin"))).unwrap();
        }
        let mut h = History::new(dir.clone(), 1);
        let (_, rebuilt) = h.reduced("box", 1_000_000_000 - 1_000_000_000 % (BASE_MS * 16), BASE_MS * 16, 40).unwrap();
        assert_eq!(live, rebuilt);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
