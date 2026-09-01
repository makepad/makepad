use crate::{Rect, SemanticId};
use std::sync::Arc;

const LEAF_SIZE: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Rect,
    left: u32,
    right: u32,
    start: u32,
    len: u16,
}

impl Node {
    fn branch(bounds: Rect, left: usize, right: usize) -> Self {
        Self {
            bounds,
            left: left as u32,
            right: right as u32,
            start: 0,
            len: 0,
        }
    }

    fn leaf(bounds: Rect, start: usize, len: usize) -> Self {
        Self {
            bounds,
            left: 0,
            right: 0,
            start: start as u32,
            len: len as u16,
        }
    }

    fn is_leaf(self) -> bool {
        self.len != 0
    }
}

/// Packed immutable bounding-volume hierarchy for one page.
///
/// Nodes and item indices are contiguous and read-only after construction.
/// Queries perform an exact bounds test before returning an item, so rendering
/// never receives a false positive. Result indices are in paint order.
#[derive(Clone, Debug, Default)]
pub struct SpatialIndex {
    nodes: Arc<[Node]>,
    item_indices: Arc<[u32]>,
    item_bounds: Arc<[Rect]>,
}

impl SpatialIndex {
    pub fn build(item_bounds: &[Rect]) -> Self {
        if item_bounds.is_empty() {
            return Self::default();
        }
        let mut order: Vec<usize> = (0..item_bounds.len()).collect();
        let mut nodes = Vec::with_capacity(item_bounds.len() * 2);
        let mut packed = Vec::with_capacity(item_bounds.len());
        build_node(&mut order, item_bounds, &mut nodes, &mut packed);
        Self {
            nodes: nodes.into(),
            item_indices: packed.into(),
            item_bounds: item_bounds.to_vec().into(),
        }
    }

    pub fn query(&self, viewport: Rect) -> Vec<usize> {
        if self.nodes.is_empty() || viewport.is_empty() {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut stack = Vec::with_capacity(32);
        stack.push(0usize);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index];
            if !node.bounds.intersects(viewport) {
                continue;
            }
            if node.is_leaf() {
                let start = node.start as usize;
                let end = start + node.len as usize;
                for index in self.item_indices[start..end].iter().copied() {
                    let index = index as usize;
                    if self.item_bounds[index].intersects(viewport) {
                        result.push(index);
                    }
                }
            } else {
                // Push right first so the deterministic left subtree is visited first.
                stack.push(node.right as usize);
                stack.push(node.left as usize);
            }
        }
        result.sort_unstable();
        result
    }

    pub fn memory_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>()
            + self.item_indices.len() * std::mem::size_of::<u32>()
            + self.item_bounds.len() * std::mem::size_of::<Rect>()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn build_node(
    order: &mut [usize],
    bounds: &[Rect],
    nodes: &mut Vec<Node>,
    packed: &mut Vec<u32>,
) -> usize {
    let node_index = nodes.len();
    nodes.push(Node::leaf(Rect::EMPTY, 0, 0));
    let node_bounds = order
        .iter()
        .fold(Rect::EMPTY, |acc, index| acc.union(bounds[*index]));
    if order.len() <= LEAF_SIZE {
        let start = packed.len();
        order.sort_unstable();
        packed.extend(order.iter().map(|index| *index as u32));
        nodes[node_index] = Node::leaf(node_bounds, start, order.len());
        return node_index;
    }

    let split_x = node_bounds.width() >= node_bounds.height();
    order.sort_by(|a, b| {
        let ca = bounds[*a].center();
        let cb = bounds[*b].center();
        let primary = if split_x {
            ca.x.total_cmp(&cb.x)
        } else {
            ca.y.total_cmp(&cb.y)
        };
        primary.then_with(|| a.cmp(b))
    });
    let middle = order.len() / 2;
    let (left_order, right_order) = order.split_at_mut(middle);
    let left = build_node(left_order, bounds, nodes, packed);
    let right = build_node(right_order, bounds, nodes, packed);
    nodes[node_index] = Node::branch(node_bounds, left, right);
    node_index
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: SemanticId,
    pub paint_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point;

    #[test]
    fn culling_matches_brute_force_exactly() {
        let mut bounds = Vec::new();
        for i in 0..4096 {
            let x = ((i * 73) % 211) as f64 * 0.7;
            let y = ((i * 151) % 307) as f64 * 0.4;
            bounds.push(Rect::from_xywh(x, y, 0.2 + (i % 9) as f64, 0.3));
        }
        let index = SpatialIndex::build(&bounds);
        for step in 0..80 {
            let viewport = Rect::from_xywh(
                (step * 17 % 120) as f64,
                (step * 29 % 100) as f64,
                9.25,
                13.75,
            );
            let expected: Vec<_> = bounds
                .iter()
                .enumerate()
                .filter_map(|(i, bounds)| bounds.intersects(viewport).then_some(i))
                .collect();
            assert_eq!(index.query(viewport), expected);
        }
        assert!(index.node_count() > 1);
        assert!(index
            .query(Rect::new(
                Point::new(500.0, 500.0),
                Point::new(501.0, 501.0),
            ))
            .is_empty());
    }
}
