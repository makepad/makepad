use std::collections::{HashMap};

#[derive(Clone, Default, Debug)]
pub struct TextIndexEnd {
    gen: u32,
    file: u16,
    token: u16
}

#[derive(Clone, Default)]
pub struct TextIndexNode {
    stem: [char; 6],
    used: usize,
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
    
    pub fn write(&mut self, what: &[char], gen: u32, file: u16, token: u16) {
        let mut o = 0;
        let mut id = 0;
        loop {
            if self.nodes[id].used > 0 { // we have a stem to compare/split
                for s in 0..self.nodes[id].used { // read stem
                    let sc = self.nodes[id].stem[s];
                    if o >= what.len() || sc != what[o] { // split
                        // lets make a new node and swap in our end/map
                        let new_id = self.nodes.len();
                        let mut new_end = Vec::new();
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
        
        // lets add val, overwriting old gen
        if let Some(end) = self.nodes[id].end.iter_mut().find( | v | v.file == file && v.gen != gen) {
            end.gen = gen;
            end.token = token;
        }
        else {
            self.nodes[id].end.push(TextIndexEnd {
                gen: gen,
                file: file,
                token: token
            });
        }
        
    }
    
    pub fn write_str(&mut self, what: &str, gen: u32, file: u16, token: u16) {
        let mut whatv = Vec::new();
        for c in what.chars() {
            whatv.push(c);
        }
        self.write(&whatv, gen, file, token);
    }
    
    pub fn begins_with(&self, what: &str) -> Vec<TextIndexEnd> {
        // ok so if i type a beginning of a word, i'd want all the endpoints
        let mut results = Vec::new();
        let mut node_id = 0;
        let mut stem_eat = 0;
        for c in what.chars() {
            // first eat whats in the stem
            if stem_eat < self.nodes[node_id].used {
                if c != self.nodes[node_id].stem[stem_eat] {
                    return vec![]
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
                    return vec![]
                };
            }
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
    
    pub fn dump_tree(&self, key: char, id: usize, depth: usize) {
        let mut indent = String::new();
        for _ in 0..depth {indent.push_str(" - ");};
        let mut stem = String::new();
        for i in 0..self.nodes[id].used {
            stem.push(self.nodes[id].stem[i]);
        }
        println!("{}{} #{}#", indent, key, stem);
        for (key, val) in &self.nodes[id].map {
            self.dump_tree(*key, *val, depth + 1);
        }
    }
    
    pub fn calc_total_size(&self) -> usize {
        let mut total = 0;
        for node in &self.nodes {
            total += std::mem::size_of::<TextIndexEnd>() * node.end.capacity();
            total += std::mem::size_of::<usize>() * node.map.capacity();
        }
        total += std::mem::size_of::<TextIndexNode>() * self.nodes.capacity();
        total
    }
}

fn main() {
    let mut t = TextIndex::new();
    t.write_str("hello world", 0, 0, 0);
    t.write_str("hell12345678901", 1, 1, 1);
    t.dump_tree(' ', 0, 0);
    let res = t.begins_with("hell");
    println!("{:?}", res);
}
