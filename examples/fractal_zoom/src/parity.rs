//! Local tiles against the node's: the same tile through the app's SIMD
//! kernels and through the fractal program a node runs (in-process here: the
//! same Splash, the same bits), texel by texel.

use crate::kernels::{render_tile, FractalKind, Params, Precision, TileRect};
use crate::remote::{tile_message, SLOTS};
use makepad_compute_program::programs::FRACTAL;
use makepad_compute_program::{OutputData, Program, Session, SessionConfig};

/// The node's tile: one message to the program, its output back.
fn node_tile(node: &Session, p: &Params, rect: TileRect, res: usize) -> Vec<u32> {
    let rev = node.send("tile", tile_message(0, p, rect.x, rect.y, rect.size, res));
    loop {
        let u = node.wait();
        if let Some(Err(e)) = &u.end {
            panic!("the fractal program ended: {e}");
        }
        if let Some(o) = u.output(SLOTS[0]).filter(|o| o.rev == rev) {
            match &o.data {
                OutputData::Words(words) => return words.to_vec(),
                other => panic!("a tile is words: {other:?}"),
            }
        }
    }
}

/// (differing texels, differing class/code bytes, largest low-24 difference)
fn compare(a: &[u32], b: &[u32]) -> (usize, usize, u32) {
    let mut n = 0;
    let mut top = 0;
    let mut low = 0;
    for (x, y) in a.iter().zip(b) {
        if x != y {
            n += 1;
            if x >> 24 != y >> 24 {
                top += 1;
            }
            low = low.max(((x & 0xff_ffff) as i64 - (y & 0xff_ffff) as i64).unsigned_abs() as u32);
        }
    }
    (n, top, low)
}

pub fn cases() -> Vec<(&'static str, Params, (f64, f64, f64))> {
    let base = Params::default();
    let mut v = Vec::new();
    for (name, de, trap) in [("plain", false, false), ("de", true, false), ("trap", false, true)] {
        let p = Params { de, trap, ..base };
        v.push((name, Params { kind: FractalKind::Mandelbrot, ..p }, (-0.6, 0.0, 3.4)));
        v.push((name, Params { kind: FractalKind::Mandelbrot, max_iter: 1500, ..p }, (-0.743_643_887_037_151, 0.131_825_904_205_330, 2.0e-9)));
        v.push((name, Params { kind: FractalKind::Julia, julia: (-0.123, 0.745), ..p }, (0.0, 0.0, 3.2)));
        v.push((name, Params { kind: FractalKind::BurningShip, ..p }, (-1.762, -0.028, 0.06)));
        v.push((name, Params { kind: FractalKind::Tricorn, ..p }, (-0.3, 0.0, 3.8)));
        v.push((name, Params { kind: FractalKind::Multibrot, power: 4, ..p }, (0.0, 0.0, 3.2)));
    }
    // past one pass (2048 iterations): the orbit state carried from pass to pass
    v.push(("long", Params { kind: FractalKind::Mandelbrot, max_iter: 20000, de: true, ..base }, (-0.743_643_887_037_151, 0.131_825_904_205_330, 2.0e-9)));
    v.push(("long", Params { kind: FractalKind::Multibrot, power: 3, max_iter: 7000, trap: true, ..base }, (0.0, 0.0, 3.2)));
    v.push(("newton", Params { kind: FractalKind::Newton, power: 3, de: false, ..base }, (0.0, 0.0, 3.0)));
    v.push(("newton5", Params { kind: FractalKind::Newton, power: 5, de: false, ..base }, (0.3, 0.1, 0.5)));
    v
}

/// Prints the per-case numbers; fails on any differing f64 texel.
#[test]
fn local_tiles_match_node_tiles() {
    let res = 64;
    let mut bad = Vec::new();
    let node = Session::open(&Program::new("fractal.splash", FRACTAL), SessionConfig::default());
    for (name, p, (cx, cy, w)) in cases() {
        // a 3x3 grid of tiles over the view, at the app's precision for that step and at f64
        let size = w / 3.0;
        let rects: Vec<TileRect> = (0..9).map(|k| TileRect { x: cx - 1.5 * size + (k % 3) as f64 * size, y: cy - 1.5 * size + (k / 3) as f64 * size, size }).collect();
        let nodes: Vec<Vec<u32>> = rects.iter().map(|rect| node_tile(&node, &p, *rect, res)).collect();
        for precision in [Precision::for_step(size / res as f64, cx, cy), Precision::F64] {
            let mut sum = (0, 0, 0);
            for j in 0..3 {
                for i in 0..3 {
                    let rect = rects[j * 3 + i];
                    let mut local = vec![0u32; res * res];
                    render_tile(&p, (0.0, 0.0), rect, res, precision, None, &mut local, &|| false);
                    let node = &nodes[j * 3 + i];
                    let (n, top, low) = compare(&local, node);
                    if n > 0 && n < 5 && precision == Precision::F64 {
                        for (k, (a, b)) in local.iter().zip(node).enumerate().filter(|(_, (a, b))| a != b) {
                            let (col, row) = (k % res, k / res);
                            let step = rect.size / res as f64;
                            println!("PARITY-TEXEL {name} {:?} tile ({i},{j}) texel {col},{row} x {:e} y {:e}: local {a:08x} node {b:08x}", p.kind, rect.x + (col as f64 + 0.5) * step, rect.y + (row as f64 + 0.5) * step);
                        }
                    }
                    sum = (sum.0 + n, sum.1 + top, sum.2.max(low));
                }
            }
            let line = format!("{name:7} {:?} {precision:?}: {} of {} texels differ, {} in the top byte, low-24 max {}", p.kind, sum.0, 9 * res * res, sum.1, sum.2);
            println!("PARITY {line}");
            // f32 tiles never go to a node (remote_can), only f64 ones must match
            if sum.0 > 0 && precision == Precision::F64 {
                bad.push(line);
            }
        }
    }
    assert!(bad.is_empty(), "local and node tiles differ:\n{}", bad.join("\n"));
}

