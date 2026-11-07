//! The one move of stored pins and revs to the named form ([`super::Pin::key`],
//! [`super::pin_revs`]), as a hub of protocol [`super::NESTED_PROTO`] starts: a pin key
//! `<makepad sha>,<sha>` becomes `<makepad sha>,commercial=<sha>`, a rev
//! `<branch>@<sha>,<branch>@<sha>` becomes `<branch>@<sha>,commercial=<branch>@<sha>` (the one
//! nested repository a pin could name before). The master's queued and granted leases, its state
//! (jobs, live sessions, chains) and its job snapshots, a node's lease records and its overlay
//! lanes' pins are rewritten and saved once; nothing reads the old form after. Goes when the land
//! takes nested repositories (stage 3).

use std::path::Path;

/// The fields that hold a pin key or a rev.
const FIELDS: [&str; 4] = ["rev", "pin", "deploy_pin", "deployment"];

fn hex40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A stored value in the named form, if it was in the old one.
fn named(value: &str) -> Option<String> {
    let (first, second) = value.split_once(',')?;
    let side = |s: &str| s.split_once('@').is_some_and(|(branch, sha)| !branch.is_empty() && !branch.contains(['=', ',']) && hex40(sha));
    ((hex40(first) && hex40(second)) || (side(first) && side(second))).then(|| format!("{first},{}={second}", crate::state::PAIRED))
}

/// `text` (JSON lines, a JSON document, or JSON quoted inside one) with each old-form value of a
/// pin or rev field named; None: it held none.
pub fn convert(text: &str) -> Option<String> {
    let mut out = text.to_string();
    let mut changed = false;
    for field in FIELDS {
        for (open, close) in [(format!("\"{field}\":\""), "\""), (format!("\\\"{field}\\\":\\\""), "\\\"")] {
            let mut from = 0;
            while let Some(at) = out[from..].find(&open).map(|i| from + i + open.len()) {
                let Some(end) = out[at..].find(close).map(|i| at + i) else { break };
                match named(&out[at..end]) {
                    Some(new) => {
                        from = at + new.len();
                        out.replace_range(at..end, &new);
                        changed = true;
                    }
                    None => from = end,
                }
            }
        }
    }
    changed.then_some(out)
}

/// Rewrites `path` in the named form when it holds the old one (a whole new file, renamed over it).
fn convert_file(path: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(false) };
    let Some(new) = convert(&text).or_else(|| named(text.trim()).filter(|_| !text.trim().contains('"'))) else { return Ok(false) };
    let tmp = path.with_extension("pre-nested");
    std::fs::write(&tmp, new).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn convert_all(paths: impl IntoIterator<Item = std::path::PathBuf>) -> Result<usize, String> {
    let mut n = 0;
    for p in paths {
        n += convert_file(&p)? as usize;
    }
    Ok(n)
}

/// The master's records, before its state and leases are read: how many files changed.
pub fn master(home: &Path) -> Result<usize, String> {
    let snaps = std::fs::read_dir(home.join("jobs/snap")).into_iter().flatten().flatten().map(|e| e.path().join("manifest.json"));
    convert_all(["leases.live", "state.json", "state.journal"].map(|f| home.join(f)).into_iter().chain(snaps))
}

/// A node's records, before its hub takes its leases back: its lease records (`state`) and its
/// overlay lanes' pins (`work`).
pub fn node(state: &Path, work: &Path) -> Result<usize, String> {
    let leases = std::fs::read_dir(state).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json"));
    let pins = std::fs::read_dir(work.join("dev/lane")).into_iter().flatten().flatten().map(|e| e.path().join("overlay-pin"));
    convert_all(leases.chain(pins))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_pins_and_revs_move_to_the_named_form_once() {
        let (a, c) = ("a".repeat(40), "c".repeat(40));
        let line = format!(r#"{{"id":"L1","req":{{"rev":"rc-x@{a},rc-x@{c}","lane":"x"}},"deploy_pin":"{a},{c}","deployment":"{a},{c}","pin":"{a}"}}"#);
        let new = convert(&line).unwrap();
        assert_eq!(new, format!(r#"{{"id":"L1","req":{{"rev":"rc-x@{a},commercial=rc-x@{c}","lane":"x"}},"deploy_pin":"{a},commercial={c}","deployment":"{a},commercial={c}","pin":"{a}"}}"#));
        assert_eq!(convert(&new), None, "the named form stays");
        // JSON quoted in a JSON string (a stored reply), and several in one text
        let quoted = format!(r#"{{"text":"{{\"rev\":\"work@{a},main@{c}\"}}","rev":"work@{a},main@{c}"}}"#);
        assert_eq!(convert(&quoted).unwrap(), format!(r#"{{"text":"{{\"rev\":\"work@{a},commercial=main@{c}\"}}","rev":"work@{a},commercial=main@{c}"}}"#));
        // what is no pin is left: a node's name, a single side, a CI id elsewhere
        for keep in [r#"{"pin":"node-a"}"#.to_string(), format!(r#"{{"rev":"work@{a}"}}"#), format!(r#"{{"id":"{a},{c}"}}"#)] {
            assert_eq!(convert(&keep), None, "{keep}");
        }
        // the files: a lease record and an overlay lane's pin on a node, a manifest on the master
        let d = std::env::temp_dir().join(format!("devhub-pre-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (state, work, home) = (d.join("leases"), d.join("work"), d.join("home"));
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(work.join("dev/lane/x")).unwrap();
        std::fs::create_dir_all(home.join("jobs/snap/S1")).unwrap();
        std::fs::write(state.join("L1.json"), format!(r#"{{"id":"L1","pin":"{a},{c}"}}"#)).unwrap();
        std::fs::write(state.join("L1-1.exit"), "0").unwrap();
        std::fs::write(work.join("dev/lane/x/overlay-pin"), format!("{a},{c}")).unwrap();
        std::fs::write(home.join("leases.live"), &line).unwrap();
        std::fs::write(home.join("jobs/snap/S1/manifest.json"), format!(r#"{{"rev":"rc-x@{a},rc-x@{c}","changed":[]}}"#)).unwrap();
        assert_eq!(node(&state, &work), Ok(2));
        assert_eq!(std::fs::read_to_string(work.join("dev/lane/x/overlay-pin")).unwrap(), format!("{a},commercial={c}"));
        assert_eq!(super::super::Pin::parse(&std::fs::read_to_string(work.join("dev/lane/x/overlay-pin")).unwrap()).map(|p| p.nested.len()), Some(1));
        assert_eq!(master(&home), Ok(2));
        assert!(std::fs::read_to_string(home.join("jobs/snap/S1/manifest.json")).unwrap().contains(&format!("rc-x@{a},commercial=rc-x@{c}")));
        assert_eq!((node(&state, &work), master(&home)), (Ok(0), Ok(0)), "once");
        let _ = std::fs::remove_dir_all(&d);
    }
}
