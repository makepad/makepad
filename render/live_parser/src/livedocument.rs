use crate::id::{Id,IdFmt};
use std::fmt;
use crate::span::Span;
use crate::util::PrettyPrintedF64;
use crate::token::{TokenWithSpan};
use crate::liveerror::LiveError;
use crate::livenode::{LiveNode, LiveValue};

pub struct LiveDocument {
    pub root: usize,
    pub live_file_id: usize,
    pub nodes: Vec<Vec<LiveNode >>,
    pub multi_ids: Vec<Id>,
    pub strings: Vec<char>,
    pub tokens: Vec<TokenWithSpan>,
}

impl LiveDocument {
    pub fn new() -> Self {
        Self {
            root: 0,
            live_file_id: 0,
            nodes: vec![Vec::new()],
            multi_ids: Vec::new(),
            strings: Vec::new(),
            tokens: Vec::new(),
        }
    }
}

#[derive(Copy, Clone)]
pub struct LiveNodePtr {
    pub level: usize,
    pub index: usize
}

impl LiveDocument {
    
    pub fn get_level_len(&mut self, level: usize) -> usize {
        let len = self.nodes.len() - 1;
        for _ in len..level {
            self.nodes.push(Vec::new())
        }
        self.nodes[level].len()
    }
    
    pub fn push_node(&mut self, level: usize, node: LiveNode) {
        self.nodes[level].push(node);
    }
    
    pub fn scan_for_multi(&self, level: usize, node_start: usize, node_count: usize, id_start: usize, id_count: usize, multi_ids: &Vec<Id>, span: Span) -> Result<LiveNodePtr, LiveError> {
        let mut node_start = node_start as usize;
        let mut node_count = node_count as usize;
        let mut level = level;
        for i in 1..id_count {
            let id = multi_ids[i + id_start];
            let mut found = false;
            for j in 0..node_count {
                let node = &self.nodes[level][j + node_start];
                if node.id == id {
                    // we found the node.
                    if i == id_count - 1 { // last item
                        return Ok(LiveNodePtr {
                            level: level,
                            index: j + node_start
                        });
                    }
                    else { // we need to be either an object or a class
                        level += 1;
                        match node.value {
                            LiveValue::Class {node_start: ns, node_count: nc, ..} => {
                                node_start = ns as usize;
                                node_count = nc as usize;
                            },
                            _ => return Err(LiveError {
                                span,
                                message: format!("Cannont find property {} is not an object path", IdFmt::dot(&multi_ids, Id::multi(id_start, id_count)))
                            })
                        }
                        found = true;
                        break
                    }
                }
            }
            if !found {
                return Err(LiveError {
                    span,
                    message: format!("Cannot find class {}", IdFmt::dot(&multi_ids, Id::multi(id_start, id_count)))
                })
            }
        }
        return Err(LiveError {
            span,
            message: format!("Cannot find class {}", IdFmt::dot(&multi_ids, Id::multi(id_start, id_count)))
        })
    }
    
    pub fn write_or_add_node(
        &mut self,
        level: usize,
        node_start: usize,
        node_count: usize,
        in_doc: &LiveDocument,
        in_node: &LiveNode
    ) -> Result<Option<usize>, LiveError> {
        // I really need to learn to learn functional programming. This is absurd
        if in_node.id.is_multi() { // we need a multi-level overwrite or insert
            let (id_start, id_count) = in_node.id.get_multi();
            let mut node_start = node_start;
            let mut node_count = node_count;
            let mut level = level;
            let mut last_class = None;
            for i in 0..id_count {
                let id = in_doc.multi_ids[i + id_start];
                let mut found = false;
                for j in 0..node_count {
                    let node = &mut self.nodes[level][j + node_start];
                    if node.id == id {
                        // we found the node.
                        if i == id_count - 1 { // last item
                            // ok now we need to replace this node
                            node.span = in_node.span;
                            node.value = in_node.value;
                            return Ok(None)
                        }
                        else { // we need to be either an object or a class
                            level += 1;
                            match node.value {
                                LiveValue::Class {node_start: ns, node_count: nc, ..} => {
                                    last_class = Some(j + node_start);
                                    node_start = ns as usize;
                                    node_count = nc as usize;
                                },
                                _ => return Err(LiveError {
                                    span: in_node.span,
                                    message: format!("Setting property {} is not an object path", IdFmt::dot(&in_doc.multi_ids, in_node.id))
                                })
                            }
                            found = true;
                            break
                        }
                    }
                }
                if !found { //
                    if i != id_count - 1 || last_class.is_none() { // not last item, so object doesnt exist
                        return Err(LiveError {
                            span: in_node.span,
                            message: format!("Setting property {} is not an object path", IdFmt::dot(&in_doc.multi_ids, in_node.id))
                        })
                    }
                    let last_class = last_class.unwrap();
                    let nodes_len = self.nodes[level].len();
                    if nodes_len == node_start + node_count { // can append to level
                        if let LiveValue::Class {node_count, ..} = &mut self.nodes[level - 1][last_class].value {
                            *node_count += 1;
                        }
                    }
                    else { // have to move all levelnodes. Someday test this with real data and do it better (maybe shift the rest up)
                        let ns = if let LiveValue::Class {node_start, node_count, ..} = &mut self.nodes[level - 1][last_class].value {
                            let ret = *node_start;
                            *node_start = nodes_len as u32;
                            *node_count += 1;
                            ret
                        }
                        else {
                            return Err(LiveError {
                                span: in_node.span,
                                message: format!("Unexpected problem 1 in overwrite_or_add_node with {}", IdFmt::dot(&in_doc.multi_ids, in_node.id))
                            })
                        };
                        let nodes = &mut self.nodes[level];
                        for i in 0..node_count {
                            let node = nodes[i as usize + ns as usize];
                            nodes.push(node);
                        }
                    }
                    // for object, string and array make sure we copy the values
                    
                    // push the final node
                    self.nodes[level].push(LiveNode {
                        span: in_node.span,
                        id: in_doc.multi_ids[id_start + id_count - 1],
                        value: in_node.value
                    });
                    return Ok(None)
                }
            }
            return Err(LiveError {
                span: in_node.span,
                message: format!("Unexpected problem 2 in overwrite_or_add_node with {}", IdFmt::dot(&in_doc.multi_ids, in_node.id))
            })
        }
        else { // single-id scan, safe to just append since this only occurs at the edges
            let nodes = &mut self.nodes[level];
            if in_node.id.is_single() {
                for i in node_start..nodes.len() {
                    if nodes[i].id == in_node.id { // overwrite and exit
                        nodes[i] = *in_node;
                        return Ok(None)
                    }
                }
            }
            // not found.. append
            let index = nodes.len();
            nodes.push(*in_node);
            return Ok(Some(index))
        }
    }
    
    pub fn push_use(&mut self, span: Span, level: usize, crate_name: Id, crate_import: Id) {
        let multi_index = self.multi_ids.len();
        self.multi_ids.push(crate_name);
        self.multi_ids.push(crate_import);
        let imp_id = Id::multi(multi_index, 2);
        
        self.push_node(level, LiveNode {
            span: span,
            id: crate_import,
            value: LiveValue::Use(imp_id)
        });
    }
}


impl fmt::Display for LiveDocument {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // lets iterate the items on level0
        fn indent(depth: usize, f: &mut fmt::Formatter) {
            for _ in 0..depth {
                let _ = write!(f, "    ");
            }
        }
        
        fn prefix(prep_id: Id, ld: &LiveDocument, f: &mut fmt::Formatter) {
            if !prep_id.is_empty() {
                let _ = write!(f, "{}:", IdFmt::dot(&ld.multi_ids, prep_id));
            }
        }
        
        fn recur(ld: &LiveDocument, level: usize, node_index: usize, f: &mut fmt::Formatter) {
            let node = &ld.nodes[level][node_index];
            //let (row,col) = byte_to_row_col(node.span.start(), &ld.source);
            //let _ = write!(f, "/*{},{} {}*/", row+1, col, node.span.len());
            match node.value {
                LiveValue::String {string_index, string_len} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "\"");
                    for i in 0..string_len {
                        let _ = write!(f, "{}", ld.strings[(i + string_index) as usize]);
                    }
                    let _ = write!(f, "\"");
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
                    for i in 0..node_count {
                        if i>0 {
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                    }
                    let _ = write!(f, ")");
                },
                LiveValue::Array {node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "[");
                    for i in 0..node_count {
                        if i>0 {
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                    }
                    let _ = write!(f, "]");
                },
                LiveValue::Object {node_start, node_count} => {
                    prefix(node.id, ld, f);
                    let _ = write!(f, "{{");
                    for i in 0..(node_count >> 1) {
                        if i>0 {
                            let _ = write!(f, ", ");
                        }
                        recur(ld, level + 1, (i * 2) as usize + node_start as usize, f);
                        let _ = write!(f, ":");
                        recur(ld, level + 1, (i * 2 + 1) as usize + node_start as usize, f);
                    }
                    let _ = write!(f, "}}");
                },
                LiveValue::Fn {token_start, token_count} => {
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
                    for i in 0..node_count {
                        if !ld.nodes[level + 1][i as usize + node_start as usize].value.is_simple() {
                            is_simple = false;
                        }
                    }
                    if !is_simple && node_count > 0 {
                        let _ = write!(f, "\n");
                    }
                    for i in 0..node_count {
                        if !is_simple {
                            indent(level + 1, f);
                        }
                        else {
                            if i >0 {
                                let _ = write!(f, ", ");
                            }
                        }
                        recur(ld, level + 1, i as usize + node_start as usize, f);
                        if !is_simple {
                            let _ = write!(f, "\n");
                        }
                    }
                    if !is_simple && node_count > 0 {
                        indent(level, f);
                    }
                    let _ = write!(f, "}}");
                }
            }
        }
        
        let len = self.nodes[0].len();
        for i in 0..len {
            recur(self, 0, i, f);
            let _ = write!(f, "\n");
        }
        
        fmt::Result::Ok(())
    }
}

