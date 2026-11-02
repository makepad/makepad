//! `devhub push`: the collapsed push of the requested commit or newest landed tip to origin/work.
//! One commit whose tree is the selected work commit's tree and whose parent is origin/work (plus
//! any merged PR heads named as extra parents, which must be in the tip). CI reports regressions
//! independently. A delta with credential-like text is refused (said as file:line, never the text).
//! The standing push follows the landed tip; explicit requests may select a commit.

use super::{git, is_ancestor, Ctx};
use crate::proto::{wall_ms, Sev};
use crate::state::{Pair, Pushed};
use makepad_remote::json_str;

pub const TRAILER: &str = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>";

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
            let what = if stem.is_some_and(|s| has_email(&low, s)) {
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

/// The standing push: when work or main moved (a landing or a proof commit), the newest landed
/// pair goes to origin/work when origin does not have its tree yet (the credential scan still
/// refuses). Unmoved tips are not pushed again.
pub fn start(ctx: std::sync::Arc<Ctx>) {
    let _ = std::thread::Builder::new().name("devhub-push".into()).spawn(move || {
        let mut said = String::new();
        let mut seen: Option<Pair> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(120));
            let tip = super::snapshot(&ctx.repo);
            if tip.is_some() && tip == seen {
                continue;
            }
            seen = tip;
            let line = match serve(&ctx, "push {}") {
                Ok(v) if v.contains("\"pushed\":true") => {
                    ctx.store.event("push", Sev::Info, format!("push: {v}"), None);
                    String::new()
                }
                Ok(_) => String::new(),
                Err(e) => e,
            };
            // (a refusal is said once, until it changes)
            if !line.is_empty() && line != said {
                ctx.store.event("push", Sev::Warn, line.clone(), None);
            }
            // a push that failed (origin, the scan) is tried again
            if !line.is_empty() {
                seen = None;
            }
            said = line;
        }
    });
}

/// Select the requested work commit with current commercial main, or the stable landed pair.
fn push_tip(ctx: &Ctx, requested: Option<&str>) -> Result<Pair, String> {
    match requested {
        Some(sha) => Ok(Pair::new(sha, &git(&ctx.commercial(), &["rev-parse", "refs/heads/main"])?)),
        None => super::snapshot(&ctx.repo).ok_or_else(|| "push: no stable landed pair".into()),
    }
}

/// `push {"sha"?,"message"?,"parents"?:[..],"dry"?}`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let json = line.split_once(' ').map_or("{}", |(_, j)| j.trim());
    let v = makepad_strict_json::parse(json.as_bytes()).map_err(|_| "push: bad request")?;
    let st = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|x| !x.is_empty());
    let dry = v.get("dry").and_then(|x| x.as_bool()).unwrap_or(false);
    let parents: Vec<String> = v.get("parents").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    let repo = &ctx.repo;
    let requested = st("sha").map(|r| git(repo, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")])).transpose()?;
    let tip = push_tip(ctx, requested.as_deref())?;
    let sha = tip.work.clone();
    git(repo, &["fetch", "-q", "origin", "work"])?;
    let origin = git(repo, &["rev-parse", "refs/remotes/origin/work"])?;
    let tree = |c: &str| git(repo, &["rev-parse", &format!("{c}^{{tree}}")]);
    if tree(&origin)? == tree(&sha)? {
        return Ok(format!("{{\"pushed\":false,\"why\":\"origin/work already has the tree of {}\",\"origin\":{}}}", super::short(&sha), json_str(&origin)));
    }
    let files = git(repo, &["diff", "--name-only", &origin, &sha])?.lines().count() as u32;
    let diff = git(repo, &["diff", "--unified=0", "--no-color", "--no-ext-diff", &origin, &sha])?;
    let stem = git(repo, &["config", "user.email"]).ok().and_then(|e| email_stem(&e));
    // a line that already stands in origin's tree is public, whatever it looks like
    let hits: Vec<Hit> = scan(&diff, stem.as_ref()).into_iter().filter(|h| !git(repo, &["grep", "-h", "-F", "-e", &h.text, "refs/remotes/origin/work"]).is_ok_and(|found| found.lines().any(|l| l.trim() == h.text.trim()))).collect();
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
    let message = st("message").unwrap_or_else(|| format!("work: tip {} (with commercial {})", super::short(&sha), super::short(&tip.main)));
    if dry {
        return Ok(format!("{{\"pushed\":false,\"dry\":true,\"tip\":{},\"origin\":{},\"files\":{files},\"scan\":\"clean\"}}", json_str(&sha), json_str(&origin)));
    }
    let mut args: Vec<String> = vec!["commit-tree".into(), format!("{sha}^{{tree}}"), "-p".into(), origin.clone()];
    for p in &parents {
        args.push("-p".into());
        args.push(p.clone());
    }
    args.push("-m".into());
    args.push(format!("{message}\n\n{TRAILER}"));
    let commit = git(repo, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    git(repo, &["push", "-q", "origin", &format!("{commit}:refs/heads/work")])?;
    let p = Pushed { pair: tip.clone(), commit: commit.clone(), origin_was: origin.clone(), at_ms: wall_ms(), files };
    ctx.store.update(|d| d.pushes.push(p));
    ctx.store.event("push", Sev::Info, format!("push: origin/work {} -> {} (tree of tip {}, {files} files)", super::short(&origin), super::short(&commit), tip.short()), None);
    Ok(format!("{{\"pushed\":true,\"commit\":{},\"origin_was\":{},\"tip\":{},\"files\":{files}}}", json_str(&commit), json_str(&origin), json_str(&sha)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
