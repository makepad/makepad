// ported from N:rustc_data_structures/src/graph/scc/mod.rs @8d1a764
use crate::fxhash::HashSet as FxHashSet;
#[derive(Clone, Debug)]
pub struct Sccs { scc_indices: Vec<usize>, scc_data: Vec<Vec<usize>> }
impl Sccs {
    pub fn new(graph: &[Vec<usize>]) -> Self {
        SccsConstruction::construct(graph)
    }
    pub fn scc(&self, r: usize) -> usize { self.scc_indices[r] }
    pub fn num_sccs(&self) -> usize { self.scc_data.len() }
    pub fn all_sccs(&self) -> std::ops::Range<usize> { 0..self.num_sccs() }
    pub fn successors(&self, scc: usize) -> &[usize] { &self.scc_data[scc] }
    pub fn reverse(&self) -> Vec<Vec<usize>> {
        let mut graph = vec![vec![]; self.num_sccs()];
        for s in self.all_sccs() { for t in self.successors(s) { graph[*t].push(s); } }
        graph
    }
}
struct SccsConstruction<'c>
{
    graph: &'c [Vec<usize>],
    node_states: Vec<NodeState>,
    node_stack: Vec<usize>,
    successors_stack: Vec<usize>,
    duplicate_set: FxHashSet<usize>,
    scc_data: Vec<Vec<usize>>,
}
#[derive(Copy, Clone, Debug)]
enum NodeState {
    NotVisited,
    BeingVisited { depth: usize },
    InCycle { scc_index: usize },
    InCycleWith { parent: usize },
}
#[derive(Copy, Clone, Debug)]
enum WalkReturn {
    Cycle { min_depth: usize },
    Complete { scc_index: usize },
}
impl<'c> SccsConstruction<'c>
{
    fn construct(graph: &'c [Vec<usize>]) -> Sccs {
        let num_nodes = graph.len();
        let mut this = Self {
            graph,
            node_states: vec![NodeState::NotVisited; num_nodes],
            node_stack: Vec::with_capacity(num_nodes),
            successors_stack: Vec::new(),
            scc_data: Vec::new(),
            duplicate_set: FxHashSet::default(),
        };
        let scc_indices = (0..graph.len())
            .map(|node| match this.start_walk_from(node) {
                WalkReturn::Complete { scc_index, .. } => scc_index,
                WalkReturn::Cycle { min_depth, .. } => {
                    panic!("`start_walk_node({node:?})` returned cycle with depth {min_depth:?}")
                }
            })
            .collect();
        Sccs { scc_indices, scc_data: this.scc_data }
    }
    fn start_walk_from(&mut self, node: usize) -> WalkReturn {
        self.inspect_node(node).unwrap_or_else(|| self.walk_unvisited_node(node))
    }
    fn inspect_node(&mut self, node: usize) -> Option<WalkReturn> {
        Some(match self.find_state(node) {
            NodeState::InCycle { scc_index } => {
                WalkReturn::Complete { scc_index }
            }
            NodeState::BeingVisited { depth: min_depth } => {
                WalkReturn::Cycle { min_depth }
            }
            NodeState::NotVisited => return None,
            NodeState::InCycleWith { parent } => panic!(
                "`find_state` returned `InCycleWith({parent:?})`, which ought to be impossible"
            ),
        })
    }
    fn find_state(&mut self, mut node: usize) -> NodeState {
        let mut previous_node = node;
        let (root_state, assigned_state) = {
            loop {
                match self.node_states[node] {
                    s @ NodeState::NotVisited => return s,
                    s @ NodeState::InCycle { .. } => break (s, s),
                    s @ NodeState::BeingVisited { depth, .. } => {
                        break (s, NodeState::InCycleWith { parent: self.node_stack[depth] });
                    }
                    NodeState::InCycleWith { parent } => {
                        assert!(node != parent, "Node can not be in cycle with itself");
                        self.node_states[node] = NodeState::InCycleWith { parent: previous_node };
                        previous_node = node;
                        node = parent;
                    }
                }
            }
        };
        loop {
            if previous_node == node {
                return root_state;
            }
            match self.node_states[previous_node] {
                NodeState::InCycleWith { parent: previous } => {
                    node = previous_node;
                    previous_node = previous;
                }
                other => unreachable!("Invalid previous link while compressing cycle: {other:?}"),
            }
            self.node_states[node] = assigned_state;
        }
    }
    fn walk_unvisited_node(&mut self, initial: usize) -> WalkReturn {
        struct VisitingNodeFrame<'g> {
            node: usize,
            successors: Option<std::iter::Copied<std::slice::Iter<'g, usize>>>,
            depth: usize,
            min_depth: usize,
            successors_len: usize,
            min_cycle_root: usize,
            successor_node: usize,
        }
        let mut successors_stack = core::mem::take(&mut self.successors_stack);
        debug_assert_eq!(successors_stack.len(), 0);
        let mut stack: Vec<VisitingNodeFrame<'_>> = vec![VisitingNodeFrame {
            node: initial,
            depth: 0,
            min_depth: 0,
            successors: None,
            successors_len: 0,
            min_cycle_root: initial,
            successor_node: initial,
        }];
        let mut return_value = None;
        'recurse: while let Some(frame) = stack.last_mut() {
            let VisitingNodeFrame {
                node,
                depth,
                successors,
                successors_len,
                min_depth,
                min_cycle_root,
                successor_node,
            } = frame;
            let node = *node;
            let depth = *depth;
            let successors = match successors {
                Some(successors) => successors,
                None => {
                    debug_assert!(matches!(self.node_states[node], NodeState::NotVisited));
                    self.node_states[node] = NodeState::BeingVisited {
                        depth,
                    };
                    self.node_stack.push(node);
                    *successors_len = successors_stack.len();
                    successors.get_or_insert(self.graph[node].iter().copied())
                }
            };
            let successors_len = *successors_len;
            let returned_walk =
                return_value.take().into_iter().map(|walk| (*successor_node, Some(walk)));
            let successor_walk = successors.map(|successor_node| {
                (successor_node, self.inspect_node(successor_node))
            });
            for (successor_node, walk) in returned_walk.chain(successor_walk) {
                match walk {
                    Some(WalkReturn::Cycle {
                        min_depth: successor_min_depth,
                    }) => {
                        assert!(successor_min_depth <= depth);
                        if successor_min_depth < *min_depth {
                            *min_depth = successor_min_depth;
                            *min_cycle_root = successor_node;
                        }
                    }
                    Some(WalkReturn::Complete {
                        scc_index: successor_scc_index,
                    }) => {
                        successors_stack.push(successor_scc_index);
                    }
                    None => {
                        let depth = depth + 1;
                        frame.successor_node = successor_node;
                        stack.push(VisitingNodeFrame {
                            node: successor_node,
                            depth,
                            successors: None,
                            successors_len: 0,
                            min_depth: depth,
                            min_cycle_root: successor_node,
                            successor_node,
                        });
                        continue 'recurse;
                    }
                }
            }
            let r = self.node_stack.pop();
            debug_assert_eq!(r, Some(node));
            let frame = stack.pop().unwrap();
            debug_assert_eq!(frame.node, node);
            debug_assert!(return_value.is_none());
            return_value = Some(if frame.min_depth == depth {
                let deduplicated_successors = {
                    let duplicate_set = &mut self.duplicate_set;
                    duplicate_set.clear();
                    successors_stack
                        .drain(successors_len..)
                        .filter(move |&i| duplicate_set.insert(i))
                };
                let scc_index = self.scc_data.len();
                self.scc_data.push(deduplicated_successors.collect());
                self.node_states[node] =
                    NodeState::InCycle { scc_index };
                WalkReturn::Complete { scc_index }
            } else {
                self.node_states[node] = NodeState::InCycleWith { parent: frame.min_cycle_root };
                WalkReturn::Cycle {
                    min_depth: frame.min_depth,
                }
            });
        }
        self.successors_stack = successors_stack;
        debug_assert_eq!(self.successors_stack.len(), 0);
        return_value.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycles_and_cross_edges_keep_successors_before_predecessors() {
        let graph = vec![vec![1,4],vec![2],vec![1,3],vec![],vec![3,4],vec![]];
        let sccs = Sccs::new(&graph);
        assert_eq!(sccs.num_sccs(),5);
        assert_eq!(sccs.scc(1),sccs.scc(2));
        for (source,edges) in graph.iter().enumerate() {
            for &target in edges {
                let (a,b) = (sccs.scc(source),sccs.scc(target));
                assert!(a == b || (b < a && sccs.successors(a).contains(&b)));
            }
        }
        assert!(sccs.successors(sccs.scc(5)).is_empty());
        assert_eq!(Sccs::new(&[]).num_sccs(),0);
    }
    #[test]
    fn iterative_walk_handles_deep_cycle_and_path_compression() {
        let mut graph = vec![vec![]; 20000];
        for node in 0..graph.len()-1 { graph[node].push(node+1); }
        graph.last_mut().unwrap().push(0);
        let sccs = Sccs::new(&graph);
        assert_eq!(sccs.num_sccs(),1);
        for node in 0..graph.len() { assert_eq!(sccs.scc(node),0); }
    }
}
