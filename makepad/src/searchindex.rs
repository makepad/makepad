use std::collections::{HashMap};
use crate::appstorage::*;
use makepad_widget::*;

#[derive(Clone)]
pub struct SearchIndex {
    identifiers: TextIndex,
}

impl SearchIndex {
    
    pub fn new() -> Self {
        Self {
            identifiers: TextIndex::new()
        }
    }
    
    pub fn new_rust_token(&mut self, text_buffer: &TextBuffer) {
        // pass it to the textindex
        let chunk_id = text_buffer.token_chunks.len() - 1;
        let chunk = &text_buffer.token_chunks[chunk_id];
        
        match chunk.token_type {
            TokenType::Identifier | TokenType::Call | TokenType::TypeName => {
                let chars = &text_buffer.flat_text[chunk.offset..(chunk.offset + chunk.len)];
                self.identifiers.write(
                    chars,
                    text_buffer.text_buffer_id,
                    text_buffer.mutation_id as u32,
                    chunk_id as u16
                );
            },
            _ => ()
        }
    }
    
    pub fn begins_with(&mut self, what: &str, storage:&AppStorage) -> Vec<SearchResult>{
        let mut out = Vec::new();
        self.identifiers.begins_with(what, storage, &mut out);
        out
    }
}


#[derive(Clone, Default)]
pub struct SearchResult {
    pub text_buffer_id: TextBufferId,
    pub token: u16
}

#[derive(Clone, Default)]
pub struct TextIndexNode {
    stem: [char; 6],
    used: usize,
    map: HashMap<char, usize>,
    end: HashMap<(TextBufferId, u16), u32>
}

#[derive(Clone)]
pub struct TextIndex {
    pub nodes: Vec<TextIndexNode>
}

impl TextIndex {
    
    pub fn new() -> TextIndex {
        TextIndex {
            nodes: vec![TextIndexNode::default()]
        }
    }
    
    pub fn write(&mut self, what: &[char], text_buffer_id: TextBufferId, mut_id: u32, token: u16) {
        let mut o = 0;
        let mut id = 0;
        loop {
            if self.nodes[id].used > 0 { // we have a stem to compare/split
                for s in 0..self.nodes[id].used { // read stem
                    let sc = self.nodes[id].stem[s];
                    if o >= what.len() || sc != what[o] { // split
                        // lets make a new node and swap in our end/map
                        let new_id = self.nodes.len();
                        let mut new_end = HashMap::new();
                        let mut new_map = HashMap::new();
                        std::mem::swap(&mut new_end, &mut self.nodes[id].end);
                        std::mem::swap(&mut new_map, &mut self.nodes[id].map);
                        self.nodes.push(TextIndexNode {
                            end: new_end,
                            map: new_map,
                            ..TextIndexNode::default()
                        });
                        // copy whatever stem is left
                        for s2 in (s + 1)..self.nodes[id].used {
                            self.nodes[new_id].stem[s2 - s - 1] = self.nodes[id].stem[s2];
                        }
                        self.nodes[new_id].used = self.nodes[id].used - (s + 1);
                        self.nodes[id].used = s;
                        self.nodes[id].map.insert(sc, new_id);
                        break;
                    }
                    o += 1;
                }
            }
            else if self.nodes[id].map.len() == 0 { // write stem
                for s in 0..self.nodes[id].stem.len() {
                    if o >= what.len() { // we are done
                        break;
                    }
                    self.nodes[id].stem[s] = what[o];
                    self.nodes[id].used = s + 1;
                    o += 1;
                }
            }
            if o >= what.len() { // what is consumed
                break;
            }
            // jump/insert next node
            id = if let Some(next_id) = self.nodes[id].map.get(&what[o]) {
                o += 1;
                *next_id
            }
            else {
                let new_id = self.nodes.len();
                self.nodes.push(TextIndexNode::default());
                self.nodes[id].map.insert(what[o], new_id);
                o += 1;
                new_id
            };
        }
        
        self.nodes[id].end.insert((text_buffer_id, token), mut_id);
    }
    
    pub fn _write_str(&mut self, what: &str, text_buffer_id: TextBufferId, gen: u32, token: u16) {
        let mut whatv = Vec::new();
        for c in what.chars() {
            whatv.push(c);
        }
        self.write(&whatv, text_buffer_id, gen, token);
    }
    
    pub fn begins_with(&mut self, what: &str, storage:&AppStorage, out:&mut Vec<SearchResult>)  {
        // ok so if i type a beginning of a word, i'd want all the endpoints
        
        let mut node_id = 0;
        let mut stem_eat = 0;
        for c in what.chars() {
            // first eat whats in the stem
            if stem_eat < self.nodes[node_id].used {
                if c != self.nodes[node_id].stem[stem_eat] {
                    return 
                }
                stem_eat += 1;
            }
            else {
                node_id = if let Some(next_id) = self.nodes[node_id].map.get(&c) {
                    stem_eat = 0;
                    *next_id
                }
                else {
                    // nothing
                    return 
                };
            }
        }

        let mut nexts = Vec::new();
        let mut cleanup = Vec::new();
        loop {
            for (_key, next) in &self.nodes[node_id].map {
                nexts.push(*next);
            }
            cleanup.truncate(0);
            for ((text_buffer_id, token), mut_id) in &self.nodes[node_id].end {
                if storage.text_buffers[text_buffer_id.0 as usize].text_buffer.mutation_id == *mut_id{
                    out.push(SearchResult{text_buffer_id:*text_buffer_id, token:*token});
                }
                else{
                    cleanup.push((*text_buffer_id, *token));
                }
            }
            for pair in &cleanup{
                self.nodes[node_id].end.remove(pair);
            }
            if nexts.len() == 0 {
                break;
            }
            node_id = nexts.pop().unwrap();
        }
    }
    
    pub fn _dump_tree(&self, key: char, id: usize, depth: usize) {
        let mut indent = String::new();
        for _ in 0..depth {indent.push_str(" - ");};
        let mut stem = String::new();
        for i in 0..self.nodes[id].used {
            stem.push(self.nodes[id].stem[i]);
        }
        println!("{}{} #{}#", indent, key, stem);
        for (key, val) in &self.nodes[id].map {
            self._dump_tree(*key, *val, depth + 1);
        }
    }
    /*
    pub fn _calc_total_size(&self) -> usize {
        let mut total = 0;
        for node in &self.nodes {
            total += std::mem::size_of::<TextIndexResult>() * node.end.capacity();
            total += std::mem::size_of::<usize>() * node.map.capacity();
        }
        total += std::mem::size_of::<TextIndexNode>() * self.nodes.capacity();
        total
    }*/
}
