use std::collections::{HashMap};

#[derive(Clone, Default, Debug)]
pub struct TextIndexEnd {
    gen: u64,
    file: usize,
    token: usize
}

#[derive(Clone, Default)]
pub struct TextIndexNode {
    map: HashMap<char, usize>,
    end: Vec<TextIndexEnd>
}

#[derive(Clone, Default)]
pub struct TextIndex {
    pub nodes: Vec<TextIndexNode>
}

impl TextIndex {
    
    pub fn new() -> TextIndex {
        TextIndex {
            nodes: vec![TextIndexNode::default()]
        }
    }
    
    pub fn write(&mut self, what: &[char], gen: u64, file: usize, token: usize) {
        let mut node_id = 0;
        for c in what {
            // lets go to the next node
            node_id = if let Some(next_id) = self.nodes[node_id].map.get(&c) {
                *next_id
            }
            else {
                let id = self.nodes.len();
                self.nodes.push(TextIndexNode::default());
                self.nodes[node_id].map.insert(*c, id);
                id
            };
        }
        // lets add val, overwriting old gen
        if let Some(end) = self.nodes[node_id].end.iter_mut().find( | v | v.file == file && v.gen != gen) {
            end.gen = gen;
            end.token = token;
        }
        else {
            self.nodes[node_id].end.push(TextIndexEnd {
                gen: gen,
                file: file,
                token: token
            });
        }
    }
    
    pub fn begins_with(&self, what: &str) -> Vec<TextIndexEnd> {
        // ok so if i type a beginning of a word, i'd want all the endpoints
        let mut results = Vec::new();
        let mut node_id = 0;
        for c in what.chars() {
            // lets go to the next node
            node_id = if let Some(next_id) = self.nodes[node_id].map.get(&c) {
                *next_id
            }
            else {
                // nothing
                return vec![]
            };
        }
        let mut nexts = Vec::new();
        loop {
            for (_key, next) in &self.nodes[node_id].map {
                nexts.push(*next);
            }
            results.extend_from_slice(&self.nodes[node_id].end);
            if nexts.len() == 0 {
                break;
            }
            node_id = nexts.pop().unwrap();
        }
        results
    }

    pub fn calc_total_size(&self)->usize{
        let mut total = 0;
        for node in &self.nodes{
            total += std::mem::size_of::<TextIndexEnd>() * node.end.capacity();
            total += std::mem::size_of::<usize>() * node.map.capacity();
        }
        total += std::mem::size_of::<TextIndexNode>() * self.nodes.capacity();
        total
    }
}

