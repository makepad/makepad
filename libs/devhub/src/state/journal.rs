//! The state document's change journal. Each committed update appends one line to
//! `state.journal`: the edit that turns the last committed document into the new one. `state.json`
//! is rewritten whole only as a checkpoint, when the journal outgrows [`CHECKPOINT_BYTES`]. A job's
//! step then costs the bytes of its job record, not of the document (the whole 907 KB document was
//! rewritten about 0.4 times a second, 11-03). Loading reads the checkpoint and replays the lines
//! after its revision. A torn last line (a crash mid-append) was never committed; the next append
//! cuts it off.
use makepad_strict_json::{obj, s, Value};
use std::io::{Seek, Write};
use std::path::Path;

/// `state.json` is rewritten (and the journal emptied) once the journal would pass this.
pub const CHECKPOINT_BYTES: u64 = 4 << 20;
/// An array edit with more changes than this is written as the whole array.
const MAX_EDITS: usize = 64;

/// One edit of the document. A path names object keys from the root; arrays are edited by
/// splices of whole elements.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// the value at the path is replaced (the empty path: the whole document)
    Set(Vec<String>, Value),
    /// in the array at the path, `remove` elements at `start` are replaced by the values
    Splice(Vec<String>, usize, usize, Vec<Value>),
}

/// The edits that turn `old` into `new`: `apply`ing them in order to `old` gives exactly `new`.
pub fn diff(old: &Value, new: &Value) -> Vec<Op> {
    let mut ops = Vec::new();
    diff_at(&mut Vec::new(), old, new, &mut ops);
    ops
}

fn diff_at(path: &mut Vec<String>, old: &Value, new: &Value, ops: &mut Vec<Op>) {
    if old == new {
        return;
    }
    match (old, new) {
        // the same keys in the same order: each field on its own
        (Value::Obj(a), Value::Obj(b)) if a.len() == b.len() && a.iter().zip(b).all(|((x, _), (y, _))| x == y) => {
            for ((key, x), (_, y)) in a.iter().zip(b) {
                path.push(key.clone());
                diff_at(path, x, y, ops);
                path.pop();
            }
        }
        (Value::Arr(a), Value::Arr(b)) => match array_edits(a, b, MAX_EDITS) {
            Some(hunks) => {
                for (_, a_len, b_pos, b_len) in hunks {
                    ops.push(Op::Splice(path.clone(), b_pos, a_len, b[b_pos..b_pos + b_len].to_vec()));
                }
            }
            None => ops.push(Op::Set(path.clone(), new.clone())),
        },
        _ => ops.push(Op::Set(path.clone(), new.clone())),
    }
}

/// The changed runs between `a` and `b` (shortest edit script, Myers), in order, as
/// `(a_pos, a_len, b_pos, b_len)`; None past `max_edits` single-element changes.
fn array_edits(a: &[Value], b: &[Value], max_edits: usize) -> Option<Vec<(usize, usize, usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let off = n + m + 1;
    let at = |k: isize| (off + k) as usize;
    let mut v = vec![0isize; (2 * off + 1) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=((n + m).min(max_edits as isize)) {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                end = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    let end = end?;
    // back from (n, m): each step of d is one deletion or insertion, between diagonal runs
    let (mut x, mut y) = (n, m);
    let mut steps = Vec::new();
    for d in (1..=end).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { k + 1 } else { k - 1 };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        steps.push((prev_x as usize, prev_y as usize, x as usize, y as usize));
        x = prev_x;
        y = prev_y;
    }
    steps.reverse();
    let mut hunks: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (px, py, x, y) in steps {
        match hunks.last_mut() {
            Some(h) if h.0 + h.1 == px && h.2 + h.3 == py => {
                h.1 += x - px;
                h.3 += y - py;
            }
            _ => hunks.push((px, x - px, py, y - py)),
        }
    }
    Some(hunks)
}

/// Applies one edit to `doc`.
pub fn apply(doc: &mut Value, op: &Op) -> Result<(), String> {
    let path = match op {
        Op::Set(p, _) | Op::Splice(p, ..) => p,
    };
    let mut at = doc;
    for key in path {
        let Value::Obj(fields) = at else { return Err(format!("journal: {} is no object", path.join("/"))) };
        at = fields.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v).ok_or_else(|| format!("journal: no {}", path.join("/")))?;
    }
    match op {
        Op::Set(_, value) => *at = value.clone(),
        Op::Splice(_, start, remove, values) => {
            let Value::Arr(items) = at else { return Err(format!("journal: {} is no array", path.join("/"))) };
            if start + remove > items.len() {
                return Err(format!("journal: splice past the end of {}", path.join("/")));
            }
            items.splice(*start..start + remove, values.iter().cloned());
        }
    }
    Ok(())
}

/// One journal line (with its newline): revision `revision`, the history boundary it committed,
/// and its edits.
pub fn line(revision: u64, history_len: u64, ops: &[Op]) -> String {
    let path = |p: &[String]| Value::Arr(p.iter().map(|k| s(k)).collect());
    let ops = ops.iter().map(|op| match op {
        Op::Set(p, v) => Value::Arr(vec![s("set"), path(p), v.clone()]),
        Op::Splice(p, start, remove, values) => Value::Arr(vec![s("splice"), path(p), Value::Int(*start as i64), Value::Int(*remove as i64), Value::Arr(values.clone())]),
    }).collect();
    format!("{}\n", obj(vec![("revision", Value::Int(revision as i64)), ("history_len", Value::Int(history_len as i64)), ("ops", Value::Arr(ops))]).to_json())
}

fn parse_line(text: &[u8]) -> Result<(u64, u64, Vec<Op>), String> {
    let v = makepad_strict_json::parse_depth(text, 64).map_err(|e| format!("{e:?}"))?;
    let num = |v: Option<&Value>| v.and_then(Value::as_u64).ok_or("no number");
    let revision = num(v.get("revision"))?;
    let history_len = num(v.get("history_len"))?;
    let path = |p: Option<&Value>| -> Result<Vec<String>, String> {
        p.and_then(Value::as_arr).ok_or("no path")?.iter().map(|k| k.as_str().map(str::to_string).ok_or_else(|| "a path key is no text".to_string())).collect()
    };
    let mut ops = Vec::new();
    for op in v.get("ops").and_then(Value::as_arr).ok_or("no ops")? {
        let parts = op.as_arr().ok_or("an op is no array")?;
        ops.push(match parts.first().and_then(Value::as_str) {
            Some("set") => Op::Set(path(parts.get(1))?, parts.get(2).cloned().ok_or("set without a value")?),
            Some("splice") => Op::Splice(
                path(parts.get(1))?,
                num(parts.get(2))? as usize,
                num(parts.get(3))? as usize,
                parts.get(4).and_then(Value::as_arr).ok_or("splice without values")?.to_vec(),
            ),
            _ => return Err("an unknown op".into()),
        });
    }
    Ok((revision, history_len, ops))
}

/// The committed document at `state` (`state.json` and the journal beside it): the value
/// without its `revision` and `history_len`, those two, and the journal's committed length.
/// None: no state yet.
pub fn load(state: &Path) -> Result<Option<(Value, u64, u64, u64)>, String> {
    let journal = state.with_extension("journal");
    let text = match std::fs::read(state) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::metadata(&journal).is_ok_and(|m| m.len() > 0) {
                return Err(format!("{} without its checkpoint {}; files kept", journal.display(), state.display()));
            }
            return Ok(None);
        }
        Err(e) => return Err(format!("read workflow state {}: {e}; file kept", state.display())),
    };
    let mut value = makepad_strict_json::parse_depth(&text, 16).map_err(|e| format!("invalid workflow state {}; file kept: {e:?}", state.display()))?;
    let Value::Obj(fields) = &mut value else { return Err(format!("invalid workflow state {}; expected object; file kept", state.display())) };
    let field = |k: &str| fields.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.as_u64()).unwrap_or(0);
    let (mut revision, mut history_len) = (field("revision"), field("history_len"));
    fields.retain(|(k, _)| k != "revision" && k != "history_len");
    let bytes = match std::fs::read(&journal) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("read {}: {e}; file kept", journal.display())),
    };
    let mut committed = 0;
    while let Some(end) = bytes[committed..].iter().position(|b| *b == b'\n').map(|i| committed + i) {
        let (rev, history, ops) = parse_line(&bytes[committed..end]).map_err(|e| format!("invalid line at byte {committed} of {}: {e}; file kept", journal.display()))?;
        if rev > revision {
            if rev != revision + 1 {
                return Err(format!("{} skips from revision {revision} to {rev}; file kept", journal.display()));
            }
            for op in &ops {
                apply(&mut value, op).map_err(|e| format!("{e} (revision {rev}); file kept"))?;
            }
            (revision, history_len) = (rev, history);
        }
        committed = end + 1;
    }
    Ok(Some((value, revision, history_len, committed as u64)))
}

/// Appends `line` after the journal's committed `len` bytes (cutting off a torn tail), durably.
pub fn append(state: &Path, len: u64, line: &str) -> Result<(), String> {
    let journal = state.with_extension("journal");
    let write = || -> std::io::Result<()> {
        let fresh = !journal.exists();
        let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&journal)?;
        file.set_len(len)?;
        file.seek(std::io::SeekFrom::Start(len))?;
        file.write_all(line.as_bytes())?;
        file.sync_data()?;
        #[cfg(unix)]
        if fresh {
            std::fs::File::open(journal.parent().unwrap_or(Path::new(".")))?.sync_all()?;
        }
        #[cfg(not(unix))]
        let _ = fresh;
        Ok(())
    };
    write().map_err(|e| format!("persist {}: {e}", journal.display()))
}

/// Empties the journal after a checkpoint made its lines redundant.
pub fn clear(state: &Path) -> Result<(), String> {
    let journal = state.with_extension("journal");
    match std::fs::OpenOptions::new().write(true).open(&journal) {
        Ok(file) => file.set_len(0).and_then(|_| file.sync_data()).map_err(|e| format!("persist {}: {e}", journal.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("persist {}: {e}", journal.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32, state: &str) -> Value {
        obj(vec![("id", s(&format!("J{id}"))), ("state", s(state))])
    }

    fn doc(items: Vec<Value>, next: u32) -> Value {
        obj(vec![("jobs", Value::Arr(items)), ("jobs_next", Value::Int(next as i64)), ("verify", obj(vec![("tips", Value::Arr(vec![]))]))])
    }

    fn round_trip(old: &Value, new: &Value) -> Vec<Op> {
        let ops = diff(old, new);
        let mut at = old.clone();
        for op in &ops {
            apply(&mut at, op).unwrap();
        }
        assert_eq!(&at, new, "{ops:?}");
        let text = line(7, 0, &ops);
        assert_eq!(parse_line(text.trim_end().as_bytes()).unwrap().2, ops);
        ops
    }

    /// A job's step is a splice of that one record; a trim at the front with an append at the
    /// end is two small splices, never the whole list; any edit replays to exactly the new doc.
    #[test]
    fn edits_are_as_small_as_the_change_and_replay_exactly() {
        let base: Vec<Value> = (0..600).map(|i| rec(i, "done")).collect();
        let old = doc(base.clone(), 600);
        let mut step = base.clone();
        step[300] = rec(300, "running");
        let ops = round_trip(&old, &doc(step, 600));
        assert_eq!(ops.len(), 1);
        assert!(line(1, 0, &ops).len() < 200, "{}", line(1, 0, &ops));
        let mut rolled = base[1..].to_vec();
        rolled.push(rec(600, "queued"));
        let ops = round_trip(&old, &doc(rolled, 601));
        assert!(line(1, 0, &ops).len() < 300, "{}", line(1, 0, &ops));
        // a pseudo-random series of inserts, removals and changes
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut rand = |n: usize| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % n.max(1) as u64) as usize };
        let mut items = base;
        for round in 0..200u32 {
            let mut next = items.clone();
            for _ in 0..rand(5) + 1 {
                match rand(3) {
                    0 => { let at = rand(next.len() + 1); next.insert(at, rec(10_000 + round, "new")); }
                    1 if !next.is_empty() => { let at = rand(next.len()); next.remove(at); }
                    _ if !next.is_empty() => { let at = rand(next.len()); next[at] = rec(20_000 + round, "changed"); }
                    _ => {}
                }
            }
            round_trip(&doc(items.clone(), round), &doc(next.clone(), round + 1));
            items = next;
        }
        // past the edit bound: the whole list, still exact
        let reversed: Vec<Value> = items.iter().rev().cloned().collect();
        let ops = round_trip(&doc(items.clone(), 1), &doc(reversed, 1));
        assert!(matches!(&ops[0], Op::Set(p, _) if p == &["jobs".to_string()]));
        // keys added or reordered: that object whole
        round_trip(&obj(vec![("a", Value::Int(1))]), &obj(vec![("a", Value::Int(1)), ("b", Value::Int(2))]));
        round_trip(&Value::Arr(vec![]), &Value::Arr(vec![rec(1, "x")]));
    }

    /// Load replays the lines past the checkpoint, skips those it already holds, keeps a torn
    /// tail out, and refuses a gap.
    #[test]
    fn load_replays_committed_lines_only() {
        let dir = std::env::temp_dir().join(format!("devhub-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let d0 = doc(vec![rec(1, "queued")], 2);
        let mut full = d0.clone();
        if let Value::Obj(f) = &mut full { f.push(("revision".into(), Value::Int(1))); f.push(("history_len".into(), Value::Int(0))); }
        std::fs::write(&state, full.to_json()).unwrap();
        let d1 = doc(vec![rec(1, "running")], 2);
        let d2 = doc(vec![rec(1, "running"), rec(2, "queued")], 3);
        let stale = line(1, 0, &diff(&doc(vec![], 1), &d0));
        let l2 = line(2, 0, &diff(&d0, &d1));
        let l3 = line(3, 5, &diff(&d1, &d2));
        std::fs::write(dir.join("state.journal"), format!("{stale}{l2}{l3}")).unwrap();
        let (value, revision, history, len) = load(&state).unwrap().unwrap();
        assert_eq!((value, revision, history, len as usize), (d2.clone(), 3, 5, stale.len() + l2.len() + l3.len()));
        // a torn last line is not committed
        std::fs::write(dir.join("state.journal"), format!("{l2}{}", &l3[..l3.len() / 2])).unwrap();
        let (value, revision, _, len) = load(&state).unwrap().unwrap();
        assert_eq!((value, revision, len as usize), (d1.clone(), 2, l2.len()));
        append(&state, len, &l3).unwrap();
        assert_eq!(load(&state).unwrap().unwrap().0, d2, "the next append cuts the torn tail off");
        std::fs::write(dir.join("state.journal"), &l3).unwrap();
        assert!(load(&state).unwrap_err().contains("skips"));
        clear(&state).unwrap();
        assert_eq!(load(&state).unwrap().unwrap().1, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
