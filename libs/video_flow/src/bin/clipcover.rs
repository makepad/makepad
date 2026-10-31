//! Decode clips frame by frame and print how much of each frame is flat
//! (blank) rows / columns: `clipcover a.mp4 [b.mp4 ...]`. Tells a half-empty
//! FILE from a half-empty PLAYER.

use makepad_video::VideoFileDecoder;
use makepad_video_flow::coverage::measure;

fn main() {
    // CLIPCOVER_DUMP=<frame>:<file> writes that frame's luma plane raw.
    let dump = std::env::var("CLIPCOVER_DUMP").ok().and_then(|v| { let (n, f) = v.split_once(':')?; Some((n.parse::<usize>().ok()?, f.to_string())) });
    let verbose = std::env::var("CLIPCOVER_V").is_ok();
    for path in std::env::args().skip(1) {
        let mut d = match VideoFileDecoder::open(&path) {
            Ok(d) => d,
            Err(e) => {
                println!("{path}: open failed: {e}");
                continue;
            }
        };
        let info = d.info().clone();
        let (mut frames, mut worst_rows, mut worst_cols, mut bad) = (0usize, 0f32, 0f32, 0usize);
        let mut first_bad = None;
        while let Ok(Some(f)) = d.next_frame() {
            let (w, h) = (f.width as usize, f.height as usize);
            if let Some((n, out)) = &dump {
                if *n == frames {
                    let _ = std::fs::write(out, &f.nv12[..w * h]);
                }
            }
            let c = measure(&f.nv12[..w * h], w, h, w, 2);
            if verbose && frames % 15 == 0 {
                println!("  frame {frames}: flat rows {} (longest {}) of {h}, flat cols {} (longest {}) of {w}", c.flat_rows, c.longest_flat_rows, c.flat_cols, c.longest_flat_cols);
            }
            worst_rows = worst_rows.max(c.flat_row_band());
            worst_cols = worst_cols.max(c.flat_col_band());
            if c.flat_row_band() >= 0.25 || c.flat_col_band() >= 0.25 {
                bad += 1;
                first_bad.get_or_insert(frames);
            }
            frames += 1;
        }
        println!(
            "{path}: {}x{} {frames} frames; worst flat row band {:.0}%, col band {:.0}%; frames with a >=25% band: {bad} (first {:?})",
            info.width, info.height, worst_rows * 100.0, worst_cols * 100.0, first_bad
        );
    }
}
