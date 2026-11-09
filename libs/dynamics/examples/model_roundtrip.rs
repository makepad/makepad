//! Compare a binary reference model with its Rust serialization.
use makepad_dynamics::model_file::{load_model, model_field_at, save_model};
use std::path::Path;

fn roundtrip(path: &Path) -> Result<bool, String> {
    let original = std::fs::read(path).map_err(|error| error.to_string())?;
    let m = load_model(path)?;
    let saved = save_model(&m);
    let different = original.iter().zip(&saved).position(|(a, b)| a != b)
        .or_else(|| (original.len() != saved.len()).then_some(original.len().min(saved.len())));
    print!("{}: {} {} {} {} {} {}, {} bytes, ",
        path.display(), m.nq, m.nv, m.nbody, m.njnt, m.ngeom, m.nmesh, original.len());
    if let Some(offset) = different {
        println!("first differing byte {offset} in {} (file {:?}, saved {:?})",
            model_field_at(&m, offset).unwrap_or("end of file"),
            original.get(offset), saved.get(offset));
        Ok(false)
    } else {
        println!("identical");
        Ok(true)
    }
}

fn main() {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: model_roundtrip <file>...");
        std::process::exit(1);
    }
    let mut failed = false;
    for path in paths {
        match roundtrip(Path::new(&path)) {
            Ok(identical) => failed |= !identical,
            Err(error) => {
                eprintln!("{}: {error}", Path::new(&path).display());
                failed = true;
            }
        }
    }
    if failed { std::process::exit(1); }
}
