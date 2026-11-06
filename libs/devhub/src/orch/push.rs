//! `devhub push`: the requested commit or newest landed tip to origin/work, as one commit per area
//! ([`super::group`], the blocks of `devhub promote`): on origin/work, each with the files of its
//! block and a message that says what they are, the last with the tip's tree and any merged PR heads
//! named as extra parents, which must be in the tip. Only the tip is promised to build: promote
//! generates a root manifest for each commit because it checks each one, a push does not.
//! `--dry` builds the same set, lists it and every crate that is new since origin with who depends
//! on it, the commercial checkout's crates included; nothing moves.
//! CI reports regressions independently. A delta with credential-like text is refused (said as
//! file:line, never the text), and the pre-push hook checks what goes out.
//! Nothing pushes on its own: the manager runs it after the daily review.

use super::group::{self, Groups, NewCrate};
use super::{git, is_ancestor, Ctx};
use crate::proto::{wall_ms, Sev};
use crate::state::{Pair, Pushed};
use makepad_remote::json_str;

pub const TRAILER: &str = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>";
/// The most new commits the pre-push hook takes in one push.
const HOOK_COMMITS: usize = 20;
/// Where the commercial checkout's crates stand in the tree (`Ctx::commercial`).
const COMMERCIAL: &str = "apps/commercial";

/// A credential-like line in the delta.
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub file: String,
    pub line: usize,
    pub what: &'static str,
    /// The added line itself (never printed).
    pub text: String,
}

/// The user's address as a pattern: its local part without trailing digits and the first label of
/// its domain (`name4@example.io` -> `name`, `example`), from git's `user.email`.
pub fn email_stem(email: &str) -> Option<(String, String)> {
    let (local, domain) = email.trim().to_ascii_lowercase().split_once('@').map(|(l, d)| (l.to_string(), d.to_string()))?;
    let stem = local.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    let label = domain.split('.').next().unwrap_or("").to_string();
    (!stem.is_empty() && !label.is_empty()).then_some((stem, label))
}

/// `<stem>[0-9]?@<label>` anywhere in `l` (lowercase).
fn has_email(l: &str, (stem, label): &(String, String)) -> bool {
    let at = format!("@{label}");
    l.match_indices(&at).any(|(i, _)| {
        let before = &l[..i];
        let before = before.strip_suffix(|c: char| c.is_ascii_digit()).filter(|b| b.ends_with(stem.as_str())).unwrap_or(before);
        before.ends_with(stem.as_str())
    })
}

/// A fleet credential: `mkc2.<name>.<lan|node|device>.<digits>.<16+ token chars>` (lowercase).
fn has_fleet_key(l: &str) -> bool {
    let tok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    l.match_indices("mkc2.").any(|(i, _)| {
        let rest = &l[i + 5..];
        let name_end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).unwrap_or(rest.len());
        if name_end == 0 {
            return false;
        }
        let Some(rest) = rest[name_end..].strip_prefix('.') else { return false };
        let Some(rest) = ["lan.", "node.", "device."].iter().find_map(|k| rest.strip_prefix(k)) else { return false };
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if digits == 0 {
            return false;
        }
        let Some(rest) = rest[digits..].strip_prefix('.') else { return false };
        rest.chars().take_while(|c| tok(*c)).count() >= 16
    })
}

/// The words that name a credential (`pass` `word=`, `sec` `ret=`), kept in parts so this file
/// never matches its own scan.
const NAMED: [(&str, &str, &str); 2] = [("pass", "word=", "a password"), ("sec", "ret=", "a secret")];

/// The project's own address, public in its manifests: git's `user.email` here, so every run named it.
const PUBLIC_ADDRESS: &str = "info@makepad.nl";

/// The added lines of a unified diff with credential-like text: the user's address, an assigned
/// password or secret, a fleet credential (push-tree.sh's scan, case-insensitive).
pub fn scan(diff: &str, stem: Option<&(String, String)>) -> Vec<Hit> {
    let mut hits = Vec::new();
    let (mut file, mut line) = (String::new(), 0usize);
    for l in diff.lines() {
        if let Some(f) = l.strip_prefix("+++ ") {
            file = f.strip_prefix("b/").unwrap_or(f).to_string();
            continue;
        }
        if l.starts_with("--- ") {
            continue;
        }
        if let Some(h) = l.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: the next added line is line c
            line = h.split_whitespace().find_map(|w| w.strip_prefix('+')).and_then(|w| w.split(',').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
            continue;
        }
        if let Some(added) = l.strip_prefix('+') {
            let low = added.to_ascii_lowercase();
            let named = NAMED.iter().find(|(a, b, _)| low.contains(&format!("{a}{b}"))).map(|n| n.2);
            let what = if stem.is_some_and(|s| has_email(&low.replace(PUBLIC_ADDRESS, ""), s)) {
                Some("the user's email")
            } else if named.is_some() {
                named
            } else if has_fleet_key(&low) {
                Some("a fleet credential")
            } else {
                None
            };
            if let Some(what) = what {
                hits.push(Hit { file: file.clone(), line, what, text: added.to_string() });
            }
            line += 1;
        } else if l.starts_with(' ') {
            line += 1;
        }
    }
    hits
}

/// Select the requested work commit with current commercial main, or the stable landed pair.
fn push_tip(ctx: &Ctx, requested: Option<&str>) -> Result<Pair, String> {
    match requested {
        Some(sha) => Ok(Pair::new(sha, &git(&ctx.commercial(), &["rev-parse", "refs/heads/main"])?)),
        None => super::snapshot(&ctx.repo).ok_or_else(|| "push: no stable landed pair".into()),
    }
}

/// Freeze an extra parent to a commit already reachable from origin's public refs.
fn public_parent(repo: &std::path::Path, parent: &str) -> Result<String, String> {
    let oid = git(repo, &["rev-parse", "--verify", &format!("{parent}^{{commit}}")])?;
    let refs = git(repo, &["for-each-ref", "--format=%(refname)", &format!("--contains={oid}"), "refs/remotes/origin/"])?;
    if refs.trim().is_empty() {
        return Err(format!("push: extra parent {parent} is private: not reachable from any refs/remotes/origin/* ref"));
    }
    Ok(oid)
}

/// The set as JSON rows, for the list `devhub push` prints.
fn commit_rows(groups: &Groups, set: &[(usize, String)]) -> String {
    let rows: Vec<String> = set
        .iter()
        .map(|(at, sha)| {
            let stat = &groups.stats[*at];
            let areas: Vec<String> = groups.areas(*at).iter().map(|a| json_str(a)).collect();
            let credits: Vec<String> = groups.co_authors.get(*at).map_or(Vec::new(), |c| c.iter().map(|l| json_str(l.trim_start_matches("Co-authored-by: "))).collect());
            format!("{{\"sha\":{},\"title\":{},\"files\":{},\"added\":{},\"removed\":{},\"areas\":[{}],\"co_authors\":[{}]}}", json_str(sha), json_str(&groups.blocks[*at].title), stat.files.len(), stat.added, stat.removed, areas.join(","), credits.join(","))
        })
        .collect();
    rows.join(",")
}

fn crate_rows(crates: &[NewCrate]) -> String {
    let rows: Vec<String> = crates
        .iter()
        .map(|c| {
            let users: Vec<String> = c.users.iter().map(|u| json_str(u)).collect();
            format!("{{\"dir\":{},\"name\":{},\"users\":[{}],\"only_commercial\":{}}}", json_str(&c.dir), json_str(&c.name), users.join(","), c.only_private)
        })
        .collect();
    rows.join(",")
}

/// `push {"sha"?,"message"?,"parents"?:[..],"dry"?}`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let json = line.split_once(' ').map_or("{}", |(_, j)| j.trim());
    let v = makepad_strict_json::parse(json.as_bytes()).map_err(|_| "push: bad request")?;
    let st = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|x| !x.is_empty());
    let dry = v.get("dry").and_then(|x| x.as_bool()).unwrap_or(false);
    let parents: Vec<String> = v.get("parents").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    let repo = &ctx.repo;
    let parents = parents.iter().map(|p| public_parent(repo, p)).collect::<Result<Vec<_>, _>>()?;
    let requested = st("sha").map(|r| git(repo, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")])).transpose()?;
    let tip = push_tip(ctx, requested.as_deref())?;
    let sha = tip.work.clone();
    git(repo, &["fetch", "-q", "origin", "work"])?;
    let origin = git(repo, &["rev-parse", "refs/remotes/origin/work"])?;
    let mut groups = Groups::between(repo, &origin, &sha)?;
    if groups.is_empty() {
        return Ok(format!("{{\"pushed\":false,\"why\":\"origin/work already has the tree of {}\",\"origin\":{}}}", super::short(&sha), json_str(&origin)));
    }
    let files = groups.files() as u32;
    let stem = git(repo, &["config", "user.email"]).ok().and_then(|e| email_stem(&e));
    // a line that already stands in origin's tree is public, whatever it looks like
    let hits: Vec<Hit> = scan(&groups.diff, stem.as_ref()).into_iter().filter(|h| !git(repo, &["grep", "-h", "-F", "-e", &h.text, "refs/remotes/origin/work"]).is_ok_and(|found| found.lines().any(|l| l.trim() == h.text.trim()))).collect();
    if !hits.is_empty() {
        let list: Vec<String> = hits.iter().take(20).map(|h| format!("{}:{} ({})", h.file, h.line, h.what)).collect();
        ctx.store.event("push", Sev::Err, format!("push REFUSED {}: credential-like text in the delta: {}", super::short(&sha), list.join(", ")), None);
        return Err(format!("SCAN-FAILED: credential-like text in the delta at {}", list.join(", ")));
    }
    for p in &parents {
        if !is_ancestor(repo, p, &sha) {
            return Err(format!("push: extra parent {p} is not in {}", super::short(&sha)));
        }
    }
    if groups.commits() > HOOK_COMMITS {
        return Err(format!("push: {} groups, the pre-push hook takes {HOOK_COMMITS} commits: join blocks in {}", groups.commits(), group::BLOCKS_FILE));
    }
    // the commits it carries: since the work commit origin's tip was pushed from (all of
    // origin..tip when no push of ours made it)
    let since = ctx.store.read(|d| d.pushes.iter().rev().find(|p| p.commit == origin).map(|p| p.pair.work.clone())).unwrap_or_else(|| origin.clone());
    groups.credit(repo, &since)?;
    let message = st("message");
    let from = format!("work tip {} (with commercial {})", super::short(&sha), super::short(&tip.main));
    let note = |n: usize, of: usize| Some(format!("Part {n} of {of} of {from}.{}", message.as_deref().map_or(String::new(), |m| format!("\n\n{m}"))));
    let set = groups.build(repo, &parents, &note, &|_| None)?;
    let commits = commit_rows(&groups, &set);
    let last = set.last().map(|(_, sha)| sha.clone()).ok_or("push: no commits")?;
    if dry {
        let commercial = ctx.commercial();
        let private = if commercial.is_dir() { group::manifests_at(&commercial, &tip.main, COMMERCIAL)? } else { group::Manifests::default() };
        let new = crate_rows(&groups.new_crates(&private.docs));
        let unread: Vec<String> = groups.manifests.unread.iter().chain(&private.unread).map(|p| json_str(p)).collect();
        let main = if commercial.is_dir() { json_str(&tip.main) } else { "null".into() };
        return Ok(format!("{{\"pushed\":false,\"dry\":true,\"tip\":{},\"origin\":{},\"files\":{files},\"scan\":\"clean\",\"commits\":[{commits}],\"new_crates\":[{new}],\"commercial\":{main},\"unread\":[{}]}}", json_str(&sha), json_str(&origin), unread.join(",")));
    }
    git(repo, &["push", "-q", "origin", &format!("{last}:refs/heads/work")])?;
    let p = Pushed { pair: tip.clone(), commit: last.clone(), origin_was: origin.clone(), at_ms: wall_ms(), files };
    ctx.store.update(|d| d.pushes.push(p))?;
    ctx.store.event("push", Sev::Info, format!("push: origin/work {} -> {}, {} commits (tree of tip {}, {files} files)", super::short(&origin), super::short(&last), set.len(), tip.short()), None);
    Ok(format!("{{\"pushed\":true,\"commit\":{},\"origin_was\":{},\"tip\":{},\"files\":{files},\"commits\":[{commits}]}}", json_str(&last), json_str(&origin), json_str(&sha)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_extra_parents_must_be_reachable_from_origin() {
        use makepad_git::object::{write_loose_object, ObjectKind};
        let dir = std::env::temp_dir().join(format!("devhub-public-parent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        let objects = dir.join(".git");
        let tree = write_loose_object(&objects, ObjectKind::Tree, b"").unwrap();
        let commit = |parent: &str, message: &str| write_loose_object(&objects, ObjectKind::Commit,
            format!("tree {tree}\n{parent}author test <> 1 +0000\ncommitter test <> 1 +0000\n\n{message}\n").as_bytes()).unwrap().to_hex();
        let base = commit("", "public");
        let public = commit(&format!("parent {base}\n"), "public child");
        let private = commit(&format!("parent {public}\n"), "private");
        git(&dir, &["update-ref", "refs/remotes/origin/main", &public]).unwrap();
        git(&dir, &["update-ref", "refs/heads/local", &private]).unwrap();
        assert_eq!(public_parent(&dir, &base).unwrap(), base, "public ancestors are allowed");
        assert_eq!(public_parent(&dir, "origin/main").unwrap(), public);
        assert!(public_parent(&dir, "local").unwrap_err().contains("private"));
        assert!(public_parent(&dir, &private).is_err(), "a raw private oid is also refused");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scan_finds_credentials_by_place_only() {
        let stem = email_stem("Someone42@Example.io").unwrap();
        assert_eq!(stem, ("someone".to_string(), "example".to_string()));
        // PW, SC and MK stand for the scanned words, so this file never matches its own scan
        let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,0 +10,4 @@
+let ok = 1;
+// mail someone4@example.io
+url?PW=x
+key MK.box-1.node.17.AbCdEfGhIjKlMnOpQr
@@ -30 +40 @@
-old SC=gone
+SC=new
"
        .replace("PW", &["pass", "word"].concat())
        .replace("SC", &["SEC", "RET"].concat())
        .replace("MK", &["mkc", "2"].concat());
        let diff = diff.as_str();
        let hits = scan(diff, Some(&stem));
        let at: Vec<(usize, &str)> = hits.iter().map(|h| (h.line, h.what)).collect();
        assert_eq!(at, vec![(11, "the user's email"), (12, "a password"), (13, "a fleet credential"), (40, "a secret")]);
        assert!(hits.iter().all(|h| h.file == "a.rs"));
        // near misses
        let clean = "+++ b/x\n@@ -0,0 +1,3 @@\n+someonex@example.io\n+MK.box.lan.1.short\n+other@example.io\n".replace("MK", &["mkc", "2"].concat());
        assert!(scan(&clean, Some(&stem)).is_empty());
        assert_eq!(scan("+++ b/x\n@@ -0,0 +1 @@\n+someone@example.io\n", Some(&stem)).len(), 1);
        let public = email_stem(PUBLIC_ADDRESS).unwrap();
        let line = |l: &str| format!("+++ b/x\n@@ -0,0 +1 @@\n+{l}\n");
        assert!(scan(&line("authors = [\"Makepad <INFO@makepad.nl>\"]"), Some(&public)).is_empty(), "the public address");
        // (in two parts: whole, this line is what a push's own scan refuses)
        assert_eq!(scan(&line(concat!("info7", "@makepad.nl <info@makepad.nl>")), Some(&public)).len(), 1, "beside it, another still counts");
    }
}
