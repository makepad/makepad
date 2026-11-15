//! The master's side of the crossing: `pull` copies finished items from the jail's outbox,
//! `list` shows them as numbers and enums, `reply` prints one of three fixed replies and `credit`
//! the author's trailer. An item's record is parsed against the strict schema; its whitepaper is
//! stored opaque: its bytes are hashed and the crossing gate's shape rule is run again, nothing
//! is ever printed from it.

use super::item::{GateReason, Item, Status};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_PAPER: usize = super::gate::MAX_BYTES;

pub fn items_dir() -> PathBuf {
    super::protect::dir().join("items")
}

/// `~/.makepad/devhub/intake/pull.conf`: `jail=<ssh destination>` and `outbox=<path>` (machine
/// specific, so never tracked).
fn pull_conf() -> Result<(String, String), String> {
    let p = super::protect::dir().join("pull.conf");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e} (write jail=<ssh destination> and outbox=/var/lib/intake/outbox)", p.display()))?;
    let get = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}="))).map(|v| v.trim().to_string());
    let jail = get("jail").filter(|j| !j.is_empty() && j.bytes().all(|b| b.is_ascii_alphanumeric() || b"@.-_".contains(&b))).ok_or("pull.conf: no valid jail=")?;
    let outbox = get("outbox").unwrap_or_else(|| "/var/lib/intake/outbox".into());
    if !outbox.starts_with('/') || !outbox.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b)) {
        return Err("pull.conf: outbox must be a plain absolute path".into());
    }
    Ok((jail, outbox))
}

/// One remote read, at most `max` bytes (a remote file that does not exist: None).
fn remote(jail: &str, cmd: &str, max: usize) -> Result<Option<Vec<u8>>, String> {
    let out = Command::new("ssh").args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", jail, cmd]).stdin(Stdio::null()).output().map_err(|e| format!("ssh: {e}"))?;
    if !out.status.success() {
        return Ok(None);
    }
    if out.stdout.len() > max {
        return Err(format!("a remote answer over {max} bytes"));
    }
    Ok(Some(out.stdout))
}

pub fn valid_key(k: &str) -> bool {
    k.split_once('-').is_some_and(|(r, n)| matches!(r, "relax" | "makepad") && !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Take one item from its record and (when ready) its whitepaper: Ok with what was stored.
pub fn take(dir: &Path, key: &str, record: &[u8], paper: Option<&[u8]>) -> Result<Item, String> {
    let mut item = Item::parse(record)?;
    if item.key() != key {
        return Err(format!("{key}: the record names another item"));
    }
    let d = dir.join(key);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let _ = std::fs::remove_file(d.join("whitepaper.md"));
    if item.status == Status::Ready {
        let paper = paper.ok_or_else(|| format!("{key}: ready without a whitepaper"))?;
        let digest = super::sha256_hex(paper);
        // the jail's gate, run again on this side
        let gate = if paper.len() > MAX_PAPER || item.whitepaper_sha256.as_deref() != Some(digest.as_str()) { GateReason::Missing } else { super::gate::shape(paper, &item) };
        if gate != GateReason::None {
            item.status = Status::GateFailed;
            item.gate = gate;
        } else {
            std::fs::write(d.join("whitepaper.md"), paper).map_err(|e| format!("{key}: {e}"))?;
        }
    }
    std::fs::write(d.join("item.json"), item.to_json()).map_err(|e| format!("{key}: {e}"))?;
    Ok(item)
}

/// `intake pull`: the jail's finished items into the store.
pub fn pull(_args: &[&str]) -> i32 {
    let run = || -> Result<(u32, u32), String> {
        let (jail, outbox) = pull_conf()?;
        let listing = remote(&jail, &format!("ls -1 {outbox}"), 1 << 20)?.unwrap_or_default();
        let (mut taken, mut refused) = (0, 0);
        for key in String::from_utf8_lossy(&listing).lines().map(str::trim) {
            if !valid_key(key) {
                refused += 1;
                continue;
            }
            let Some(record) = remote(&jail, &format!("cat {outbox}/{key}/item.json"), super::item::MAX_ITEM_JSON)? else { continue };
            let stored = std::fs::read(items_dir().join(key).join("item.json")).ok();
            let same = stored.as_deref().and_then(|s| Item::parse(s).ok()).zip(Item::parse(&record).ok()).is_some_and(|(a, b)| a.updated == b.updated && a.status == b.status && a.whitepaper_sha256 == b.whitepaper_sha256);
            if same {
                continue;
            }
            let paper = remote(&jail, &format!("cat {outbox}/{key}/whitepaper.md"), MAX_PAPER)?;
            match take(&items_dir(), key, &record, paper.as_deref()) {
                Ok(_) => taken += 1,
                Err(e) => {
                    eprintln!("intake pull: {}", e.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(120).collect::<String>());
                    refused += 1;
                }
            }
        }
        Ok((taken, refused))
    };
    match run() {
        Ok((t, r)) => {
            println!("intake pull: {t} item(s) taken, {r} refused");
            0
        }
        Err(e) => {
            eprintln!("intake pull: {e}");
            1
        }
    }
}

pub fn stored() -> Vec<Item> {
    let mut out: Vec<Item> = std::fs::read_dir(items_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("item.json")).ok())
        .filter_map(|b| Item::parse(&b).ok())
        .collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.updated));
    out
}

/// `intake list`: numbers and enums only.
pub fn list(_args: &[&str]) -> i32 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    println!("{:<14} {:<5} {:<5} {:<9} {:<12} {:<16} {:<6} {:>5} {:>6}", "item", "kind", "class", "verdict", "status", "gate", "model", "turns", "age h");
    for i in stored() {
        let verdict = i.prefilter.as_ref().map_or("-", |p| p.verdict.word());
        println!("{:<14} {:<5} {:<5} {:<9} {:<12} {:<16} {:<6} {:>5} {:>6}", i.tag(), i.kind.word(), i.class.word(), verdict, i.status.word(), i.gate.word(), i.paper_model.word(), i.paper_turns, now.saturating_sub(i.updated) / 3600);
    }
    0
}

fn find(tag: &str) -> Option<Item> {
    stored().into_iter().find(|i| i.tag() == tag)
}

/// The three replies; the user posts them.
pub fn reply_text(kind: &str, commit: Option<&str>) -> Option<String> {
    match (kind, commit) {
        ("fixed", Some(c)) => Some(format!("Thank you for the report. This is fixed in {c}. The fix was written from our own reproduction of the problem.")),
        ("not-reproducible", None) => Some("Thank you for the report. We could not reproduce this with the current relax under the conditions we tried. If it still happens for you, please open a new issue with a minimal program and the exact command.".into()),
        ("declined", None) => Some("Thank you for the contribution. We have decided not to take this change. Relax changes are written by the maintainers from our own reproductions, so pull requests are closed rather than merged.".into()),
        _ => None,
    }
}

/// `intake reply <item> fixed <commit> | not-reproducible | declined`.
pub fn reply(args: &[&str]) -> i32 {
    let (Some(tag), Some(kind)) = (args.first(), args.get(1)) else {
        eprintln!("usage: makepad devhub intake reply <relax#N> fixed <commit> | not-reproducible | declined");
        return 2;
    };
    if find(tag).is_none() {
        eprintln!("intake reply: {tag} is not in the store");
        return 1;
    }
    let commit = args.get(2).copied();
    if let Some(c) = commit {
        if !(7..=40).contains(&c.len()) || !c.bytes().all(|b| b.is_ascii_hexdigit()) {
            eprintln!("intake reply: a commit is 7 to 40 hex digits");
            return 2;
        }
    }
    match reply_text(kind, commit) {
        Some(t) => {
            println!("{t}");
            0
        }
        None => {
            eprintln!("intake reply: fixed <commit>, not-reproducible or declined");
            2
        }
    }
}

/// `intake credit <item>`: the author's co-author trailer for a clean-room fix.
pub fn credit(args: &[&str]) -> i32 {
    match args.first().and_then(|t| find(t)) {
        Some(i) => match i.credit() {
            Some(line) => {
                println!("Intake: {}\n{line}", i.tag());
                0
            }
            None => {
                eprintln!("intake credit: {} has no valid author login", i.tag());
                1
            }
        },
        None => {
            eprintln!("usage: makepad devhub intake credit <relax#N> (an item in the store)");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_takes_records_and_keeps_papers_opaque() {
        let tmp = std::env::temp_dir().join(format!("intake-store-{}", std::process::id()));
        let (item, paper) = crate::intake::gate::tests::good_paper();
        let mut ready = item.clone();
        ready.status = Status::Ready;
        ready.gate = GateReason::None;
        ready.whitepaper_sha256 = Some(super::super::sha256_hex(paper.as_bytes()));
        let taken = take(&tmp, &ready.key(), ready.to_json().as_bytes(), Some(paper.as_bytes())).unwrap();
        assert_eq!(taken.status, Status::Ready, "a ready item with a good paper stays ready");
        assert!(tmp.join(ready.key()).join("whitepaper.md").exists(), "its paper is stored");
        let tampered = format!("{paper}\nmore");
        let taken = take(&tmp, &ready.key(), ready.to_json().as_bytes(), Some(tampered.as_bytes())).unwrap();
        assert_eq!((taken.status, taken.gate), (Status::GateFailed, GateReason::Missing), "a paper that is not the one the jail hashed is refused");
        assert!(!tmp.join(ready.key()).join("whitepaper.md").exists(), "and not stored");
        assert!(take(&tmp, "relax-999", ready.to_json().as_bytes(), None).is_err(), "a record under another item's key is refused");
        assert!(take(&tmp, &ready.key(), b"{\"repo\":\"relax\"}", None).is_err(), "a record off the schema is refused");
        assert!(valid_key("relax-12") && !valid_key("relax-") && !valid_key("../x") && !valid_key("other-1"), "outbox keys");
        assert!(reply_text("fixed", Some("abc1234")).is_some_and(|t| t.contains("abc1234")) && reply_text("fixed", None).is_none() && reply_text("other", None).is_none(), "three fixed replies");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
