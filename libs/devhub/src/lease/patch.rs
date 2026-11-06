//! A run's debug patches (`devhub run --patch <diff>`): unified diffs, as `git diff` writes them,
//! that the client applies to the files it ships. A rig's instrumentation is then part of its
//! lease's source: there from the run's start, gone at its end (a tree lease undoes every shipped
//! path at release; an overlay lease's edits go with it). Neither the lane's worktree nor the
//! node's tree holds it. Two cancelled rigs had left their edits in a node's tree for devhub to
//! recover (11-06).
//!
//! A hunk applies where its old lines stand, the nearest place to the line it names, and only
//! after the hunk before it. A patch adds or changes files; it deletes, renames or binary-patches
//! none.

use std::path::Path;

/// The files `diff` changes or adds, each with its content after the patch, in the diff's order.
/// `read` gives a path's content before it (None: there is no such file).
pub fn apply(diff: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Vec<(String, Vec<u8>)>, String> {
    let lines: Vec<&str> = diff.split_inclusive('\n').collect();
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("GIT binary patch") || line.starts_with("Binary files ") {
            return Err(format!("{}: a binary patch", line.trim_end()));
        }
        if line.starts_with("rename from ") || line.starts_with("copy from ") {
            return Err(format!("{}: a debug patch only changes or adds files", line.trim_end()));
        }
        let Some(old) = line.strip_prefix("--- ") else { i += 1; continue };
        let new = lines.get(i + 1).and_then(|l| l.strip_prefix("+++ ")).ok_or_else(|| format!("line {}: `--- {}` without its `+++` line", i + 1, old.trim_end()))?;
        let (old, new) = (path_of(old, "a/"), path_of(new, "b/"));
        let path = new.ok_or_else(|| format!("deletes {}: a debug patch only changes or adds files", old.unwrap_or("?")))?;
        if old.is_some_and(|o| o != path) {
            return Err(format!("renames {} to {path}: a debug patch only changes or adds files", old.unwrap_or("?")));
        }
        if out.iter().any(|(p, _)| p == path) {
            return Err(format!("{path}: twice in one patch"));
        }
        let before = match old {
            None if read(path).is_some() => return Err(format!("adds {path}, which the worktree has")),
            None => Vec::new(),
            Some(_) => read(path).ok_or_else(|| format!("{path}: not in the worktree"))?,
        };
        let text = String::from_utf8(before).map_err(|_| format!("{path}: not text"))?;
        i += 2;
        let mut file = File { lines: text.split_inclusive('\n').collect(), at: 0, out: String::new() };
        let mut n = 0;
        while let Some(head) = lines.get(i).filter(|l| l.starts_with("@@ ")) {
            n += 1;
            let (start, old_n, new_n) = hunk_range(head).ok_or_else(|| format!("{path}: hunk {n}: not a hunk header: {}", head.trim_end()))?;
            i += 1;
            let (mut want, mut put) = (Vec::new(), Vec::new());
            while want.len() < old_n || put.len() < new_n {
                let l = lines.get(i).ok_or_else(|| format!("{path}: hunk {n} ends early"))?;
                match l.as_bytes().first() {
                    Some(b' ') => { want.push(&l[1..]); put.push(&l[1..]); }
                    Some(b'-') => want.push(&l[1..]),
                    Some(b'+') => put.push(&l[1..]),
                    // an empty context line (some editors strip the space)
                    Some(b'\n') => { want.push(*l); put.push(*l); }
                    _ => return Err(format!("{path}: hunk {n}: line {}: not a hunk line: {}", i + 1, l.trim_end())),
                }
                i += 1;
                no_newline(&lines, &mut i, &mut want, &mut put);
            }
            if want.len() > old_n || put.len() > new_n {
                return Err(format!("{path}: hunk {n}: more lines than its header counts"));
            }
            file.hunk(start, &want, &put).map_err(|e| format!("{path}: hunk {n} ({}): {e}", head.trim_end()))?;
        }
        if n == 0 && old.is_some() {
            return Err(format!("{path}: no hunks"));
        }
        out.push((path.to_string(), file.finish().into_bytes()));
    }
    if out.is_empty() {
        return Err("no file in it".into());
    }
    Ok(out)
}

/// [`apply`] with each file's content as the worktree at `root` holds it.
pub fn apply_in(root: &Path, diff: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    apply(diff, &|p| std::fs::read(root.join(p)).ok())
}

/// A diff line's path without its `a/` or `b/` (None: /dev/null).
fn path_of<'a>(field: &'a str, prefix: &str) -> Option<&'a str> {
    // git ends a path with a space before a tab only when it holds a space; a tab ends it
    let p = field.trim_end_matches(['\n', '\r']).split('\t').next().unwrap_or("");
    (p != "/dev/null").then(|| p.strip_prefix(prefix).unwrap_or(p))
}

/// `@@ -start[,n] +start[,n] @@ ..`: the old start line and both line counts.
fn hunk_range(head: &str) -> Option<(usize, usize, usize)> {
    let mut w = head.strip_prefix("@@ ")?.split(' ');
    let range = |r: &str| -> Option<(usize, usize)> {
        let (s, n) = r.split_once(',').unwrap_or((r, "1"));
        Some((s.parse().ok()?, n.parse().ok()?))
    };
    let (start, old_n) = range(w.next()?.strip_prefix('-')?)?;
    let (_, new_n) = range(w.next()?.strip_prefix('+')?)?;
    Some((start, old_n, new_n))
}

/// A `\ No newline at end of file` after a hunk line: that line has no newline on its side (a
/// context line: on both).
fn no_newline<'a>(lines: &[&'a str], i: &mut usize, want: &mut Vec<&'a str>, put: &mut Vec<&'a str>) {
    if !lines.get(*i).is_some_and(|l| l.starts_with('\\')) { return; }
    let prev = lines[*i - 1].as_bytes()[0];
    let strip = |v: &mut Vec<&'a str>| if let Some(last) = v.last_mut() { *last = last.strip_suffix('\n').unwrap_or(last); };
    if prev != b'+' { strip(want); }
    if prev != b'-' { strip(put); }
    *i += 1;
}

/// A file as the hunks rewrite it: its lines, the first one no hunk has passed, the text so far.
struct File<'a> {
    lines: Vec<&'a str>,
    at: usize,
    out: String,
}

impl File<'_> {
    fn hunk(&mut self, start: usize, want: &[&str], put: &[&str]) -> Result<(), String> {
        let fits = |p: usize| p + want.len() <= self.lines.len() && self.lines[p..p + want.len()] == *want;
        // a hunk that only adds names the line before it (0: the file's start)
        let named = if want.is_empty() { start } else { start.saturating_sub(1) };
        // the nearest place to the named line, after the hunk before
        let found = (0..=self.lines.len()).flat_map(|d| [Some(named + d), named.checked_sub(d).filter(|_| d > 0)]).flatten().find(|&p| p >= self.at && fits(p));
        let Some(p) = found else {
            let at = named.min(self.lines.len());
            let has = self.lines.get(at).map_or("the end of the file".to_string(), |l| format!("`{}`", l.trim()));
            let expects = want.first().map_or("nothing".to_string(), |l| format!("`{}`", l.trim()));
            return Err(format!("its lines are not in the file: line {} holds {has}, the patch expects {expects}", at + 1));
        };
        for l in &self.lines[self.at..p] { self.out.push_str(l); }
        for l in put { self.out.push_str(l); }
        self.at = p + want.len();
        Ok(())
    }

    fn finish(mut self) -> String {
        for l in &self.lines[self.at..] { self.out.push_str(l); }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(files: &[(&str, &str)], diff: &str) -> Result<Vec<(String, String)>, String> {
        let files: HashMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
        Ok(apply(diff, &|p| files.get(p).cloned())?.into_iter().map(|(p, t)| (p, String::from_utf8(t).unwrap())).collect())
    }

    const SRC: &str = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n";

    /// A rig's instrumentation as `git diff` wrote it: two hunks of one file and a new file.
    #[test]
    fn a_git_diff_applies_to_the_worktree_files() {
        let diff = "diff --git a/src/x.rs b/src/x.rs\nindex 1111111..2222222 100644\n--- a/src/x.rs\n+++ b/src/x.rs\n@@ -1,3 +1,4 @@\n fn a() {\n+    probe(1);\n     one();\n }\n@@ -5,3 +6,3 @@ fn a() {\n fn b() {\n-    two();\n+    two_probed();\n }\ndiff --git a/src/probe.rs b/src/probe.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/probe.rs\n@@ -0,0 +1 @@\n+pub fn probe(_: u32) {}\n";
        let out = run(&[("src/x.rs", SRC)], diff).unwrap();
        assert_eq!(out[0], ("src/x.rs".into(), "fn a() {\n    probe(1);\n    one();\n}\n\nfn b() {\n    two_probed();\n}\n".into()));
        assert_eq!(out[1], ("src/probe.rs".into(), "pub fn probe(_: u32) {}\n".into()));
    }

    /// The lane moved the lines since the diff was made: the hunk goes where its lines stand.
    #[test]
    fn a_hunk_applies_where_its_lines_moved() {
        let moved = format!("// a header\n// of two lines\n{SRC}");
        let diff = "--- a/src/x.rs\n+++ b/src/x.rs\n@@ -5,3 +5,3 @@\n fn b() {\n-    two();\n+    three();\n }\n";
        let out = run(&[("src/x.rs", &moved)], diff).unwrap();
        assert_eq!(out[0].1, moved.replace("two();", "three();"));
    }

    /// Lines the file no longer has, a deletion and a patch of a missing file are refused, saying
    /// which file and where: the rig's run never starts on a half-patched source.
    #[test]
    fn a_patch_that_does_not_fit_is_refused_with_its_place() {
        let stale = "--- a/src/x.rs\n+++ b/src/x.rs\n@@ -2,1 +2,1 @@\n-    zero();\n+    nil();\n";
        let e = run(&[("src/x.rs", SRC)], stale).unwrap_err();
        assert!(e.starts_with("src/x.rs: hunk 1 (@@ -2,1 +2,1 @@): its lines are not in the file: line 2 holds `one();`"), "{e}");
        let delete = "--- a/src/x.rs\n+++ /dev/null\n@@ -1,7 +0,0 @@\n";
        assert!(run(&[("src/x.rs", SRC)], delete).unwrap_err().starts_with("deletes src/x.rs"));
        let missing = "--- a/src/y.rs\n+++ b/src/y.rs\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(run(&[("src/x.rs", SRC)], missing).unwrap_err(), "src/y.rs: not in the worktree");
        assert_eq!(run(&[], "just words\n").unwrap_err(), "no file in it");
    }

    /// A last line without a newline keeps it so, on either side.
    #[test]
    fn a_missing_final_newline_stays_as_the_diff_says() {
        let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+zwei\n\\ No newline at end of file\n";
        assert_eq!(run(&[("a.txt", "one\ntwo")], diff).unwrap()[0].1, "one\nzwei");
        let add_newline = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+two\n";
        assert_eq!(run(&[("a.txt", "one\ntwo")], add_newline).unwrap()[0].1, "one\ntwo\n");
    }
}
