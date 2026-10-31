mod emit;
mod model;
// Shared dependency-free readers/digest; public modules keep their complete reusable APIs.
#[path = "../../makepad_builder/src/sha256.rs"]
pub mod sha256;
#[path = "../../bindgen_xml.rs"]
pub mod xml;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

fn verified(path: &Path, hashes: &BTreeMap<String, String>) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid input filename")?;
    let expected = hashes
        .get(name)
        .ok_or_else(|| format!("provenance.txt has no SHA-256 for {name}"))?;
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let actual = sha256::sha256_hex(&bytes);
    if actual != *expected {
        return Err(format!(
            "SHA-256 mismatch for {name}: expected {expected}, got {actual}"
        ));
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: makepad-dbus-codegen <xml dir> <constants.h> <out.rs>".into());
    }
    let dir = Path::new(&args[0]);
    let manifest = fs::read_to_string(dir.join("provenance.txt")).map_err(|e| e.to_string())?;
    let mut hashes = BTreeMap::new();
    for line in manifest.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() == 2
            && fields[0].len() == 64
            && fields[0].bytes().all(|c| c.is_ascii_hexdigit())
        {
            if hashes
                .insert(fields[1].to_owned(), fields[0].to_ascii_lowercase())
                .is_some()
            {
                return Err(format!("duplicate provenance entry {}", fields[1]));
            }
        }
    }
    let mut files = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    files.retain(|p| p.extension().is_some_and(|s| s == "xml"));
    files.sort();
    if files.is_empty() {
        return Err("no XML inputs".into());
    }
    let present: BTreeSet<_> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for name in hashes.keys().filter(|s| s.ends_with(".xml")) {
        if !present.contains(name) {
            return Err(format!("missing provenance input {name}"));
        }
    }
    let header = verified(Path::new(&args[1]), &hashes)?;
    let mut interfaces = Vec::new();
    let mut names = BTreeSet::new();
    for file in files {
        for interface in model::parse(&verified(&file, &hashes)?)
            .map_err(|e| format!("{}: {e}", file.display()))?
        {
            if !names.insert(interface.name.clone()) {
                return Err(format!("duplicate interface {}", interface.name));
            }
            interfaces.push(interface);
        }
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    // interfaces.txt (committed, next to the inputs) names the interfaces to generate: only what the
    // adapter serves or calls, so unused ones cost no compile time.
    let wanted = fs::read_to_string(dir.join("interfaces.txt")).map_err(|e| format!("interfaces.txt: {e}"))?;
    let wanted: BTreeSet<&str> = wanted
        .lines()
        .map(|l| l.split('#').next().unwrap().trim())
        .filter(|l| !l.is_empty())
        .collect();
    for name in &wanted {
        if !names.contains(*name) {
            return Err(format!("interfaces.txt names {name}, which no input defines"));
        }
    }
    interfaces.retain(|i| wanted.contains(i.name.as_str()));
    let mut output = emit::generate(&interfaces);
    output.push_str(&emit::constants(&header)?);
    let path = Path::new(&args[2]);
    // Format before replacing the destination, including when rustfmt reports an error.
    let temporary = path.with_extension("rs.tmp");
    fs::write(&temporary, output).map_err(|e| e.to_string())?;
    let status = Command::new("rustfmt")
        .args([
            "--edition",
            "2021",
            "--config",
            "disable_all_formatting=false,skip_children=true",
        ])
        .arg(&temporary)
        .status()
        .map_err(|e| format!("rustfmt: {e}"))?;
    if !status.success() {
        return Err("rustfmt failed; destination was not replaced".into());
    }
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    eprintln!(
        "generated {} interfaces into {}",
        interfaces.len(),
        path.display()
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("makepad-dbus-codegen: {error}");
        std::process::exit(1);
    }
}
