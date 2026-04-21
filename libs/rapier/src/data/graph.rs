// This is basically a stripped down version of petgraph's UnGraph.
// - It is not generic with respect to the index type (we always use u32).
// - It preserves associated edge iteration order after Serialization/Deserialization.
// - It is always undirected.
//! A stripped-down version of petgraph's UnGraph.

use std::cmp::max;
use std::ops::{Index, IndexMut};

/// Node identifier.
#[derive(Copy, Clone, Default, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct NodeIndex(u32);

impl NodeIndex {
    #[inline]
    pub fn new(x: u32) -> Self {
        NodeIndex(x)
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub fn end() -> Self {
        NodeIndex(crate::INVALID_U32)
    }

    fn _into_edge(self) -> EdgeIndex {
        EdgeIndex(self.0)
    }
}

impl From<u32> for NodeIndex {
    fn from(ix: u32) -> Self {
        NodeIndex(ix)
    }
}

/// Edge identifier.
#[derive(Copy, Clone, Default, PartialEq, PartialOrd, Eq, Ord, Hash, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct EdgeIndex(u32);

impl EdgeIndex {
    #[inline]
    pub fn new(x: u32) -> Self {
        EdgeIndex(x)
    }

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// An invalid `EdgeIndex` used to denote absence of an edge, for example
    /// to end an adjacency list.
    #[inline]
    pub fn end() -> Self {
        EdgeIndex(crate::INVALID_U32)
    }

    fn _into_node(self) -> NodeIndex {
        NodeIndex(self.0)
    }
}

impl From<u32> for EdgeIndex {
    fn from(ix: u32) -> Self {
        EdgeIndex(ix)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub enum Direction {
    Outgoing = 0,
    Incoming = 1,
}

impl Direction {
    fn opposite(self) -> Direction {
        match self {
            Direction::Outgoing => Direction::Incoming,
            Direction::Incoming => Direction::Outgoing,
        }
    }
}

const DIRECTIONS: [Direction; 2] = [Direction::Outgoing, Direction::Incoming];

/// The graph's node type.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Node<N> {
    /// Associated node data.
    pub weight: N,
    /// Next edge in outgoing and incoming edge lists.
    next: [EdgeIndex; 2],
}

/// The graph's edge type.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Edge<E> {
    /// Associated edge data.
    pub weight: E,
    /// Next edge in outgoing and incoming edge lists.
    next: [EdgeIndex; 2],
    /// Start and End node index
    node: [NodeIndex; 2],
}

impl<E> Edge<E> {
    /// Return the source node index.
    pub fn source(&self) -> NodeIndex {
        self.node[0]
    }

    /// Return the target node index.
    pub fn target(&self) -> NodeIndex {
        self.node[1]
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Graph<N, E> {
    pub(crate) nodes: Vec<Node<N>>,
    pub(crate) edges: Vec<Edge<E>>,
}

enum Pair<T> {
    Both(T, T),
    One(T),
    None,
}

/// Get mutable references at index `a` and `b`.
fn index_twice<T>(arr: &mut [T], a: usize, b: usize) -> Pair<&mut T> {
    if max(a, b) >= arr.len() {
        Pair::None
    } else if a == b {
        Pair::One(&mut arr[max(a, b)])
    } else {
        // safe because a, b are in bounds and distinct
        unsafe {
            let ar = &mut *(arr.get_unchecked_mut(a) as *mut _);
            let br = &mut *(arr.get_unchecked_mut(b) as *mut _);
            Pair::Both(ar, br)
        }
    }
}

impl<N, E> Graph<N, E> {
    /// Create a new `Graph` with estimated capacity.
    pub fn with_capacity(nodes: usize, edges: usize) -> Self {
        Graph {
            nodes: Vec::with_capacity(nodes),
            edges: Vec::with_capacity(edges),
        }
    }

    /// Add a node (also called vertex) with associated data `weight` to the graph.
    ///
    /// Computes in **O(1)** time.
    ///
    /// Return the index of the new node.
    ///
    /// **Panics** if the Graph is at the maximum number of nodes for its index
    /// type (N/A if usize).
    pub fn add_node(&mut self, weight: N) -> NodeIndex {
        let node = Node {
            weight,
            next: [EdgeIndex::end(), EdgeIndex::end()],
        };
        assert!(self.nodes.len() != crate::INVALID_USIZE);
        let node_idx = NodeIndex::new(self.nodes.len() as u32);
        self.nodes.push(node);
        node_idx
    }

    /// Access the weight for node `a`.
    ///
    /// Also available with indexing syntax: `&graph[a]`.
    pub fn node_weight(&self, a: NodeIndex) -> Option<&N> {
        self.nodes.get(a.index()).map(|n| &n.weight)
    }

    /// Access the weight for edge `a`.
    ///
    /// Also available with indexing syntax: `&graph[a]`.
    pub fn edge_weight(&self, a: EdgeIndex) -> Option<&E> {
        self.edges.get(a.index()).map(|e| &e.weight)
    }

    /// Access the weight for edge `a` mutably.
    ///
    /// Also available with indexing syntax: `&mut graph[a]`.
    pub fn edge_weight_mut(&mut self, a: EdgeIndex) -> Option<&mut E> {
        self.edges.get_mut(a.index()).map(|e| &mut e.weight)
    }

    /// Add an edge from `a` to `b` to the graph, with its associated
    /// data `weight`.
    ///
    /// Return the index of the new edge.
    ///
    /// Computes in **O(1)** time.
    ///
    /// **Panics** if any of the nodes don't exist.<br>
    /// **Panics** if the Graph is at the maximum number of edges for its index
    /// type (N/A if usize).
    ///
    /// **Note:** `Graph` allows adding parallel (“duplicate”) edges. If you want
    /// to avoid this, use [`.update_edge(a, b, weight)`](#method.update_edge) instead.
    pub fn add_edge(&mut self, a: NodeIndex, b: NodeIndex, weight: E) -> EdgeIndex {
        assert!(self.edges.len() != crate::INVALID_USIZE);
        let edge_idx = EdgeIndex::new(self.edges.len() as u32);
        let mut edge = Edge {
            weight,
            node: [a, b],
            next: [EdgeIndex::end(); 2],
        };
        match index_twice(&mut self.nodes, a.index(), b.index()) {
            Pair::None => panic!("Graph::add_edge: node indices out of bounds"),
            Pair::One(an) => {
                edge.next = an.next;
                an.next[0] = edge_idx;
                an.next[1] = edge_idx;
            }
            Pair::Both(an, bn) => {
                // a and b are different indices
                edge.next = [an.next[0], bn.next[1]];
                an.next[0] = edge_idx;
                bn.next[1] = edge_idx;
            }
        }
        self.edges.push(edge);
        edge_idx
    }

    /// Access the source and target nodes for `e`.
    pub fn edge_endpoints(&self, e: EdgeIndex) -> Option<(NodeIndex, NodeIndex)> {
        self.edges
            .get(e.index())
            .map(|ed| (ed.source(), ed.target()))
    }

    /// Remove `a` from the graph if it exists, and return its weight.
    /// If it doesn't exist in the graph, return `None`.
    ///
    /// Apart from `a`, this invalidates the last node index in the graph
    /// (that node will adopt the removed node index). Edge indices are
    /// invalidated as they would be following the removal of each edge
    /// with an endpoint in `a`.
    ///
    /// Computes in **O(e')** time, where **e'** is the number of affected
    /// edges, including *n* calls to `.remove_edge()` where *n* is the number
    /// of edges with an endpoint in `a`, and including the edges with an
    /// endpoint in the displaced node.
    pub fn remove_node(&mut self, a: NodeIndex) -> Option<N> {
        self.nodes.get(a.index())?;
        for d in &DIRECTIONS {
            let k = *d as usize;

            // Remove all edges from and to this node.
            loop {
                let next = self.nodes[a.index()].next[k];
                if next == EdgeIndex::end() {
                    break;
                }
                let ret = self.remove_edge(next);
                debug_assert!(ret.is_some());
                let _ = ret;
            }
        }

        // Use swap_remove -- only the swapped-in node is going to change
        // NodeIndex, so we only have to walk its edges and update them.

        let node = self.nodes.swap_remove(a.index());

        // Find the edge lists of the node that had to relocate.
        // It may be that no node had to relocate, then we are done already.
        let swap_edges = match self.nodes.get(a.index()) {
            None => return Some(node.weight),
            Some(ed) => ed.next,
        };

        // The swapped element's old index
        let old_index = NodeIndex::new(self.nodes.len() as u32);
        let new_index = a;

        // Adjust the starts of the out edges, and ends of the in edges.
        for &d in &DIRECTIONS {
            let k = d as usize;
            let mut edges = edges_walker_mut(&mut self.edges, swap_edges[k], d);
            while let Some(curedge) = edges.next_edge() {
                debug_assert!(curedge.node[k] == old_index);
                curedge.node[k] = new_index;
            }
        }
        Some(node.weight)
    }

    /// For edge `e` with endpoints `edge_node`, replace links to it,
    /// with links to `edge_next`.
    fn change_edge_links(
        &mut self,
        edge_node: [NodeIndex; 2],
        e: EdgeIndex,
        edge_next: [EdgeIndex; 2],
    ) {
        for &d in &DIRECTIONS {
            let k = d as usize;
            let node = match self.nodes.get_mut(edge_node[k].index()) {
                Some(r) => r,
                None => {
                    debug_assert!(
                        false,
                        "Edge's endpoint dir={:?} index={:?} not found",
                        d, edge_node[k]
                    );
                    return;
                }
            };
            let fst = node.next[k];
            if fst == e {
                //println!("Updating first edge 0 for node {}, set to {}", edge_node[0], edge_next[0]);
                node.next[k] = edge_next[k];
            } else {
                let mut edges = edges_walker_mut(&mut self.edges, fst, d);
                while let Some(curedge) = edges.next_edge() {
                    if curedge.next[k] == e {
                        curedge.next[k] = edge_next[k];
                        break; // the edge can only be present once in the list.
                    }
                }
            }
        }
    }

    /// Remove an edge and return its edge weight, or `None` if it didn't exist.
    ///
    /// Apart from `e`, this invalidates the last edge index in the graph
    /// (that edge will adopt the removed edge index).
    ///
    /// Computes in **O(e')** time, where **e'** is the size of four particular edge lists, for
    /// the vertices of `e` and the vertices of another affected edge.
    pub fn remove_edge(&mut self, e: EdgeIndex) -> Option<E> {
        // every edge is part of two lists,
        // outgoing and incoming edges.
        // Remove it from both
        let (edge_node, edge_next) = match self.edges.get(e.index()) {
            None => return None,
            Some(x) => (x.node, x.next),
        };
        // Remove the edge from its in and out lists by replacing it with
        // a link to the next in the list.
        self.change_edge_links(edge_node, e, edge_next);
        self.remove_edge_adjust_indices(e)
    }

    fn remove_edge_adjust_indices(&mut self, e: EdgeIndex) -> Option<E> {
        // swap_remove the edge -- only the removed edge
        // and the edge swapped into place are affected and need updating
        // indices.
        let edge = self.edges.swap_remove(e.index());
        let swap = match self.edges.get(e.index()) {
            // no element needed to be swapped.
            None => return Some(edge.weight),
            Some(ed) => ed.node,
        };
        let swapped_e = EdgeIndex::new(self.edges.len() as u32);

        // Update the edge lists by replacing links to the old index by references to the new
        // edge index.
        self.change_edge_links(swap, swapped_e, [e, e]);
        Some(edge.weight)
    }

    /// Return an iterator of all edges of `a`.
    ///
    /// - `Directed`: Outgoing edges from `a`.
    /// - `Undirected`: All edges connected to `a`.
    ///
    /// Produces an empty iterator if the node doesn't exist.<br>
    /// Iterator element type is `EdgeReference<E, Ix>`.
    pub fn edges(&self, a: NodeIndex) -> Edges<'_, E> {
        self.edges_directed(a, Direction::Outgoing)
    }

    /// Return an iterator of all edges of `a`, in the specified direction.
    ///
    /// - `Directed`, `Outgoing`: All edges from `a`.
    /// - `Directed`, `Incoming`: All edges to `a`.
    /// - `Undirected`, `Outgoing`: All edges connected to `a`, with `a` being the source of each
    ///   edge.
    /// - `Undirected`, `Incoming`: All edges connected to `a`, with `a` being the target of each
    ///   edge.
    ///
    /// Produces an empty iterator if the node `a` doesn't exist.<br>
    /// Iterator element type is `EdgeReference<E, Ix>`.
    pub fn edges_directed(&self, a: NodeIndex, dir: Direction) -> Edges<'_, E> {
        Edges {
            skip_start: a,
            edges: &self.edges,
            direction: dir,
            next: match self.nodes.get(a.index()) {
                None => [EdgeIndex::end(), EdgeIndex::end()],
                Some(n) => n.next,
            },
        }
    }

    /*
    /// Return an iterator over all the edges connecting `a` and `b`.
    ///
    /// - `Directed`: Outgoing edges from `a`.
    /// - `Undirected`: All edges connected to `a`.
    ///
    /// Iterator element type is `EdgeReference<E, Ix>`.
    pub fn edges_connecting(&self, a: NodeIndex, b: NodeIndex) -> EdgesConnecting<E, Ty, Ix> {
        EdgesConnecting {
            target_node: b,
            edges: self.edges_directed(a, Direction::Outgoing),
            ty: PhantomData,
        }
    }
    */

    /// Lookup an edge from `a` to `b`.
    ///
    /// Computes in **O(e')** time, where **e'** is the number of edges
    /// connected to `a` (and `b`, if the graph edges are undirected).
    pub fn find_edge(&self, a: NodeIndex, b: NodeIndex) -> Option<EdgeIndex> {
        self.find_edge_undirected(a, b).map(|(ix, _)| ix)
    }

    /// Lookup an edge between `a` and `b`, in either direction.
    ///
    /// If the graph is undirected, then this is equivalent to `.find_edge()`.
    ///
    /// Return the edge index and its directionality, with `Outgoing` meaning
    /// from `a` to `b` and `Incoming` the reverse,
    /// or `None` if the edge does not exist.
    pub fn find_edge_undirected(
        &self,
        a: NodeIndex,
        b: NodeIndex,
    ) -> Option<(EdgeIndex, Direction)> {
        match self.nodes.get(a.index()) {
            None => None,
            Some(node) => self.find_edge_undirected_from_node(node, b),
        }
    }

    fn find_edge_undirected_from_node(
        &self,
        node: &Node<N>,
        b: NodeIndex,
    ) -> Option<(EdgeIndex, Direction)> {
        for &d in &DIRECTIONS {
            let k = d as usize;
            let mut edix = node.next[k];
            while let Some(edge) = self.edges.get(edix.index()) {
                if edge.node[1 - k] == b {
                    return Some((edix, d));
                }
                edix = edge.next[k];
            }
        }
        None
    }

    /// Access the internal node array.
    pub fn raw_nodes(&self) -> &[Node<N>] {
        &self.nodes
    }

    /// Access the internal edge array.
    pub fn raw_edges(&self) -> &[Edge<E>] {
        &self.edges
    }

    /// Accessor for data structure internals: the first edge in the given direction.
    pub fn first_edge(&self, a: NodeIndex, dir: Direction) -> Option<EdgeIndex> {
        match self.nodes.get(a.index()) {
            None => None,
            Some(node) => {
                let edix = node.next[dir as usize];
                if edix == EdgeIndex::end() {
                    None
                } else {
                    Some(edix)
                }
            }
        }
    }

    /// Accessor for data structure internals: the next edge for the given direction.
    pub fn next_edge(&self, e: EdgeIndex, dir: Direction) -> Option<EdgeIndex> {
        match self.edges.get(e.index()) {
            None => None,
            Some(node) => {
                let edix = node.next[dir as usize];
                if edix == EdgeIndex::end() {
                    None
                } else {
                    Some(edix)
                }
            }
        }
    }
}

struct EdgesWalkerMut<'a, E: 'a> {
    edges: &'a mut [Edge<E>],
    next: EdgeIndex,
    dir: Direction,
}

fn edges_walker_mut<E>(
    edges: &mut [Edge<E>],
    next: EdgeIndex,
    dir: Direction,
) -> EdgesWalkerMut<'_, E> {
    EdgesWalkerMut { edges, next, dir }
}

impl<E> EdgesWalkerMut<'_, E> {
    fn next_edge(&mut self) -> Option<&mut Edge<E>> {
        self.next().map(|t| t.1)
    }

    fn next(&mut self) -> Option<(EdgeIndex, &mut Edge<E>)> {
        let this_index = self.next;
        let k = self.dir as usize;
        match self.edges.get_mut(self.next.index()) {
            None => None,
            Some(edge) => {
                self.next = edge.next[k];
                Some((this_index, edge))
            }
        }
    }
}

/// Iterator over the edges of from or to a node
pub struct Edges<'a, E: 'a> {
    /// starting node to skip over
    skip_start: NodeIndex,
    edges: &'a [Edge<E>],

    /// Next edge to visit.
    next: [EdgeIndex; 2],

    /// For directed graphs: the direction to iterate in
    /// For undirected graphs: the direction of edges
    direction: Direction,
}

impl<'a, E> Iterator for Edges<'a, E> {
    type Item = EdgeReference<'a, E>;

    fn next(&mut self) -> Option<Self::Item> {
        //      type        direction    |    iterate over    reverse
        //                               |
        //    Directed      Outgoing     |      outgoing        no
        //    Directed      Incoming     |      incoming        no
        //   Undirected     Outgoing     |        both       incoming
        //   Undirected     Incoming     |        both       outgoing

        // For iterate_over, "both" is represented as None.
        // For reverse, "no" is represented as None.
        let (iterate_over, _reverse) = (None, Some(self.direction.opposite()));

        if iterate_over.unwrap_or(Direction::Outgoing) == Direction::Outgoing {
            let i = self.next[0].index();
            if let Some(Edge {
                node: _node,
                weight,
                next,
            }) = self.edges.get(i)
            {
                self.next[0] = next[0];
                return Some(EdgeReference {
                    index: EdgeIndex(i as u32),
                    // node: if reverse == Some(Direction::Outgoing) {
                    //     swap_pair(*node)
                    // } else {
                    //     *node
                    // },
                    weight,
                });
            }
        }

        if iterate_over.unwrap_or(Direction::Incoming) == Direction::Incoming {
            while let Some(Edge { node, weight, next }) = self.edges.get(self.next[1].index()) {
                let edge_index = self.next[1];
                self.next[1] = next[1];
                // In any of the "both" situations, self-loops would be iterated over twice.
                // Skip them here.
                if iterate_over.is_none() && node[0] == self.skip_start {
                    continue;
                }

                return Some(EdgeReference {
                    index: edge_index,
                    // node: if reverse == Some(Direction::Incoming) {
                    //     swap_pair(*node)
                    // } else {
                    //     *node
                    // },
                    weight,
                });
            }
        }

        None
    }
}

// fn swap_pair<T>(mut x: [T; 2]) -> [T; 2] {
//     x.swap(0, 1);
//     x
// }

impl<E> Clone for Edges<'_, E> {
    fn clone(&self) -> Self {
        Edges {
            skip_start: self.skip_start,
            edges: self.edges,
            next: self.next,
            direction: self.direction,
        }
    }
}

/// Index the `Graph` by `NodeIndex` to access node weights.
///
/// **Panics** if the node doesn't exist.
impl<N, E> Index<NodeIndex> for Graph<N, E> {
    type Output = N;
    fn index(&self, index: NodeIndex) -> &N {
        &self.nodes[index.index()].weight
    }
}

/// Index the `Graph` by `NodeIndex` to access node weights.
///
/// **Panics** if the node doesn't exist.
impl<N, E> IndexMut<NodeIndex> for Graph<N, E> {
    fn index_mut(&mut self, index: NodeIndex) -> &mut N {
        &mut self.nodes[index.index()].weight
    }
}

/// Index the `Graph` by `EdgeIndex` to access edge weights.
///
/// **Panics** if the edge doesn't exist.
impl<N, E> Index<EdgeIndex> for Graph<N, E> {
    type Output = E;
    fn index(&self, index: EdgeIndex) -> &E {
        &self.edges[index.index()].weight
    }
}

/// Index the `Graph` by `EdgeIndex` to access edge weights.
///
/// **Panics** if the edge doesn't exist.
impl<N, E> IndexMut<EdgeIndex> for Graph<N, E> {
    fn index_mut(&mut self, index: EdgeIndex) -> &mut E {
        &mut self.edges[index.index()].weight
    }
}

/// Reference to a `Graph` edge.
#[derive(Debug)]
pub struct EdgeReference<'a, E: 'a> {
    index: EdgeIndex,
    // node: [NodeIndex; 2],
    weight: &'a E,
}

impl<'a, E: 'a> EdgeReference<'a, E> {
    #[inline]
    pub fn id(&self) -> EdgeIndex {
        self.index
    }

    #[inline]
    pub fn weight(&self) -> &'a E {
        self.weight
    }
}

impl<E> Clone for EdgeReference<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for EdgeReference<'_, E> {}

impl<E> PartialEq for EdgeReference<'_, E>
where
    E: PartialEq,
{
    fn eq(&self, rhs: &Self) -> bool {
        self.index == rhs.index && self.weight == rhs.weight
    }
}
