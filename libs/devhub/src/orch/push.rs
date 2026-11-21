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
/// Codex's credit on a pushed commit that carries code Codex wrote: GitHub's `codex` account, which
/// links (`noreply@openai.com` links to no account).
pub const CODEX_TRAILER: &str = "Co-Authored-By: Codex <267193182+codex@users.noreply.github.com>";
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
            let credits: Vec<String> = groups.contributors.get(*at).filter(|c| !c.is_empty()).map(|c| json_str(c.trim_start_matches("Contributors: "))).into_iter().collect();
            let coauthors: Vec<String> = groups.coauthors.get(*at).map_or(&[][..], Vec::as_slice).iter().map(|l| json_str(l)).collect();
            format!("{{\"sha\":{},\"title\":{},\"files\":{},\"added\":{},\"removed\":{},\"areas\":[{}],\"contributors\":[{}],\"coauthors\":[{}]}}", json_str(sha), json_str(&groups.blocks[*at].title), stat.files.len(), stat.added, stat.removed, areas.join(","), credits.join(","), coauthors.join(","))
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
    if let Some(path) = st("remap") {
        return remap(ctx, &path);
    }
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

/// The pushed commit that carries local work commit `local` (a full sha not on origin/work
/// itself), as the push that sent it gave it its credits: of the oldest push on origin/work whose
/// tip holds it, the area commit of the first block holding a file it changed (for a merge, what it
/// brought in over its first parent), or the set's last when none does ([`group::coauthor_lines`]).
/// The pushes origin/work stands on come from the ledger, each standing on the one before
/// (`origin_was`), the chain a push reads its `since` from. None: no push of that chain carried it,
/// or origin/work is no push of the ledger (`devhub promote` re-pointed it). An ancestry it cannot
/// tell (a tip it cannot read), a commit the chain's oldest push holds (that push stands on a
/// commit the ledger has no push of, so the change may have gone out there or before: the ledger
/// began later, a promote re-pointed work, or a commit reached origin outside a push) and a blocks
/// file whose titles repeat (an area commit is found by its title) are errors, never a guess.
/// Intake's post gate names this commit in a close of a local sha and checks it as it would the
/// sha itself: on origin/work, and for a port, carrying the author's credit line.
pub fn carrier(repo: &std::path::Path, pushes: &[Pushed], local: &str) -> Result<Option<String>, String> {
    let origin = git(repo, &["rev-parse", "refs/remotes/origin/work"])?;
    let mut chain: Vec<&Pushed> = Vec::new();
    let mut at = origin.as_str();
    while let Some(p) = pushes.iter().rev().find(|p| p.commit == at && !chain.iter().any(|c| c.commit == at)) {
        chain.push(p);
        at = p.origin_was.as_str();
    }
    // newest first: the oldest whose tip holds it, the one before it read and found without it
    let mut held = 0usize;
    for p in &chain {
        if !ancestor(repo, local, &p.pair.work)? {
            break;
        }
        held += 1;
    }
    let Some(&push) = held.checked_sub(1).map(|n| &chain[n]) else { return Ok(None) };
    if held == chain.len() {
        return Err(format!(
            "push carrier: {} is in the tip of {}, the oldest push the ledger chains under origin/work, which stands on {}, no push of the ledger: it may have gone out there or before, so name its public commit",
            super::short(local),
            super::short(&push.commit),
            super::short(&push.origin_was)
        ));
    }
    let log = git(repo, &["log", "--first-parent", "--reverse", "--format=%H %s", &format!("{}..{}", push.origin_was, push.commit)])?;
    let set: Vec<(&str, &str)> = log.lines().filter_map(|l| l.split_once(' ')).collect();
    let blocks = group::parse_blocks(&git(repo, &["show", &format!("{}:{}", push.pair.work, group::BLOCKS_FILE)])?);
    if let Some(title) = title_twice(&blocks) {
        return Err(format!("push carrier: {} at {} names block `{title}` twice: an area commit is found by its title", group::BLOCKS_FILE, super::short(&push.pair.work)));
    }
    let parents = git(repo, &["log", "-1", "--format=%P", local])?;
    let files = match parents.split_whitespace().collect::<Vec<_>>()[..] {
        [first, _, ..] => git(repo, &["-c", "core.quotePath=false", "diff", "--name-only", "--no-renames", first, local])?,
        _ => git(repo, &["-c", "core.quotePath=false", "log", "-1", "--format=", "--name-only", local])?,
    };
    let first = if blocks.is_empty() { None } else { files.lines().filter(|f| !f.trim().is_empty()).filter_map(|f| set.iter().position(|(_, title)| *title == blocks[group::block_of(&blocks, f)].title)).min() };
    Ok(first.or(set.len().checked_sub(1)).map(|n| set[n].0.to_string()))
}

/// A block title the blocks file gives twice.
fn title_twice(blocks: &[group::Block]) -> Option<&str> {
    let mut titles: Vec<&str> = blocks.iter().map(|b| b.title.as_str()).collect();
    titles.sort_unstable();
    titles.windows(2).find(|w| w[0] == w[1]).map(|w| w[0])
}

/// `a` is `b` or one of its ancestors: an error, not "no", when either does not read as a commit or
/// the walk fails ([`is_ancestor`] reads those as "no").
fn ancestor(repo: &std::path::Path, a: &str, b: &str) -> Result<bool, String> {
    crate::gitread::with(repo, |p, r| {
        let a = crate::gitread::resolve(p, r, a).ok_or_else(|| format!("push carrier: {a} reads as no commit"))?;
        let b = crate::gitread::resolve(p, r, b).ok_or_else(|| format!("push carrier: {b} reads as no commit"))?;
        makepad_git::is_ancestor(r, a, b).map_err(|e| format!("push carrier: is {} in {}: {e}", a.to_hex(), b.to_hex()))
    })
    .unwrap_or_else(|| Err(format!("push carrier: {}: no repository", repo.display())))
}

/// `push {"remap":"<file>"}`: after a rewrite of origin's public history the ledger's public names
/// follow it. The file has one `<old> <new>` line per rewritten commit (the rewrite's commit map);
/// each entry's `commit` and `origin_was` found there takes its new name. Refused, nothing changed,
/// unless the newest entry then names origin/work's tip: the next push's `since`, and so its
/// credits, stay those of the last push.
fn remap(ctx: &Ctx, path: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("push remap: {path}: {e}"))?;
    let oid = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let mut map = std::collections::HashMap::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let mut w = line.split_whitespace();
        match (w.next(), w.next(), w.next()) {
            (Some(old), Some(new), None) if oid(old) && oid(new) => map.insert(old.to_string(), new.to_string()),
            _ => return Err(format!("push remap: {path}:{}: not `<old sha> <new sha>`", n + 1)),
        };
    }
    let repo = &ctx.repo;
    git(repo, &["fetch", "-q", "origin", "work"])?;
    let origin = git(repo, &["rev-parse", "refs/remotes/origin/work"])?;
    let changed = ctx.store.update(|d| {
        let mut pushes = d.pushes.clone();
        let changed = remap_pushes(&mut pushes, &map);
        match pushes.last() {
            Some(p) if p.commit == origin => {
                d.pushes = pushes;
                Ok(changed)
            }
            last => Err(format!("push remap: the newest push would name {}, origin/work is {}: nothing changed", last.map_or("nothing".into(), |p| super::short(&p.commit)), super::short(&origin))),
        }
    })??;
    ctx.store.event("push", Sev::Info, format!("push remap: {changed} ledger names follow the rewrite of origin ({} commits mapped); origin/work {}", map.len(), super::short(&origin)), None);
    Ok(format!("{{\"remapped\":{changed},\"mapped\":{},\"origin\":{}}}", map.len(), json_str(&origin)))
}

/// Renames each push's `commit` and `origin_was` found in `map`; how many names changed.
fn remap_pushes(pushes: &mut [Pushed], map: &std::collections::HashMap<String, String>) -> usize {
    let mut changed = 0;
    for p in pushes {
        for name in [&mut p.commit, &mut p.origin_was] {
            if let Some(new) = map.get(name.as_str()) {
                *name = new.clone();
                changed += 1;
            }
        }
    }
    changed
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

    /// A push's area commits carry the person co-authors and the intake's `Contributors:` line of
    /// what they squash, and a local commit resolves to the area commit that carries it: of the
    /// oldest push of origin's chain whose tip holds it, the first block holding its files; none
    /// before a push sent it, none through a push origin/work does not stand on, and none once
    /// origin/work is no push of the ledger. A merge goes by what it brought in, a change that came
    /// and went by the set's last commit; a tip it cannot read and a title the blocks file repeats
    /// are errors.
    #[test]
    fn a_local_commit_resolves_to_the_pushed_commit_that_carries_its_credit() {
        let dir = std::env::temp_dir().join(format!("devhub-carrier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [vec!["init", "-q"], vec!["config", "user.name", "t"], vec!["config", "user.email", "t@t"]] {
            git(&dir, &args).unwrap();
        }
        let commit = |files: &[(&str, &str)], message: &str| {
            for (path, text) in files {
                let p = dir.join(path);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, text).unwrap();
            }
            git(&dir, &["add", "-A"]).unwrap();
            git(&dir, &["commit", "-q", "-m", message]).unwrap();
            git(&dir, &["rev-parse", "HEAD"]).unwrap()
        };
        let push = |since: &str, origin: &str, tip: &str| {
            let mut groups = Groups::between(&dir, origin, tip).unwrap();
            groups.credit(&dir, since).unwrap();
            let set = groups.build(&dir, &[], &|_, _| None, &|_| None).unwrap();
            let last = set.last().unwrap().1.clone();
            git(&dir, &["update-ref", "refs/remotes/origin/work", &last]).unwrap();
            (commit_rows(&groups, &set), set.into_iter().map(|(_, sha)| sha).collect::<Vec<_>>(), Pushed { pair: Pair::new(tip, ""), commit: last, origin_was: origin.to_string(), at_ms: 0, files: 0 })
        };
        let base = commit(&[(group::BLOCKS_FILE, "block First\nlibs/a/*\nblock Second\nlibs/b/*\nblock Rest\n*\n"), ("libs/a/x.rs", "1\n"), ("libs/b/y.rs", "1\n")], "base");
        git(&dir, &["update-ref", "refs/remotes/origin/work", &base]).unwrap();
        let credit = "Co-Authored-By: octo-cat <1+octo-cat@users.noreply.github.com>";
        let port = commit(&[("libs/b/y.rs", "2\n"), ("README.md", "r\n")], &format!("a port\n\nIntake: makepad#7\n{credit}\nCo-Authored-By: Some One <one@example.net>\n"));
        let ours = commit(&[("libs/a/x.rs", "2\n")], "ours");
        let (rows, first, p1) = push(&base, &base, &ours);
        assert_eq!(first.len(), 3, "First, Second and Rest");
        let message = git(&dir, &["log", "-1", "--format=%B", &first[1]]).unwrap();
        assert!(message.lines().any(|l| l == credit) && message.lines().any(|l| l == "Contributors: octo-cat (#7)"), "the port's area commit credits its author: {message}");
        assert!(!message.contains("example.net"), "no other address of a person is pushed");
        assert!(rows.contains(&format!("\"coauthors\":[{}]", json_str(credit))), "the dry run lists it: {rows}");
        let mut pushes = vec![p1.clone()];
        let held = carrier(&dir, &pushes, &port).unwrap_err();
        assert!(held.contains("may have gone out there or before"), "the chain's oldest push holds it, and nothing tells what went out before it: {held}");
        // the push origin's base was, which the first one stands on
        pushes.insert(0, Pushed { pair: Pair::new(&base, ""), commit: base.clone(), origin_was: "f".repeat(40), at_ms: 0, files: 0 });
        assert_eq!(carrier(&dir, &pushes, &port).unwrap().as_deref(), Some(first[1].as_str()), "the first block holding its files");
        assert_eq!(carrier(&dir, &pushes, &ours).unwrap().as_deref(), Some(first[0].as_str()));
        let later = commit(&[("libs/a/x.rs", "3\n")], "later");
        assert_eq!(carrier(&dir, &pushes, &later).unwrap(), None, "not pushed yet");
        // a push that origin/work does not stand on (it was replaced), then the one that it does
        pushes.push(Pushed { pair: Pair::new(&later, ""), commit: "e".repeat(40), origin_was: p1.commit.clone(), at_ms: 0, files: 0 });
        let (_, second, p2) = push(&ours, &p1.commit, &later);
        let after_second = p2.commit.clone();
        pushes.push(p2);
        assert_eq!(carrier(&dir, &pushes, &port).unwrap().as_deref(), Some(first[1].as_str()), "the push that first sent it, not a later one");
        assert_eq!(carrier(&dir, &pushes, &later).unwrap().as_deref(), Some(second[0].as_str()));
        // a merge that brings in libs/a, a change of libs/b beside it, and one in Rest that came and went
        git(&dir, &["checkout", "-q", "-b", "lane"]).unwrap();
        commit(&[("libs/a/x.rs", "4\n")], "on the lane");
        git(&dir, &["checkout", "-q", "-"]).unwrap();
        commit(&[("libs/b/y.rs", "3\n")], "b again");
        let came = commit(&[("libs/c/z.txt", "z\n")], "came");
        std::fs::remove_file(dir.join("libs/c/z.txt")).unwrap();
        commit(&[], "went");
        git(&dir, &["merge", "-q", "--no-ff", "-m", "Merge lane", "lane"]).unwrap();
        let merge = git(&dir, &["rev-parse", "HEAD"]).unwrap();
        let (_, third, p3) = push(&later, &after_second, &merge);
        pushes.push(p3);
        assert_eq!(third.len(), 2, "First and Second; Rest is unchanged");
        assert_eq!(carrier(&dir, &pushes, &merge).unwrap().as_deref(), Some(third[0].as_str()), "a merge: the block of what it brought in");
        assert_eq!(carrier(&dir, &pushes, &came).unwrap().as_deref(), Some(third[1].as_str()), "only a block the set does not hold: its last commit");
        let mut unreadable = pushes.clone();
        unreadable[0].pair.work = "0".repeat(40);
        assert!(carrier(&dir, &unreadable, &port).is_err(), "an older tip it cannot read: an error, never a newer push's commit");
        assert_eq!(title_twice(&group::parse_blocks("block A\nx/*\nblock B\ny/*\nblock A\n*\n")), Some("A"));
        assert_eq!(title_twice(&group::parse_blocks("block A\nx/*\nblock B\n*\n")), None);
        git(&dir, &["update-ref", "refs/remotes/origin/work", &ours]).unwrap();
        assert_eq!(carrier(&dir, &pushes, &port).unwrap(), None, "origin/work is no push of the ledger");
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
