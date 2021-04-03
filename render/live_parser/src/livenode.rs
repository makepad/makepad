#![allow(unused_variables)]
use crate::id::Id;
use std::fmt;
use crate::math::{Vec2, Vec3};
use crate::util::PrettyPrintedF64;

pub struct LiveDocument {
    root: usize,
    live_file_id: usize,
    nodes: Vec<Vec<LiveNode >>,
    dirty: Vec<u64>,
    pub multi_ids: Vec<Id>,
    pub strings: Vec<char>,
}

impl LiveDocument {
    pub fn new() -> Self {
        Self {
            root: 0,
            live_file_id: 0,
            nodes: vec![Vec::new()],
            dirty: Vec::new(),
            multi_ids: Vec::new(),
            strings: Vec::new(),
        }
    }
}

impl LiveDocument {
    
    pub fn level_len(&mut self, level: usize) -> usize {
        let len = self.nodes.len() - 1;
        for i in len..level {
            self.nodes.push(Vec::new())
        }
        self.nodes[level].len()
    }
    
    pub fn push_node(&mut self, level: usize, node: LiveNode) {
        self.nodes[level].push(node);
    }
    
    pub fn add_use_import(&mut self, crate_name: Id, crate_import: Id) {
        
        let multi_index = self.multi_ids.len();
        self.multi_ids.push(crate_name);
        self.multi_ids.push(crate_import);
        let imp_id = Id::multi(multi_index as u32, 2);
        
        self.nodes[0].push(LiveNode {id: Id::empty(), value: LiveValue::Use(imp_id)});
    }
}

struct  IdFmt<'a>{
    multi_ids: &'a Vec<Id>,
    is_dot: bool,
    id: Id
}

impl <'a> IdFmt<'a>{
    fn dot(multi_ids:&'a Vec<Id>, id:Id)->Self{
        Self{multi_ids, is_dot:true, id}
    }
    fn col(multi_ids:&'a Vec<Id>, id:Id)->Self{
        Self{multi_ids, is_dot:false, id}
    }
}

impl <'a> fmt::Display for IdFmt<'a>{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.id.is_multi(){
            let (index,len) = self.id.get_multi();
            for i in 0..len{
                let _ = write!(f, "{}", self.multi_ids[(i+index) as usize]);
                if i < len - 1{
                    if self.is_dot{
                        let _ = write!(f, ".");
                    }
                    else{
                        let _ = write!(f, "::");
                    }
                }
            }
            fmt::Result::Ok(())
        }
        else{
            write!(f, "{}", self.id)
        }
    }
}

impl fmt::Display for LiveDocument {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // lets iterate the items on level0
        let len = self.nodes[0].len();
        fn indent(depth:usize,  f: &mut fmt::Formatter){
            for j in 0..depth{
                let _ = write!(f, "  ");
            }
        }
        
        fn prefix(prep_id:Id, ld:&LiveDocument, f: &mut fmt::Formatter){
            if !prep_id.is_empty(){
                let _ = write!(f, "{}:",IdFmt::dot(&ld.multi_ids, prep_id));
            }
        }
        
        fn recur(ld: &LiveDocument, level: usize, node_index: usize, f: &mut fmt::Formatter) {
            let node = &ld.nodes[level][node_index];
            match node.value {
                LiveValue::String {string_index, string_len} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "\"\"");
                },
                LiveValue::Bool(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Int(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", val);
                }
                LiveValue::Float(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", PrettyPrintedF64(val));
                },
                LiveValue::Color(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Vec2(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Vec3(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Id(val) => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}", IdFmt::col(&ld.multi_ids, val));
                },
                LiveValue::Call {target, node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{}(", IdFmt::dot(&ld.multi_ids, target));
                    for i in 0..node_count{
                        if i>0{
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                    }
                    let _ = write!(f, ")");
                },
                LiveValue::Array {node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "[");
                    for i in 0..node_count{
                        if i>0{
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                    }
                    let _ = write!(f, "]");
                },
                LiveValue::Object {node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{{");
                    for i in 0..(node_count>>1){
                        if i>0{
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, (i*2) as usize + node_start as usize, f);
                        let _ = write!(f, ":");
                        recur(ld, level + 1, (i*2+1) as usize + node_start as usize, f);
                    }
                    let _ = write!(f, "}}");
                },
                LiveValue::Const {live_file_id, token_start, token_count} => {
                    let _ = write!(f, "const {}", IdFmt::col(&ld.multi_ids, node.id));
                },
                LiveValue::Fn {live_file_id, token_start, token_count} => {
                    let _ = write!(f, "fn {}(){{}}", IdFmt::col(&ld.multi_ids, node.id));
                },
                LiveValue::Use(id) => {
                    let _ = write!(f, "use {}", IdFmt::col(&ld.multi_ids, id));
                }
                LiveValue::Class {class, node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{} {{", IdFmt::col(&ld.multi_ids, class));
                    // lets do a pass to check if its all simple values
                    let mut is_simple = true;
                    for i in 0..node_count{
                        if !ld.nodes[level+1][i as usize + node_start as usize].value.is_simple(){
                            is_simple = false;
                        }
                    }
                    if !is_simple && node_count > 0{
                        let _ = write!(f, "\n");
                    }
                    for i in 0..node_count{
                        if !is_simple{
                            indent(level+1, f);
                        }
                        else{
                            if i >0{
                                let _ = write!(f, ",");
                            }
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                        if !is_simple{
                            let _ = write!(f, "\n");
                        }
                    }
                    if !is_simple && node_count > 0{
                        indent(level, f);
                    }
                    let _ = write!(f, "}}");
                }
            }
        }
        for i in 0..len {
            recur(self, 0, i, f);
            let _ = write!(f, "\n");
        }
        fmt::Result::Ok(())
    }
}

pub struct LiveNode { // 3x u64
    pub id: Id,
    pub value: LiveValue,
}

impl LiveValue{
    fn is_simple(&self)->bool{
        match self{
            LiveValue::Bool(_) => true,
            LiveValue::Int(_)  => true,
            LiveValue::Float(_)  => true,
            LiveValue::Color(_) => true,
            LiveValue::Vec2(_) => true,
            LiveValue::Vec3(_) => true,
            LiveValue::Id(_) => true,
            _=>false
        }
    }
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
    Vec2(Vec2),
    Vec3(Vec3),
    Id(Id),
    Call {
        target: Id,
        node_start: u32,
        node_count: u16
    },
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

