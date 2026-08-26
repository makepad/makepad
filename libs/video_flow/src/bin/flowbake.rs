//! DEBUG RIG (never committed): bake every local/flowtest/*.mp4 that is
//! not already a bake into a *_baked.mp4 beside it — the offline classical
//! flow reference the realtime GPU tweener is compared against.

use makepad_video_flow::{convert_video, ConvertOptions};
use std::path::PathBuf;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/flowtest"));
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("flowtest dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("mp4")
                && !p.to_string_lossy().contains("_baked")
        })
        .collect();
    entries.sort();
    for input in entries {
        let stem = input.file_stem().unwrap().to_string_lossy().into_owned();
        let output = dir.join(format!("{stem}_baked.mp4"));
        print!("bake {stem:>18} … ");
        let report = convert_video(
            &input,
            &output,
            &ConvertOptions::default(),
            &mut |_| {},
            &|| false,
        );
        match report {
            Ok(r) => println!(
                "{}x{} {} frames {} pairs warps={}",
                r.width, r.height, r.frames, r.pairs, r.warps
            ),
            Err(e) => println!("FAILED: {e:?}"),
        }
    }
}
