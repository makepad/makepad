//! The on-disk demo home: a generated, plausible directory tree so a
//! terminal opened during a recording has something for `ls` that is NOT
//! the user's real files. Built once under the user cache dir and reused;
//! deterministic (seeded LCG picks names), so retakes look the same.
//!
//! Terminals default here whenever the desktop runs in demo mode (the same
//! `MAKEPAD_WM_FILES_REAL` knob that flips the Files app back to the real disk).

use std::fs;
use std::path::PathBuf;

/// A tiny deterministic LCG — no rand dep, stable across runs.
struct Lcg(u64);
impl Lcg {
    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        list[(self.0 >> 33) as usize % list.len()]
    }
}

/// Where the demo home lives.
pub fn demo_home_path() -> Option<PathBuf> {
    Some(crate::theme::makepad_home().join("wm/demo-home"))
}

/// Ensure the demo home exists (generate on first use) and return it.
pub fn ensure_demo_home() -> Option<PathBuf> {
    let root = demo_home_path()?;
    if root.join(".generated").exists() {
        return Some(root);
    }
    generate(&root).ok()?;
    Some(root)
}

fn generate(root: &PathBuf) -> std::io::Result<()> {
    let mut rng = Lcg(0x6d70776d_2026);
    let projects = ["helios", "tidepool", "quartz", "larkspur", "foxglove"];
    let clients = ["northwind", "aurora-labs", "bluenote", "kestrel"];
    let places = ["lisbon", "kyoto", "dolomites", "reykjavik"];

    let p1 = rng.pick(&projects);
    let mut p2 = rng.pick(&projects);
    if p2 == p1 {
        p2 = "tidepool";
    }
    let client = rng.pick(&clients);
    let place = rng.pick(&places);

    let dirs = [
        format!("Documents/invoices"),
        format!("Documents/notes"),
        format!("Projects/{p1}/src"),
        format!("Projects/{p1}/tests"),
        format!("Projects/{p2}/src"),
        format!("Pictures/{place}-2026"),
        format!("Downloads"),
        format!("Music"),
        format!("Videos"),
        format!(".config/{p1}"),
    ];
    for d in &dirs {
        fs::create_dir_all(root.join(d))?;
    }

    let files: &[(String, String)] = &[
        (
            "Documents/notes/standup.txt".into(),
            "- ship the importer fix\n- call the venue about friday\n- renew the domain\n".into(),
        ),
        (
            format!("Documents/invoices/{client}-2026-041.txt"),
            format!("Invoice 2026-041\nClient: {client}\nAmount: EUR 1,840.00\nStatus: paid\n"),
        ),
        (
            format!("Documents/invoices/{client}-2026-042.txt"),
            format!("Invoice 2026-042\nClient: {client}\nAmount: EUR 960.00\nStatus: sent\n"),
        ),
        (
            "Documents/expenses-q3.csv".into(),
            "date,category,amount\n2026-07-04,travel,231.90\n2026-07-19,hosting,48.00\n2026-08-02,hardware,899.00\n".into(),
        ),
        (
            format!("Projects/{p1}/Cargo.toml"),
            format!("[package]\nname = \"{p1}\"\nversion = \"0.3.1\"\nedition = \"2021\"\n"),
        ),
        (
            format!("Projects/{p1}/src/main.rs"),
            "fn main() {\n    println!(\"hello from the demo tree\");\n}\n".into(),
        ),
        (
            format!("Projects/{p1}/src/lib.rs"),
            "pub fn answer() -> u32 {\n    42\n}\n".into(),
        ),
        (
            format!("Projects/{p1}/tests/smoke.rs"),
            "#[test]\nfn it_works() {\n    assert_eq!(2 + 2, 4);\n}\n".into(),
        ),
        (
            format!("Projects/{p1}/build.log"),
            "   Compiling demo v0.3.1\n    Finished release [optimized] in 2.41s\n".into(),
        ),
        (
            format!("Projects/{p2}/src/main.rs"),
            "fn main() {\n    println!(\"tide levels nominal\");\n}\n".into(),
        ),
        (
            format!("Projects/{p2}/README.txt"),
            "A small experiment. Nothing to see yet.\n".into(),
        ),
        (
            "Downloads/receipt-8841.txt".into(),
            "Order 8841 — 1x USB-C dock — delivered\n".into(),
        ),
        (
            "Downloads/setlist-friday.txt".into(),
            "01 warmup loop\n02 sunset drive\n03 midnight arcade\n".into(),
        ),
        (
            format!(".config/{p1}/config.toml"),
            "theme = \"tokyo-night\"\nautosave = true\n".into(),
        ),
        (".generated".into(), "wm demo home v1\n".into()),
    ];
    for (path, body) in files {
        fs::write(root.join(path), body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_once_and_reuses() {
        let tmp = std::env::temp_dir().join(format!("wm-demo-home-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        generate(&tmp).unwrap();
        assert!(tmp.join("Documents/expenses-q3.csv").exists());
        assert!(tmp.join("Projects").exists());
        assert!(tmp.join(".generated").exists());
        // Deterministic: a second generation writes the same names.
        let listing = |p: &PathBuf| {
            let mut v: Vec<String> = fs::read_dir(p.join("Documents/invoices"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .collect();
            v.sort();
            v
        };
        let first = listing(&tmp);
        let tmp2 = std::env::temp_dir().join(format!("wm-demo-home-test2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp2);
        generate(&tmp2).unwrap();
        assert_eq!(first, listing(&tmp2));
        let _ = fs::remove_dir_all(&tmp);
        let _ = fs::remove_dir_all(&tmp2);
    }
}
