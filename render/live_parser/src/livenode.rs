use crate::id::Id;
pub struct LiveDocument {
    root: usize,
    live_file_id: usize,
    nodes: Vec<Vec<LiveNode>>,
    dirty: Vec<u64>,
    pub multi_ids: Vec<Id>,
    pub strings: Vec<char>,
}

impl LiveDocument{
    pub fn new()->Self{
        Self{
            root:0,
            live_file_id:0,
            nodes:vec![Vec::new()],
            dirty: Vec::new(),
            multi_ids: Vec::new(),
            strings:Vec::new(),
        }
    }
}

impl LiveDocument{
    
    pub fn level_len(&mut self, level: usize)->usize{
        let len = self.nodes.len() - 1;
        for i in len..level{
            self.nodes.push(Vec::new())
        }
        self.nodes[level].len()
    }
    
    pub fn push_node(&mut self, level:usize, node:LiveNode){
        self.nodes[level].push(node);
    }

    pub fn add_use_import(&mut self, crate_name: Id, crate_import: Id){
        
        let multi_index = self.multi_ids.len();
        self.multi_ids.push(crate_name);
        self.multi_ids.push(crate_import);
        let imp_id = Id::multi(multi_index as u32, 2);
        
        self.nodes[0].push(LiveNode{id:Id::empty(), value:LiveValue::Use(imp_id)});
    }
}

pub struct LiveNode { // 3x u64
    pub id: Id,
    pub value: LiveValue,
}

pub enum LiveValue {
    String {
        string_index: u32,
        string_len: u32
    },
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(f32,f32),
    Vec3(f32,f32,f32),
    Id(Id),
    Array {
        node_start: u32,
        node_count: u32
    },
    Object {
        node_start: u32,
        node_count: u32
    },
    Const {
        live_file_id: u32,
        token_start: u32,
        token_count: u32,
    },
    Fn {
        live_file_id: u32,
        token_start: u32,
        token_count: u32,
    },
    Use(Id),
    Class {
        class: Id,
        node_start: u32, // how about
        node_count: u16 //65535 class items is plenty keeps this structure at 24 bytes
    },
}

//so we start walking the base 'truth'
//and every reference we run into we need to look up
// then we need to make a list of 'overrides'
// then walk the original, checking against overrides.
// all the while writing a new document as output

