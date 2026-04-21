use makepad_mlx::MlxSafetensorsHeader;
use std::env;

fn usage() -> ! {
    eprintln!("usage: safetensors-query <file.safetensors> [substring]");
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).unwrap_or_else(|| usage());
    let pattern = env::args().nth(2);
    let header = MlxSafetensorsHeader::load(&path)?;

    let mut names = header.tensors.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        if let Some(pattern) = &pattern {
            if !name.contains(pattern) {
                continue;
            }
        }
        let entry = header.tensor(&name).unwrap();
        println!("{}\t{:?}\t{:?}", name, entry.dtype, entry.shape);
    }

    Ok(())
}
