//! Writes one generator preset as a Radiance .hdr, for the phase-2 visual
//! checks (`game.environment({hdri: "golden.hdr"})`, Scene3D):
//!
//!   cargo run -p makepad-render --release --example hdri_sample -- out.hdr "Golden hour" 1024
use makepad_render::hdri::{self, export, presets};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "golden.hdr".to_string()));
    let name = args.next().unwrap_or_else(|| "Golden hour".to_string());
    let width: usize = args.next().and_then(|w| w.parse().ok()).unwrap_or(1024);
    let params = presets::preset(&name).unwrap_or_else(|| {
        eprintln!("unknown preset {name}; one of {}", presets::PRESET_NAMES.join(", "));
        std::process::exit(2)
    });
    let (map, sun) = hdri::envmap::bake_env_map(&params, width, |n, f| (0..n).for_each(f));
    export::write_atomic(&out, &export::encode_hdr(&map)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let sun = sun.map(|s| hdri::az_el_from_dir(s.dir));
    println!("wrote {} ({}x{}, preset {name}, sun (az, el) {sun:?})", out.display(), map.width, map.height);
}
