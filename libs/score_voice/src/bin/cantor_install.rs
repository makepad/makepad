//! Install a neural voice where the score's singer loads it from
//! (`neural::model_path()`): `cantor_install <voice.mksing> [note]`.
//!
//! The file is checked to load, copied beside its destination and renamed
//! into place (a running Stage picks it up on its next start), and a
//! provenance note is written beside it: where it came from, when, and what
//! it may be used for.

use makepad_score_voice::neural;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(src) = args.get(1).map(PathBuf::from) else {
        eprintln!("usage: cantor_install <voice.mksing> [note]");
        std::process::exit(2);
    };
    if let Err(e) = neural::NeuralSinger::load(&src) {
        eprintln!("not a voice: {e}");
        std::process::exit(1);
    }
    let dest = neural::model_path();
    std::fs::create_dir_all(dest.parent().unwrap()).expect("model folder");
    let part = dest.with_extension("mksing.part");
    let bytes = std::fs::copy(&src, &part).expect("copy");
    std::fs::rename(&part, &dest).expect("rename into place");
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let note = args.get(2).cloned().unwrap_or_default();
    let provenance = format!(
        "model: Cantor neural singing voice\nfile: {}\nbytes: {bytes}\nsource: {}\ninstalled_unix: {unix}\nuse: development-only (trained on generated singing; not cleared for a shipped product)\nnote: {note}\n",
        dest.display(),
        src.display()
    );
    std::fs::write(dest.with_extension("provenance.txt"), provenance).expect("provenance");
    println!("installed {} ({bytes} bytes)", dest.display());
}
