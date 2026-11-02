mod toml;
mod tree;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

const HEADER: &str = "# CI (devhub ci): [ci] is hand-written settings; the other sections are proofs devhub writes.\n";
type Section = BTreeMap<String, String>;
type Proofs = BTreeMap<String, Section>;

fn read(text: &str) -> Result<Proofs, ()> {
    let mut proofs = Proofs::new();
    for table in toml::parse(text)? {
        if table.name.is_empty() && table.fields.is_empty() { continue; }
        let parts: Vec<_> = table.name.split('.').collect();
        let valid = match parts.as_slice() {
            ["app" | "tests", os] | ["job", _, os] => matches!(*os, "linux" | "macos" | "windows"),
            ["ci"] => true,
            _ => false,
        };
        if !valid || parts.iter().any(|p| p.is_empty()) || table.array || !table.name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) { return Err(()); }
        let mut section = Section::new();
        for (key, value) in table.fields {
            if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) { return Err(()); }
            section.insert(key, toml::string(&value).ok_or(())?.to_string());
        }
        if proofs.insert(table.name, section).is_some() { return Err(()); }
    }
    Ok(proofs)
}

fn write(proofs: &Proofs) -> String {
    let mut text = HEADER.to_string();
    for (name, section) in proofs {
        text.push_str(&format!("\n[{name}]\n"));
        for (key, value) in section { text.push_str(&format!("{key} = {}\n", toml::quote(value))); }
    }
    text
}

fn merge(ancestor: &Proofs, current: &Proofs, other: &Proofs, mut digest: impl FnMut(&str) -> Option<String>) -> Proofs {
    let keys: BTreeSet<_> = ancestor.keys().chain(current.keys()).chain(other.keys()).collect();
    let mut out = Proofs::new();
    for key in keys {
        let (base, a, b) = (ancestor.get(key), current.get(key), other.get(key));
        let chosen = if a == b { a }
            else if a == base { b }
            else if b == base { a }
            // hand-written settings changed on both sides: this side's stand
            else if key == "ci" { a }
            else {
                let wanted = digest(key);
                let matches = |proof: Option<&Section>| wanted.as_ref().is_some_and(|d| proof.and_then(|p| p.get("digest")) == Some(d));
                match (matches(a), matches(b)) {
                    (true, false) => a,
                    (false, true) => b,
                    _ => match (a.and_then(|p| p.get("date")), b.and_then(|p| p.get("date"))) {
                        (Some(ad), Some(bd)) if ad > bd => a,
                        (Some(ad), Some(bd)) if bd > ad => b,
                        _ => None,
                    },
                }
            };
        if let Some(section) = chosen { out.insert(key.clone(), section.clone()); }
    }
    out
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        let _ = writeln!(std::io::stderr(), "usage: ci-merge <ancestor %O> <current %A> <other %B> <path %P>");
        return;
    }
    // Invalid/unreadable inputs cannot establish a proof. Clear them rather than retaining
    // conflict markers or guessing that a malformed ancestor was an empty file.
    let inputs: Option<Vec<Proofs>> = args[..3].iter().map(|p| std::fs::read_to_string(p).ok().and_then(|s| read(&s).ok())).collect();
    let out = match inputs {
        Some(p) => {
            let mut lookup = None;
            write(&merge(&p[0], &p[1], &p[2], |section| {
                lookup.get_or_insert_with(|| tree::Digests::new(Path::new(&args[3]))).get(section)
            }))
        }
        None => HEADER.to_string(),
    };
    if let Err(e) = std::fs::write(&args[1], out) {
        let _ = writeln!(std::io::stderr(), "ci-merge: cannot write current proof file: {e}");
    }
    // Even I/O or parse failure must never be returned to Git as a content conflict.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof(digest: &str, date: &str) -> Section {
        [("digest".into(), digest.into()), ("date".into(), date.into())].into()
    }

    #[test]
    fn concurrent_adjacent_sections_and_one_sided_changes() {
        let base: Proofs = [("app.linux".into(), proof("a", "2026-10-01")), ("tests.macos".into(), proof("b", "2026-10-01"))].into();
        let mut left = base.clone();
        let mut right = base.clone();
        left.insert("app.linux".into(), proof("c", "2026-10-02"));
        right.insert("tests.macos".into(), proof("d", "2026-10-03"));
        let got = merge(&base, &left, &right, |_| panic!("one-sided changes need no digest"));
        assert_eq!(got["app.linux"], left["app.linux"]);
        assert_eq!(got["tests.macos"], right["tests.macos"]);
        assert_eq!(read(&write(&got)).unwrap(), got);
        assert!(!write(&got).contains("<<<<<<<"));
        assert!(!write(&got).contains("======="));
        assert!(!write(&got).contains(">>>>>>>"));
        left.remove("app.linux");
        assert!(!merge(&base, &left, &base, |_| None).contains_key("app.linux"));
    }

    #[test]
    fn conflicting_proofs_prefer_digest_then_date_and_drop_ties() {
        let base: Proofs = [("app.linux".into(), proof("old", "2026-10-01"))].into();
        let left: Proofs = [("app.linux".into(), proof("left", "2026-10-02"))].into();
        let right: Proofs = [("app.linux".into(), proof("right", "2026-10-03"))].into();
        assert_eq!(merge(&base, &left, &right, |_| Some("left".into())), left);
        assert_eq!(merge(&base, &left, &right, |_| None), right);
        assert_eq!(merge(&base, &right, &left, |_| None), right);
        let tie: Proofs = [("app.linux".into(), proof("tie", "2026-10-02"))].into();
        assert!(merge(&base, &left, &tie, |_| None).is_empty());
        assert_eq!(merge(&base, &left, &left, |_| None), left);
    }

    // Moved from land.rs: the three unit types resolve independently. The driver now
    // retains the newer unmatched proof, instead of unconditionally discarding it.
    #[test]
    fn proof_conflicts_resolve_each_section() {
        let left: Proofs = [
            ("app.linux".into(), proof("1", "2026-10-01")),
            ("tests.macos".into(), proof("2", "2026-10-02")),
            ("job.jit.windows".into(), proof("3", "2026-10-03")),
        ].into();
        let right: Proofs = [
            ("app.linux".into(), proof("4", "2026-10-04")),
            ("tests.macos".into(), proof("2", "2026-10-05")),
            ("job.jit.windows".into(), proof("5", "2026-10-06")),
        ].into();
        let digest = |name: &str| Some(match name { "app.linux" => "1", "tests.macos" => "2", _ => "0" }.to_string());
        let got = merge(&Proofs::new(), &left, &right, digest);
        assert_eq!(got["app.linux"], left["app.linux"]);
        assert_eq!(got["tests.macos"], right["tests.macos"]);
        assert_eq!(got["job.jit.windows"], right["job.jit.windows"]);
        assert_eq!(got, merge(&Proofs::new(), &right, &left, digest));
    }

    #[test]
    fn hashes_and_parser() {
        assert_eq!(makepad_ci_merge::sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(makepad_ci_merge::sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(makepad_ci_merge::sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        let text = "# proofs\r\n[job.jit.windows]\r\ncapture = \"a\\\\b\\\"c\\n\"\r\n";
        assert_eq!(read(&write(&read(text).unwrap())).unwrap(), read(text).unwrap());
        assert!(read("<<<<<<< ours\n=======\n>>>>>>> theirs\n").is_err());
        let manifest = toml::parse("[dependencies]\nlocal = { version = \"1\", path = \"../local\", features = [\"a\", \"b\"] } # path = \"wrong\"\n[target.'cfg(windows)'.dependencies.win]\npath = \"../win\"\n").unwrap();
        assert_eq!(toml::string(toml::field(&manifest[1].fields["local"], "path").unwrap()), Some("../local"));
        assert_eq!(toml::string(&manifest[2].fields["path"]), Some("../win"));
    }
}
