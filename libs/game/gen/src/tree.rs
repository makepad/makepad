//! Species presets and skeleton → low-poly mesh.
//!
//! Presets are the unit an AI composes with: `game.tree({})` must give a
//! decent tree with no knobs at all, and `species: "pine"` must give a
//! recognisable pine. Every preset takes a seed, so a forest of one species
//! is still a forest of individuals.

use crate::lsystem::{interpret, LSystem, Leaf, Rule, Segment, Skeleton, TurtleParams};
use crate::mesh::{GenMesh, GenVertex, MeshBuilder};
use crate::rng::GenRng;
use makepad_game_math as gm;
use makepad_math::*;

/// Level of detail. Controls radial sides on branches and whether small
/// branches survive at all — a forest's distant trees should not pay for
/// geometry nobody can resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lod {
    Low,
    Medium,
    High,
}

impl Lod {
    fn radial(self) -> usize {
        match self {
            Lod::Low => 3,
            Lod::Medium => 5,
            Lod::High => 7,
        }
    }
    /// Branches thinner than this fraction of the trunk are dropped.
    fn min_radius_ratio(self) -> f32 {
        match self {
            Lod::Low => 0.22,
            Lod::Medium => 0.10,
            Lod::High => 0.0,
        }
    }
    fn leaf_stride(self) -> usize {
        match self {
            Lod::Low => 3,
            Lod::Medium => 2,
            Lod::High => 1,
        }
    }
}

/// Knobs a caller (or the AI) can set. Defaults produce a good generic tree.
#[derive(Clone, Copy, Debug)]
pub struct TreeParams {
    pub seed: u64,
    pub height: f32,
    /// 0 = bare, 1 = dense canopy.
    pub bushiness: f32,
    /// Lean from vertical, in radians.
    pub lean: f32,
    pub lod: Lod,
    pub bark: [f32; 3],
    pub foliage: [f32; 3],
}

impl Default for TreeParams {
    fn default() -> Self {
        Self {
            seed: 0,
            height: 4.0,
            bushiness: 1.0,
            lean: 0.0,
            lod: Lod::Medium,
            bark: [0.36, 0.26, 0.18],
            foliage: [0.24, 0.52, 0.22],
        }
    }
}

struct Species {
    name: &'static str,
    system: LSystem,
    iterations: usize,
    turtle: TurtleParams,
    /// Multiplies the height knob to reach the preset's natural size.
    height_scale: f32,
    leaf_kind: LeafKind,
    bark: [f32; 3],
    foliage: [f32; 3],
}

#[derive(Clone, Copy, PartialEq)]
enum LeafKind {
    /// Crossed cards — cheap canopy blob, used by broadleaf trees.
    Cards,
    /// Flat needle fans, for conifers.
    Needles,
    /// Long arcing fronds, for palms and ferns.
    Frond,
    /// No foliage at all.
    None,
}

const OAK: LSystem = LSystem {
    axiom: "F",
    rules: &[
        Rule { symbol: 'F', produces: "FF[&+F!L][&-F!L][/&F!L]", weight: 1.0 },
        Rule { symbol: 'F', produces: "FF[&-F!L][/&+F!L]", weight: 1.0 },
        Rule { symbol: 'F', produces: "F[&+F!L][&\\-F!L]", weight: 0.7 },
    ],
};

const PINE: LSystem = LSystem {
    axiom: "F",
    rules: &[Rule {
        symbol: 'F',
        produces: "F[&&+FL][&&-FL][&&/FL][&&\\FL]F",
        weight: 1.0,
    }],
};

const BUSH: LSystem = LSystem {
    axiom: "F",
    rules: &[
        Rule { symbol: 'F', produces: "F[+FL][-FL][/FL][\\FL]", weight: 1.0 },
        Rule { symbol: 'F', produces: "F[+FL][\\-FL]", weight: 0.8 },
    ],
};

// A palm is a bare trunk plus a whorl of fronds at the top. `A` is a crown
// marker that expands once into the whorl; without it the trunk grew but no
// `L` was ever emitted, so the preset produced a bare stick.
const PALM: LSystem = LSystem {
    axiom: "FFFFA",
    rules: &[
        Rule {
            symbol: 'F',
            produces: "FF",
            weight: 1.0,
        },
        Rule {
            symbol: 'A',
            // Roll between fronds to spread them around the trunk; pitch down
            // so they arc outward rather than standing straight up.
            produces: "[&&L][/&&L][//&&L][///&&L][////&&L][/////&&L][//////&&L][///////&&L]",
            weight: 1.0,
        },
    ],
};

const FERN: LSystem = LSystem {
    axiom: "F",
    rules: &[Rule {
        symbol: 'F',
        produces: "F[&+FL][&-FL]F",
        weight: 1.0,
    }],
};

// A saguaro's arms go OUT and then turn back UP. Branching straight out
// (`[&+FF]`) gave a sprawling mat that read as a shrub, not a cactus, so each
// arm pitches away and then pitches back skyward.
const CACTUS: LSystem = LSystem {
    axiom: "FFF",
    rules: &[
        Rule { symbol: 'F', produces: "FF[&+F^^FF]", weight: 1.0 },
        Rule { symbol: 'F', produces: "FF[&-F^^FF]", weight: 1.0 },
        Rule { symbol: 'F', produces: "FF", weight: 1.4 },
    ],
};

const DEAD: LSystem = LSystem {
    axiom: "F",
    rules: &[
        Rule { symbol: 'F', produces: "F[&++F][&--F]F", weight: 1.0 },
        Rule { symbol: 'F', produces: "F[&+F][&\\--F]", weight: 1.0 },
    ],
};

const GRASS: LSystem = LSystem {
    axiom: "F",
    rules: &[Rule {
        symbol: 'F',
        produces: "F[+FL][-FL]L",
        weight: 1.0,
    }],
};

fn species(name: &str) -> &'static Species {
    // Keyed by the names a kid or an AI would actually say; unknown names
    // fall through to the generic broadleaf rather than erroring, because a
    // missing tree reads as a bug and a slightly-wrong tree does not.
    const TABLE: &[Species] = &[
        Species {
            name: "oak",
            system: OAK,
            iterations: 4,
            turtle: TurtleParams { angle: 0.44, taper: 0.74, jitter: 0.16, step_falloff: 0.78, leaf_size: 0.6, start_radius: 0.13, step: 1.0 },
            height_scale: 0.30,
            leaf_kind: LeafKind::Cards,
            bark: [0.36, 0.26, 0.18],
            foliage: [0.24, 0.52, 0.22],
        },
        Species {
            name: "pine",
            system: PINE,
            iterations: 3,
            turtle: TurtleParams { angle: 0.62, taper: 0.68, jitter: 0.10, step_falloff: 0.74, leaf_size: 0.5, start_radius: 0.11, step: 1.0 },
            height_scale: 0.26,
            leaf_kind: LeafKind::Needles,
            bark: [0.30, 0.22, 0.16],
            foliage: [0.16, 0.38, 0.20],
        },
        Species {
            name: "palm",
            system: PALM,
            iterations: 2,
            turtle: TurtleParams { angle: 0.55, taper: 0.94, jitter: 0.06, step_falloff: 1.0, leaf_size: 2.6, start_radius: 0.10, step: 1.0 },
            height_scale: 0.42,
            leaf_kind: LeafKind::Frond,
            bark: [0.44, 0.34, 0.22],
            foliage: [0.26, 0.50, 0.20],
        },
        Species {
            name: "bush",
            system: BUSH,
            iterations: 3,
            turtle: TurtleParams { angle: 0.55, taper: 0.70, jitter: 0.22, step_falloff: 0.72, leaf_size: 0.45, start_radius: 0.06, step: 1.0 },
            height_scale: 0.34,
            leaf_kind: LeafKind::Cards,
            bark: [0.30, 0.24, 0.16],
            foliage: [0.22, 0.46, 0.20],
        },
        Species {
            name: "fern",
            system: FERN,
            iterations: 3,
            turtle: TurtleParams { angle: 0.50, taper: 0.78, jitter: 0.14, step_falloff: 0.70, leaf_size: 0.7, start_radius: 0.035, step: 1.0 },
            height_scale: 0.34,
            leaf_kind: LeafKind::Frond,
            bark: [0.26, 0.34, 0.18],
            foliage: [0.20, 0.48, 0.22],
        },
        Species {
            name: "cactus",
            system: CACTUS,
            iterations: 3,
            turtle: TurtleParams { angle: 0.60, taper: 0.92, jitter: 0.06, step_falloff: 0.86, leaf_size: 0.0, start_radius: 0.20, step: 1.0 },
            height_scale: 0.30,
            leaf_kind: LeafKind::None,
            bark: [0.28, 0.44, 0.26],
            foliage: [0.28, 0.44, 0.26],
        },
        Species {
            name: "dead",
            system: DEAD,
            iterations: 4,
            turtle: TurtleParams { angle: 0.52, taper: 0.70, jitter: 0.26, step_falloff: 0.76, leaf_size: 0.0, start_radius: 0.11, step: 1.0 },
            height_scale: 0.30,
            leaf_kind: LeafKind::None,
            bark: [0.34, 0.30, 0.26],
            foliage: [0.34, 0.30, 0.26],
        },
        Species {
            name: "grass",
            system: GRASS,
            iterations: 2,
            turtle: TurtleParams { angle: 0.30, taper: 0.80, jitter: 0.30, step_falloff: 0.80, leaf_size: 0.30, start_radius: 0.015, step: 1.0 },
            height_scale: 0.40,
            leaf_kind: LeafKind::Frond,
            bark: [0.34, 0.46, 0.22],
            foliage: [0.36, 0.56, 0.24],
        },
    ];
    TABLE
        .iter()
        .find(|s| s.name == name)
        .unwrap_or(&TABLE[0])
}

/// Every species name the `species:` knob accepts.
pub const SPECIES: &[&str] = &[
    "oak", "pine", "palm", "bush", "fern", "cactus", "dead", "grass",
];

/// Build a tree mesh. Deterministic in `params.seed`.
pub fn tree(name: &str, params: TreeParams) -> GenMesh {
    let sp = species(name);
    let mut rng = GenRng::new(params.seed);

    let mut turtle = sp.turtle;
    // The preset's step is in "natural units"; scale so the finished plant
    // lands near the requested height.
    turtle.step = params.height * sp.height_scale;
    turtle.leaf_size *= params.height * sp.height_scale;
    turtle.start_radius *= params.height * sp.height_scale * 2.0;

    let expanded = sp.system.expand(sp.iterations, &mut rng);
    let mut sk = interpret(&expanded, turtle, &mut rng);

    // Normalise to the requested height.
    //
    // `height_scale` only sets the per-SEGMENT step; the finished height also
    // depends on how many segments the L-system happens to chain along the
    // trunk, which changes with the rules, the iteration count and the seed.
    // Estimating that per preset is exactly the kind of constant that drifts
    // silently — measuring the result and rescaling cannot. `game.tree({height:
    // 4})` has to give a 4-unit tree or the AI's mental model of the world is
    // wrong and every generated scene is out of proportion.
    normalise_height(&mut sk, params.height.max(0.01));

    if params.lean.abs() > 1.0e-4 {
        apply_lean(&mut sk, params.lean, &mut rng);
    }

    let bark = if params.bark == TreeParams::default().bark {
        sp.bark
    } else {
        params.bark
    };
    let foliage = if params.foliage == TreeParams::default().foliage {
        sp.foliage
    } else {
        params.foliage
    };

    let mut b = MeshBuilder::new();
    let trunk_radius = sk
        .segments
        .first()
        .map(|s| s.radius_a)
        .unwrap_or(0.1)
        .max(1.0e-5);

    for seg in &sk.segments {
        if seg.radius_a / trunk_radius < params.lod.min_radius_ratio() {
            continue;
        }
        emit_branch(&mut b, seg, &sk, trunk_radius, params.lod.radial(), bark);
    }

    if sp.leaf_kind != LeafKind::None && params.bushiness > 0.0 {
        let stride = params.lod.leaf_stride();
        for (i, leaf) in sk.leaves.iter().enumerate() {
            if i % stride != 0 {
                continue;
            }
            if params.bushiness < 1.0 && !rng.chance(params.bushiness) {
                continue;
            }
            emit_leaf(&mut b, leaf, &sk, sp.leaf_kind, foliage, &mut rng);
        }
    }

    // Canopy interiors and trunk bases want the contact darkening; the
    // height bias keeps the top bright so the silhouette still reads.
    b.bake_ambient(0.45, 0.7);
    b.finish()
}

/// Uniformly scale a skeleton so its tallest point sits at `target`.
///
/// Radii and leaf sizes scale with it, so the plant keeps its proportions —
/// only its size changes.
fn normalise_height(sk: &mut Skeleton, target: f32) {
    let top = sk
        .segments
        .iter()
        .fold(0.0f32, |m, s| m.max(s.a.y.max(s.b.y)))
        .max(
            sk.leaves
                .iter()
                .fold(0.0f32, |m, l| m.max(l.pos.y + l.size)),
        );
    if top <= 1.0e-4 {
        return;
    }
    let k = target / top;
    if (k - 1.0).abs() < 1.0e-4 {
        return;
    }
    let s = |v: Vec3f| vec3f(v.x * k, v.y * k, v.z * k);
    for seg in &mut sk.segments {
        seg.a = s(seg.a);
        seg.b = s(seg.b);
        seg.radius_a *= k;
        seg.radius_b *= k;
        seg.arc_a *= k;
        seg.arc_b *= k;
    }
    for l in &mut sk.leaves {
        l.pos = s(l.pos);
        l.size *= k;
        l.arc *= k;
    }
    sk.max_arc *= k;
}

fn apply_lean(sk: &mut Skeleton, lean: f32, rng: &mut GenRng) {
    // Lean about a random horizontal axis, scaled by height so the base
    // stays planted and the crown displaces — bending, not tipping over.
    let theta = rng.range(0.0, 6.283_185);
    let (s, c) = gm::sincos(theta);
    let axis = vec3f(c, 0.0, s);
    let max_y = sk
        .segments
        .iter()
        .fold(1.0e-6f32, |m, seg| m.max(seg.b.y.max(seg.a.y)));
    let bend = |p: Vec3f| -> Vec3f {
        let t = (p.y / max_y).clamp(0.0, 1.0);
        let a = lean * t * t;
        let (sa, ca) = gm::sincos(a);
        // Rotate about `axis` through the origin.
        let d = axis.x * p.x + axis.y * p.y + axis.z * p.z;
        let cr = vec3f(
            axis.y * p.z - axis.z * p.y,
            axis.z * p.x - axis.x * p.z,
            axis.x * p.y - axis.y * p.x,
        );
        vec3f(
            p.x * ca + cr.x * sa + axis.x * d * (1.0 - ca),
            p.y * ca + cr.y * sa + axis.y * d * (1.0 - ca),
            p.z * ca + cr.z * sa + axis.z * d * (1.0 - ca),
        )
    };
    for seg in &mut sk.segments {
        seg.a = bend(seg.a);
        seg.b = bend(seg.b);
    }
    for l in &mut sk.leaves {
        l.pos = bend(l.pos);
    }
}

/// Emit one tapered prism for a branch segment.
fn emit_branch(
    b: &mut MeshBuilder,
    seg: &Segment,
    sk: &Skeleton,
    trunk_radius: f32,
    radial: usize,
    color: [f32; 3],
) {
    let axis = vec3f(seg.b.x - seg.a.x, seg.b.y - seg.a.y, seg.b.z - seg.a.z);
    let len = (axis.x * axis.x + axis.y * axis.y + axis.z * axis.z).sqrt();
    if len < 1.0e-6 {
        return;
    }
    let h = vec3f(axis.x / len, axis.y / len, axis.z / len);
    // Any vector not parallel to h gives a usable reference for the ring.
    let refv = if h.y.abs() > 0.9 {
        vec3f(1.0, 0.0, 0.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    let u = {
        let c = vec3f(
            h.y * refv.z - h.z * refv.y,
            h.z * refv.x - h.x * refv.z,
            h.x * refv.y - h.y * refv.x,
        );
        let l = (c.x * c.x + c.y * c.y + c.z * c.z).sqrt();
        vec3f(c.x / l, c.y / l, c.z / l)
    };
    let v = vec3f(
        h.y * u.z - h.z * u.y,
        h.z * u.x - h.x * u.z,
        h.x * u.y - h.y * u.x,
    );

    let g_a = sk.growth_at(seg.arc_a);
    let g_b = sk.growth_at(seg.arc_b);
    let f_a = sk.flex_at(seg.arc_a, seg.radius_a / trunk_radius);
    let f_b = sk.flex_at(seg.arc_b, seg.radius_b / trunk_radius);

    let base = b.verts.len() as u32;
    for i in 0..radial {
        let ang = (i as f32 / radial as f32) * 6.283_185_3;
        let (s, c) = gm::sincos(ang);
        let dir = vec3f(u.x * c + v.x * s, u.y * c + v.y * s, u.z * c + v.z * s);
        let uv_u = i as f32 / radial as f32;
        b.vertex(GenVertex {
            pos: vec3f(
                seg.a.x + dir.x * seg.radius_a,
                seg.a.y + dir.y * seg.radius_a,
                seg.a.z + dir.z * seg.radius_a,
            ),
            normal: dir,
            uv: [uv_u, seg.arc_a * 0.35],
            color,
            growth: g_a,
            flex: f_a,
        });
        b.vertex(GenVertex {
            pos: vec3f(
                seg.b.x + dir.x * seg.radius_b,
                seg.b.y + dir.y * seg.radius_b,
                seg.b.z + dir.z * seg.radius_b,
            ),
            normal: dir,
            uv: [uv_u, seg.arc_b * 0.35],
            color,
            growth: g_b,
            flex: f_b,
        });
    }
    for i in 0..radial {
        let j = (i + 1) % radial;
        let (a0, a1) = (base + i as u32 * 2, base + i as u32 * 2 + 1);
        let (b0, b1) = (base + j as u32 * 2, base + j as u32 * 2 + 1);
        b.quad(a0, a1, b1, b0);
    }
}

fn emit_leaf(
    b: &mut MeshBuilder,
    leaf: &Leaf,
    sk: &Skeleton,
    kind: LeafKind,
    color: [f32; 3],
    rng: &mut GenRng,
) {
    let growth = sk.growth_at(leaf.arc);
    // Foliage flexes more than the twig it hangs on — that differential is
    // what makes wind read as leaves rustling rather than branches waving.
    let flex = (sk.flex_at(leaf.arc, 0.05) * 1.15).min(1.0);
    let size = leaf.size * rng.range(0.75, 1.25);

    let h = leaf.heading;
    let l = leaf.left;
    let up = vec3f(
        h.y * l.z - h.z * l.y,
        h.z * l.x - h.x * l.z,
        h.x * l.y - h.y * l.x,
    );

    let mut card = |axis_a: Vec3f, axis_b: Vec3f, len: f32, wid: f32| {
        let n = vec3f(
            axis_a.y * axis_b.z - axis_a.z * axis_b.y,
            axis_a.z * axis_b.x - axis_a.x * axis_b.z,
            axis_a.x * axis_b.y - axis_a.y * axis_b.x,
        );
        let p = leaf.pos;
        let corners = [
            (-wid, 0.0, [0.0, 0.0]),
            (wid, 0.0, [1.0, 0.0]),
            (wid, len, [1.0, 1.0]),
            (-wid, len, [0.0, 1.0]),
        ];
        let base = b.verts.len() as u32;
        for (w, ln, uv) in corners {
            b.vertex(GenVertex {
                pos: vec3f(
                    p.x + axis_a.x * w + axis_b.x * ln,
                    p.y + axis_a.y * w + axis_b.y * ln,
                    p.z + axis_a.z * w + axis_b.z * ln,
                ),
                normal: n,
                uv,
                color,
                growth,
                // The outer edge of a leaf moves more than its stem.
                flex: (flex * (0.7 + 0.3 * (ln / len.max(1.0e-6)))).min(1.0),
            });
        }
        b.quad(base, base + 1, base + 2, base + 3);
        // Leaves are two-sided; without the back face a card vanishes from
        // half the angles under backface culling.
        b.quad(base + 3, base + 2, base + 1, base);
    };

    match kind {
        LeafKind::Cards => {
            card(l, h, size, size * 0.6);
            card(up, h, size, size * 0.6);
        }
        LeafKind::Needles => {
            card(l, h, size, size * 0.35);
        }
        LeafKind::Frond => {
            // A frond is a long narrow card drooping away from the heading.
            let droop = vec3f(h.x * 0.6 - 0.0, h.y * 0.6 - 0.55, h.z * 0.6);
            let dl = (droop.x * droop.x + droop.y * droop.y + droop.z * droop.z).sqrt();
            let d = if dl > 1.0e-6 {
                vec3f(droop.x / dl, droop.y / dl, droop.z / dl)
            } else {
                h
            };
            card(l, d, size * 1.6, size * 0.22);
        }
        LeafKind::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_species_builds_a_nonempty_mesh() {
        for name in SPECIES {
            let m = tree(name, TreeParams::default());
            assert!(m.triangle_count() > 0, "{name} produced no triangles");
            assert!(m.vertex_count() > 0, "{name} produced no vertices");
            for f in &m.vertices {
                assert!(f.is_finite(), "{name} emitted a non-finite vertex float");
            }
            for i in &m.indices {
                assert!(
                    (*i as usize) < m.vertex_count(),
                    "{name} index {i} out of range"
                );
            }
        }
    }

    #[test]
    fn unknown_species_falls_back_rather_than_failing() {
        let m = tree("banana-hovercraft", TreeParams::default());
        assert!(m.triangle_count() > 0);
    }

    #[test]
    fn same_seed_is_byte_identical_and_different_seeds_differ() {
        let a = tree("oak", TreeParams { seed: 7, ..Default::default() });
        let b = tree("oak", TreeParams { seed: 7, ..Default::default() });
        assert_eq!(a.vertices, b.vertices);
        assert_eq!(a.indices, b.indices);
        let c = tree("oak", TreeParams { seed: 8, ..Default::default() });
        assert_ne!(a.vertices, c.vertices, "seed had no effect");
    }

    #[test]
    fn lod_reduces_triangles_monotonically() {
        let mk = |lod| {
            tree(
                "oak",
                TreeParams {
                    seed: 3,
                    lod,
                    ..Default::default()
                },
            )
            .triangle_count()
        };
        let (lo, me, hi) = (mk(Lod::Low), mk(Lod::Medium), mk(Lod::High));
        assert!(lo < me, "Low {lo} should be under Medium {me}");
        assert!(me < hi, "Medium {me} should be under High {hi}");
    }

    #[test]
    fn height_knob_is_absolute_not_merely_relative() {
        // The original version of this test only compared two sizes, which a
        // 4x-overshooting scale factor passed happily — every species came out
        // several times the requested height. An AI asking for a 4-unit tree
        // must get a 4-unit tree.
        for name in SPECIES {
            for want in [1.0f32, 4.0, 9.0] {
                let m = tree(
                    name,
                    TreeParams {
                        seed: 2,
                        height: want,
                        ..Default::default()
                    },
                );
                let got = m.max.y;
                assert!(
                    (got - want).abs() < want * 0.2,
                    "{name} asked for {want}, got {got}"
                );
            }
        }
    }

    #[test]
    fn a_tree_is_not_absurdly_wider_than_it_is_tall() {
        // A "tree" that comes out as a low sprawling mat is a broken preset,
        // not a tree. Bushes and cacti are allowed to be squat; the canopy
        // species are not.
        for name in ["oak", "pine", "palm"] {
            let m = tree(
                name,
                TreeParams {
                    seed: 2,
                    height: 5.0,
                    ..Default::default()
                },
            );
            let s = m.size();
            let spread = s.x.max(s.z);
            assert!(
                spread < s.y * 1.6,
                "{name} is {spread:.1} wide vs {:.1} tall",
                s.y
            );
        }
    }

    #[test]
    fn trunk_base_is_anchored_and_tips_are_flexible() {
        use crate::mesh::{unpack_growth_flex, MESH_VERTEX_FLOATS};
        let m = tree("oak", TreeParams { seed: 4, ..Default::default() });
        let mut base_flex: f32 = 1.0;
        let mut top_flex: f32 = 0.0;
        let top_y = m.max.y;
        for v in m.vertices.chunks_exact(MESH_VERTEX_FLOATS) {
            let a = ((v[5].to_bits() >> 24) & 0xff) as f32 / 255.0;
            let (_g, f) = unpack_growth_flex(a);
            if v[1] < m.min.y + 0.05 {
                base_flex = base_flex.min(f);
            }
            if v[1] > top_y - 0.3 {
                top_flex = top_flex.max(f);
            }
        }
        assert!(base_flex < 0.15, "trunk base flex {base_flex} — would detach");
        assert!(top_flex > 0.5, "canopy flex {top_flex} — would not sway");
    }

    #[test]
    fn bushiness_zero_drops_the_canopy() {
        let bare = tree("oak", TreeParams { seed: 5, bushiness: 0.0, ..Default::default() });
        let full = tree("oak", TreeParams { seed: 5, bushiness: 1.0, ..Default::default() });
        assert!(bare.triangle_count() < full.triangle_count());
    }

    #[test]
    fn a_tree_stands_on_the_origin_plane() {
        // Generated plants are placed by entity transform, so the mesh must
        // start at y=0 rather than being centred.
        let m = tree("pine", TreeParams { seed: 6, ..Default::default() });
        assert!(m.min.y.abs() < 0.2, "base at y={}", m.min.y);
        assert!(m.max.y > 1.0);
    }
}
