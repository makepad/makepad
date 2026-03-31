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
        }
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.grads.clear();
        self.grad_accs.clear();
        self.leafs.clear();
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

    pub fn add_node(&mut self, tensor: TensorId) {
        if !self.nodes.contains(&tensor) {
            self.nodes.push(tensor);
        }
    }

    pub fn add_leaf(&mut self, tensor: TensorId) {
        if !self.leafs.contains(&tensor) {
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

    pub fn build_forward_expand(
        &mut self,
        ctx: &Context,
        tensor: TensorId,
    ) -> Result<(), String> {
        let mut visited = HashSet::new();
        self.build_forward_expand_impl(ctx, tensor, &mut visited)
    }

    pub fn view(&self, i0: usize, i1: usize) -> Self {
        let end = i1.min(self.nodes.len());
        let start = i0.min(end);
        Self {
            size: end - start,
            nodes: self.nodes[start..end].to_vec(),
            grads: Vec::new(),
            grad_accs: Vec::new(),
            leafs: Vec::new(),
            order: self.order,
        }
    }

    fn build_forward_expand_impl(
        &mut self,
        ctx: &Context,
        tensor: TensorId,
        visited: &mut HashSet<TensorId>,
    ) -> Result<(), String> {
        if !visited.insert(tensor) {
            return Ok(());
        }

        let t = ctx
            .tensor(tensor)
            .ok_or_else(|| format!("invalid tensor id {}", tensor))?;

        for src in t.src.iter().flatten().copied() {
            self.build_forward_expand_impl(ctx, src, visited)?;
        }
        if let Some(view_src) = t.view_src {
            self.build_forward_expand_impl(ctx, view_src, visited)?;
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
