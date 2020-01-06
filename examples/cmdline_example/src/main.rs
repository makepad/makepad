#[derive(Clone, Default)]
struct TextNodeMap {
    chr: char,
    next: usize
}

#[derive(Clone, Default, Debug)]
struct TextNodeEnd {
    gen: u64,
    file: usize,
    token: usize
}

#[derive(Clone, Default)]
struct TextNode {
    map: Vec<TextNodeMap>,
    end: Vec<TextNodeEnd>
}

#[derive(Clone, Default)]
struct TextIndex {
    nodes: Vec<TextNode>
}

impl TextIndex {
    
    fn new() -> TextIndex {
        TextIndex {
            nodes: vec![TextNode::default()]
        }
    }
    
    fn write(&mut self, what: &str, gen: u64, file: usize, token: usize) {
        let mut node_id = 0;
        for c in what.chars() {
            // lets go to the next node
            node_id = if let Some(v) = self.nodes[node_id].map.iter().find( | v | v.chr == c) {
                v.next
            }
            else {
                let id = self.nodes.len();
                self.nodes.push(TextNode::default());
                self.nodes[node_id].map.push(TextNodeMap {chr: c, next: id});
                id
            };
        }
        // lets add val, overwriting old gen
        if let Some(end) = self.nodes[node_id].end.iter_mut().find( | v | v.file == file && v.gen != gen) {
            end.gen = gen;
            end.token = token;
        }
        else {
            self.nodes[node_id].end.push(TextNodeEnd {
                gen: gen,
                file: file,
                token: token
            });
        }
    }
    
    fn begins_with(&self, what: &str) -> Vec<TextNodeEnd> {
        // ok so if i type a beginning of a word, i'd want all the endpoints
        let mut results = Vec::new();
        let mut node_id = 0;
        for c in what.chars() {
            // lets go to the next node
            node_id = if let Some(v) = self.nodes[node_id].map.iter().find( | v | v.chr == c) {
                v.next
            }
            else {
                // nothing
                return vec![]
            };
        }
        let mut nexts = Vec::new();
        loop {
            for map in &self.nodes[node_id].map {
                nexts.push(map.next);
            }
            results.extend_from_slice(&self.nodes[node_id].end);
            if nexts.len() == 0 {
                break;
            }
            node_id = nexts.pop().unwrap();
        }
        results
    }
    
    fn calc_total_size(&self)->usize{
        let mut total = 0;
        for node in &self.nodes{
            total += std::mem::size_of::<TextNode>();
            total += std::mem::size_of::<TextNodeEnd>() * node.end.capacity();
            total += std::mem::size_of::<TextNodeMap>() * node.map.capacity();
        }
        total
    }
}

fn main() {
    let mut t = TextIndex::new();
    for i in 0..1000000 {
         t.write(&format!("hi {}", i), 1, 1, i);
    }
    t.write("ho", 1, 1, 2);
    let res = t.begins_with("hi 1001");
    println!("hello {:?}", res.len());
    println!("{}", t.calc_total_size());
}
