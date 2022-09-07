use {
    std::{
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        live_node_vec::*,
        live_node::{ LiveNode, LiveProp},
    }
};

const MAX_CLONE_STACK_DEPTH_SAFETY: usize = 100;

pub struct LiveNodeReader<'a> {
    eot: bool,
    depth: usize,
    index: usize,
    nodes: &'a[LiveNode]
}

impl<'a> LiveNodeReader<'a> {
    pub fn new(index: usize, nodes: &'a[LiveNode]) -> Self {
        
        Self {
            eot: false,
            depth: 0,
            index,
            nodes
        }
    }
    
    pub fn index_option(&self, index: Option<usize>, depth_change: isize) -> Option<Self> {
        if self.eot {panic!();}
        if let Some(index) = index {
            Some(Self {
                eot: self.eot,
                depth: (self.depth as isize + depth_change) as usize,
                index: index,
                nodes: self.nodes
            })
        }
        else {
            None
        }
    }
    
    pub fn node(&self) -> &LiveNode {
        if self.eot {panic!();}
        &self.nodes[self.index]
    }
    
    pub fn parent(&self) -> Option<Self> {self.index_option(self.nodes.parent(self.index), -1)}
    pub fn append_child_index(&self) -> usize {self.nodes.append_child_index(self.index)}
    pub fn first_child(&self) -> Option<Self> {self.index_option(self.nodes.first_child(self.index), 1)}
    pub fn last_child(&self) -> Option<Self> {self.index_option(self.nodes.last_child(self.index), 1)}
    pub fn next_child(&self) -> Option<Self> {self.index_option(self.nodes.next_child(self.index), 0)}
    
    pub fn node_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.node_slice(self.index)
    }
    
    pub fn children_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.children_slice(self.index)
    }
    
    pub fn child_by_number(&self, child_number: usize) -> Option<Self> {
        self.index_option(self.nodes.child_by_number(self.index, child_number), 1)
    }
    
    pub fn child_by_name(&self, name: LiveProp) -> Option<Self> {
        self.index_option(self.nodes.child_by_name(self.index, name), 1)
    }
    
    fn child_by_path(&self, path: &[LiveProp]) -> Option<Self> {
        self.index_option(self.nodes.child_by_path(self.index, path), 1)
    }
    
    pub fn scope_up_by_name(&self, name: LiveProp) -> Option<Self> {
        self.index_option(self.nodes.scope_up_by_name(self.index, name), 0)
    }
    
    pub fn count_children(&self) -> usize {self.nodes.count_children(self.index)}
    pub fn clone_child(&self, out_vec: &mut Vec<LiveNode>) {
        if self.eot {panic!();}
        self.nodes.clone_child(self.index, out_vec)
    }
    
    pub fn to_string(&self, max_depth: usize) -> String {
        if self.eot {panic!();}
        self.nodes.to_string(self.index, max_depth)
    }
    
    pub fn skip(&mut self) {
        if self.eot {panic!();}
        self.index = self.nodes.skip_node(self.index);
        // check eot
        if self.nodes[self.index].is_close() { // standing on a close node
            if self.depth == 1 {
                self.eot = true;
                self.index += 1;
            }
        }
    }
    
    pub fn walk(&mut self) {
        if self.eot {panic!();}
        if self.nodes[self.index].is_open() {
            self.depth += 1;
        }
        else if self.nodes[self.index].is_close() {
            if self.depth == 0 {panic!()}
            self.depth -= 1;
            if self.depth == 0 {
                self.eot = true;
            }
        }
        self.index += 1;
    }
    
    pub fn is_eot(&self) -> bool {
        return self.eot
    }
    
    pub fn index(&self) -> usize {
        self.index
    }
    
    pub fn depth(&self) -> usize {
        self.depth
    }
    
    pub fn nodes(&self) -> &[LiveNode] {
        self.nodes
    }
    
}

impl<'a> Deref for LiveNodeReader<'a> {
    type Target = LiveNode;
    fn deref(&self) -> &Self::Target {&self.nodes[self.index]}
}


pub struct LiveNodeMutReader<'a> {
    eot: bool,
    depth: usize,
    index: usize,
    nodes: &'a mut [LiveNode]
}

impl<'a> LiveNodeMutReader<'a> {
    pub fn new(index: usize, nodes: &'a mut [LiveNode]) -> Self {
        Self {
            eot: false,
            depth: 0,
            index,
            nodes
        }
    }
    
    pub fn node(&mut self) -> &mut LiveNode {
        if self.eot {panic!();}
        &mut self.nodes[self.index]
    }
    
    pub fn node_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.node_slice(self.index)
    }
    
    pub fn children_slice(&self) -> &[LiveNode] {
        if self.eot {panic!()}
        self.nodes.children_slice(self.index)
    }
    
    pub fn count_children(&self) -> usize {self.nodes.count_children(self.index)}
    
    pub fn clone_child(&self, out_vec: &mut Vec<LiveNode>) {
        if self.eot {panic!();}
        self.nodes.clone_child(self.index, out_vec)
    }
    
    pub fn to_string(&self, max_depth: usize) -> String {
        if self.eot {panic!();}
        self.nodes.to_string(self.index, max_depth)
    }
    
    pub fn skip(&mut self) {
        if self.eot {panic!();}
        self.index = self.nodes.skip_node(self.index);
        if self.nodes[self.index].is_close() { // standing on a close node
            if self.depth == 1 {
                self.eot = true;
                self.index += 1;
            }
        }
    }
    
    pub fn walk(&mut self) {
        if self.eot {panic!();}
        if self.nodes[self.index].is_open() {
            self.depth += 1;
        }
        else if self.nodes[self.index].value.is_close() {
            if self.depth == 0 {panic!()}
            self.depth -= 1;
            if self.depth == 0 {
                self.eot = true;
            }
        }
        self.index += 1;
    }
    
    pub fn is_eot(&mut self) -> bool {
        return self.eot
    }
    
    pub fn index(&mut self) -> usize {
        self.index
    }
    
    pub fn depth(&mut self) -> usize {
        self.depth
    }
    
    pub fn nodes(&mut self) -> &mut [LiveNode] {
        self.nodes
    }
    
}

impl<'a> Deref for LiveNodeMutReader<'a> {
    type Target = LiveNode;
    fn deref(&self) -> &Self::Target {&self.nodes[self.index]}
}

impl<'a> DerefMut for LiveNodeMutReader<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.nodes[self.index]}
}