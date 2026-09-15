//! The architecture overview layout (phase 0 of the cached architecture
//! plans): lanes as horizontal swimlanes stacked top → bottom, the plan's
//! nodes as uniform cards ranked left → right by their edges (a longest
//! path over the plan's graph with its cycles broken first), the cards of
//! one lane and rank stacked and ordered by the barycentre of their
//! neighbours, and orthogonal edge routes through the corridors between
//! the rank columns and the channels between the lanes. Everything is in
//! world units (one unit = one px at zoom 1); the overview is generous
//! by design — "a quick overview of what something does", not a dense
//! graph — so the gaps are wide and the cards uniform.
//!
//! `greedy_feedback_arcs` and `longest_paths` are the two graph helpers
//! this layout needs, kept here so the plan view depends on no code
//! analyser.
use makepad_widgets::{dvec2, DVec2, Rect};
use std::collections::{BTreeSet, VecDeque};

/// A card's size: a title row, a summary of up to two lines and a chip row.
pub const CARD_W: f64 = 240.0;
pub const CARD_H: f64 = 98.0;
/// The routing corridor between two rank columns.
pub const GAP_X: f64 = 76.0;
/// Between two stacked cards of one lane and rank.
pub const GAP_Y: f64 = 24.0;
/// A lane's inner padding around its cards, and its title band.
pub const LANE_PAD: f64 = 20.0;
pub const LANE_TITLE_H: f64 = 38.0;
/// The channel between two lanes (backward and long routes run here).
pub const LANE_GAP: f64 = 26.0;
/// The margin around the whole plan.
pub const MARGIN: f64 = 40.0;
/// Ports on one side of a card are spread by this much.
pub const PORT_SPREAD: f64 = 14.0;
/// A route's bends stay this far from the cards they pass.
pub const ROUTE_INSET: f64 = 18.0;

/// One node to lay out: its lane (none = the trailing unlaned band).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesignNodeSpec {
    pub lane: Option<usize>,
}

/// The layout of a plan: every node's card, every lane's band (the
/// trailing band is the unlaned nodes' when any exist), the rank column of
/// every node, one route per edge, and the bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct DesignLayout {
    pub node_rects: Vec<Rect>,
    pub lane_rects: Vec<Rect>,
    /// Lane indices in drawing order, top → bottom; `lane_count` means the
    /// unlaned band.
    pub lane_order: Vec<usize>,
    pub ranks: Vec<u32>,
    pub routes: Vec<DesignRoute>,
    pub bounds: Rect,
}

/// An edge's orthogonal polyline in world units, from the source card's
/// port to the target card's port (the arrowhead goes at the last point).
#[derive(Clone, Debug, PartialEq)]
pub struct DesignRoute {
    pub points: Vec<DVec2>,
    /// The edge ran against the rank direction (a feedback edge).
    pub back: bool,
}

/// Which side of a card a port sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

fn card_rect(x: f64, y: f64) -> Rect {
    Rect { pos: dvec2(x, y), size: dvec2(CARD_W, CARD_H) }
}

/// Greedy feedback-arc set: peel sinks and sources through queues, otherwise
/// take the node with the largest out-degree minus in-degree from an ordered
/// score set; edges from later to earlier in the resulting order are the
/// feedback set. Returns feedback edge indices into `edges`, ascending.
pub fn greedy_feedback_arcs(node_count: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    if node_count == 0 || edges.is_empty() {
        return Vec::new();
    }
    let mut out_deg = vec![0i64; node_count];
    let mut in_deg = vec![0i64; node_count];
    let mut out_adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let mut in_adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (i, (a, b)) in edges.iter().enumerate() {
        if a == b {
            continue;
        }
        out_deg[*a] += 1;
        in_deg[*b] += 1;
        out_adj[*a].push(i);
        in_adj[*b].push(i);
    }
    let mut removed = vec![false; node_count];
    let mut scores: BTreeSet<(i64, std::cmp::Reverse<usize>)> = (0..node_count).map(|v| (out_deg[v] - in_deg[v], std::cmp::Reverse(v))).collect();
    let mut sinks: VecDeque<usize> = (0..node_count).filter(|v| out_deg[*v] == 0).collect();
    let mut sources: VecDeque<usize> = (0..node_count).filter(|v| in_deg[*v] == 0 && out_deg[*v] != 0).collect();
    let mut s1: Vec<usize> = Vec::new();
    let mut s2: Vec<usize> = Vec::new();
    let mut remaining = node_count;
    let remove = |v: usize, removed: &mut Vec<bool>, out_deg: &mut Vec<i64>, in_deg: &mut Vec<i64>, scores: &mut BTreeSet<(i64, std::cmp::Reverse<usize>)>, sinks: &mut VecDeque<usize>, sources: &mut VecDeque<usize>| {
        if removed[v] {
            return false;
        }
        removed[v] = true;
        scores.remove(&(out_deg[v] - in_deg[v], std::cmp::Reverse(v)));
        for &ei in &out_adj[v] {
            let (_, b) = edges[ei];
            if !removed[b] {
                scores.remove(&(out_deg[b] - in_deg[b], std::cmp::Reverse(b)));
                in_deg[b] -= 1;
                scores.insert((out_deg[b] - in_deg[b], std::cmp::Reverse(b)));
                if in_deg[b] == 0 {
                    sources.push_back(b);
                }
            }
        }
        for &ei in &in_adj[v] {
            let (a, _) = edges[ei];
            if !removed[a] {
                scores.remove(&(out_deg[a] - in_deg[a], std::cmp::Reverse(a)));
                out_deg[a] -= 1;
                scores.insert((out_deg[a] - in_deg[a], std::cmp::Reverse(a)));
                if out_deg[a] == 0 {
                    sinks.push_back(a);
                }
            }
        }
        true
    };
    while remaining > 0 {
        let mut progressed = false;
        while let Some(v) = sinks.pop_front() {
            if !removed[v] && out_deg[v] == 0 && remove(v, &mut removed, &mut out_deg, &mut in_deg, &mut scores, &mut sinks, &mut sources) {
                s2.push(v);
                remaining -= 1;
                progressed = true;
            }
        }
        while let Some(v) = sources.pop_front() {
            if !removed[v] && in_deg[v] == 0 && remove(v, &mut removed, &mut out_deg, &mut in_deg, &mut scores, &mut sinks, &mut sources) {
                s1.push(v);
                remaining -= 1;
                progressed = true;
            }
        }
        if progressed && (!sinks.is_empty() || !sources.is_empty()) {
            continue;
        }
        if remaining == 0 {
            break;
        }
        let Some(&(_, std::cmp::Reverse(best))) = scores.iter().next_back() else { break };
        if remove(best, &mut removed, &mut out_deg, &mut in_deg, &mut scores, &mut sinks, &mut sources) {
            s1.push(best);
            remaining -= 1;
        }
    }
    let mut order = s1;
    s2.reverse();
    order.extend(s2);
    let mut position = vec![usize::MAX; node_count];
    for (p, v) in order.iter().enumerate() {
        position[*v] = p;
    }
    let mut feedback: Vec<usize> = edges.iter().enumerate().filter(|(_, (a, b))| a != b && position[*a] > position[*b]).map(|(i, _)| i).collect();
    feedback.sort_unstable();
    feedback
}

/// Longest path to a sink. `edges` must be acyclic. Returns None on a cycle.
pub fn longest_paths(node_count: usize, edges: &[(usize, usize)]) -> Option<Vec<u32>> {
    let mut out_adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let mut in_deg = vec![0usize; node_count];
    for (a, b) in edges {
        if a == b {
            continue;
        }
        out_adj[*a].push(*b);
        in_deg[*b] += 1;
    }
    let mut order = Vec::with_capacity(node_count);
    let mut ready: Vec<usize> = (0..node_count).filter(|v| in_deg[*v] == 0).collect();
    ready.reverse();
    while let Some(v) = ready.pop() {
        order.push(v);
        for &w in &out_adj[v] {
            in_deg[w] -= 1;
            if in_deg[w] == 0 {
                ready.push(w);
            }
        }
    }
    if order.len() != node_count {
        return None;
    }
    let mut rank = vec![0u32; node_count];
    for &v in order.iter().rev() {
        let mut r = 0u32;
        for &w in &out_adj[v] {
            r = r.max(rank[w] + 1);
        }
        rank[v] = r;
    }
    Some(rank)
}

/// Lay a plan out. `lane_count` lanes exist (nodes name them by index);
/// `edges` are `(from, to)` node indices; self-edges are ignored.
pub fn layout(nodes: &[DesignNodeSpec], lane_count: usize, edges: &[(usize, usize)]) -> DesignLayout {
    let n = nodes.len();
    let clean: Vec<(usize, usize)> = edges.iter().copied().filter(|(a, b)| a != b && *a < n && *b < n).collect();
    // ranks: break the cycles, then the longest path to a sink; the
    // sources (largest path) go left
    let feedback: std::collections::HashSet<usize> = greedy_feedback_arcs(n, &clean).into_iter().collect();
    let dag: Vec<(usize, usize)> = clean.iter().enumerate().map(|(i, (a, b))| if feedback.contains(&i) { (*b, *a) } else { (*a, *b) }).collect();
    let to_sink = longest_paths(n, &dag).unwrap_or_else(|| vec![0; n]);
    let max_r = to_sink.iter().copied().max().unwrap_or(0);
    let ranks: Vec<u32> = to_sink.iter().map(|r| max_r - r).collect();
    let cols = ranks.iter().copied().max().map(|c| c as usize + 1).unwrap_or(1);

    // lane order: lanes whose nodes mostly point at other lanes go on top
    // (dependents above, what they use below), ties by index; the unlaned
    // band always trails
    let unlaned = nodes.iter().any(|s| s.lane.is_none());
    let band_of = |i: usize| nodes[i].lane.unwrap_or(lane_count);
    let band_count = lane_count + usize::from(unlaned);
    let mut score = vec![0i64; band_count];
    for (a, b) in &clean {
        let (la, lb) = (band_of(*a), band_of(*b));
        if la != lb {
            score[la] += 1;
            score[lb] -= 1;
        }
    }
    let mut lane_order: Vec<usize> = (0..lane_count).collect();
    lane_order.sort_by(|a, b| score[*b].cmp(&score[*a]).then(a.cmp(b)));
    if unlaned {
        lane_order.push(lane_count);
    }

    // rows within (band, column): ordered by the barycentre of the
    // neighbours' current vertical position, a few sweeps, deterministic
    let mut groups: Vec<Vec<Vec<usize>>> = vec![vec![Vec::new(); cols]; band_count];
    for i in 0..n {
        groups[band_of(i)][ranks[i] as usize].push(i);
    }
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, b) in &clean {
        neighbours[*a].push(*b);
        neighbours[*b].push(*a);
    }
    // a node's ordinal position: band order, then row within its group
    let mut position = vec![0.0f64; n];
    let assign_positions = |groups: &Vec<Vec<Vec<usize>>>, position: &mut Vec<f64>| {
        for (oi, band) in lane_order.iter().enumerate() {
            for col in &groups[*band] {
                for (row, i) in col.iter().enumerate() {
                    position[*i] = oi as f64 * 1000.0 + row as f64;
                }
            }
        }
    };
    assign_positions(&groups, &mut position);
    for _ in 0..4 {
        for band in 0..band_count {
            for col in 0..cols {
                let group = &mut groups[band][col];
                if group.len() < 2 {
                    continue;
                }
                let key = |i: usize| -> (f64, usize) {
                    let ns = &neighbours[i];
                    if ns.is_empty() {
                        (position[i], i)
                    } else {
                        (ns.iter().map(|j| position[*j]).sum::<f64>() / ns.len() as f64, i)
                    }
                };
                group.sort_by(|a, b| key(*a).partial_cmp(&key(*b)).unwrap_or(std::cmp::Ordering::Equal));
            }
        }
        assign_positions(&groups, &mut position);
    }

    // geometry: every lane spans the full width; its height comes from its
    // tallest column; a column with fewer rows than the lane's tallest is
    // centred vertically
    let width = MARGIN * 2.0 + LANE_PAD * 2.0 + cols as f64 * CARD_W + cols.saturating_sub(1) as f64 * GAP_X;
    let mut lane_rects = vec![Rect::default(); band_count];
    let mut node_rects = vec![Rect::default(); n];
    let mut y = MARGIN;
    for band in &lane_order {
        let rows = groups[*band].iter().map(|c| c.len()).max().unwrap_or(0).max(1);
        let title_h = if *band == lane_count { 0.0 } else { LANE_TITLE_H };
        let content_h = rows as f64 * CARD_H + (rows - 1) as f64 * GAP_Y;
        let lane_h = title_h + LANE_PAD * 2.0 + content_h;
        let lane = Rect { pos: dvec2(MARGIN, y), size: dvec2(width - MARGIN * 2.0, lane_h) };
        lane_rects[*band] = lane;
        let content_top = lane.pos.y + title_h + LANE_PAD;
        for (col, group) in groups[*band].iter().enumerate() {
            let own_h = group.len() as f64 * CARD_H + group.len().saturating_sub(1) as f64 * GAP_Y;
            let top = content_top + (content_h - own_h) * 0.5;
            for (row, i) in group.iter().enumerate() {
                let x = lane.pos.x + LANE_PAD + col as f64 * (CARD_W + GAP_X);
                node_rects[*i] = card_rect(x, top + row as f64 * (CARD_H + GAP_Y));
            }
        }
        y += lane_h + LANE_GAP;
    }
    let bounds = Rect { pos: dvec2(0.0, 0.0), size: dvec2(width, (y - LANE_GAP + MARGIN).max(MARGIN * 2.0)) };

    // ports: the edges leaving or entering one side of a card are spread
    // along that side, ordered by the other end's vertical position
    let mut ports: std::collections::HashMap<(usize, Side), Vec<(usize, f64)>> = std::collections::HashMap::new();
    let mut sides: Vec<(Side, Side)> = Vec::with_capacity(clean.len());
    for (ei, (a, b)) in clean.iter().enumerate() {
        let (ra, rb) = (ranks[*a], ranks[*b]);
        let (sa, sb) = if rb > ra {
            (Side::Right, Side::Left)
        } else if rb == ra {
            if node_rects[*b].pos.y > node_rects[*a].pos.y {
                (Side::Bottom, Side::Top)
            } else {
                (Side::Top, Side::Bottom)
            }
        } else {
            (Side::Left, Side::Right)
        };
        sides.push((sa, sb));
        ports.entry((*a, sa)).or_default().push((ei, node_rects[*b].center().y + node_rects[*b].center().x * 1e-3));
        ports.entry((*b, sb)).or_default().push((ei, node_rects[*a].center().y + node_rects[*a].center().x * 1e-3));
    }
    let mut port_at: std::collections::HashMap<(usize, usize), DVec2> = std::collections::HashMap::new();
    for ((node, side), list) in ports.iter_mut() {
        list.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal).then(x.0.cmp(&y.0)));
        let r = node_rects[*node];
        let k = list.len();
        for (slot, (ei, _)) in list.iter().enumerate() {
            let offset = (slot as f64 - (k as f64 - 1.0) * 0.5) * PORT_SPREAD;
            let p = match side {
                Side::Right => dvec2(r.pos.x + r.size.x, (r.center().y + offset).clamp(r.pos.y + 8.0, r.pos.y + r.size.y - 8.0)),
                Side::Left => dvec2(r.pos.x, (r.center().y + offset).clamp(r.pos.y + 8.0, r.pos.y + r.size.y - 8.0)),
                Side::Top => dvec2((r.center().x + offset).clamp(r.pos.x + 12.0, r.pos.x + r.size.x - 12.0), r.pos.y),
                Side::Bottom => dvec2((r.center().x + offset).clamp(r.pos.x + 12.0, r.pos.x + r.size.x - 12.0), r.pos.y + r.size.y),
            };
            port_at.insert((*ei, *node), p);
        }
    }

    // routes
    let lane_bottom = |i: usize| -> f64 {
        let l = lane_rects[band_of(i)];
        l.pos.y + l.size.y
    };
    let mut routes = Vec::with_capacity(clean.len());
    for (ei, (a, b)) in clean.iter().enumerate() {
        let (ra, rb) = (ranks[*a], ranks[*b]);
        let pa = port_at[&(ei, *a)];
        let pb = port_at[&(ei, *b)];
        let (sa, _) = sides[ei];
        let back = ra > rb;
        let points = if rb > ra {
            // forward: through the corridor after `a`; a jump over a
            // column runs along the channel under the lower lane so it
            // crosses no card
            let corridor_a = node_rects[*a].pos.x + CARD_W + GAP_X * 0.5;
            if rb - ra == 1 {
                if (pa.y - pb.y).abs() < 0.5 {
                    vec![pa, pb]
                } else {
                    vec![pa, dvec2(corridor_a, pa.y), dvec2(corridor_a, pb.y), pb]
                }
            } else {
                let corridor_b = node_rects[*b].pos.x - GAP_X * 0.5;
                let channel = lane_bottom(*a).max(lane_bottom(*b)) + LANE_GAP * 0.5;
                vec![pa, dvec2(corridor_a, pa.y), dvec2(corridor_a, channel), dvec2(corridor_b, channel), dvec2(corridor_b, pb.y), pb]
            }
        } else if rb == ra {
            // the same column: down (or up) through the gap between the
            // two cards' rows, past whatever sits between them along the
            // corridor to the right
            if matches!(sa, Side::Bottom) && node_rects[*b].pos.y - (node_rects[*a].pos.y + CARD_H) <= GAP_Y + 0.5 && band_of(*a) == band_of(*b) {
                vec![pa, pb]
            } else {
                let corridor = node_rects[*a].pos.x + CARD_W + GAP_X * 0.35;
                let ya = if matches!(sa, Side::Bottom) { pa.y + ROUTE_INSET } else { pa.y - ROUTE_INSET };
                let yb = if matches!(sa, Side::Bottom) { pb.y - ROUTE_INSET } else { pb.y + ROUTE_INSET };
                vec![pa, dvec2(pa.x, ya), dvec2(corridor, ya), dvec2(corridor, yb), dvec2(pb.x, yb), pb]
            }
        } else {
            // backward: out of the left side, down the corridor before
            // `a` to the channel under the lower lane, back along it, up
            // the corridor after `b`, into its right side
            let corridor_a = node_rects[*a].pos.x - GAP_X * 0.5;
            let corridor_b = node_rects[*b].pos.x + CARD_W + GAP_X * 0.5;
            let channel = lane_bottom(*a).max(lane_bottom(*b)) + LANE_GAP * 0.5;
            if ra - rb == 1 && band_of(*a) == band_of(*b) && (pa.y - pb.y).abs() < 0.5 {
                vec![pa, pb]
            } else {
                vec![pa, dvec2(corridor_a, pa.y), dvec2(corridor_a, channel), dvec2(corridor_b, channel), dvec2(corridor_b, pb.y), pb]
            }
        };
        routes.push(DesignRoute { points: dedup(points), back });
    }
    DesignLayout { node_rects, lane_rects, lane_order, ranks, routes, bounds }
}

fn dedup(points: Vec<DVec2>) -> Vec<DVec2> {
    let mut out: Vec<DVec2> = Vec::with_capacity(points.len());
    for p in points {
        if out.last().is_some_and(|q| (q.x - p.x).abs() < 1e-9 && (q.y - p.y).abs() < 1e-9) {
            continue;
        }
        out.push(p);
    }
    // drop the middle of three collinear points
    let mut i = 1;
    while i + 1 < out.len() {
        let (a, b, c) = (out[i - 1], out[i], out[i + 1]);
        let collinear = ((a.x - b.x).abs() < 1e-9 && (b.x - c.x).abs() < 1e-9) || ((a.y - b.y).abs() < 1e-9 && (b.y - c.y).abs() < 1e-9);
        if collinear {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(lanes: &[Option<usize>]) -> Vec<DesignNodeSpec> {
        lanes.iter().map(|l| DesignNodeSpec { lane: *l }).collect()
    }

    #[test]
    fn a_chain_ranks_left_to_right_and_routes_forward() {
        let l = layout(&specs(&[Some(0), Some(0), Some(0)]), 1, &[(0, 1), (1, 2)]);
        assert_eq!(l.ranks, vec![0, 1, 2]);
        assert!(l.node_rects[0].pos.x < l.node_rects[1].pos.x && l.node_rects[1].pos.x < l.node_rects[2].pos.x);
        for r in &l.routes {
            assert!(!r.back);
            assert!(r.points.len() >= 2);
        }
        assert_eq!(l.lane_order, vec![0]);
    }

    #[test]
    fn a_cycle_is_broken_and_the_feedback_edge_routes_back() {
        let l = layout(&specs(&[Some(0), Some(0)]), 1, &[(0, 1), (1, 0)]);
        assert_eq!(l.routes.iter().filter(|r| r.back).count(), 1);
        assert_ne!(l.ranks[0], l.ranks[1]);
    }

    #[test]
    fn lanes_stack_with_the_pointing_lane_on_top_and_unlaned_last() {
        // lane 1's node uses lane 0's node: lane 1 goes on top
        let l = layout(&specs(&[Some(0), Some(1), None]), 2, &[(1, 0), (2, 0)]);
        assert_eq!(l.lane_order, vec![1, 0, 2]);
        assert!(l.lane_rects[1].pos.y < l.lane_rects[0].pos.y && l.lane_rects[0].pos.y < l.lane_rects[2].pos.y);
        // every card lies inside its band
        for (i, r) in l.node_rects.iter().enumerate() {
            let band = l.lane_rects[[0usize, 1, 2][i]];
            assert!(r.is_inside_of(band), "card {i} outside its lane");
        }
    }

    #[test]
    fn cards_never_overlap_and_the_bounds_hold_everything() {
        let lanes: Vec<Option<usize>> = (0..12).map(|i| Some(i % 3)).collect();
        let edges: Vec<(usize, usize)> = (0..11).map(|i| (i, i + 1)).chain([(0, 5), (2, 9), (11, 3)]).collect();
        let l = layout(&specs(&lanes), 3, &edges);
        for i in 0..12 {
            for j in i + 1..12 {
                assert!(!l.node_rects[i].intersects(l.node_rects[j]), "{i} overlaps {j}");
            }
            assert!(l.node_rects[i].is_inside_of(l.bounds));
        }
        assert_eq!(l.routes.len(), edges.len());
    }

    #[test]
    fn feedback_arcs_break_every_cycle() {
        let edges = [(0, 1), (1, 2), (2, 0), (2, 3)];
        let feedback = greedy_feedback_arcs(4, &edges);
        assert_eq!(feedback.len(), 1);
        let dag: Vec<(usize, usize)> = edges.iter().enumerate().map(|(i, (a, b))| if feedback.contains(&i) { (*b, *a) } else { (*a, *b) }).collect();
        assert!(longest_paths(4, &dag).is_some());
        assert!(longest_paths(3, &[(0, 1), (1, 2), (2, 0)]).is_none());
    }
}
