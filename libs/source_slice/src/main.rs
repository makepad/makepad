//! makepad-source-slice slice --repo R --commit C --roots makepad-amp [--app amp] [--out DIR]
//! makepad-source-slice lint  --repo R --commit C --roots makepad-amp
//!
//! `slice` writes the slice commit into R and prints it with its crates;
//! `--out` also writes its files into DIR (`git archive`). `lint` prints the
//! references the slice's crates make outside their directory that their
//! `include` does not cover, and fails when there are any. `--roots` takes
//! package names, comma separated or repeated. `--app` names the app in the
//! commit message (default: the first root without its `makepad-` prefix);
//! the server and the Builder's publish pass their app ID, so the same
//! source, roots and app give the same commit everywhere.
use makepad_source_slice as slicer;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("makepad-source-slice: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("Expected slice or lint")?;
    let (mut repo, mut commit, mut app, mut out) = (None, None, None, None);
    let mut roots = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--repo" => repo = Some(PathBuf::from(value()?)),
            "--commit" => commit = Some(value()?),
            "--app" => app = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--roots" => roots.extend(value()?.split(',').filter(|s| !s.is_empty()).map(str::to_owned)),
            _ => return Err(format!("Unknown argument {arg}")),
        }
    }
    let repo = repo.unwrap_or_else(|| PathBuf::from("."));
    let commit = commit.unwrap_or_else(|| "HEAD".into());
    let source = slicer::Source::load(&repo, &commit)?;
    match command.as_str() {
        "slice" => {
            let first = roots.first().ok_or("--roots is required")?;
            let app = app.unwrap_or_else(|| first.strip_prefix("makepad-").unwrap_or(first).to_owned());
            let slice = source.slice(&repo, &app, &roots)?;
            println!("slice {} of {} for {app}: {} crates, {} files, {} bytes", slice.commit, slice.source, slice.crates.len(), slice.files, slice.bytes);
            for (name, dir) in &slice.crates {
                println!("  {name}  {dir}");
            }
            if let Some(out) = out {
                slicer::materialize(&repo, &slice.commit, &out)?;
                println!("written to {}", out.display());
            }
            Ok(())
        }
        "lint" => {
            let findings = source.lint(&repo, &roots)?;
            for finding in &findings {
                println!("{finding}");
            }
            if findings.is_empty() {
                println!("lint: no uncovered references in {} crates", source.closure(&roots)?.len());
                Ok(())
            } else {
                Err(format!("{} uncovered references", findings.len()))
            }
        }
        _ => Err(format!("Unknown command {command}; expected slice or lint")),
    }
}
