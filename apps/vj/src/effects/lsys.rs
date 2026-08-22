//! L-system compiler + turtle bytecode interpreter.
//!
//! The splash document carries the PROGRAM as text (`axiom` + `rules`), the
//! way an AI will author it. At load time it is compiled once:
//!
//!   text rules ──compile──> bytecode (one op per symbol, `Vec<Op>`)
//!   bytecode  ──expand───> final op stream (iterative rewrite, budgeted)
//!   op stream ──turtle───> tube mesh with per-vertex growth attributes
//!
//! Nothing here runs per frame: the emitted mesh encodes everything the
//! turtle knows onto the vertex stream and the VERTEX SHADER animates from
//! it (wind, growth, color). Regeneration only happens when the document
//! changes.
//!
//! # Vertex attribute encoding (the tree's whole story, per vertex)
//!
//! | channel   | meaning                                                  |
//! |-----------|----------------------------------------------------------|
//! | `a_id`    | bracket depth (trunk 0, twigs N) — flutter + color        |
//! | `a_aux`   | arc length from the root, normalized 0..1 — growth front  |
//! |           | + wind lever (CONTINUOUS across joints, so the shader's   |
//! |           | bend can never tear a branch off its parent)              |
//! | `a_r0`    | hue channel 0..1 (drifts per branch, shifts on `'`)       |
//! | `a_r1`    | tube radius at this vertex (the shader re-expands the     |
//! |           | tube from its centerline for growth)                      |
//! | `uv.x`    | angle around the tube 0..1                                |
//! | `uv.y`    | branch index hash 0..1 (per-branch flutter phase)         |
//! | normal    | radial direction (unit): `center = pos - normal * a_r1`   |
//!
//! # Connectivity
//!
//! Consecutive segments share their joint ring position exactly (the turtle
//! walks; a segment starts where the previous ended), a branch starts at
//! its parent's joint, and every animated displacement in the shader is a
//! function of CONTINUOUS per-vertex data (rest position, arc length) — so
//! the tree bends as one connected body. Per-branch flutter uses the branch
//! hash at deliberately small amplitude.
//!
//! # Alphabet
//!
//! | symbol | op                                            |
//! |--------|-----------------------------------------------|
//! | `F` `G`| move forward, drawing a tube segment          |
//! | `f`    | move forward without drawing                  |
//! | `+` `-`| yaw   left / right by `angle`                 |
//! | `& ` `^`| pitch down / up   by `angle`                 |
//! | `/` `\`| roll  right / left by `angle`                 |
//! | `[` `]`| push / pop turtle state (branch)              |
//! | `!`    | multiply radius by `radius_decay`             |
//! | `'`    | advance the color/hue channel (`a_r0`)        |
//! | other  | structure symbols (`X`,`Y`,…): no-op          |
//!
//! Rules are `"SYMBOL=REPLACEMENT"` strings, e.g. `"X=F[+X][-X]FX"`.

use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

/// One compiled op. `Sym(n)` is a structure symbol (no-op at draw time, but
/// subject to rewriting during expansion).
#[derive(Clone, Copy, PartialEq)]
enum Op {
    Draw,
    Move,
    YawP,
    YawN,
    PitchP,
    PitchN,
    RollP,
    RollN,
    Push,
    Pop,
    Shrink,
    Hue,
    Sym(u8),
}

fn compile_symbol(c: char) -> Op {
    match c {
        'F' | 'G' => Op::Draw,
        'f' => Op::Move,
        '+' => Op::YawP,
        '-' => Op::YawN,
        '&' => Op::PitchP,
        '^' => Op::PitchN,
        '/' => Op::RollP,
        '\\' => Op::RollN,
        '[' => Op::Push,
        ']' => Op::Pop,
        '!' => Op::Shrink,
        '\'' => Op::Hue,
        c => Op::Sym(c as u8),
    }
}

pub struct LsysProgram {
    /// Final expanded op stream.
    ops: Vec<Op>,
    /// True when the expansion hit the budget before finishing all
    /// iterations — reported honestly in the widget status.
    pub truncated: bool,
    pub iterations_done: usize,
}

/// Compile + expand the textual L-system. `budget` caps the op-stream length
/// (and thereby the segment count) so a hostile/misjudged document cannot
/// explode memory: expansion stops at the last completed iteration that fits.
pub fn compile(axiom: &str, rules: &[(char, String)], iterations: usize, budget: usize) -> LsysProgram {
    // Rule table: index by symbol byte.
    let mut table: Vec<Option<Vec<Op>>> = vec![None; 256];
    for (sym, replacement) in rules {
        let compiled: Vec<Op> = replacement.chars().map(compile_symbol).collect();
        table[*sym as usize % 256] = Some(compiled);
    }
    // A symbol's rewrite key: structure symbols use their own byte; drawing
    // symbols F/f can also be rewritten (classic "F=FF").
    fn key(op: Op) -> Option<u8> {
        match op {
            Op::Sym(b) => Some(b),
            Op::Draw => Some(b'F'),
            Op::Move => Some(b'f'),
            _ => None,
        }
    }

    let mut cur: Vec<Op> = axiom.chars().map(compile_symbol).collect();
    let mut next: Vec<Op> = Vec::new();
    let mut truncated = false;
    let mut done = 0;
    for _ in 0..iterations {
        next.clear();
        let mut grew = false;
        for &op in &cur {
            match key(op).and_then(|k| table[k as usize].as_ref()) {
                Some(replacement) => {
                    next.extend_from_slice(replacement);
                    grew = true;
                }
                None => next.push(op),
            }
        }
        if next.len() > budget {
            truncated = true;
            break;
        }
        std::mem::swap(&mut cur, &mut next);
        done += 1;
        if !grew {
            break;
        }
    }
    LsysProgram { ops: cur, truncated, iterations_done: done }
}

/// Turtle parameters, straight from the splash document.
pub struct TurtleParams {
    pub angle: f32,
    pub angle_jitter: f32,
    pub step: f32,
    pub radius: f32,
    pub radius_decay: f32,
    /// Cross-section sides of the emitted tube (3..=8).
    pub sides: usize,
    pub seed: u64,
}

#[derive(Clone, Copy)]
struct TurtleFrame {
    pos: Vec3f,
    // Orthonormal frame: heading, left, up.
    h: Vec3f,
    l: Vec3f,
    u: Vec3f,
    radius: f32,
    depth: f32,
    hue: f32,
    /// Path length from the root, world units.
    arc: f32,
    /// Branch index hash 0..1 (stable per branch).
    branch: f32,
}

/// Rotate the orthonormal pair (a, b) in their own plane by `angle`.
fn rotate(a: Vec3f, b: Vec3f, angle: f32) -> (Vec3f, Vec3f) {
    let (s, c) = angle.sin_cos();
    (a * c + b * s, b * c - a * s)
}

fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Run the compiled program through the turtle, emitting a tube mesh.
/// Returns (segments emitted, max arc length) — the caller normalizes
/// `a_aux` by max arc via [`normalize_arc`].
pub fn run_turtle(program: &LsysProgram, params: &TurtleParams, mesh: &mut FxMesh) -> (usize, f32) {
    let mut rng = FxRng::new(params.seed);
    let sides = params.sides.clamp(3, 8);
    let base_angle = params.angle.to_radians();
    let start_vert = mesh.vertex_count();

    let mut t = TurtleFrame {
        pos: vec3f(0.0, 0.0, 0.0),
        h: vec3f(0.0, 1.0, 0.0),
        l: vec3f(1.0, 0.0, 0.0),
        u: vec3f(0.0, 0.0, 1.0),
        radius: params.radius,
        depth: 0.0,
        hue: 0.0,
        arc: 0.0,
        branch: 0.0,
    };
    let mut stack: Vec<TurtleFrame> = Vec::with_capacity(32);
    let mut branch_counter: u32 = 0;
    let mut draws = 0usize;
    let mut max_arc = 0.0f32;

    let jitter = params.angle_jitter.to_radians();
    let angle_of = |rng: &mut FxRng| {
        if jitter > 0.0 {
            base_angle + rng.range(-jitter, jitter)
        } else {
            base_angle
        }
    };

    for &op in &program.ops {
        match op {
            Op::Draw => {
                let start = t.pos;
                let end = t.pos + t.h * params.step;
                let arc_end = t.arc + params.step;
                emit_tube_segment(mesh, &t, start, end, arc_end, sides);
                draws += 1;
                t.pos = end;
                t.arc = arc_end;
                max_arc = max_arc.max(t.arc);
            }
            Op::Move => {
                t.pos = t.pos + t.h * params.step;
                t.arc += params.step;
                max_arc = max_arc.max(t.arc);
            }
            Op::YawP => (t.h, t.l) = rotate(t.h, t.l, angle_of(&mut rng)),
            Op::YawN => (t.h, t.l) = rotate(t.h, t.l, -angle_of(&mut rng)),
            Op::PitchP => (t.h, t.u) = rotate(t.h, t.u, angle_of(&mut rng)),
            Op::PitchN => (t.h, t.u) = rotate(t.h, t.u, -angle_of(&mut rng)),
            Op::RollP => (t.l, t.u) = rotate(t.l, t.u, angle_of(&mut rng)),
            Op::RollN => (t.l, t.u) = rotate(t.l, t.u, -angle_of(&mut rng)),
            Op::Push => {
                stack.push(t);
                branch_counter += 1;
                t.depth += 1.0;
                t.branch = hash01(branch_counter);
                // Each branch drifts its hue so siblings separate visually.
                t.hue = (t.hue + 0.11 + rng.next_f32() * 0.05).fract();
            }
            Op::Pop => {
                if let Some(prev) = stack.pop() {
                    t = prev;
                }
            }
            Op::Shrink => {
                t.radius *= params.radius_decay;
            }
            Op::Hue => {
                t.hue = (t.hue + 0.21).fract();
            }
            Op::Sym(_) => {}
        }
    }

    // Normalize arc (a_aux) in place for the vertices this run emitted.
    let floats = super::mesh::VERT_FLOATS;
    let inv = 1.0 / max_arc.max(1e-5);
    for v in mesh.verts[start_vert * floats..].chunks_mut(floats) {
        v[7] *= inv;
    }
    (draws, max_arc)
}

/// One straight tube segment: `sides` vertices at each end ring, quads
/// between. Rings carry the SAME radius at both ends (radius only changes
/// via `!`), so consecutive segments meet flush at their shared joint.
fn emit_tube_segment(
    mesh: &mut FxMesh,
    t: &TurtleFrame,
    start: Vec3f,
    end: Vec3f,
    arc_end: f32,
    sides: usize,
) {
    let mut ring_start = [0u32; 8];
    let mut ring_end = [0u32; 8];
    for s in 0..sides {
        let around = s as f32 / sides as f32;
        let a = around * std::f32::consts::TAU;
        let radial = t.l * a.cos() + t.u * a.sin();
        // a_aux carries the RAW arc here; run_turtle normalizes it after
        // the walk (max arc is only known then).
        ring_start[s] = mesh.push_vert(
            start + radial * t.radius,
            t.depth,
            radial,
            t.arc,
            vec2f(around, t.branch),
            t.hue,
            t.radius,
        );
        ring_end[s] = mesh.push_vert(
            end + radial * t.radius,
            t.depth,
            radial,
            arc_end,
            vec2f(around, t.branch),
            t.hue,
            t.radius,
        );
    }
    for s in 0..sides {
        let n = (s + 1) % sides;
        mesh.push_quad(ring_start[s], ring_start[n], ring_end[n], ring_end[s]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::mesh::VERT_FLOATS;

    fn params() -> TurtleParams {
        TurtleParams {
            angle: 25.0,
            angle_jitter: 0.0,
            step: 0.2,
            radius: 0.05,
            radius_decay: 0.85,
            sides: 5,
            seed: 7,
        }
    }

    #[test]
    fn expansion_respects_the_budget() {
        let p = compile("X", &[('X', "F[+X][-X]FX".into()), ('F', "FF".into())], 12, 20_000);
        assert!(p.truncated, "12 iterations of this rule must overflow 20k ops");
        assert!(p.ops.len() <= 20_000);
        assert!(p.iterations_done >= 3);
    }

    #[test]
    fn the_turtle_emits_connected_tube_geometry() {
        let p = compile("X", &[('X', "F[+X][-X]FX".into())], 4, 100_000);
        let mut mesh = FxMesh::default();
        let (segs, max_arc) = run_turtle(&p, &params(), &mut mesh);
        assert!(segs > 10, "expected a real plant, got {segs} segments");
        assert!(max_arc > 0.5);
        assert_eq!(mesh.vertex_count(), segs * 2 * 5, "two 5-side rings per segment");
        assert_eq!(mesh.triangle_count(), segs * 5 * 2, "two tris per tube quad");
    }

    #[test]
    fn consecutive_segments_share_their_joint_ring_positions() {
        // A straight run of two segments: segment 0's end ring must coincide
        // with segment 1's start ring, vertex for vertex. Emission order is
        // interleaved per side: (start, end) pairs.
        let p = compile("FF", &[], 1, 1000);
        let mut mesh = FxMesh::default();
        run_turtle(&p, &params(), &mut mesh);
        let sides = 5;
        for s in 0..sides {
            let end0 = (s * 2 + 1) * VERT_FLOATS; // segment 0, end of side s
            let start1 = (2 * sides + s * 2) * VERT_FLOATS; // segment 1, start
            for k in 0..3 {
                assert!(
                    (mesh.verts[end0 + k] - mesh.verts[start1 + k]).abs() < 1e-6,
                    "joint ring tears at side {s} axis {k}"
                );
            }
        }
    }

    #[test]
    fn arc_length_is_normalized_and_reaches_the_tip() {
        let p = compile("FFFF", &[], 1, 1000);
        let mut mesh = FxMesh::default();
        run_turtle(&p, &params(), &mut mesh);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for pair in mesh.verts.chunks(VERT_FLOATS * 2) {
            let start_arc = pair[7];
            let end_arc = pair[VERT_FLOATS + 7];
            assert!((0.0..=1.0).contains(&start_arc));
            assert!(end_arc > start_arc, "a segment's far ring must sit further from the root");
            lo = lo.min(start_arc);
            hi = hi.max(end_arc);
        }
        assert_eq!(lo, 0.0, "the root ring must sit at arc 0");
        assert!((hi - 1.0).abs() < 1e-4, "the tip must sit at arc 1.0, got {hi}");
    }

    #[test]
    fn branch_depth_and_branch_hash_ride_the_vertex_stream() {
        let p = compile("F[+F[+F]]", &[], 1, 1000);
        let mut mesh = FxMesh::default();
        run_turtle(&p, &params(), &mut mesh);
        let depths: Vec<f32> = mesh.verts.chunks(VERT_FLOATS).map(|v| v[3]).collect();
        assert!(depths.iter().any(|&d| d == 0.0));
        assert!(depths.iter().any(|&d| d == 1.0));
        assert!(depths.iter().any(|&d| d == 2.0));
        // Two distinct branches must carry two distinct branch hashes (uv.y).
        let hashes: std::collections::BTreeSet<u32> = mesh
            .verts
            .chunks(VERT_FLOATS)
            .map(|v| (v[9] * 65535.0) as u32)
            .collect();
        assert!(hashes.len() >= 3, "expected 3 distinct branch hashes, got {}", hashes.len());
    }
}
