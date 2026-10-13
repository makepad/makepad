//! Whole-corpus checks, run only when `MILKDROP_CORPUS` names a directory of
//! `.milk` files (a downloaded preset pack). Prints the success counts and
//! the most common failure causes; fails only on a panic.

use makepad_milkdrop::expr::{Context, Program};
use makepad_milkdrop::Preset;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn corpus() -> Option<Vec<PathBuf>> {
    let dir = std::env::var_os("MILKDROP_CORPUS")?;
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::from(dir)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
                out.push(path);
            }
        }
    }
    out.sort();
    Some(out)
}

fn short(message: &str) -> String {
    message.chars().take(60).collect()
}

#[test]
fn every_preset_parses_and_its_equations_compile() {
    let Some(files) = corpus() else { return };
    let mut blocks = 0usize;
    let mut failed_blocks = 0usize;
    let mut presets_ok = 0usize;
    let mut causes: HashMap<String, usize> = HashMap::new();
    let start = std::time::Instant::now();
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let preset = Preset::parse(&bytes);
        let mut ok = true;
        let mut check = |src: &str| {
            if src.trim().is_empty() {
                return;
            }
            blocks += 1;
            let mut ctx = Context::new(1);
            if let Err(e) = Program::compile(src, &mut ctx) {
                failed_blocks += 1;
                ok = false;
                *causes.entry(short(&e.message)).or_default() += 1;
            }
        };
        check(&preset.per_frame_init);
        check(&preset.per_frame);
        check(&preset.per_pixel);
        for w in &preset.waves {
            check(&w.init);
            check(&w.per_frame);
            check(&w.per_point);
        }
        for s in &preset.shapes {
            check(&s.init);
            check(&s.per_frame);
        }
        if ok {
            presets_ok += 1;
        }
    }
    let mut causes: Vec<_> = causes.into_iter().collect();
    causes.sort_by(|a, b| b.1.cmp(&a.1));
    println!(
        "presets {} parsed, {} with every block compiling; blocks {} compiled of {} ({:?})",
        files.len(),
        presets_ok,
        blocks - failed_blocks,
        blocks,
        start.elapsed()
    );
    for (cause, n) in causes.iter().take(10) {
        println!("  {n:5} {cause}");
    }
    let _ = Path::new("");
}

