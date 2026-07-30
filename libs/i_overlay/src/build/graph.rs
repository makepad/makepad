use crate::build::builder::{GraphBuilder, GraphNode};
use crate::core::edge_data::OverlayEdgeData;
use crate::core::solver::Solver;
use crate::geom::end::End;
use crate::segm::winding::WindingCount;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_key_sort::sort::two_keys::TwoKeysSort;
use i_tree::Expiration;

impl<I, C, N, D> GraphBuilder<C, N, I, D>
where
    I: IntNumber + Expiration + SortKey,
    C: WindingCount,
    N: GraphNode,
    D: OverlayEdgeData<C>,
{
    pub(super) fn build_nodes_and_connect_links(&mut self, solver: &Solver) {
        let n = self.links.len();
        if n == 0 {
            return;
        }

        self.build_ends(solver);

        self.nodes.clear();
        self.nodes.reserve(n);

        let mut ai = 0;
        let mut bi = 0;
        let mut indices = Vec::with_capacity(4);

        while ai < n || bi < n {
            let (a_cnt, next_ai, a_point) = if ai < n {
                let point = self.links[ai].a.point;
                let mut end = ai + 1;
                while end < n && self.links[end].a.point == point {
                    end += 1;
                }
                (end - ai, end, Some(point))
            } else {
                (0, ai, None)
            };

            let (b_cnt, next_bi, b_point) = if bi < n {
                let point = self.ends[bi].point;
                let mut end = bi + 1;
                while end < n && self.ends[end].point == point {
                    end += 1;
                }
                (end - bi, end, Some(point))
            } else {
                (0, bi, None)
            };

            let (consume_a, consume_b) = match (a_point, b_point) {
                (Some(a), Some(b)) if a == b => (a_cnt, b_cnt),
                (Some(a), Some(b)) if a < b => (a_cnt, 0),
                (Some(_), Some(_)) => (0, b_cnt),
                (Some(_), None) => (a_cnt, 0),
                (None, Some(_)) => (0, b_cnt),
                (None, None) => break,
            };

            let node_id = self.nodes.len();

            if consume_a > 0 {
                let start = ai;
                let end = ai + consume_a;
                for idx in start..end {
                    self.links[idx].a.id = node_id;
                    indices.push(idx);
                }
                ai = next_ai;
            }

            if consume_b > 0 {
                let start = bi;
                let end = bi + consume_b;
                for idx in start..end {
                    let link_idx = self.ends[idx].index;
                    indices.push(link_idx);
                    self.links[link_idx].b.id = node_id;
                }
                bi = next_bi;
            }

            debug_assert!(!indices.is_empty());
            self.nodes.push(N::with_indices(indices.as_slice()));
            indices.clear();
        }
    }

    #[inline]
    fn build_ends(&mut self, solver: &Solver) {
        self.ends.clear();
        self.ends.reserve(self.links.len());
        for (i, link) in self.links.iter().enumerate() {
            self.ends.push(End {
                index: i,
                point: link.b.point,
            });
        }
        self.ends
            .sort_by_two_keys(solver.is_parallel_sort_allowed(), |e| e.point.x, |e| e.point.y);
    }
}
