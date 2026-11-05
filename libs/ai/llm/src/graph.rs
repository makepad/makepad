use std::collections::HashSet;

use crate::context::Context;
use crate::tensor::{Tensor, TensorId};

pub type NodeId = TensorId;

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum GraphEvalOrder {
    LeftToRight = 0,
    RightToLeft = 1,
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub size: usize,
    pub nodes: Vec<TensorId>,
    pub grads: Vec<Option<TensorId>>,
    pub grad_accs: Vec<Option<TensorId>>,
    pub leafs: Vec<TensorId>,
    pub order: GraphEvalOrder,
    /// What `nodes` and `leafs` hold, so asking is a lookup and not a scan.
    in_nodes: HashSet<TensorId>,
    in_leafs: HashSet<TensorId>,
}

impl Default for Graph {
    fn default() -> Self {
        Self::with_size(crate::core::GGML_DEFAULT_GRAPH_SIZE, false)
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_size(size: usize, grads: bool) -> Self {
        Self {
            size,
            nodes: Vec::with_capacity(size),
            grads: if grads {
                Vec::with_capacity(size)
            } else {
                Vec::new()
            },
            grad_accs: if grads {
                Vec::with_capacity(size)
            } else {
                Vec::new()
            },
            leafs: Vec::with_capacity(size),
            order: GraphEvalOrder::LeftToRight,
            in_nodes: HashSet::new(),
            in_leafs: HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.grads.clear();
        self.grad_accs.clear();
        self.leafs.clear();
        self.in_nodes.clear();
        self.in_leafs.clear();
    }

    pub fn graph_size(&self) -> usize {
        self.size
    }

    pub fn n_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn n_leafs(&self) -> usize {
        self.leafs.len()
    }

    pub fn node(&self, i: isize) -> Option<TensorId> {
        if i >= 0 {
            self.nodes.get(i as usize).copied()
        } else {
            let idx = self.nodes.len().checked_sub(i.unsigned_abs())?;
            self.nodes.get(idx).copied()
        }
    }

    /// Add a node by hand. Everything it reads must be in the graph already:
    /// [`Self::build_forward_expand`] stops at a tensor it finds here.
    pub fn add_node(&mut self, tensor: TensorId) {
        if self.in_nodes.insert(tensor) {
            self.nodes.push(tensor);
        }
    }

    /// Add a leaf by hand, under the same rule as [`Self::add_node`].
    pub fn add_leaf(&mut self, tensor: TensorId) {
        if self.in_leafs.insert(tensor) {
            self.leafs.push(tensor);
        }
    }

    pub fn get_tensor<'a>(&self, ctx: &'a Context, name: &str) -> Option<&'a Tensor> {
        self.nodes
            .iter()
            .chain(self.leafs.iter())
            .copied()
            .find_map(|id| {
                let tensor = ctx.tensor(id)?;
                (tensor.name() == Some(name)).then_some(tensor)
            })
    }

    pub fn get_grad(&self, node: TensorId) -> Option<TensorId> {
        self.nodes
            .iter()
            .position(|&current| current == node)
            .and_then(|index| self.grads.get(index).copied().flatten())
    }

    pub fn get_grad_acc(&self, node: TensorId) -> Option<TensorId> {
        self.nodes
            .iter()
            .position(|&current| current == node)
            .and_then(|index| self.grad_accs.get(index).copied().flatten())
    }

    /// Add `tensor` and everything it reads, each after its sources.
    ///
    /// A tensor already in the graph came in after everything it reads, so
    /// the walk stops there. A decode graph expands one output per state
    /// write, hundreds of them over shared layers, and walking the shared part
    /// again for each made building it quadratic.
    pub fn build_forward_expand(&mut self, ctx: &Context, tensor: TensorId) -> Result<(), String> {
        if self.in_nodes.contains(&tensor) || self.in_leafs.contains(&tensor) {
            return Ok(());
        }

        let t = ctx
            .tensor(tensor)
            .ok_or_else(|| format!("invalid tensor id {}", tensor))?;

        for src in t.src.iter().flatten().copied() {
            self.build_forward_expand(ctx, src)?;
        }
        if let Some(view_src) = t.view_src {
            self.build_forward_expand(ctx, view_src)?;
        }

        let is_leaf = t.op == crate::op::Op::None && t.src.iter().all(Option::is_none);
        if is_leaf {
            self.add_leaf(tensor);
        } else {
            self.add_node(tensor);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Building a graph must keep the order a walk from scratch per output
    //! gives: every plan downstream is laid out from it.
    use super::Graph;
    use crate::op::Op;
    use crate::{BufferUsage, Context, InitParams, TensorId, TensorType};
    use std::collections::HashSet;

    /// The walk the graph used to do: every output walks everything below
    /// it again, adding what is not there yet.
    fn walk(
        ctx: &Context,
        nodes: &mut Vec<TensorId>,
        leafs: &mut Vec<TensorId>,
        id: TensorId,
        seen: &mut HashSet<TensorId>,
    ) {
        if !seen.insert(id) {
            return;
        }
        let t = ctx.tensor(id).unwrap();
        for src in t.src.iter().flatten().copied() {
            walk(ctx, nodes, leafs, src, seen);
        }
        if let Some(view_src) = t.view_src {
            walk(ctx, nodes, leafs, view_src, seen);
        }
        let list = if t.op == Op::None && t.src.iter().all(Option::is_none) { leafs } else { nodes };
        if !list.contains(&id) {
            list.push(id);
        }
    }

    #[test]
    fn expanding_many_outputs_over_shared_layers_keeps_the_walk_order() {
        let mut ctx = Context::new(InitParams { mem_size: 1 << 20, mem_buffer: None, no_alloc: true });
        let mut all: Vec<TensorId> = (0..4)
            .map(|i| {
                ctx.new_named_tensor(format!("in{i}"), TensorType::F32, 1, &[64], BufferUsage::Activations)
                    .unwrap()
            })
            .collect();
        let mut seed = 0x9a7e_u64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        // Layers that read earlier layers, views of them, and a leaf now and
        // then, like a decode graph's residual stream and its state writes.
        for i in 0..400 {
            let a = all[all.len() - 1 - next(all.len().min(12))];
            let b = all[next(all.len())];
            let half = ctx.tensor(a).unwrap().ne[0] as usize / 2;
            let id = match next(3) {
                _ if i % 40 == 20 => ctx
                    .new_named_tensor(format!("late{i}"), TensorType::F32, 1, &[64], BufferUsage::Activations)
                    .unwrap(),
                0 if half > 0 => ctx.view_1d(a, half as i64, 4 * next(half + 1)).unwrap(),
                1 => ctx.scale(a, 0.5, BufferUsage::Activations).unwrap(),
                _ => ctx.binary_like_a(Op::Add, a, b, BufferUsage::Activations).unwrap(),
            };
            all.push(id);
        }
        let outputs: Vec<TensorId> = (0..120).map(|_| all[4 + next(all.len() - 4)]).collect();

        let mut graph = Graph::new();
        let (mut nodes, mut leafs) = (Vec::new(), Vec::new());
        for &out in &outputs {
            graph.build_forward_expand(&ctx, out).unwrap();
            walk(&ctx, &mut nodes, &mut leafs, out, &mut HashSet::new());
        }
        assert!(nodes.len() > 200 && leafs.len() > 4, "{} nodes {} leafs", nodes.len(), leafs.len());
        assert_eq!(graph.nodes, nodes);
        assert_eq!(graph.leafs, leafs);
    }
}
