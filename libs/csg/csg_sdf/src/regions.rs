//! The fill's classes in field form (S1+S2): the planner's class chain (fab toolpath stages.rs:
//! faces, skin, shells, bridges, classes) restated as rounds over region fields.
//!
//! Every region is a set of loops on one plane. A round makes one region on every plane, from a
//! composition of terms (a region's field at a level, on this plane or a neighbour) or from a
//! per-component filter of an earlier region. Every plane runs the same rounds in the same order,
//! so a term on another plane reads what an earlier round made there.
//!
//! A composition at level 0 is exact as a set (all = max, any = min, a negated term = -phi). A term
//! at a non-zero level reads the region's own field in the planner's offset metric (its Miter(3)
//! joins), so the level is that region's offset. The only rewrite of the planner's operations is
//! that an erosion distributes over intersection and complement; every other round is the
//! planner's own step, unions and conditions included.
use crate::plan::{Region, ShellLevels, ShellSpec};

/// The regions of one plane, in the order the rounds make them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reg {
    /// What the outline above leaves of this plane's outline, then without specks.
    TopFaceRaw,
    TopFace,
    /// What the outline below leaves, then without specks.
    BottomFaceRaw,
    BottomFace,
    /// The faces clipped to the fill, then without specks.
    TopRaw,
    Top,
    BottomRaw,
    Bottom,
    /// The face pieces clear of the fill (grown into the shells) and the rest, and both together
    /// as the shells of the planes below (above) take them.
    TopFar,
    TopNear,
    TopProjected,
    BottomFar,
    BottomNear,
    BottomProjected,
    /// The bottom grown into its anchor inside the fill (the bottom on the first plane).
    Grown,
    /// The projected faces of the shell window.
    Acc,
    /// The shells' opening: its erosion, then that grown back.
    Eroded,
    Opened,
    /// The shells after the bridge below reaches up and the own grown bottom takes its share.
    Solid2,
    /// The fill left over, its pieces over the minimum area, and the small ones.
    Sparse0,
    Sparse1,
    Small,
    /// The shells with the small sparse pieces.
    Solid3,
    /// Narrow solid pieces over the sparse below, and the solid without them.
    Vertical0,
    Solid4,
    /// Internal bridges: the opening of the solid over the sparse below, its pieces over the
    /// bridge area, and the solid within the grow of those.
    BridgeEroded,
    BridgeOpened,
    BridgeKept,
    Bridge,
    Solid5,
    /// The anchor band under the bridge above, and its pieces over the band area.
    Band0,
    Band,
    Sparse2,
    /// Narrow solid pieces beside the band under the bridge above, and the solid without them.
    Adjacent,
    Solid6,
    Vertical,
    /// The top without the bottom (the bottom wins).
    TopFinal,
}

/// Where a term's field comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    /// The plane's outline (IslandPlan.outline).
    Outline,
    /// The fill inside the walls (WallPlan.fill of every island).
    Fill,
    Reg(Reg),
}

/// A region's field at a level (mm; positive grows), on the plane `dz` away, or its complement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Term {
    pub src: Src,
    pub dz: i8,
    pub level: f32,
    pub neg: bool,
}

/// A composition: All is the intersection (max), Any the union (min).
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Term(Term),
    All(Vec<Expr>),
    Any(Vec<Expr>),
}

/// A per-component test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Test {
    /// Its area is over this (mm²).
    AreaOver(f32),
    /// Nothing of it lies this far (mm) inside its edge: its erosion is empty.
    Thin(f32),
    /// It meets that region (on the plane `dz` away).
    Meets(Src, i8),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Empty,
    /// The same loops as a region of this plane.
    Same(Src),
    Compose(Expr),
    /// The components of `of` for which every test holds (`hold`) or not every test holds (`!hold`).
    Keep { of: Reg, tests: Vec<Test>, hold: bool },
    /// The loops of these regions, one after the other (no union).
    Concat(Vec<Reg>),
    /// `then` when the region `dz` away has a loop, else `otherwise`.
    IfAny { of: Reg, dz: i8, then: Box<Op>, otherwise: Box<Op> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    pub out: Reg,
    pub op: Op,
}

/// The planner's classes and the regions that hold them.
pub const CLASSES: [(Region, Reg); 7] = [
    (Region::Top, Reg::TopFinal),
    (Region::Bottom, Reg::Grown),
    (Region::Solid, Reg::Solid6),
    (Region::Sparse, Reg::Sparse2),
    (Region::InternalBridge, Reg::Bridge),
    (Region::VerticalShell, Reg::Vertical),
    (Region::ShellBand, Reg::Band),
];

/// The per-component narrow flag of the solid class (IslandRegions::narrow), when the spec asks.
pub fn narrow(spec: &ShellSpec, l: &ShellLevels) -> Option<(Reg, Test)> {
    spec.narrow_solid.then_some((Reg::Solid6, Test::Thin(l.narrow)))
}

fn term(src: Src, dz: i8, level: f32) -> Expr {
    Expr::Term(Term { src, dz, level, neg: false })
}

fn not(src: Src, dz: i8, level: f32) -> Expr {
    Expr::Term(Term { src, dz, level, neg: true })
}

fn reg(r: Reg) -> Src {
    Src::Reg(r)
}

fn all(of: Vec<Expr>) -> Op {
    Op::Compose(Expr::All(of))
}

fn keep(of: Reg, tests: Vec<Test>, hold: bool) -> Op {
    Op::Keep { of, tests, hold }
}

fn if_any(of: Reg, dz: i8, then: Op, otherwise: Op) -> Op {
    Op::IfAny { of, dz, then: Box::new(then), otherwise: Box::new(otherwise) }
}

/// The rounds of plane `k` of `n`, with that plane's levels. Every plane gets the same regions in
/// the same order; only the operations differ at the ends of the part.
pub fn rounds(spec: &ShellSpec, l: &ShellLevels, k: usize, n: usize) -> Vec<Round> {
    use Reg::*;
    let (t, b) = (spec.top_layers as usize, spec.bottom_layers as usize);
    let (o, f) = (Src::Outline, Src::Fill);
    let floating = spec.floating_shells;
    let mut out = Vec::new();
    let mut put = |r: Reg, op: Op| out.push(Round { out: r, op });

    // Faces: what the outlines above and below leave (the outline itself at the part's ends).
    put(TopFaceRaw, if k + 1 < n { all(vec![term(o, 0, 0.0), not(o, 1, 0.0)]) } else { Op::Same(o) });
    put(TopFace, keep(TopFaceRaw, vec![Test::AreaOver(l.speck)], true));
    put(BottomFaceRaw, if k > 0 { all(vec![term(o, 0, 0.0), not(o, -1, 0.0)]) } else { Op::Same(o) });
    put(BottomFace, keep(BottomFaceRaw, vec![Test::AreaOver(l.speck)], true));

    // Skin: the faces clipped to the fill.
    put(TopRaw, all(vec![term(f, 0, 0.0), term(reg(TopFace), 0, 0.0)]));
    put(Top, keep(TopRaw, vec![Test::AreaOver(l.speck)], true));
    put(BottomRaw, all(vec![term(f, 0, 0.0), term(reg(BottomFace), 0, 0.0)]));
    put(Bottom, keep(BottomRaw, vec![Test::AreaOver(l.speck)], true));

    // The faces as the shells take them: a piece clear of the fill grows by face_grow.
    for (far, near, projected, face, layers) in [(TopFar, TopNear, TopProjected, TopFace, t), (BottomFar, BottomNear, BottomProjected, BottomFace, b)] {
        if layers > 1 {
            let meets = vec![Test::Meets(f, 0)];
            if spec.vertical_shells {
                put(far, keep(face, meets.clone(), false));
                put(near, keep(face, meets, true));
            } else {
                put(far, Op::Empty);
                put(near, Op::Same(reg(face)));
            }
            put(projected, Op::Compose(Expr::Any(vec![term(reg(far), 0, l.face_grow), term(reg(near), 0, 0.0)])));
        } else {
            put(far, Op::Empty);
            put(near, Op::Empty);
            put(projected, Op::Empty);
        }
    }

    // External bridges grow into their anchor inside the fill.
    put(Grown, if k >= 1 { all(vec![term(f, 0, 0.0), term(reg(Bottom), 0, l.bottom_grow)]) } else { Op::Same(reg(Bottom)) });

    // Shells: the projected faces of the window, opened.
    let window: Vec<Expr> = (1..t)
        .filter(|m| k + m < n)
        .map(|m| term(reg(TopProjected), m as i8, 0.0))
        .chain((1..b).filter(|&m| k >= m).map(|m| term(reg(BottomProjected), -(m as i8), 0.0)))
        .collect();
    let so = l.shell_open;
    if window.is_empty() {
        put(Acc, Op::Empty);
        put(Eroded, Op::Empty);
        put(Opened, Op::Empty);
    } else {
        put(Acc, Op::Compose(Expr::Any(window)));
        put(Eroded, all(vec![term(f, 0, -so), term(reg(Acc), 0, -so), not(reg(Top), 0, so), not(reg(Bottom), 0, so)]));
        put(Opened, Op::Compose(term(reg(Eroded), 0, so)));
    }

    // The bridge below reaches up (once its bottom is there), and the own grown bottom takes its
    // share (once there is one).
    let take_grown = |solid: Expr| -> Op {
        let kept = match &solid {
            Expr::Term(Term { src, dz: 0, level, neg: false }) if *level == 0.0 => Op::Same(*src),
            _ => Op::Compose(solid.clone()),
        };
        if k >= 1 { if_any(Bottom, 0, all(vec![solid, not(reg(Grown), 0, 0.0)]), kept) } else { kept }
    };
    let opened = term(reg(Opened), 0, 0.0);
    put(Solid2, if k >= 2 && b > 1 {
        let up = Expr::All(vec![Expr::Any(vec![opened.clone(), Expr::All(vec![term(f, 0, 0.0), term(reg(Grown), -1, 0.0)])]), not(reg(Top), 0, 0.0), not(reg(Bottom), 0, 0.0)]);
        if_any(Bottom, -1, take_grown(up), take_grown(opened))
    } else {
        take_grown(opened)
    });

    // Sparse: the fill left over; small pieces become solid.
    put(Sparse0, all(vec![term(f, 0, 0.0), not(reg(Top), 0, 0.0), not(reg(Grown), 0, 0.0), not(reg(Solid2), 0, 0.0)]));
    put(Sparse1, keep(Sparse0, vec![Test::AreaOver(l.min_sparse_area)], true));
    put(Small, keep(Sparse0, vec![Test::AreaOver(l.min_sparse_area)], false));
    put(Solid3, if_any(Small, 0, Op::Compose(Expr::Any(vec![term(reg(Solid2), 0, 0.0), term(reg(Small), 0, 0.0)])), Op::Same(reg(Solid2))));

    // Narrow vertical shells over the sparse below stay whole.
    if floating && k >= 1 {
        let tests = vec![Test::Thin(l.vertical), Test::Meets(reg(Sparse1), -1)];
        put(Vertical0, keep(Solid3, tests.clone(), true));
        put(Solid4, keep(Solid3, tests, false));
    } else {
        put(Vertical0, Op::Empty);
        put(Solid4, Op::Same(reg(Solid3)));
    }

    // Internal bridges: solid over the sparse below, opened, big enough, grown into the solid.
    if k >= 1 {
        let bo = l.bridge_open;
        put(BridgeEroded, all(vec![term(reg(Solid4), 0, -bo), term(reg(Sparse1), -1, -bo)]));
        put(BridgeOpened, Op::Compose(term(reg(BridgeEroded), 0, bo)));
        put(BridgeKept, keep(BridgeOpened, vec![Test::AreaOver(l.bridge_area)], true));
        put(Bridge, all(vec![term(reg(Solid4), 0, 0.0), term(reg(BridgeKept), 0, l.bridge_grow)]));
        put(Solid5, if_any(Bridge, 0, all(vec![term(reg(Solid4), 0, 0.0), not(reg(Bridge), 0, 0.0)]), Op::Same(reg(Solid4))));
    } else {
        for r in [BridgeEroded, BridgeOpened, BridgeKept, Bridge] {
            put(r, Op::Empty);
        }
        put(Solid5, Op::Same(reg(Solid4)));
    }

    // Floating vertical shells: under the bridge above, a band along the walls carries its ends,
    // and the narrow solid beside it joins the vertical shells.
    if floating && k + 1 < n {
        put(Band0, all(vec![term(reg(Sparse1), 0, 0.0), term(reg(Bridge), 1, l.under_grow), not(f, 0, -l.band_core)]));
        put(Band, keep(Band0, vec![Test::AreaOver(l.band_area)], true));
        put(Sparse2, if_any(Band, 0, all(vec![term(reg(Sparse1), 0, 0.0), not(reg(Band), 0, 0.0)]), Op::Same(reg(Sparse1))));
        let tests = vec![Test::Thin(l.adjacent), Test::Meets(reg(Bridge), 1)];
        put(Adjacent, if_any(Band, 0, keep(Solid5, tests.clone(), true), Op::Empty));
        put(Solid6, if_any(Band, 0, keep(Solid5, tests, false), Op::Same(reg(Solid5))));
        put(Vertical, Op::Concat(vec![Vertical0, Adjacent]));
    } else {
        put(Band0, Op::Empty);
        put(Band, Op::Empty);
        put(Sparse2, Op::Same(reg(Sparse1)));
        put(Adjacent, Op::Empty);
        put(Solid6, Op::Same(reg(Solid5)));
        put(Vertical, Op::Same(reg(Vertical0)));
    }

    // Classes are disjoint: the bottom wins over the top.
    put(TopFinal, if_any(Grown, 0, all(vec![term(reg(Top), 0, 0.0), not(reg(Grown), 0, 0.0)]), Op::Same(reg(Top))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(t: u32, b: u32, flags: bool) -> ShellSpec {
        let l = ShellLevels { speck: 0.05, face_grow: 0.6, bottom_grow: 0.9, shell_open: 0.25, min_sparse_area: 15.0, vertical: 1.1, bridge_open: 0.45, bridge_area: 1.0, bridge_grow: 0.9, under_grow: 0.01, band_core: 0.45, band_area: 0.5, adjacent: 0.67, narrow: 2.7 };
        ShellSpec { top_layers: t, bottom_layers: b, vertical_shells: flags, floating_shells: flags, narrow_solid: flags, sets: vec![l] }
    }

    fn reads(op: &Op, out: &mut Vec<(Src, i8)>) {
        fn expr(e: &Expr, out: &mut Vec<(Src, i8)>) {
            match e {
                Expr::Term(t) => out.push((t.src, t.dz)),
                Expr::All(v) | Expr::Any(v) => v.iter().for_each(|e| expr(e, out)),
            }
        }
        match op {
            Op::Empty => {}
            Op::Same(s) => out.push((*s, 0)),
            Op::Compose(e) => expr(e, out),
            Op::Keep { of, tests, .. } => {
                out.push((Src::Reg(*of), 0));
                out.extend(tests.iter().filter_map(|t| match t { Test::Meets(s, dz) => Some((*s, *dz)), _ => None }));
            }
            Op::Concat(v) => out.extend(v.iter().map(|r| (Src::Reg(*r), 0))),
            Op::IfAny { of, dz, then, otherwise } => {
                out.push((Src::Reg(*of), *dz));
                reads(then, out);
                reads(otherwise, out);
            }
        }
    }

    /// Every plane makes the same regions in the same order, every region once, and a round reads
    /// only what an earlier round made (on its own plane or a neighbour), within the part.
    #[test]
    fn rounds_line_up_across_planes_and_read_only_earlier_regions() {
        for (t, b, flags) in [(1, 1, false), (4, 3, true), (5, 1, true), (1, 4, false)] {
            let s = spec(t, b, flags);
            let n = 9;
            let all: Vec<Vec<Round>> = (0..n).map(|k| rounds(&s, &s.sets[0], k, n)).collect();
            let order: Vec<Reg> = all[0].iter().map(|r| r.out).collect();
            let mut unique = order.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), order.len(), "each region once");
            for (k, plane) in all.iter().enumerate() {
                assert_eq!(plane.iter().map(|r| r.out).collect::<Vec<_>>(), order, "plane {k}: the same order");
                for (i, round) in plane.iter().enumerate() {
                    let mut read = Vec::new();
                    reads(&round.op, &mut read);
                    for (src, dz) in read {
                        let j = k as i64 + dz as i64;
                        assert!((0..n as i64).contains(&j), "plane {k} {:?} reads {src:?} off the part", round.out);
                        if let Src::Reg(r) = src {
                            let at = order.iter().position(|o| *o == r).unwrap();
                            assert!(at < i, "plane {k} {:?} reads {r:?} before it is made", round.out);
                        }
                    }
                }
            }
            for (_, r) in CLASSES {
                assert!(order.contains(&r));
            }
        }
    }
}
