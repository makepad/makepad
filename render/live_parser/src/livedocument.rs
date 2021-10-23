#![allow(unused_variables)]
use makepad_id_macros::*;
use crate::id::{Id, IdPack, IdUnpack, IdFmt};
use std::fmt;
use crate::span::Span;
use crate::util::PrettyPrintedF64;
use crate::token::{TokenWithSpan, TokenId};
use crate::liveerror::LiveError;
use crate::liveerror::LiveErrorOrigin;
use crate::livenode::{LiveNode, LiveValue};
use crate::id::ModulePath;
use crate::id::LocalPtr;
use crate::id::LivePtr;
use crate::id::FileId;

#[derive(Debug)]
pub struct LiveDocument {
    pub recompile: bool,
    pub nodes: Vec<Vec<LiveNode >>,
    pub multi_ids: Vec<Id>,
    pub strings: Vec<char>,
    pub tokens: Vec<TokenWithSpan>,
    pub scopes: Vec<LiveScopeItem>,
}

impl fmt::Display for LiveScopeTarget {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LiveScopeTarget::LocalPtr {..} => {
                write!(f, "[local]")
            },
            LiveScopeTarget::LivePtr (ptr) => {
                write!(f, "[F:{} L:{} I:{}]", ptr.file_id.to_index(), ptr.local_ptr.level, ptr.local_ptr.index)
            }
        }
    }
}

#[derive(Copy, Debug, Clone)]
pub enum LiveScopeTarget {
    LocalPtr(LocalPtr),
    LivePtr(LivePtr)
} 

impl LiveScopeTarget{
    pub fn to_full_node_ptr(&self, file_id:FileId)->LivePtr{
        match self{
            LiveScopeTarget::LocalPtr(local_ptr)=>{
                LivePtr{file_id:file_id, local_ptr:*local_ptr}
            }
            LiveScopeTarget::LivePtr(live_ptr)=>{
                *live_ptr
            }
        }
    }
}

#[derive(Copy, Debug, Clone)]
pub struct LiveScopeItem {
    pub id: Id,
    pub target: LiveScopeTarget
}


impl LiveDocument {
    pub fn new() -> Self {
        Self {
            recompile: true,
            nodes: vec![Vec::new()],
            multi_ids: Vec::new(),
            strings: Vec::new(),
            tokens: Vec::new(),
            scopes: Vec::new(),
        }
    }
    
    pub fn resolve_ptr(&self, local_ptr:LocalPtr)->&LiveNode{
        &self.nodes[local_ptr.level][local_ptr.index]
    }
    
    pub fn get_tokens(&self, token_start:u32, token_count:u32)->&[TokenWithSpan]{
        &self.tokens[token_start as usize..(token_start + token_count)as usize]
    }

    pub fn get_scopes(&self, scope_start:u32, scope_count:u16)->&[LiveScopeItem]{
        &self.scopes[scope_start as usize..(scope_start + scope_count as u32)as usize]
    }


    pub fn restart_from(&mut self, other:&LiveDocument){
        for node in &mut self.nodes{
            node.truncate(0);
        }
        self.multi_ids.clone_from(&other.multi_ids.clone());
        self.strings.clone_from(&other.strings);
        self.tokens.clone_from(&other.tokens.clone());
        self.scopes.truncate(0);
    }
    
    pub fn token_id_to_span(&self, token_id: TokenId) -> Span {
        self.tokens[token_id.token_id as usize].span
    }
    
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
    
    pub fn scan_for_object_path_from(&self, object_path: &[Id], mut node_start: usize, mut node_count: usize, mut level: usize) -> Option<LocalPtr> {
        for i in 0..object_path.len() {
            let id = object_path[i];
            let mut found = false;
            for j in 0..node_count {
                let node = &self.nodes[level][j + node_start];
                if node.id_pack == IdPack::single(id) {
                    // we found the node.
                    if i == object_path.len() - 1 { // last item
                        return Some(LocalPtr {
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
                            _ => return None
                            //LiveError {
                            //   span:self.token_id_to_span(token_id),
                            //   message: format!("Cannont find property {} is not an object path", IdFmt::dot(&multi_ids, Id::multi(id_start, id_count)))
                            // })
                        }
                        found = true;
                        break
                    }
                }
            }
            if !found {
                return None
            }
        }
        None
    }
    
    pub fn scan_for_object_path(&self, object_path: &[Id]) -> Option<LocalPtr> {
        self.scan_for_object_path_from(object_path, 0, self.nodes[0].len(), 0)
    }
    
    pub fn scan_for_multi_for_expand(&self, level: usize, node_start: usize, node_count: usize, id_start: usize, id_count: usize, multi_ids: &Vec<Id>) -> Result<LocalPtr, String> {
        let mut node_start = node_start as usize;
        let mut node_count = node_count as usize;
        let mut level = level;
        for i in 1..id_count {
            let id = multi_ids[i + id_start];
            let mut found = false;
            for j in 0..node_count {
                let node = &self.nodes[level][j + node_start];
                if node.id_pack == IdPack::single(id) {
                    // we found the node.
                    if i == id_count - 1 { // last item
                        return Ok(LocalPtr {
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
                            _ => return Err(format!("Cannont find property {} is not an object path", IdFmt::dot(&multi_ids, IdPack::multi(id_start, id_count))))
                            //LiveError {
                            //   span:self.token_id_to_span(token_id),
                            //   message: format!("Cannont find property {} is not an object path", IdFmt::dot(&multi_ids, Id::multi(id_start, id_count)))
                            // })
                        }
                        found = true;
                        break
                    }
                }
            }
            if !found {
                return Err(format!("Cannot find class {}", IdFmt::dot(&multi_ids, IdPack::multi(id_start, id_count))))
            }
        }
        return Err(format!("Cannot find class {}", IdFmt::dot(&multi_ids, IdPack::multi(id_start, id_count))))
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
        match in_node.id_pack.unpack() {
            IdUnpack::Multi {index: id_start, count: id_count} => {
                let mut node_start = node_start;
                let mut node_count = node_count;
                let mut level = level;
                let mut last_class = None;
                for i in 0..id_count {
                    let id = in_doc.multi_ids[i + id_start];
                    let mut found = false;
                    for j in 0..node_count {
                        let node = &mut self.nodes[level][j + node_start];
                        if node.id_pack == IdPack::single(id) {
                            // we found the node.
                            if i == id_count - 1 { // last item
                                // ok now we need to replace this node
                                
                                if node.value.get_type_nr() != in_node.value.get_type_nr(){
                                    if node.value.is_var_def(){ // we can replace a vardef with something else
                                        continue;
                                    }
                                    // we cant replace a VarDef with something else
                                    return Err(LiveError {
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.token_id),
                                        message: format!("Cannot inherit with different node type {}", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
                                    })
                                }
                                
                                node.token_id = in_node.token_id;
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
                                        origin: live_error_origin!(),
                                        span: in_doc.token_id_to_span(in_node.token_id),
                                        message: format!("Setting property {} is not an object path", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
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
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(in_node.token_id),
                                message: format!("Setting property {} is not an object path", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
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
                                    origin: live_error_origin!(),
                                    span: in_doc.token_id_to_span(in_node.token_id),
                                    message: format!("Unexpected problem 1 in overwrite_or_add_node with {}", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
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
                            token_id: in_node.token_id,
                            id_pack: IdPack::single(in_doc.multi_ids[id_start + id_count - 1]),
                            value: in_node.value
                        });
                        return Ok(None)
                    }
                }
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: in_doc.token_id_to_span(in_node.token_id),
                    message: format!("Unexpected problem 2 in overwrite_or_add_node with {}", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
                })
            }
            IdUnpack::Single(id) => {
                let nodes = &mut self.nodes[level];
                for i in node_start..nodes.len() {
                    if nodes[i].id_pack == in_node.id_pack { // overwrite and exit
                        // lets error if the overwrite value type changed.
                        if nodes[i].value.get_type_nr() != in_node.value.get_type_nr(){
                            if nodes[i].value.is_var_def(){ // we can replace a vardef with something else
                                continue;
                            }
                            return Err(LiveError {
                                origin: live_error_origin!(),
                                span: in_doc.token_id_to_span(in_node.token_id),
                                message: format!("Cannot inherit with different node type {}", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
                            })
                        }
                        nodes[i] = *in_node;
                        return Ok(None)
                    }
                }
                let index = nodes.len();
                nodes.push(*in_node);
                return Ok(Some(index))
            }
            IdUnpack::Empty => {
                let nodes = &mut self.nodes[level];
                let index = nodes.len();
                nodes.push(*in_node);
                return Ok(Some(index))
            },
            _ => {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: in_doc.token_id_to_span(in_node.token_id),
                    message: format!("Unexpected id type {}", IdFmt::dot(&in_doc.multi_ids, in_node.id_pack))
                })
            }
        }
    }
    
    pub fn create_multi_id(&mut self, ids: &[Id]) -> IdPack {
        let multi_index = self.multi_ids.len();
        for id in ids {
            self.multi_ids.push(*id);
        }
        IdPack::multi(multi_index, ids.len())
    }
    
    pub fn clone_multi_id(&mut self, id:IdPack , other_ids:&[Id]) -> IdPack {
         match id.unpack() {
            IdUnpack::Multi {index, count}  => {
                let multi_index = self.multi_ids.len();
                for i in 0..count {
                    self.multi_ids.push(other_ids[i + index]);
                }
                IdPack::multi(multi_index, self.multi_ids.len() - multi_index)
            }
            _ => {
                id
            }
        }
    }
    
    pub fn fetch_module_path(&self, id_pack: IdPack, outer_crate_id: Id) -> ModulePath {
        match id_pack.unpack() {
            IdUnpack::Multi {index, count} if count == 2 => {
                let crate_id = self.multi_ids[index];
                let crate_id = if crate_id == id!(crate) {
                    outer_crate_id
                }else {
                    crate_id
                };
                ModulePath(crate_id, self.multi_ids[index + 1])
            }
            _ => {
                panic!("Unexpected id type {:?}", id_pack.unpack())
            }
        }
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
        
        fn prefix(prep_id: IdPack, ld: &LiveDocument, f: &mut fmt::Formatter) {
            if !prep_id.is_empty() {
                let _ = write!(f, "{}:", IdFmt::dot(&ld.multi_ids, prep_id));
            }
        }
        
        fn recur(ld: &LiveDocument, level: usize, node_index: usize, f: &mut fmt::Formatter) {
            let node = &ld.nodes[level][node_index];
            //let (row,col) = byte_to_row_col(node.span.start(), &ld.source);
            //let _ = write!(f, "/*{},{} {}*/", row+1, col, node.span.len());
            match node.value {
                LiveValue::String {string_start, string_count} => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "\"");
                    for i in 0..string_count {
                        let _ = write!(f, "{}", ld.strings[(i + string_start) as usize]);
                    }
                    let _ = write!(f, "\"");
                },
                LiveValue::Bool(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Int(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", val);
                }
                LiveValue::Float(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", PrettyPrintedF64(val));
                },
                LiveValue::Color(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "#{:08x}", val);
                },
                LiveValue::Vec2(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::Vec3(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", val);
                },
                LiveValue::IdPack(val) => {
                    prefix(node.id_pack, ld, f);
                    let _ = write!(f, "{}", IdFmt::col(&ld.multi_ids, val));
                },
                LiveValue::Call {target, node_start, node_count} => {
                    prefix(node.id_pack, ld, f);
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
                    prefix(node.id_pack, ld, f);
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
                    prefix(node.id_pack, ld, f);
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
                LiveValue::Fn {token_start, token_count, scope_start, scope_count} => {
                    let _ = write!(f, "fn {}", IdFmt::col(&ld.multi_ids, node.id_pack));
                    for i in 0..token_count {
                        let _ = write!(f, "{}", ld.tokens[(i + token_start) as usize]);
                    }
                    let _ = write!(f, "\"");
                    for i in 0..(scope_count as u32) {
                        let item = &ld.scopes[(i + scope_start) as usize];
                        let _ = write!(f, "{}:{}", item.id, item.target);
                        if i != (scope_count - 1) as u32 {
                            let _ = write!(f, ", ");
                            
                        }
                    }
                    let _ = write!(f, "\"");
                },
                //LiveValue::ResourceRef {target}=>{
                //    prefix(node.id_pack, ld, f);
                //    let _ = write!(f, "{}", IdFmt::col(&ld.multi_ids, target));
                // }
                LiveValue::VarDef {token_start, token_count, scope_start, scope_count} => {
                    for i in 0..token_count {
                        let _ = write!(f, "{}", ld.tokens[(i + token_start) as usize]);
                    }
                    let _ = write!(f, "\"");
                    for i in 0..(scope_count as u32) {
                        let item = &ld.scopes[(i + scope_start) as usize];
                        let _ = write!(f, "{}:{}", item.id, item.target);
                        if i != (scope_count - 1) as u32 {
                            let _ = write!(f, ", ");
                            
                        }
                    }
                    let _ = write!(f, "\"");
                },
                LiveValue::Use {module_path_ids} => {
                    let _ = write!(f, "use {}::{}", IdFmt::col(&ld.multi_ids, node.id_pack), IdFmt::col(&ld.multi_ids, module_path_ids));
                }
                LiveValue::LiveType(id)=>{
                    let _ = write!(f, "TypeId {:?}", id);
                }
                LiveValue::Class {class, node_start, node_count} => {
                    prefix(node.id_pack, ld, f);
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
            if i != len - 1 {
                let _ = write!(f, "\n");
            }
        }
        
        fmt::Result::Ok(())
    }
}

